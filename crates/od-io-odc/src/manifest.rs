//! The manifest, and the compatibility contract it encodes.
//!
//! This is the part of the format that matters most, and it is not about
//! geometry. The damage that actually happens in practice is: someone opens a
//! drawing in an older build, that build silently drops what it did not
//! understand, and the file is saved back with the loss baked in. Nobody sees
//! an error; the information is simply gone, and the person who notices is
//! three weeks downstream.
//!
//! So every part of a document declares how a reader that does not understand
//! it must behave (`docs/03-data-model.md` §5.2):
//!
//! - `required` — refuse to open. Better a build that says "I am too old for
//!   this file" than one that opens it and quietly destroys half of it.
//! - `optional-preserve` — keep the bytes, write them back on save.
//! - `optional-ignore` — a cache; regenerate or drop it freely.

use serde::{Deserialize, Serialize};

/// The container's identifying string. A file whose manifest says anything else
/// is not ours, whatever its extension claims.
pub const FORMAT: &str = "opendraft-document";

/// The container layout this build writes and understands.
///
/// Bumped only when the *layout* changes — where things live and how they are
/// framed. A new kind of content gets a new feature entry instead, which is what
/// lets an older build keep working.
pub const SCHEMA_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    /// A reader that does not understand this must refuse the file.
    Required,
    /// Unknown to this reader: keep the bytes and write them back unchanged.
    OptionalPreserve,
    /// Derived data. Safe to ignore, safe to drop.
    OptionalIgnore,
}

/// One kind of content in the container, and where it lives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    /// Namespaced, e.g. `core.objects`, `mep.routes`, `index.rtree`.
    pub name: String,
    pub level: Level,
    /// Entry path prefixes this feature owns, so a reader can tell which files
    /// belong to a feature it has never heard of.
    pub paths: Vec<String>,
}

impl Feature {
    #[must_use]
    pub fn new(name: &str, level: Level, paths: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            level,
            paths: paths.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    #[must_use]
    pub fn owns(&self, path: &str) -> bool {
        self.paths.iter().any(|prefix| path.starts_with(prefix))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub schema_version: String,
    /// What wrote this file. Diagnostic only — never branch on it, or the
    /// format acquires a compatibility matrix nobody can test.
    pub generator: String,
    pub features: Vec<Feature>,
}

impl Manifest {
    /// The manifest this build writes for a document it produced itself.
    #[must_use]
    pub fn current() -> Self {
        Self {
            format: FORMAT.to_owned(),
            schema_version: SCHEMA_VERSION.to_owned(),
            generator: format!("OpenDraft {}", env!("CARGO_PKG_VERSION")),
            features: vec![
                Feature::new("core.header", Level::Required, &["header.cbor"]),
                Feature::new("core.tables", Level::Required, &["tables/"]),
                Feature::new("core.objects", Level::Required, &["objects/"]),
                // A schema is what keeps an attribute meaningful without the
                // plugin that wrote it, so losing one is losing the meaning of
                // the data it describes — but a reader can still show the raw
                // values, so this stops short of `required`.
                Feature::new("core.schemas", Level::OptionalPreserve, &["schemas/"]),
                Feature::new("core.preserved", Level::OptionalPreserve, &["preserved/"]),
                Feature::new("core.resources", Level::OptionalPreserve, &["resources/"]),
                Feature::new("index.spatial", Level::OptionalIgnore, &["index/"]),
                Feature::new("derived.geometry", Level::OptionalIgnore, &["derived/"]),
                Feature::new("history.log", Level::OptionalPreserve, &["history/"]),
            ],
        }
    }

    #[must_use]
    pub fn feature(&self, name: &str) -> Option<&Feature> {
        self.features.iter().find(|f| f.name == name)
    }

    /// The feature owning `path`, if any declares it.
    #[must_use]
    pub fn owner_of(&self, path: &str) -> Option<&Feature> {
        self.features.iter().find(|f| f.owns(path))
    }

    /// Features this build has never heard of that it may not ignore.
    #[must_use]
    pub fn unsupported_required(&self) -> Vec<&Feature> {
        self.features
            .iter()
            .filter(|f| f.level == Level::Required && !is_known(&f.name))
            .collect()
    }
}

/// Feature names this build understands. Anything outside this list is treated
/// according to its declared level.
fn is_known(name: &str) -> bool {
    matches!(
        name,
        "core.header"
            | "core.tables"
            | "core.objects"
            | "core.schemas"
            | "core.preserved"
            | "core.resources"
            | "index.spatial"
            | "derived.geometry"
            | "history.log"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_manifest_declares_every_part_we_write() {
        let m = Manifest::current();
        assert_eq!(m.format, FORMAT);
        for path in ["header.cbor", "tables/layers.cbor", "objects/0000.chunk"] {
            let owner = m
                .owner_of(path)
                .unwrap_or_else(|| panic!("{path} is unowned"));
            assert_eq!(
                owner.level,
                Level::Required,
                "{path} carries the drawing itself"
            );
        }
        assert!(m.unsupported_required().is_empty());
    }

    #[test]
    fn a_future_required_feature_is_reported() {
        let mut m = Manifest::current();
        m.features
            .push(Feature::new("mep.routes", Level::Required, &["mep/"]));

        let blocked = m.unsupported_required();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].name, "mep.routes");
    }

    #[test]
    fn a_future_optional_feature_does_not_block_opening() {
        let mut m = Manifest::current();
        m.features.push(Feature::new(
            "annotation.markup",
            Level::OptionalPreserve,
            &["markup/"],
        ));
        m.features.push(Feature::new(
            "index.rtree3d",
            Level::OptionalIgnore,
            &["idx3/"],
        ));

        assert!(m.unsupported_required().is_empty());
        assert_eq!(
            m.owner_of("markup/sheet1.json").map(|f| f.level),
            Some(Level::OptionalPreserve)
        );
    }

    #[test]
    fn path_ownership_matches_on_prefix_only() {
        let f = Feature::new("core.tables", Level::Required, &["tables/"]);
        assert!(f.owns("tables/layers.cbor"));
        assert!(!f.owns("objects/0000.chunk"));
        // A name that merely starts with the same letters is not ownership.
        assert!(!f.owns("tablesaw.json"));
    }
}
