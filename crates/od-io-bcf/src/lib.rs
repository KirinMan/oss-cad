//! Writes a minimal, spec-conformant BCF 2.1 (`.bcfzip`) file.
//!
//! [BCF](https://github.com/buildingSMART/BCF-XML) (BIM Collaboration
//! Format) is how `docs/04-mep.md` §4.4 (F-108) says interference issues
//! should leave OpenDraft: a zip of per-issue "topic" folders, each holding
//! a `markup.bcf` XML file, that any BCF-reading coordination tool (Solibri,
//! BIMcollab, Revit's BCF plugin, …) can open directly. This crate covers
//! **export only** — reading a `.bcfzip` back in, and anything past a bare
//! `Topic` (viewpoints, snapshots, comments, `Header`/IFC linkage) is not
//! attempted. A viewpoint in particular would need a camera and a
//! visibility list against a real 3D scene, which is exactly what
//! `od-geom3d-lite` + a front-end 3D view (still unbuilt, per that crate's
//! own scope notes) would supply; until then a topic's `Description` is the
//! only place an issue's numbers (gap, location) are visible.
//!
//! Verified against the actual BCF 2.1 schemas
//! (`buildingSMART/BCF-XML@release_2_1/Schemas/{version,markup}.xsd`) and a
//! real conformance fixture from that repository
//! (`Test Cases/v2.1/Markup/MinimumInformation`), not just self-consistency:
//! `bcf.version`'s `Version` element and `markup.bcf`'s `Topic` child order
//! match those schemas exactly, and the output has been opened with
//! `unzip`/`xmllint` against the fetched XSDs during development.
//!
//! No dependencies — the ZIP container ([`zip`]) and its CRC-32 are
//! hand-rolled, the same way this workspace hand-rolls DXF, `.odc`, SXF and
//! glTF rather than reach for a crate per format.
//!
//! ```
//! use od_io_bcf::BcfTopic;
//!
//! let topic = BcfTopic {
//!     guid: od_io_bcf::guid_from_seed(0),
//!     title: "Hard clash: 0-10 x 0-1c".into(),
//!     topic_type: "Clash".into(),
//!     topic_status: "Open".into(),
//!     priority: "Critical".into(),
//!     description: "gap: -180.5mm at (2000, 0, 2950)".into(),
//!     creation_author: "OpenDraft".into(),
//!     creation_date: "2026-01-01T00:00:00Z".into(),
//! };
//! let bytes = od_io_bcf::to_bcfzip(std::slice::from_ref(&topic));
//! assert_eq!(&bytes[0..4], b"PK\x03\x04");
//! ```

mod zip;

use std::time::{SystemTime, UNIX_EPOCH};

/// One BCF topic — one interference issue, in this crate's intended use.
/// Every field is a plain string the caller has already formatted: this
/// crate knows the BCF file format, not what a clash-detection severity or
/// an actor's identity should look like, so it neither validates nor
/// interprets these beyond XML-escaping and folder-naming them.
#[derive(Debug, Clone)]
pub struct BcfTopic {
    /// A BCF `Guid`: lowercase or uppercase hex,
    /// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` (`markup.xsd`'s `Guid` simple
    /// type). Also the topic's folder name inside the archive, so it must be
    /// unique among the topics passed to [`to_bcfzip`] in the same call.
    /// [`guid_from_seed`] produces one deterministically.
    pub guid: String,
    pub title: String,
    /// Free text in BCF 2.1 (no `extensions.xml`-enforced vocabulary, unlike
    /// later BCF versions) — e.g. `"Clash"`, `"Duplicate"`.
    pub topic_type: String,
    /// Free text, e.g. `"Open"`.
    pub topic_status: String,
    /// Free text, e.g. `"Critical"`. Empty omits the (optional) `Priority`
    /// element entirely rather than writing one with no content.
    pub priority: String,
    /// Empty omits the (optional) `Description` element entirely.
    pub description: String,
    pub creation_author: String,
    /// ISO 8601 UTC, e.g. `"2026-01-01T00:00:00Z"`. [`iso8601_utc`] formats
    /// one from a [`SystemTime`].
    pub creation_date: String,
}

/// Builds a `.bcfzip`'s bytes: `bcf.version` at the archive root, then one
/// `{guid}/markup.bcf` per topic, in order.
#[must_use]
pub fn to_bcfzip(topics: &[BcfTopic]) -> Vec<u8> {
    let mut markup_files: Vec<(String, String)> = Vec::with_capacity(topics.len());
    for topic in topics {
        markup_files.push((format!("{}/markup.bcf", topic.guid), markup_xml(topic)));
    }

    let mut entries = vec![zip::Entry {
        name: "bcf.version",
        data: VERSION_XML.as_bytes(),
    }];
    entries.extend(markup_files.iter().map(|(name, xml)| zip::Entry {
        name,
        data: xml.as_bytes(),
    }));
    zip::write(&entries)
}

