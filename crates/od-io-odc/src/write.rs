//! Writing a `.odc` container.

use crate::manifest::Manifest;
use crate::{CHUNK_OBJECTS, OdcError, Result, StoredHeader, ZSTD_LEVEL};
use od_core::Database;
use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Serialises a document to `.odc` bytes.
///
/// Entries are ZIP-stored and their payloads compressed individually with zstd,
/// rather than leaning on ZIP's own compression. That keeps each part
/// independently readable — a tool can pull the layer table out of a 200 MB
/// drawing without inflating the geometry — which is the point of using a
/// container at all.
pub fn write_bytes(db: &Database) -> Result<Vec<u8>> {
    let snapshot = db.to_snapshot();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    // The manifest is plain, uncompressed JSON and comes first, so that any
    // tool — including one that has never heard of this format — can read what
    // a file claims to be without a decompressor.
    zip.start_file("manifest.json", stored)?;
    zip.write_all(&serde_json::to_vec_pretty(&Manifest::current())?)?;

    let header = StoredHeader {
        header: snapshot.header,
        ids: snapshot.ids,
        named_dict: snapshot.named_dict,
        model_space: snapshot.model_space,
    };
    write_cbor(&mut zip, stored, "header.cbor", &header)?;

    // Tables go in one entry each: reading a drawing's layer list is a common
    // operation (checkers, browsers, project audits) and should not require
    // parsing anything else.
    let t = &snapshot.tables;
    write_cbor(&mut zip, stored, "tables/layers.cbor", &t.layers)?;
    write_cbor(&mut zip, stored, "tables/linetypes.cbor", &t.linetypes)?;
    write_cbor(&mut zip, stored, "tables/text_styles.cbor", &t.text_styles)?;
    write_cbor(&mut zip, stored, "tables/dim_styles.cbor", &t.dim_styles)?;
    write_cbor(&mut zip, stored, "tables/blocks.cbor", &t.blocks)?;
    write_cbor(&mut zip, stored, "tables/levels.cbor", &t.levels)?;
    write_cbor(&mut zip, stored, "tables/grids.cbor", &t.grids)?;

    // Objects are chunked so a large drawing can be read incrementally.
    //
    // Chunking is by insertion order for now. The design calls for grouping by
    // spatial locality, so that panning a large drawing touches few chunks —
    // that needs the R-tree, and doing it before the index exists would bake in
    // an arbitrary grouping the reader would then have to keep supporting.
    for (i, chunk) in snapshot.objects.chunks(CHUNK_OBJECTS).enumerate() {
        write_cbor(&mut zip, stored, &format!("objects/{i:04}.chunk"), &chunk)?;
    }

    // Schemas travel with the document, so its attributes stay meaningful on a
    // machine without the plugin that produced them.
    for (app, schema) in &snapshot.schemas {
        let path = format!("schemas/{}.json", sanitise(&app.0));
        zip.start_file(&path, stored)?;
        zip.write_all(&serde_json::to_vec_pretty(schema)?)?;
    }

    // Data kept verbatim from an earlier read. Entries this container itself
    // produced go back where they came from; anything from another format goes
    // under preserved/ for that format's writer to find.
    for (i, blob) in snapshot.preserved.iter().enumerate() {
        let path = if blob.source == "odc" {
            blob.section.clone()
        } else {
            format!(
                "preserved/{}/{:04}-{}",
                sanitise(&blob.source),
                i,
                sanitise(&blob.section)
            )
        };
        zip.start_file(&path, stored)?;
        zip.write_all(&blob.payload)?;
    }

    Ok(zip.finish()?.into_inner())
}

pub fn write_file(db: &Database, path: impl AsRef<std::path::Path>) -> Result<()> {
    std::fs::write(path, write_bytes(db)?)?;
    Ok(())
}

fn write_cbor<W: Write + std::io::Seek, T: serde::Serialize>(
    zip: &mut ZipWriter<W>,
    options: SimpleFileOptions,
    path: &str,
    value: &T,
) -> Result<()> {
    let mut cbor = Vec::new();
    ciborium::into_writer(value, &mut cbor).map_err(|e| OdcError::Cbor(e.to_string()))?;
    let compressed = zstd::encode_all(&cbor[..], ZSTD_LEVEL)?;
    zip.start_file(path, options)?;
    zip.write_all(&compressed)?;
    Ok(())
}

/// Makes a string safe as a path component. Application ids and section names
/// come from other people's files, so they cannot be trusted to stay inside the
/// archive — `../` in an entry name is the classic zip-slip.
fn sanitise(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '_',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_components_cannot_escape_the_archive() {
        assert_eq!(sanitise("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(sanitise("org.opendraft.mep"), "org.opendraft.mep");
        assert_eq!(sanitise("a b\\c"), "a_b_c");
    }

    #[test]
    fn a_written_document_is_a_readable_zip_with_a_plain_manifest() {
        let db = Database::default();
        let bytes = write_bytes(&db).expect("writes");

        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("is a zip");
        let names: Vec<String> = zip.file_names().map(ToOwned::to_owned).collect();
        assert!(names.contains(&"manifest.json".to_owned()));
        assert!(names.contains(&"header.cbor".to_owned()));
        assert!(names.iter().any(|n| n.starts_with("tables/")));

        // The manifest must be readable without decompressing anything.
        let mut entry = zip.by_name("manifest.json").expect("manifest present");
        let mut text = String::new();
        std::io::Read::read_to_string(&mut entry, &mut text).expect("plain text");
        assert!(text.contains("opendraft-document"));
    }
}
