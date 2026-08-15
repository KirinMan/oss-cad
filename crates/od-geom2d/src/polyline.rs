//! Polylines with bulges.
//!
//! A bulge is `tan(sweep / 4)` for the arc between a vertex and the next one:
//! zero is a straight span, positive sweeps counter-clockwise. It is DWG/DXF's
//! representation and it is worth keeping rather than translating away, because
//! it survives round-tripping exactly and keeps arcs and lines in one ordered
//! list — which is what trim, offset and hatch boundaries all want.

use crate::aabb::Aabb2;
use crate::curve::{Arc2, Segment2};
use crate::point::Point2;
use crate::tol;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Vertex {
    pub point: Point2,
    /// `tan(sweep / 4)` of the span that *starts* at this vertex.
    pub bulge: f64,
}

impl Vertex {
    #[must_use]
    pub const fn straight(point: Point2) -> Self {
        Self { point, bulge: 0.0 }
    }

    #[must_use]
    pub const fn with_bulge(point: Point2, bulge: f64) -> Self {
        Self { point, bulge }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Polyline2 {
    pub vertices: Vec<Vertex>,
    pub closed: bool,
}

/// One span between consecutive vertices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Span {
    Line(Segment2),
    Arc(Arc2),
}

impl Span {
    #[must_use]
    pub fn start(&self) -> Point2 {
        match self {
            Span::Line(s) => s.a,
            Span::Arc(a) => a.start_point(),
        }
    }

    #[must_use]
    pub fn end(&self) -> Point2 {
        match self {
            Span::Line(s) => s.b,
            Span::Arc(a) => a.end_point(),
        }
    }

    #[must_use]
    pub fn length(&self) -> f64 {
        match self {
            Span::Line(s) => s.length(),
            Span::Arc(a) => a.length(),
        }
    }

    #[must_use]
    pub fn bounds(&self) -> Aabb2 {
        match self {
            Span::Line(s) => s.bounds(),
            Span::Arc(a) => a.bounds(),
        }
    }
}

impl Polyline2 {
    #[must_use]
    pub fn new(vertices: Vec<Vertex>, closed: bool) -> Self {
        Self { vertices, closed }
    }

    #[must_use]
    pub fn from_points(points: impl IntoIterator<Item = Point2>, closed: bool) -> Self {
        Self::new(points.into_iter().map(Vertex::straight).collect(), closed)
    }

    #[must_use]
    pub fn span_count(&self) -> usize {
        let n = self.vertices.len();
        if n < 2 {
            0
        } else if self.closed {
            n
        } else {
            n - 1
        }
    }

    /// The span starting at vertex `i`, or `None` if `i` is out of range.
    #[must_use]
    pub fn span(&self, i: usize) -> Option<Span> {
        if i >= self.span_count() {
            return None;
        }
        let v = self.vertices[i];
        let next = self.vertices[(i + 1) % self.vertices.len()];
        Some(span_between(v, next.point))
    }

    pub fn spans(&self) -> impl Iterator<Item = Span> + '_ {
        (0..self.span_count()).filter_map(|i| self.span(i))
    }

    #[must_use]
    pub fn length(&self) -> f64 {
        self.spans().map(|s| s.length()).sum()
    }

    #[must_use]
    pub fn bounds(&self) -> Aabb2 {
        self.spans()
            .fold(Aabb2::EMPTY, |acc, s| acc.union(s.bounds()))
    }

    /// Signed area, positive for counter-clockwise. Arc spans contribute their
    /// circular segment. Meaningless for an open polyline, so `None` there.
    #[must_use]
    pub fn signed_area(&self) -> Option<f64> {
        if !self.closed || self.vertices.len() < 3 {
            return None;
        }
        let mut area = 0.0;
        for span in self.spans() {
            let (a, b) = (span.start(), span.end());
            area += a.x * b.y - b.x * a.y;
            if let Span::Arc(arc) = span {
                // Circular segment between the chord and the arc.
                let seg = 0.5 * arc.radius * arc.radius * (arc.sweep - arc.sweep.sin());
                area += 2.0 * seg;
            }
        }
        Some(area * 0.5)
    }

