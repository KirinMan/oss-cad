//! A minimal ZIP writer: local file headers, a central directory, and an
//! end-of-central-directory record, per the PKWARE APPNOTE. Every entry uses
//! compression method 0 ("stored" — raw bytes, no deflate): a `.bcfzip`'s
//! contents are a handful of short XML files, where the deflate
//! implementation's complexity would buy back nothing worth having, and
//! "stored" is exactly as valid a ZIP as any other compression method.
//!
//! Timestamps are fixed at the DOS epoch (1980-01-01, the earliest date the
//! format can represent) rather than "now": a `.bcfzip`'s per-file
//! modification time carries no meaning here (there is no editable file
//! behind these entries to have been modified), and a fixed timestamp makes
//! two exports of the same issues byte-identical, which is worth more than a
//! meaningless clock reading.

/// One entry to add to the archive.
pub struct Entry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

const LOCAL_FILE_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_FILE_SIGNATURE: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIR_SIGNATURE: u32 = 0x0605_4b50;
/// DOS date for 1980-01-01, the epoch the format's date field is built on —
/// see the module docs for why a fixed timestamp is used at all.
const DOS_DATE_1980_01_01: u16 = 0x0021;
const VERSION_NEEDED: u16 = 20; // 2.0: stored entries need nothing newer.

