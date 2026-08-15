//! DXF import and export.
//!
//! DXF is written in-house rather than delegated (`docs/05-interop-license.md`):
//! it is the interchange path everything else rests on, the specification is
//! public, and the project cannot put the quality of its most important format
//! in someone else's hands.
//!
//! ```
//! # fn main() -> Result<(), od_io_dxf::DxfError> {
//! let dxf = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n11\n100.0\n21\n0.0\n0\nENDSEC\n0\nEOF\n";
//! let (db, outcome) = od_io_dxf::read_str(dxf)?;
//! assert_eq!(db.entities().count(), 1);
//! assert!(outcome.warnings.is_empty());
//!
//! let written = od_io_dxf::write_string(&db);
//! let (again, _) = od_io_dxf::read_str(&written)?;
//! assert_eq!(again.entities().count(), 1);
//! # Ok(())
//! # }
//! ```

pub mod pair;
pub mod read;
pub mod write;

pub use pair::Pair;
pub use read::{ReadOutcome, Warning, read_str};
pub use write::write_string;

#[derive(Debug, thiserror::Error)]
pub enum DxfError {
    #[error("line {line}: `{text}` is not a group code")]
    BadGroupCode { line: usize, text: String },

    #[error("line {line}: group code has no value")]
    TruncatedPair { line: usize },

    #[error("{entity} is missing required group code {code}")]
    MissingCode { entity: &'static str, code: i32 },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, DxfError>;

/// Reads a DXF file from disk.
pub fn read_file(path: impl AsRef<std::path::Path>) -> Result<(od_core::Database, ReadOutcome)> {
    let bytes = std::fs::read(path)?;
    // DXF predates UTF-8 and files from CJK versions of older applications are
    // often in a local codepage. Lossy decoding keeps the drawing usable and
    // confines the damage to the strings that were already unreadable, rather
    // than refusing the whole file.
    let text = String::from_utf8_lossy(&bytes);
    read_str(&text)
}

/// Writes a DXF file to disk.
pub fn write_file(db: &od_core::Database, path: impl AsRef<std::path::Path>) -> Result<()> {
    std::fs::write(path, write_string(db))?;
    Ok(())
}
