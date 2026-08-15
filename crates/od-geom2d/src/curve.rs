//! Segments, circles and arcs, plus the intersections between them.
//!
//! These four predicates carry most of a 2D CAD session: object snap, trim,
//! extend, fillet and hatch boundary tracing all reduce to "where do these two
//! curves meet, and where on each curve is that".

use crate::aabb::Aabb2;
use crate::point::{Point2, Vec2};
use crate::tol;

/// A bounded straight segment.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Segment2 {
    pub a: Point2,
    pub b: Point2,
}

/// A full circle.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Circle2 {
    pub center: Point2,
    pub radius: f64,
}

/// A counter-clockwise arc from `start_angle` sweeping by `sweep` radians.
///
/// Storing a signed sweep rather than an end angle removes the "which way
/// round" ambiguity that DXF's start/end angle pair leaves open, and lets an
/// arc represent a full circle without a special case.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Arc2 {
    pub center: Point2,
    pub radius: f64,
    pub start_angle: f64,
    pub sweep: f64,
}

impl Segment2 {
    #[must_use]
    pub const fn new(a: Point2, b: Point2) -> Self {
        Self { a, b }
    }

    #[must_use]
    pub fn direction(&self) -> Vec2 {
        self.b - self.a
    }

    #[must_use]
    pub fn length(&self) -> f64 {
        self.direction().length()
    }

    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        tol::is_zero_len(self.length())
    }

    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        self.a.lerp(self.b, t)
    }

    #[must_use]
    pub fn midpoint(&self) -> Point2 {
        self.a.midpoint(self.b)
    }

    #[must_use]
    pub fn bounds(&self) -> Aabb2 {
        Aabb2::new(self.a, self.b)
    }

    /// The parameter of the closest point on the *infinite* line, unclamped.
    #[must_use]
    pub fn project_param(&self, p: Point2) -> Option<f64> {
        let d = self.direction();
        let len_sq = d.length_sq();
        if len_sq <= tol::POINT_EPS * tol::POINT_EPS {
            None
        } else {
            Some((p - self.a).dot(d) / len_sq)
        }
    }

    /// The closest point on the segment, clamped to its ends.
    #[must_use]
    pub fn closest_point(&self, p: Point2) -> Point2 {
        match self.project_param(p) {
            Some(t) => self.point_at(t.clamp(0.0, 1.0)),
            None => self.a,
        }
    }

    #[must_use]
    pub fn distance_to_point(&self, p: Point2) -> f64 {
        self.closest_point(p).distance_to(p)
    }

    /// Signed perpendicular distance from the infinite line. Positive on the
    /// left of `a → b`.
    #[must_use]
    pub fn signed_distance(&self, p: Point2) -> Option<f64> {
        let n = self.direction().normalized()?.perp();
        Some((p - self.a).dot(n))
    }
}

impl Circle2 {
    #[must_use]
    pub const fn new(center: Point2, radius: f64) -> Self {
        Self { center, radius }
    }

    #[must_use]
    pub fn point_at_angle(&self, angle: f64) -> Point2 {
        self.center + Vec2::from_angle(angle) * self.radius
    }

    #[must_use]
    pub fn bounds(&self) -> Aabb2 {
        Aabb2::new(
            Point2::new(self.center.x - self.radius, self.center.y - self.radius),
            Point2::new(self.center.x + self.radius, self.center.y + self.radius),
        )
    }

    #[must_use]
    pub fn circumference(&self) -> f64 {
        std::f64::consts::TAU * self.radius
    }
}

impl Arc2 {
    #[must_use]
    pub const fn new(center: Point2, radius: f64, start_angle: f64, sweep: f64) -> Self {
        Self {
            center,
            radius,
            start_angle,
            sweep,
        }
    }

    /// Builds an arc from the DXF/DWG convention: counter-clockwise from
    /// `start_angle` to `end_angle`.
    #[must_use]
    pub fn from_start_end_ccw(center: Point2, radius: f64, start: f64, end: f64) -> Self {
        let mut sweep = tol::normalize_angle(end - start);
        if sweep <= tol::ANGLE_EPS {
            sweep = std::f64::consts::TAU;
        }
        Self::new(center, radius, start, sweep)
    }

