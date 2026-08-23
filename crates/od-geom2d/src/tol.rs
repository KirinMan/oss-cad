//! Tolerances.
//!
//! Every comparison in OpenDraft goes through this module. Per-function epsilons
//! are the single largest source of bugs in CAD codebases: two functions that
//! disagree by one order of magnitude produce geometry that is simultaneously
//! "coincident" and "distinct", and the resulting failures surface far from
//! their cause. So there are exactly three tolerances, and they are documented
//! in the units they apply to.
//!
//! Drawing coordinates are millimetres, stored as `f64`. See
//! `docs/03-data-model.md` for why absolute world coordinates are kept small.

/// Two positions closer than this are the same point. 0.1 nanometre in drawing
/// units — six orders of magnitude below anything a fabricator can hold, and
/// ten orders above `f64` noise at the ±10^9 mm range we support.
pub const POINT_EPS: f64 = 1e-7;

/// Two directions closer than this are parallel, in radians.
pub const ANGLE_EPS: f64 = 1e-9;

/// Areas below this are degenerate, in mm².
pub const AREA_EPS: f64 = 1e-12;

/// `a == b` within [`POINT_EPS`].
#[inline]
#[must_use]
pub fn eq_len(a: f64, b: f64) -> bool {
    (a - b).abs() <= POINT_EPS
}

/// `a == 0` within [`POINT_EPS`].
#[inline]
#[must_use]
pub fn is_zero_len(a: f64) -> bool {
    a.abs() <= POINT_EPS
}

/// `a == b` within [`ANGLE_EPS`], without normalising either side.
#[inline]
#[must_use]
pub fn eq_angle(a: f64, b: f64) -> bool {
    (a - b).abs() <= ANGLE_EPS
}

/// Wraps an angle into `[0, 2π)`.
#[inline]
#[must_use]
pub fn normalize_angle(a: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    let r = a % tau;
    if r < 0.0 { r + tau } else { r }
}

/// The signed difference `to - from` wrapped into `(-π, π]`.
#[inline]
#[must_use]
pub fn angle_delta(from: f64, to: f64) -> f64 {
    let pi = std::f64::consts::PI;
    let mut d = normalize_angle(to - from);
    if d > pi {
        d -= std::f64::consts::TAU;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_wraps_both_directions() {
        assert!(eq_angle(normalize_angle(0.0), 0.0));
        assert!(eq_angle(normalize_angle(std::f64::consts::TAU), 0.0));
        assert!(eq_angle(
            normalize_angle(-std::f64::consts::FRAC_PI_2),
            std::f64::consts::TAU - std::f64::consts::FRAC_PI_2
        ));
    }

    #[test]
    fn delta_takes_the_short_way_round() {
        let pi = std::f64::consts::PI;
        assert!(eq_angle(angle_delta(0.1, 0.2), 0.1));
        // 350° to 10° is +20°, not -340°.
        assert!(eq_angle(
            angle_delta(350.0_f64.to_radians(), 10.0_f64.to_radians()),
            20.0_f64.to_radians()
        ));
        // Exactly opposite resolves to +π by the half-open convention.
        assert!(eq_angle(angle_delta(0.0, pi), pi));
    }
}
