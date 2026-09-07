//! Reading a `.odc` container, honouring the compatibility contract.

use crate::manifest::{FORMAT, Level, Manifest, SCHEMA_VERSION};
use crate::{OdcError, Result, StoredHeader};
use od_core::{
    AppId, Database, DatabaseSnapshot, Object, PreservedBlob, SymbolTables, XDataSchema,
};
use std::io::{Cursor, Read};

/// What happened during a read that the caller should know about but that did
/// not stop the file opening.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReadOutcome {
    /// Features this build does not understand, kept byte-for-byte.
    pub preserved_features: Vec<String>,
    /// Features this build does not understand and was told it may drop.
    pub ignored_features: Vec<String>,
    /// Entries no feature claimed. Kept anyway — see the note in the reader.
    pub undeclared_entries: Vec<String>,
    pub warnings: Vec<String>,
}

/// Reads a `.odc` document.
pub fn read_bytes(bytes: &[u8]) -> Result<(Database, ReadOutcome)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut outcome = ReadOutcome::default();

    let manifest: Manifest = {
        let mut entry = zip
            .by_name("manifest.json")
            .map_err(|_| OdcError::MissingEntry("manifest.json"))?;
        let mut text = String::new();
        entry.read_to_string(&mut text)?;
        serde_json::from_str(&text)?
    };

    if manifest.format != FORMAT {
        return Err(OdcError::NotAnOdcFile {
            found: manifest.format,
        });
    }
    check_schema_version(&manifest.schema_version)?;

    // The contract, enforced before a single byte of drawing data is touched:
    // a file that needs something this build does not have is refused, rather
    // than opened and quietly stripped of it on the next save.
    let blocked = manifest.unsupported_required();
    if !blocked.is_empty() {
        return Err(OdcError::UnsupportedFeatures {
            features: blocked.iter().map(|f| f.name.clone()).collect(),
            generator: manifest.generator.clone(),
        });
    }

    let paths: Vec<String> = zip.file_names().map(ToOwned::to_owned).collect();
    let mut header: Option<StoredHeader> = None;
    let mut tables = SymbolTables::default();
    let mut chunks: Vec<(String, Vec<Object>)> = Vec::new();
    let mut schemas: Vec<(AppId, XDataSchema)> = Vec::new();
    let mut preserved: Vec<PreservedBlob> = Vec::new();

    for path in paths {
        if path == "manifest.json" || path.ends_with('/') {
            continue;
        }
        let raw = read_entry(&mut zip, &path)?;

        match path.as_str() {
            "header.cbor" => header = Some(decode(&raw, &path)?),
            "tables/layers.cbor" => tables.layers = decode(&raw, &path)?,
            "tables/linetypes.cbor" => tables.linetypes = decode(&raw, &path)?,
            "tables/text_styles.cbor" => tables.text_styles = decode(&raw, &path)?,
            "tables/dim_styles.cbor" => tables.dim_styles = decode(&raw, &path)?,
            "tables/blocks.cbor" => tables.blocks = decode(&raw, &path)?,
            "tables/levels.cbor" => tables.levels = decode(&raw, &path)?,
            "tables/grids.cbor" => tables.grids = decode(&raw, &path)?,

            p if p.starts_with("objects/") => {
                chunks.push((path.clone(), decode(&raw, &path)?));
            }

            p if p.starts_with("schemas/") => match serde_json::from_slice::<XDataSchema>(&raw) {
                Ok(schema) => schemas.push((schema.app_id.clone(), schema)),
                Err(e) => {
                    // A malformed schema costs the meaning of some attributes,
                    // not the drawing. Report it and keep going.
                    outcome.warnings.push(format!("{path}: {e}"));
                    preserved.push(PreservedBlob {
                        source: "odc".into(),
                        section: path.clone(),
                        payload: raw,
                    });
                }
            },

            p if p.starts_with("preserved/") => {
                let (source, section) = split_preserved(p);
                preserved.push(PreservedBlob {
                    source,
                    section,
                    payload: raw,
                });
            }

            other => match manifest.owner_of(other) {
                Some(feature) if feature.level == Level::OptionalIgnore => {
                    note(&mut outcome.ignored_features, &feature.name);
                }
                Some(feature) => {
                    note(&mut outcome.preserved_features, &feature.name);
                    preserved.push(PreservedBlob {
                        source: "odc".into(),
                        section: other.to_owned(),
                        payload: raw,
                    });
                }
                None => {
                    // Nothing claims it. Keeping it is the conservative choice:
                    // the cost of carrying bytes we do not understand is a
                    // slightly larger file, and the cost of dropping them is
                    // someone's data.
                    outcome.undeclared_entries.push(other.to_owned());
                    preserved.push(PreservedBlob {
                        source: "odc".into(),
                        section: other.to_owned(),
                        payload: raw,
                    });
                }
            },
        }
    }

    let header = header.ok_or(OdcError::MissingEntry("header.cbor"))?;

    // Chunks are numbered, and the numbering is the draw order.
    chunks.sort_by(|a, b| a.0.cmp(&b.0));
    let objects: Vec<Object> = chunks.into_iter().flat_map(|(_, o)| o).collect();

    let snapshot = DatabaseSnapshot {
        header: header.header,
        tables,
        objects,
        ids: header.ids,
        schemas: schemas.into_iter().collect(),
        named_dict: header.named_dict,
        model_space: header.model_space,
        preserved,
    };

    let db = Database::from_snapshot(snapshot);
    for problem in db.validate() {
        outcome.warnings.push(problem.to_string());
    }

    Ok((db, outcome))
}

