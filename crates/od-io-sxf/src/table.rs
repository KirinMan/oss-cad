//! Code tables: SXF's layer/colour/linetype/width/font symbols are all
//! referenced by small integers rather than by name, assigned by where they
//! land among a fixed set of "slots" — the same scheme `SfcHelper`'s
//! `SfcTable` implements, cross-checked against NILIM's own guidance
//! document for the predefined linetype/colour name tables (see crate docs
//! for both sources).
//!
//! - Layers: sequential, starting at 1, in file declaration order.
//! - Predefined colours/linetypes: matched by *name* against a fixed
//!   16-entry (colour) or 15-entry (linetype) table, so a predefined
//!   colour's code is its position in that table — not file order.
//! - User-defined colours/linetypes: sequential, starting at 17.
//! - Predefined widths: matched by nearest value against a fixed 9-entry
//!   table; anything else is a user-defined width, sequential starting at 11.
//! - Fonts: sequential, starting at 1 — there is no predefined tier.

use od_core::Color;

/// SXF's 16 predefined colour names, in table order — code = index + 1.
pub(crate) const PREDEFINED_COLORS: [(&str, u8, u8, u8); 16] = [
    ("black", 0, 0, 0),
    ("red", 255, 0, 0),
    ("green", 0, 255, 0),
    ("blue", 0, 0, 255),
    ("yellow", 255, 255, 0),
    ("magenta", 255, 0, 255),
    ("cyan", 0, 255, 255),
    ("white", 255, 255, 255),
    ("deeppink", 192, 0, 128),
    ("brown", 192, 128, 64),
    ("orange", 255, 128, 0),
    ("lightgreen", 128, 192, 128),
    ("lightblue", 0, 128, 255),
    ("lavender", 128, 64, 255),
    ("lightgray", 192, 192, 192),
    ("darkgray", 128, 128, 128),
];

/// SXF's 15 predefined linetype names, in table order — code = index + 1.
/// The pitch arrays are JIS Z 8312:1999 (ISO 128-20)'s own recommended
/// pitches, from NILIM's `ks0403012.pdf` 表6-3 — SXF's positive-only,
/// alternating draw/gap-by-position convention, not `od_core::LineType`'s
/// sign-encoded one (see `to_od_pattern`/`from_od_pattern` below).
pub(crate) const PREDEFINED_LINETYPES: [(&str, &[f64]); 15] = [
    ("continuous", &[]),
    ("dashed", &[6.0, 1.5]),
    ("dashed spaced", &[6.0, 6.0]),
    ("long dashed dotted", &[12.0, 1.5, 0.25, 1.5]),
    (
        "long dashed double-dotted",
        &[12.0, 1.5, 0.25, 1.5, 0.25, 1.5],
    ),
    (
        "long dashed triplicate-dotted",
        &[12.0, 1.5, 0.25, 1.5, 0.25, 1.5, 0.25, 1.5],
    ),
    ("dotted", &[0.25, 1.5]),
    ("chain", &[12.0, 1.5, 3.5, 1.5]),
    ("chain double dash", &[12.0, 1.5, 3.5, 1.5, 3.5, 1.5]),
    ("dashed dotted", &[6.0, 1.5, 0.25, 1.5]),
    ("double-dashed dotted", &[6.0, 1.5, 6.0, 1.5, 0.25, 1.5]),
    ("dashed double-dotted", &[6.0, 1.5, 0.25, 1.5, 0.25, 1.5]),
    (
        "double-dashed double-dotted",
        &[6.0, 1.5, 6.0, 1.5, 0.25, 1.5, 0.25, 1.5],
    ),
    (
        "dashed triplicate-dotted",
        &[6.0, 1.5, 0.25, 1.5, 0.25, 1.5, 0.25, 1.5],
    ),
    (
        "double-dashed triplicate-dotted",
        &[6.0, 1.5, 6.0, 1.5, 0.25, 1.5, 0.25, 1.5, 0.25, 1.5],
    ),
];

/// SXF's 9 predefined line widths, in millimetres — code = index + 1.
pub(crate) const PREDEFINED_WIDTHS: [f64; 9] = [0.13, 0.18, 0.25, 0.35, 0.5, 0.7, 1.0, 1.4, 2.0];

pub(crate) const FIRST_USER_COLOR: u32 = 17;
pub(crate) const FIRST_USER_LINETYPE: u32 = 17;
pub(crate) const FIRST_USER_WIDTH: u32 = 11;

/// Converts SXF's `[draw, gap, draw, gap, ...]` pitch array (all positive,
/// draw/gap told apart by position) into `od_core::LineType::pattern`'s own
/// sign-encoded convention (positive draws, negative gaps) — the same
/// convention `od-io-dxf` already writes for its own linetype table.
#[must_use]
pub(crate) fn to_od_pattern(pitch: &[f64]) -> Vec<f64> {
    pitch
        .iter()
        .enumerate()
        .map(|(i, &v)| if i % 2 == 0 { v.abs() } else { -v.abs() })
        .collect()
}

/// The inverse of [`to_od_pattern`].
#[must_use]
pub(crate) fn from_od_pattern(pattern: &[f64]) -> Vec<f64> {
    pattern.iter().map(|v| v.abs()).collect()
}

/// A colour code's resolved value, resolving predefined names against
/// [`PREDEFINED_COLORS`] and leaving RGB triples as given.
#[must_use]
pub(crate) fn predefined_color_by_name(name: &str) -> Option<Color> {
    PREDEFINED_COLORS
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|&(_, r, g, b)| Color::Rgb { r, g, b })
}

#[must_use]
pub(crate) fn predefined_linetype_by_name(name: &str) -> Option<&'static [f64]> {
    PREDEFINED_LINETYPES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, pitch)| *pitch)
}