/// Writes [`to_bcfzip`]'s output to `path`.
///
/// # Errors
/// Whatever [`std::fs::write`] returns.
pub fn write_file(topics: &[BcfTopic], path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
    std::fs::write(path, to_bcfzip(topics))
}

const VERSION_XML: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
    "<Version VersionId=\"2.1\">\n",
    "  <DetailedVersion>2.1</DetailedVersion>\n",
    "</Version>\n",
);

/// `markup.bcf`'s content for one topic. Element order follows
/// `markup.xsd`'s `Topic` sequence (`ReferenceLink*, Title, Priority?,
/// Index?, Labels*, CreationDate, CreationAuthor, ModifiedDate?, …,
/// Description?, …`) — the elements this crate does not populate are
/// simply absent, which the schema's `minOccurs="0"` allows for all of them
/// except `Title`/`CreationDate`/`CreationAuthor`.
fn markup_xml(topic: &BcfTopic) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "    <Title>{}</Title>\n",
        xml_escape(&topic.title)
    ));
    if !topic.priority.is_empty() {
        body.push_str(&format!(
            "    <Priority>{}</Priority>\n",
            xml_escape(&topic.priority)
        ));
    }
    body.push_str(&format!(
        "    <CreationDate>{}</CreationDate>\n",
        xml_escape(&topic.creation_date)
    ));
    body.push_str(&format!(
        "    <CreationAuthor>{}</CreationAuthor>\n",
        xml_escape(&topic.creation_author)
    ));
    if !topic.description.is_empty() {
        body.push_str(&format!(
            "    <Description>{}</Description>\n",
            xml_escape(&topic.description)
        ));
    }

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <Markup>\n\
         \x20 <Topic Guid=\"{guid}\" TopicType=\"{topic_type}\" TopicStatus=\"{topic_status}\">\n\
         {body}\
         \x20 </Topic>\n\
         </Markup>\n",
        guid = xml_escape(&topic.guid),
        topic_type = xml_escape(&topic.topic_type),
        topic_status = xml_escape(&topic.topic_status),
    )
}

/// The five characters XML requires escaped in text content and
/// double-quoted attribute values — the only two contexts this crate ever
/// writes a caller-supplied string into.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// A syntactically valid BCF `Guid` (`markup.xsd`'s
/// `[a-fA-F0-9]{8}-...{4}-...{4}-...{4}-...{12}` pattern), derived
/// deterministically from `seed` via
/// [SplitMix64](https://prng.di.unimi.it/splitmix64.c) rather than sourced
/// from randomness: nothing in BCF 2.1 requires a `Guid` to be
/// unpredictable, only shaped like one, and a deterministic mapping means
/// re-running the same clash check twice produces byte-identical
/// `.bcfzip`s — useful for diffing exports in version control or a CI
/// baseline. Two consecutive seeds (e.g. `0, 1, 2, …`, the natural choice
/// for a caller numbering a list of issues) must not produce
/// near-identical `Guid`s sharing a folder-name prefix; SplitMix64's
/// avalanche property is what guarantees that.
#[must_use]
pub fn guid_from_seed(seed: u64) -> String {
    let hi = splitmix64(seed);
    let lo = splitmix64(hi);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (hi >> 32) as u32,
        ((hi >> 16) & 0xFFFF) as u16,
        (hi & 0xFFFF) as u16,
        ((lo >> 48) & 0xFFFF) as u16,
        lo & 0xFFFF_FFFF_FFFF,
    )
}

fn splitmix64(x: u64) -> u64 {
    let x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let z = x;
    let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `t` as ISO 8601 UTC (`YYYY-MM-DDTHH:MM:SSZ`), the form `markup.xsd`'s
/// `CreationDate` (`xs:dateTime`) expects. Implements Howard Hinnant's
/// [`civil_from_days`](http://howardhinnant.github.io/date_algorithms.html#civil_from_days)
/// rather than pulling in a date/time crate for one field: the algorithm is
/// public domain, bounded, and checkable against known reference dates
/// (this module's tests do exactly that), which is the same bar this
/// workspace already holds its other hand-rolled formats to.
#[must_use]
pub fn iso8601_utc(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    #[expect(
        clippy::cast_possible_wrap,
        reason = "a Unix day count fits in i64 for every representable SystemTime"
    )]
    let days = (secs / 86400) as i64;
    let time_of_day = secs % 86400;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        time_of_day / 3600,
        (time_of_day / 60) % 60,
        time_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Days since the Unix epoch (1970-01-01) -> a proleptic Gregorian
