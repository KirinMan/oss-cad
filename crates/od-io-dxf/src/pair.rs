//! DXF group code/value pairs.
//!
//! An ASCII DXF file is nothing but a stream of these: a numeric group code on
//! one line, its value on the next. Everything else — sections, tables,
//! entities — is a convention layered on top of that stream.
//!
//! Values are kept as text and converted on demand. That is not laziness: a
//! code this reader does not recognise still has to survive to the writer
//! byte-for-byte, and parsing it into a typed value it does not understand is
//! how information gets rounded off in transit.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub code: i32,
    pub value: String,
}

impl Pair {
    pub fn new(code: i32, value: impl Into<String>) -> Self {
        Self {
            code,
            value: value.into(),
        }
    }

    /// Value as a float, or `None` when it does not parse.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        self.value.trim().parse::<f64>().ok()
    }

    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        let t = self.value.trim();
        t.parse::<i64>()
            .ok()
            // Some writers emit 16-bit flags as floats ("1.0"); accept those
            // rather than dropping the flag. Values outside i64 are not flags,
            // so they are rejected rather than saturated into a plausible one.
            .or_else(|| {
                let f = t.parse::<f64>().ok()?;
                (f.is_finite() && f.abs() < 9.0e18).then(|| {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "range checked immediately above"
                    )]
                    let v = f.trunc() as i64;
                    v
                })
            })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Group codes 0..=9 carry strings; 10..=18, 20..=28, 30..=38 carry the X, Y
    /// and Z of a point; 210..=212 carry an extrusion direction.
    #[must_use]
    pub fn is_point_x(&self) -> bool {
        (10..=18).contains(&self.code) || self.code == 210
    }
}

impl fmt::Display for Pair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.value)
    }
}

/// Formats a float the way DXF expects: plain decimal, enough digits to survive
/// a round trip, and no trailing noise.
///
/// `{:?}` on `f64` gives the shortest representation that round-trips exactly,
/// which is what this needs — but it emits exponent form for extreme values,
/// and some readers reject that. Coordinates are millimetres within ±10^9, so
/// the fixed-notation branch covers everything real drawings contain.
#[must_use]
pub fn format_f64(v: f64) -> String {
    if !v.is_finite() {
        return "0.0".to_owned();
    }
    if v == 0.0 {
        // Also normalises -0.0, which some readers show as "-0".
        return "0.0".to_owned();
    }
    let a = v.abs();
    if !(1e-10..1e15).contains(&a) {
        return format!("{v:e}");
    }
    let s = format!("{v:?}");
    if s.contains('e') || s.contains('E') {
        format!("{v:.10}")
    } else if s.contains('.') {
        s
    } else {
        format!("{s}.0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_round_trip_through_their_text_form() {
        for v in [
            0.0,
            1.0,
            -1.0,
            3600.0,
            0.1,
            1.0 / 3.0,
            123_456_789.123_456_78,
            -0.000_001,
            std::f64::consts::PI,
        ] {
            let s = format_f64(v);
            let back: f64 = s.parse().expect("parses back");
            assert!(
                (back - v).abs() <= f64::EPSILON * v.abs().max(1.0),
                "{v} became {s} became {back}"
            );
        }
    }

    #[test]
    fn whole_numbers_keep_a_decimal_point() {
        assert_eq!(format_f64(1.0), "1.0");
        assert_eq!(format_f64(-3600.0), "-3600.0");
    }

    #[test]
    fn negative_zero_is_normalised() {
        assert_eq!(format_f64(-0.0), "0.0");
        assert_eq!(format_f64(0.0), "0.0");
    }

    #[test]
    fn non_finite_values_do_not_escape_into_the_file() {
        assert_eq!(format_f64(f64::NAN), "0.0");
        assert_eq!(format_f64(f64::INFINITY), "0.0");
    }

    #[test]
    fn flags_written_as_floats_still_parse() {
        assert_eq!(Pair::new(70, "1.0").as_i64(), Some(1));
        assert_eq!(Pair::new(70, "1").as_i64(), Some(1));
        assert_eq!(Pair::new(70, "  2 ").as_i64(), Some(2));
        assert_eq!(Pair::new(70, "abc").as_i64(), None);
    }
}