    #[must_use]
    pub fn end_angle(&self) -> f64 {
        self.start_angle + self.sweep
    }

    #[must_use]
    pub fn start_point(&self) -> Point2 {
        self.circle().point_at_angle(self.start_angle)
    }

    #[must_use]
    pub fn end_point(&self) -> Point2 {
        self.circle().point_at_angle(self.end_angle())
    }

    #[must_use]
    pub fn midpoint(&self) -> Point2 {
        self.circle()
            .point_at_angle(self.start_angle + self.sweep * 0.5)
    }

    #[must_use]
    pub fn circle(&self) -> Circle2 {
        Circle2::new(self.center, self.radius)
    }

    #[must_use]
    pub fn length(&self) -> f64 {
        self.radius * self.sweep.abs()
    }

    /// True when `angle` lies within the swept range, in either direction.
    #[must_use]
    pub fn contains_angle(&self, angle: f64) -> bool {
        if self.sweep.abs() >= std::f64::consts::TAU - tol::ANGLE_EPS {
            return true;
        }
        let rel = tol::normalize_angle(angle - self.start_angle);
        if self.sweep >= 0.0 {
            rel <= self.sweep + tol::ANGLE_EPS
        } else {
            rel >= tol::normalize_angle(self.sweep) - tol::ANGLE_EPS
                || rel <= tol::ANGLE_EPS
                || rel >= std::f64::consts::TAU + self.sweep - tol::ANGLE_EPS
        }
    }

    /// A tight bound: only the quadrant points actually swept are included.
    /// Using the full circle's box here is the classic cause of a drawing that
    /// zooms to twice its real extent.
    #[must_use]
    pub fn bounds(&self) -> Aabb2 {
        let mut b = Aabb2::from_points([self.start_point(), self.end_point()]);
        for k in 0..4 {
            let a = std::f64::consts::FRAC_PI_2 * f64::from(k);
            if self.contains_angle(a) {
                b = b.union_point(self.circle().point_at_angle(a));
            }
        }
        b
    }
}

/// Where two curves meet. Ordered along the first curve.
pub type Intersections = Vec<Point2>;

/// Intersections of two segments, treating both as bounded.
///
/// Collinear overlap reports the overlap's endpoints rather than an arbitrary
/// single point — trim and hatch tracing both need to know the extent.
#[must_use]
pub fn segment_segment(s1: &Segment2, s2: &Segment2) -> Intersections {
    let p = s1.a;
    let r = s1.direction();
    let q = s2.a;
    let s = s2.direction();

    let rxs = r.cross(s);
    let qp = q - p;

    if rxs.abs() > tol::ANGLE_EPS {
        let t = qp.cross(s) / rxs;
        let u = qp.cross(r) / rxs;
        // Tolerance is applied in parameter space scaled by curve length, so a
        // 10 km line and a 1 mm line get the same absolute slack.
        let t_slack = tol::POINT_EPS / r.length().max(tol::POINT_EPS);
        let u_slack = tol::POINT_EPS / s.length().max(tol::POINT_EPS);
        if (-t_slack..=1.0 + t_slack).contains(&t) && (-u_slack..=1.0 + u_slack).contains(&u) {
            return vec![s1.point_at(t.clamp(0.0, 1.0))];
        }
        return Vec::new();
    }

    // Parallel. Collinear only if s2.a lies on s1's line.
    if qp.cross(r).abs() > tol::POINT_EPS * r.length().max(tol::POINT_EPS) {
        return Vec::new();
    }
    let Some(t0) = s1.project_param(s2.a) else {
        return Vec::new();
    };
    let Some(t1) = s1.project_param(s2.b) else {
        return Vec::new();
    };
    let (lo, hi) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
    let lo = lo.max(0.0);
    let hi = hi.min(1.0);
    if lo > hi {
        return Vec::new();
    }
    if (hi - lo) * s1.length() <= tol::POINT_EPS {
        vec![s1.point_at(lo)]
    } else {
        vec![s1.point_at(lo), s1.point_at(hi)]
    }
}