    /// Approximates the polyline as points, subdividing arcs so the chord never
    /// deviates from the arc by more than `sag`. This is what the renderer and
    /// the exporters consume; nothing else should be flattening geometry.
    #[must_use]
    pub fn flatten(&self, sag: f64) -> Vec<Point2> {
        let mut out: Vec<Point2> = Vec::new();
        let push = |p: Point2, out: &mut Vec<Point2>| {
            if out.last().is_none_or(|last| !last.coincides_with(p)) {
                out.push(p);
            }
        };
        for span in self.spans() {
            match span {
                Span::Line(s) => {
                    push(s.a, &mut out);
                    push(s.b, &mut out);
                }
                Span::Arc(arc) => {
                    let steps = arc_steps(arc.radius, arc.sweep, sag);
                    for k in 0..=steps {
                        let t = f64::from(k) / f64::from(steps);
                        let a = arc.start_angle + arc.sweep * t;
                        push(arc.circle().point_at_angle(a), &mut out);
                    }
                }
            }
        }
        if self.closed && out.len() > 1 {
            if let (Some(first), Some(last)) = (out.first().copied(), out.last().copied()) {
                if last.coincides_with(first) {
                    out.pop();
                }
            }
        }
        out
    }
}

/// Builds the span leaving `from` and arriving at `to`.
#[must_use]
pub fn span_between(from: Vertex, to: Point2) -> Span {
    if from.bulge.abs() <= tol::ANGLE_EPS || from.point.coincides_with(to) {
        return Span::Line(Segment2::new(from.point, to));
    }
    let sweep = 4.0 * from.bulge.atan();
    let chord = to - from.point;
    let chord_len = chord.length();
    let half = sweep * 0.5;
    let sin_half = half.sin();
    if sin_half.abs() <= tol::ANGLE_EPS {
        return Span::Line(Segment2::new(from.point, to));
    }
    // Signed radius: negative for a clockwise sweep. Carrying the sign through
    // means one formula covers all four cases (either direction, major or minor
    // arc) instead of a sign table that is easy to get subtly wrong.
    let radius = chord_len / (2.0 * sin_half);
    let mid = from.point.midpoint(to);
    let Some(dir) = chord.normalized() else {
        return Span::Line(Segment2::new(from.point, to));
    };
    let center = mid + dir.perp() * (radius * half.cos());
    let start_angle = (from.point - center).angle();
    Span::Arc(Arc2::new(center, radius.abs(), start_angle, sweep))
}