/// Builds a ZIP archive containing exactly `entries`, in order.
#[must_use]
pub fn write(entries: &[Entry<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central_directory = Vec::new();

    for entry in entries {
        let offset = as_u32(out.len());
        let crc = crc32(entry.data);
        let name = entry.name.as_bytes();
        let size = as_u32(entry.data.len());

        out.extend_from_slice(&LOCAL_FILE_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&VERSION_NEEDED.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // general purpose flag
        out.extend_from_slice(&0u16.to_le_bytes()); // compression: stored
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&DOS_DATE_1980_01_01.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes()); // compressed size
        out.extend_from_slice(&size.to_le_bytes()); // uncompressed size
        out.extend_from_slice(&as_u16(name.len()).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        out.extend_from_slice(name);
        out.extend_from_slice(entry.data);

        central_directory.extend_from_slice(&CENTRAL_FILE_SIGNATURE.to_le_bytes());
        central_directory.extend_from_slice(&VERSION_NEEDED.to_le_bytes()); // version made by
        central_directory.extend_from_slice(&VERSION_NEEDED.to_le_bytes()); // version needed
        central_directory.extend_from_slice(&0u16.to_le_bytes()); // general purpose flag
        central_directory.extend_from_slice(&0u16.to_le_bytes()); // compression: stored
        central_directory.extend_from_slice(&0u16.to_le_bytes()); // mod time
        central_directory.extend_from_slice(&DOS_DATE_1980_01_01.to_le_bytes());
        central_directory.extend_from_slice(&crc.to_le_bytes());
        central_directory.extend_from_slice(&size.to_le_bytes());
        central_directory.extend_from_slice(&size.to_le_bytes());
        central_directory.extend_from_slice(&as_u16(name.len()).to_le_bytes());
        central_directory.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        central_directory.extend_from_slice(&0u16.to_le_bytes()); // file comment length
        central_directory.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        central_directory.extend_from_slice(&0u16.to_le_bytes()); // internal file attributes
        central_directory.extend_from_slice(&0u32.to_le_bytes()); // external file attributes
        central_directory.extend_from_slice(&offset.to_le_bytes());
        central_directory.extend_from_slice(name);
    }

    let central_directory_offset = as_u32(out.len());
    let central_directory_size = as_u32(central_directory.len());
    out.extend_from_slice(&central_directory);

    out.extend_from_slice(&END_OF_CENTRAL_DIR_SIGNATURE.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // number of this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // disk where central dir starts
    out.extend_from_slice(&as_u16(entries.len()).to_le_bytes()); // entries on this disk
    out.extend_from_slice(&as_u16(entries.len()).to_le_bytes()); // total entries
    out.extend_from_slice(&central_directory_size.to_le_bytes());
    out.extend_from_slice(&central_directory_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length

    out
}

/// A `.bcfzip` never has enough entries, or entries anywhere near 4GiB, to
/// approach either format limit — saturating rather than panicking keeps
/// that a documented fact instead of an unreachable `unwrap`.
fn as_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn as_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// CRC-32 (IEEE 802.3 / ISO 3309 / PKZIP), computed bit by bit rather than
/// through a lookup table: at BCF's file sizes (short XML documents) the
/// table's setup cost would outweigh what it saves, and the bit-by-bit form
/// is the one that reads back directly as "the standard CRC-32 algorithm"
/// against a reference description, which matters more here than throughput.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    const POLY: u32 = 0xEDB8_8320;
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (POLY & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        // The check value every CRC-32/ISO-HDLC (the ZIP/PKZIP variant)
        // implementation is verified against — reproduced in the Rocksoft
        // "A Painless Guide to CRC Error Detection Algorithms" catalogue.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn crc32_of_empty_input_is_zero() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn an_empty_archive_still_has_a_valid_end_of_central_directory_record() {
        let zip = write(&[]);
        assert_eq!(zip.len(), 22, "just the EOCD record");
        assert_eq!(&zip[0..4], &END_OF_CENTRAL_DIR_SIGNATURE.to_le_bytes());
    }

    #[test]
    fn entries_round_trip_through_a_hand_written_reader() {
        let entries = [
            Entry {
                name: "bcf.version",
                data: b"<Version/>",
            },
            Entry {
                name: "topic/markup.bcf",
                data: b"<Markup/>",
            },
        ];
        let zip = write(&entries);

        // A from-scratch reader over the central directory, independent of
        // `write`'s own bookkeeping -- the point is to prove the bytes are a
        // structurally valid ZIP, not just that this module agrees with
        // itself.
        let eocd_start = zip.len() - 22;
        assert_eq!(
            u32::from_le_bytes(zip[eocd_start..eocd_start + 4].try_into().unwrap()),
            END_OF_CENTRAL_DIR_SIGNATURE
        );
        let total_entries =
            u16::from_le_bytes(zip[eocd_start + 10..eocd_start + 12].try_into().unwrap());
        let cd_size = u32::from_le_bytes(zip[eocd_start + 12..eocd_start + 16].try_into().unwrap());
        let cd_offset =
            u32::from_le_bytes(zip[eocd_start + 16..eocd_start + 20].try_into().unwrap());
        assert_eq!(total_entries as usize, entries.len());
        assert_eq!(cd_offset as usize + cd_size as usize, eocd_start);

        let mut pos = cd_offset as usize;
        for entry in &entries {
            assert_eq!(
                u32::from_le_bytes(zip[pos..pos + 4].try_into().unwrap()),
                CENTRAL_FILE_SIGNATURE
            );
            let name_len = u16::from_le_bytes(zip[pos + 28..pos + 30].try_into().unwrap()) as usize;
            let local_offset =
                u32::from_le_bytes(zip[pos + 42..pos + 46].try_into().unwrap()) as usize;
            let name = &zip[pos + 46..pos + 46 + name_len];
            assert_eq!(name, entry.name.as_bytes());

            assert_eq!(
                u32::from_le_bytes(zip[local_offset..local_offset + 4].try_into().unwrap()),
                LOCAL_FILE_SIGNATURE
            );
            let local_name_len = u16::from_le_bytes(
                zip[local_offset + 26..local_offset + 28]
                    .try_into()
                    .unwrap(),
            ) as usize;
            let data_start = local_offset + 30 + local_name_len;
            let data = &zip[data_start..data_start + entry.data.len()];
            assert_eq!(data, entry.data);

            pos += 46 + name_len;
        }
    }
}
