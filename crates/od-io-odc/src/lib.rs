//! The OpenDraft document container, `.odc`.
//!
//! DXF is how drawings are exchanged; this is how they are *kept*. The
//! difference matters because the document model holds things DXF has no way to
//! express — schema'd extension data, storeys and grids, custom domain objects —
//! and a format that cannot hold them means the work is lost every time the file
//! is saved.
//!
//! A `.odc` file is a ZIP of separately-readable parts (`docs/03-data-model.md`
//! §5). Entries are stored uncompressed and their payloads compressed
//! individually with zstd, so a tool can read the layer table of a large drawing
//! without inflating its geometry.
//!
//! ```
//! use od_core::{ActorId, Database, Entity, Geometry, Point3};
//!
//! let mut db = Database::new(ActorId::SYSTEM);
//! let layer = db.ensure_layer("M-DUCT-SA");
//! let space = db.model_space();
//! db.insert_entity(Entity::new(layer, space, Geometry::Line {
//!     a: Point3::ORIGIN,
//!     b: Point3::new(5000.0, 0.0, 0.0),
//! }))?;
//!
//! let bytes = od_io_odc::write_bytes(&db)?;
//! let (back, outcome) = od_io_odc::read_bytes(&bytes)?;
//!
//! assert_eq!(back.entities().count(), 1);
//! assert!(back.tables.layers.by_name("m-duct-sa").is_some());
//! assert!(outcome.warnings.is_empty());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The part of this that earns its complexity is [`manifest`]: every kind of
//! content declares what a reader that does not understand it must do. That is
//! what stops an older build from opening a newer file and silently destroying
//! the parts it did not recognise — the failure mode that makes version drift
//! between firms a real cost rather than an inconvenience.

pub mod manifest;
pub mod read;
pub mod write;

pub use manifest::{Feature, Level, Manifest};
pub use read::{ReadOutcome, read_bytes, read_file};
pub use write::{write_bytes, write_file};

use od_core::{HeaderVars, IdGenerator, ObjectId};
use serde::{Deserialize, Serialize};

/// Objects per chunk. Large enough that a small drawing is one entry, small
/// enough that a viewer can show something before the whole file is read.
const CHUNK_OBJECTS: usize = 1000;

/// zstd level 3 — the default, and the right trade here: drawings are saved
/// far more often than they are shipped, so save latency matters more than the
/// last few percent of size.
const ZSTD_LEVEL: i32 = 3;

/// `header.cbor`: the document-level state that is not a table and not an
/// object. The id generator travels with it, because a reload that reissues a
/// live id produces two objects claiming the same identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredHeader {
    pub header: HeaderVars,
    pub ids: IdGenerator,
    pub named_dict: ObjectId,
    pub model_space: ObjectId,
}

#[derive(Debug, thiserror::Error)]
pub enum OdcError {
    #[error("this is not an OpenDraft document (its manifest says `{found}`)")]
    NotAnOdcFile { found: String },

    #[error(
        "this document uses container layout {found}; this build understands {supported}. \
         Upgrade OpenDraft to open it."
    )]
    UnsupportedSchemaVersion { found: String, supported: String },

    #[error(
        "this document requires {features:?}, which this build does not support. \
         It was written by {generator}; upgrade to open it. \
         Opening it here would discard that data on the next save."
    )]
    UnsupportedFeatures {
        features: Vec<String>,
        generator: String,
    },

    #[error("the document is missing `{0}`")]
    MissingEntry(&'static str),

    #[error("the document is damaged: {0}")]
    Corrupt(String),

    #[error("cbor: {0}")]
    Cbor(String),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, OdcError>;