pub fn read_file(path: impl AsRef<std::path::Path>) -> Result<(Database, ReadOutcome)> {
    read_bytes(&std::fs::read(path)?)
}

/// Only the major version gates compatibility. A minor bump adds entries, and
/// what to do with an entry a reader does not recognise is already answered by
/// its feature level — so refusing the whole file for a minor difference would
/// be the compatibility contract failing at its own job.
fn check_schema_version(found: &str) -> Result<()> {
    let major = |v: &str| v.split('.').next().unwrap_or_default().to_owned();
    if major(found) == major(SCHEMA_VERSION) {
        Ok(())
    } else {
        Err(OdcError::UnsupportedSchemaVersion {
            found: found.to_owned(),
            supported: SCHEMA_VERSION.to_owned(),
        })
    }
}

fn read_entry(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, path: &str) -> Result<Vec<u8>> {
    let mut entry = zip
        .by_name(path)
        .map_err(|_| OdcError::Corrupt(format!("entry `{path}` vanished mid-read")))?;
    let mut buf = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
    entry.read_to_end(&mut buf)?;
    Ok(buf)
}

fn decode<T: serde::de::DeserializeOwned>(raw: &[u8], path: &str) -> Result<T> {
    let cbor = zstd::decode_all(raw)
        .map_err(|e| OdcError::Corrupt(format!("{path}: decompression failed: {e}")))?;
    ciborium::from_reader(&cbor[..]).map_err(|e| OdcError::Cbor(format!("{path}: {e}")))
}

/// `preserved/<source>/<index>-<section>` back into its parts.
fn split_preserved(path: &str) -> (String, String) {
    let rest = path.trim_start_matches("preserved/");
    match rest.split_once('/') {
        Some((source, name)) => {
            let section = name.split_once('-').map_or(name, |(_, s)| s);
            (source.to_owned(), section.to_owned())
        }
        None => ("odc".to_owned(), path.to_owned()),
    }
}

fn note(list: &mut Vec<String>, name: &str) {
    if !list.iter().any(|n| n == name) {
        list.push(name.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minor_versions_are_compatible_major_ones_are_not() {
        assert!(check_schema_version("0.1.0").is_ok());
        assert!(check_schema_version("0.9.3").is_ok());
        assert!(check_schema_version("1.0.0").is_err());
    }

    #[test]
    fn preserved_paths_split_back_into_source_and_section() {
        assert_eq!(
            split_preserved("preserved/dxf/0003-CLASSES"),
            ("dxf".to_owned(), "CLASSES".to_owned())
        );
        assert_eq!(
            split_preserved("preserved/dxf/0000-OBJECTS"),
            ("dxf".to_owned(), "OBJECTS".to_owned())
        );
    }

    #[test]
    fn a_file_that_is_not_ours_is_refused_by_name() {
        // A valid zip whose manifest claims another format.
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "manifest.json",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .expect("start");
        std::io::Write::write_all(
            &mut zip,
            br#"{"format":"something-else","schema_version":"0.1.0","generator":"x","features":[]}"#,
        )
        .expect("write");
        let bytes = zip.finish().expect("finish").into_inner();

        match read_bytes(&bytes) {
            Err(OdcError::NotAnOdcFile { found }) => assert_eq!(found, "something-else"),
            other => panic!("expected NotAnOdcFile, got {other:?}"),
        }
    }

    #[test]
    fn something_that_is_not_a_zip_fails_cleanly() {
        assert!(read_bytes(b"not a zip at all").is_err());
    }
}