/// The number of chords needed to hold an arc within `sag` of its true path.
fn arc_steps(radius: f64, sweep: f64, sag: f64) -> u32 {
    let sag = sag.max(1e-6);
    if radius <= sag {
        return 1;
    }
    // Max angle per chord whose sagitta stays under `sag`.
    let ratio = (1.0 - sag / radius).clamp(-1.0, 1.0);
    let max_step = 2.0 * ratio.acos();
    if max_step <= f64::EPSILON {
        return 1;
    }
    let n = (sweep.abs() / max_step).ceil();
    // 512 chords is well past the point where more subdivision is visible; the
    // clamp keeps a degenerate radius from allocating unboundedly.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clamped to 1..=512 on the line above"
    )]
    let steps = n.clamp(1.0, 512.0) as u32;
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    #[test]
    fn open_and_closed_span_counts_differ_by_one() {
        let pts = vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)];
        assert_eq!(Polyline2::from_points(pts.clone(), false).span_count(), 2);
        assert_eq!(Polyline2::from_points(pts, true).span_count(), 3);
    }

    #[test]
    fn square_perimeter_and_area() {
        let sq = Polyline2::from_points(
            [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)],
            true,
        );
        assert!(tol::eq_len(sq.length(), 40.0));
        let area = sq.signed_area().expect("closed polyline has an area");
        assert!(tol::eq_len(area, 100.0));
    }

    #[test]
    fn reversed_winding_flips_the_area_sign() {
        let cw = Polyline2::from_points(
            [p(0.0, 0.0), p(0.0, 10.0), p(10.0, 10.0), p(10.0, 0.0)],
            true,
        );
        assert!(cw.signed_area().is_some_and(|a| a < 0.0));
    }

    #[test]
    fn open_polyline_has_no_area() {
        assert!(
            Polyline2::from_points([p(0.0, 0.0), p(10.0, 0.0)], false)
                .signed_area()
                .is_none()
        );
    }

    #[test]
    fn a_bulge_of_one_is_a_half_circle() {
        // bulge = tan(π/4) = 1 → sweep = π.
        let pl = Polyline2::new(
            vec![
                Vertex::with_bulge(p(0.0, 0.0), 1.0),
                Vertex::straight(p(10.0, 0.0)),
            ],
            false,
        );
        let span = pl.span(0).expect("one span");
        match span {
            Span::Arc(arc) => {
                assert!(tol::eq_angle(arc.sweep, PI));
                assert!(tol::eq_len(arc.radius, 5.0));
                assert!(arc.center.coincides_with(p(5.0, 0.0)));
                assert!(arc.start_point().coincides_with(p(0.0, 0.0)));
                assert!(arc.end_point().coincides_with(p(10.0, 0.0)));
                assert!(tol::eq_len(pl.length(), PI * 5.0));
            }
            Span::Line(_) => panic!("a non-zero bulge must produce an arc"),
        }
    }

    #[test]
    fn a_negative_bulge_sweeps_the_other_way() {
        let pl = Polyline2::new(
            vec![
                Vertex::with_bulge(p(0.0, 0.0), -1.0),
                Vertex::straight(p(10.0, 0.0)),
            ],
            false,
        );
        match pl.span(0).expect("one span") {
            Span::Arc(arc) => {
                assert!(tol::eq_angle(arc.sweep, -PI));
                assert!(arc.end_point().coincides_with(p(10.0, 0.0)));
            }
            Span::Line(_) => panic!("expected an arc"),
        }
    }

    #[test]
    fn quarter_bulge_keeps_its_endpoints() {
        // bulge = tan(π/8) → sweep = π/2.
        let b = (FRAC_PI_2 / 4.0).tan();
        let pl = Polyline2::new(
            vec![
                Vertex::with_bulge(p(0.0, 0.0), b),
                Vertex::straight(p(10.0, 10.0)),
            ],
            false,
        );
        match pl.span(0).expect("one span") {
            Span::Arc(arc) => {
                assert!(tol::eq_angle(arc.sweep, FRAC_PI_2));
                assert!(arc.start_point().coincides_with(p(0.0, 0.0)));
                assert!(arc.end_point().coincides_with(p(10.0, 10.0)));
            }
            Span::Line(_) => panic!("expected an arc"),
        }
    }

    #[test]
    fn a_major_arc_puts_its_centre_on_the_other_side() {
        // bulge > 1 → sweep > π, the case a naive sign table gets wrong.
        let sweep = 3.0 * FRAC_PI_2;
        let pl = Polyline2::new(
            vec![
                Vertex::with_bulge(p(0.0, 0.0), (sweep / 4.0).tan()),
                Vertex::straight(p(10.0, 0.0)),
            ],
            false,
        );
        match pl.span(0).expect("one span") {
            Span::Arc(arc) => {
                assert!(tol::eq_angle(arc.sweep, sweep));
                assert!(arc.center.coincides_with(p(5.0, -5.0)));
                assert!(arc.start_point().coincides_with(p(0.0, 0.0)));
                assert!(arc.end_point().coincides_with(p(10.0, 0.0)));
            }
            Span::Line(_) => panic!("expected an arc"),
        }
    }

    #[test]
    fn flatten_respects_the_sag_budget() {
        let pl = Polyline2::new(
            vec![
                Vertex::with_bulge(p(0.0, 0.0), 1.0),
                Vertex::straight(p(10.0, 0.0)),
            ],
            false,
        );
        let coarse = pl.flatten(1.0);
        let fine = pl.flatten(0.01);
        assert!(fine.len() > coarse.len());
        // Every produced point must sit on the true arc.
        for q in &fine {
            assert!(tol::eq_len(q.distance_to(p(5.0, 0.0)), 5.0));
        }
    }

    #[test]
    fn flatten_of_a_closed_shape_does_not_repeat_the_seam() {
        let sq = Polyline2::from_points(
            [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)],
            true,
        );
        assert_eq!(sq.flatten(0.1).len(), 4);
    }

    #[test]
    fn degenerate_polylines_are_inert() {
        let empty = Polyline2::from_points([], false);
        assert_eq!(empty.span_count(), 0);
        assert!(empty.bounds().is_empty());
        assert!(tol::is_zero_len(empty.length()));

        let single = Polyline2::from_points([p(1.0, 1.0)], true);
        assert_eq!(single.span_count(), 0);
        assert!(single.signed_area().is_none());
    }
}