/// Intersections of a bounded segment with a full circle.
#[must_use]
pub fn segment_circle(seg: &Segment2, circle: &Circle2) -> Intersections {
    let d = seg.direction();
    let len_sq = d.length_sq();
    if len_sq <= tol::POINT_EPS * tol::POINT_EPS || circle.radius <= tol::POINT_EPS {
        return Vec::new();
    }
    let f = seg.a - circle.center;
    let a = len_sq;
    let b = 2.0 * f.dot(d);
    let c = f.length_sq() - circle.radius * circle.radius;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return Vec::new();
    }
    let slack = tol::POINT_EPS / len_sq.sqrt();
    let root = disc.sqrt();
    let mut out = Vec::new();
    // Tangency: the two roots coincide, so report one point.
    if root <= tol::POINT_EPS {
        let t = -b / (2.0 * a);
        if (-slack..=1.0 + slack).contains(&t) {
            out.push(seg.point_at(t.clamp(0.0, 1.0)));
        }
        return out;
    }
    for t in [(-b - root) / (2.0 * a), (-b + root) / (2.0 * a)] {
        if (-slack..=1.0 + slack).contains(&t) {
            out.push(seg.point_at(t.clamp(0.0, 1.0)));
        }
    }
    out
}

/// Intersections of a bounded segment with an arc.
#[must_use]
pub fn segment_arc(seg: &Segment2, arc: &Arc2) -> Intersections {
    segment_circle(seg, &arc.circle())
        .into_iter()
        .filter(|p| arc.contains_angle((*p - arc.center).angle()))
        .collect()
}

/// Intersections of two circles. Concentric or identical circles report none:
/// an infinite intersection set is not something a caller can act on.
#[must_use]
pub fn circle_circle(c1: &Circle2, c2: &Circle2) -> Intersections {
    let between = c2.center - c1.center;
    let d = between.length();
    if tol::is_zero_len(d) {
        return Vec::new();
    }
    if d > c1.radius + c2.radius + tol::POINT_EPS
        || d < (c1.radius - c2.radius).abs() - tol::POINT_EPS
    {
        return Vec::new();
    }
    let a = (c1.radius * c1.radius - c2.radius * c2.radius + d * d) / (2.0 * d);
    let h_sq = c1.radius * c1.radius - a * a;
    let Some(unit) = between.normalized() else {
        return Vec::new();
    };
    let base = c1.center + unit * a;
    if h_sq <= tol::POINT_EPS * tol::POINT_EPS {
        return vec![base];
    }
    let h = h_sq.sqrt();
    let off = unit.perp() * h;
    vec![base + off, base - off]
}

