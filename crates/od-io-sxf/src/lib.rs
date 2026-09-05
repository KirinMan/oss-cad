//! SXF import and export — Japan's public-works CAD exchange format
//! (SCADEC/OCF, `docs/06-roadmap.md`).
//!
//! **SFC only.** SXF has two physical encodings: SXF(P21), an ISO 10303-21
//! (STEP) physical file and the format `CAD製図基準(案)` actually mandates for
//! official 電子納品 delivery, and SXF(SFC), a simpler in-house text format
//! meant for informal exchange (`打合せ段階`). This crate implements SFC only.
//! P21 needs a general STEP physical-file parser plus a working subset of the
//! AP202-derived EXPRESS schema SXF layers on top of it — real, substantial
//! work of its own, not a corner that was cut, and not attempted here without
//! the normative spec text to build it against. Writing something
//! plausible-but-wrong for the format actually used in official submissions
//! would be worse than not writing it.
//!
//! The exact record grammar and the layer/colour/linetype/width numbering
//! rules below come from two sources cross-checked against each other: a
//! real worked example in NILIM's own CAD製図基準 guidance
//! (`ks0403012.pdf` §6.1, a Japanese government technical document) and
//! `SfcHelper` (MIT-licensed, <https://github.com/JinkiKeikaku/SfcHelper>),
//! an independent open-source SFC reader/writer — not the SCADEC/OCF
//! normative specification itself, which was not available while writing
//! this. Round-trip self-consistency (write this crate's own output, read it
//! back, compare) is what the tests here actually verify; fidelity against
//! real SFC files from commercial CAD software has not been checked against
//! a real sample file, because none was available either.
//!
//! Encoding is treated as UTF-8 (lossy on read), matching `od-io-dxf`'s own
//! choice for legacy files — real SFC files are conventionally Shift-JIS, so
//! this is a known, existing-precedent simplification, not a new one.
//!
//! An unrecognised feature record — 複合図形 (blocks/composite figures),
//! 寸法 (structured dimensions), クロソイド曲線, 既定義シンボル, and anything
//! else this build does not model — becomes [`od_core::Geometry::Unsupported`]
//! with the record's own text as its payload, so it survives a load-and-save
//! cycle rather than vanishing (rule 2, `CLAUDE.md`).
//!
//! ```
//! // The line_feature record itself is verbatim from NILIM's
//! // ks0403012.pdf §6.1 (図 6-3, 線分におけるSFCのファイル内容), the one
//! // real-world SFC record this crate's grammar was checked directly
//! // against; the layer/colour/linetype/width records around it are the
//! // minimum a real file needs so `'1'`/`'8'`/`'1'`/`'1'` resolve to
//! // something (SFC never predeclares its 16 predefined colours or 15
//! // predefined linetypes — a file uses a name only by declaring it first).
//! let sfc = "\
//! /*SXF\n#10 = layer_feature(\\'0\\',\\'1\\')\nSXF*/\n\
//! /*SXF\n#20 = pre_defined_colour_feature(\\'white\\')\nSXF*/\n\
//! /*SXF\n#30 = pre_defined_font_feature(\\'continuous\\')\nSXF*/\n\
//! /*SXF\n#35 = width_feature('0.13')\nSXF*/\n\
//! /*SXF\n#40 = line_feature('1','8','1','1','102.718015','46.660696','185.557783','149.775931')\nSXF*/\
//! ";
//! let (db, outcome) = od_io_sxf::read_str(sfc);
//! assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
//! assert_eq!(db.entities().count(), 1);
//!
//! let written = od_io_sxf::write_string(&db);
//! let (again, _) = od_io_sxf::read_str(&written);
//! assert_eq!(again.entities().count(), 1);
//! ```

pub mod read;
mod record;
mod table;
pub mod write;

pub use read::{ReadOutcome, Warning, read_str};
pub use write::write_string;

#[derive(Debug, thiserror::Error)]
pub enum SxfError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, SxfError>;

/// Reads an SFC file from disk.
pub fn read_file(path: impl AsRef<std::path::Path>) -> Result<(od_core::Database, ReadOutcome)> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(read_str(&text))
}

/// Writes an SFC file to disk.
pub fn write_file(db: &od_core::Database, path: impl AsRef<std::path::Path>) -> Result<()> {
    std::fs::write(path, write_string(db))?;
    Ok(())
}