/// `(year, month, day)`. See [`iso8601_utc`]'s docs for provenance.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    #[expect(
        clippy::cast_sign_loss,
        reason = "doe is z's remainder within one 146097-day era, always non-negative by construction"
    )]
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    #[expect(
        clippy::cast_possible_wrap,
        reason = "yoe is bounded to [0, 399], far below i64::MAX"
    )]
    let year_of_era = yoe as i64;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "doy/mp are bounded to [0, 365]/[0, 11], far below u32::MAX"
    )]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "mp is bounded to [0, 11], so month is bounded to [1, 12]"
    )]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic() -> BcfTopic {
        BcfTopic {
            guid: guid_from_seed(0),
            title: "Hard clash".into(),
            topic_type: "Clash".into(),
            topic_status: "Open".into(),
            priority: "Critical".into(),
            description: "gap: -180.5mm".into(),
            creation_author: "OpenDraft".into(),
            creation_date: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn the_archive_starts_with_the_zip_local_file_signature() {
        let bytes = to_bcfzip(&[topic()]);
        assert_eq!(&bytes[0..4], b"PK\x03\x04");
    }

    #[test]
    fn markup_xml_holds_the_topic_in_schema_order() {
        let xml = markup_xml(&topic());
        let title_at = xml.find("<Title>").unwrap();
        let priority_at = xml.find("<Priority>").unwrap();
        let created_at = xml.find("<CreationDate>").unwrap();
        let author_at = xml.find("<CreationAuthor>").unwrap();
        let desc_at = xml.find("<Description>").unwrap();
        assert!(title_at < priority_at);
        assert!(priority_at < created_at);
        assert!(created_at < author_at);
        assert!(author_at < desc_at);
    }

    #[test]
    fn empty_priority_and_description_are_omitted_not_emitted_blank() {
        let mut t = topic();
        t.priority = String::new();
        t.description = String::new();
        let xml = markup_xml(&t);
        assert!(!xml.contains("<Priority>"));
        assert!(!xml.contains("<Description>"));
    }

    #[test]
    fn xml_special_characters_in_a_title_are_escaped() {
        let mut t = topic();
        t.title = "A <duct> & \"pipe\"".into();
        let xml = markup_xml(&t);
        assert!(xml.contains("A &lt;duct&gt; &amp; &quot;pipe&quot;"));
        assert!(!xml.contains("<duct>"));
    }

    #[test]
    fn guid_from_seed_has_the_bcf_shape() {
        for seed in 0..10u64 {
            let g = guid_from_seed(seed);
            let parts: Vec<&str> = g.split('-').collect();
            assert_eq!(
                parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
                [8, 4, 4, 4, 12]
            );
            assert!(g.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        }
    }

    #[test]
    fn consecutive_seeds_do_not_collide_or_share_a_prefix() {
        let guids: Vec<String> = (0..8).map(guid_from_seed).collect();
        for (i, a) in guids.iter().enumerate() {
            for b in &guids[i + 1..] {
                assert_ne!(a, b);
                assert_ne!(
                    &a[..8],
                    &b[..8],
                    "sequential seeds must not share a folder prefix"
                );
            }
        }
    }

    #[test]
    fn iso8601_matches_known_reference_dates() {
        assert_eq!(iso8601_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            iso8601_utc(UNIX_EPOCH + std::time::Duration::from_secs(946_684_800)),
            "2000-01-01T00:00:00Z"
        );
        // 2026-01-01T00:00:00Z, cross-checked against `date -u -r 1767225600`.
        assert_eq!(
            iso8601_utc(UNIX_EPOCH + std::time::Duration::from_secs(1_767_225_600)),
            "2026-01-01T00:00:00Z"
        );
        assert_eq!(
            iso8601_utc(UNIX_EPOCH + std::time::Duration::from_secs(86_399)),
            "1970-01-01T23:59:59Z"
        );
    }

    #[test]
    #[ignore = "one-off external verification harness, see PR description"]
    fn write_smoke_file_for_external_verification() {
        let topics = vec![
            BcfTopic {
                guid: guid_from_seed(0),
                title: "Hard clash: 0-10 x 0-1c".into(),
                topic_type: "Clash".into(),
                topic_status: "Open".into(),
                priority: "Critical".into(),
                description: "gap: -180.5mm at (2000, 0, 2950)".into(),
                creation_author: "OpenDraft".into(),
                creation_date: iso8601_utc(SystemTime::now()),
            },
            BcfTopic {
                guid: guid_from_seed(1),
                title: "Clearance <34mm>: 0-10 x 0-11".into(),
                topic_type: "Clash".into(),
                topic_status: "Open".into(),
                priority: "Normal".into(),
                description: String::new(),
                creation_author: "OpenDraft".into(),
                creation_date: iso8601_utc(SystemTime::now()),
            },
        ];
        write_file(&topics, "/tmp/od-io-bcf-smoke.bcfzip").expect("writes");
    }

    #[test]
    fn no_topics_still_produces_an_archive_holding_just_bcf_version() {
        let bytes = to_bcfzip(&[]);
        assert_eq!(&bytes[0..4], b"PK\x03\x04", "bcf.version is always written");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("bcf.version"));
        assert!(!text.contains("markup.bcf"));
    }
}