/// Intersections of two arcs.
#[must_use]
pub fn arc_arc(a1: &Arc2, a2: &Arc2) -> Intersections {
    circle_circle(&a1.circle(), &a2.circle())
        .into_iter()
        .filter(|p| {
            a1.contains_angle((*p - a1.center).angle())
                && a2.contains_angle((*p - a2.center).angle())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    #[test]
    fn crossing_segments_meet_once() {
        let s1 = Segment2::new(p(0.0, 0.0), p(10.0, 10.0));
        let s2 = Segment2::new(p(0.0, 10.0), p(10.0, 0.0));
        let hits = segment_segment(&s1, &s2);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].coincides_with(p(5.0, 5.0)));
    }

    #[test]
    fn segments_that_only_touch_at_an_endpoint_still_report() {
        let s1 = Segment2::new(p(0.0, 0.0), p(10.0, 0.0));
        let s2 = Segment2::new(p(10.0, 0.0), p(10.0, 10.0));
        let hits = segment_segment(&s1, &s2);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].coincides_with(p(10.0, 0.0)));
    }

    #[test]
    fn parallel_segments_miss() {
        let s1 = Segment2::new(p(0.0, 0.0), p(10.0, 0.0));
        let s2 = Segment2::new(p(0.0, 1.0), p(10.0, 1.0));
        assert!(segment_segment(&s1, &s2).is_empty());
    }

    #[test]
    fn collinear_overlap_reports_its_extent() {
        let s1 = Segment2::new(p(0.0, 0.0), p(10.0, 0.0));
        let s2 = Segment2::new(p(4.0, 0.0), p(20.0, 0.0));
        let hits = segment_segment(&s1, &s2);
        assert_eq!(hits.len(), 2);
        assert!(hits[0].coincides_with(p(4.0, 0.0)));
        assert!(hits[1].coincides_with(p(10.0, 0.0)));
    }

    #[test]
    fn segment_through_circle_hits_twice_tangent_once() {
        let c = Circle2::new(p(0.0, 0.0), 5.0);
        let through = Segment2::new(p(-10.0, 0.0), p(10.0, 0.0));
        assert_eq!(segment_circle(&through, &c).len(), 2);

        let tangent = Segment2::new(p(-10.0, 5.0), p(10.0, 5.0));
        let hits = segment_circle(&tangent, &c);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].coincides_with(p(0.0, 5.0)));

        let miss = Segment2::new(p(-10.0, 6.0), p(10.0, 6.0));
        assert!(segment_circle(&miss, &c).is_empty());
    }

    #[test]
    fn arc_filters_intersections_outside_its_sweep() {
        // Upper half only.
        let arc = Arc2::new(p(0.0, 0.0), 5.0, 0.0, PI);
        let vertical = Segment2::new(p(0.0, -10.0), p(0.0, 10.0));
        let hits = segment_arc(&vertical, &arc);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].coincides_with(p(0.0, 5.0)));
    }

    #[test]
    fn arc_bounds_do_not_inflate_to_the_full_circle() {
        // 0°→90° quadrant of a radius-10 circle at the origin.
        let arc = Arc2::new(p(0.0, 0.0), 10.0, 0.0, FRAC_PI_2);
        let b = arc.bounds();
        assert!(tol::eq_len(b.min.x, 0.0));
        assert!(tol::eq_len(b.min.y, 0.0));
        assert!(tol::eq_len(b.max.x, 10.0));
        assert!(tol::eq_len(b.max.y, 10.0));
    }

    #[test]
    fn full_circle_arc_contains_every_angle() {
        let arc = Arc2::new(p(0.0, 0.0), 1.0, 0.0, TAU);
        assert!(arc.contains_angle(0.0));
        assert!(arc.contains_angle(PI));
        assert!(arc.contains_angle(-PI));
    }

    #[test]
    fn dxf_style_start_end_wraps_through_zero() {
        // 315° → 45° must sweep 90°, not -270°.
        let arc = Arc2::from_start_end_ccw(
            p(0.0, 0.0),
            10.0,
            315.0_f64.to_radians(),
            45.0_f64.to_radians(),
        );
        assert!(tol::eq_angle(arc.sweep, FRAC_PI_2));
        assert!(arc.contains_angle(0.0));
        assert!(!arc.contains_angle(PI));
    }

    #[test]
    fn circles_meet_twice_touch_once_and_concentric_never() {
        let a = Circle2::new(p(0.0, 0.0), 5.0);
        let b = Circle2::new(p(6.0, 0.0), 5.0);
        assert_eq!(circle_circle(&a, &b).len(), 2);

        let touching = Circle2::new(p(10.0, 0.0), 5.0);
        assert_eq!(circle_circle(&a, &touching).len(), 1);

        let concentric = Circle2::new(p(0.0, 0.0), 3.0);
        assert!(circle_circle(&a, &concentric).is_empty());
    }

    #[test]
    fn closest_point_clamps_to_the_ends() {
        let s = Segment2::new(p(0.0, 0.0), p(10.0, 0.0));
        assert!(s.closest_point(p(5.0, 5.0)).coincides_with(p(5.0, 0.0)));
        assert!(s.closest_point(p(-5.0, 5.0)).coincides_with(p(0.0, 0.0)));
        assert!(s.closest_point(p(50.0, 5.0)).coincides_with(p(10.0, 0.0)));
    }
}
