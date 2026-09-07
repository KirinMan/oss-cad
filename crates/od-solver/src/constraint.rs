use crate::system::{CircleId, ParamId, PointId};

/// A 2D geometric or dimensional constraint (F-012), expressed as one or
/// more residual functions of the system's flat parameter vector — each
/// exactly zero when the constraint is satisfied. [`crate::solve`] drives
/// every residual toward zero by adjusting the parameters; it never looks at
/// which kind of constraint produced a given residual, so adding a new
/// variant here never touches the solver itself.
///
/// A circle's radius is a dimensional constraint too ("半径"), but needs no
/// dedicated variant: it is just [`Constraint::Fixed`] on the circle's own
/// `radius` parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Constraint {
    /// Two points occupy the same position (一致).
    Coincident { a: PointId, b: PointId },
    /// The segment `a`–`b` is horizontal — equal Y (水平).
    Horizontal { a: PointId, b: PointId },
    /// The segment `a`–`b` is vertical — equal X (垂直).
    Vertical { a: PointId, b: PointId },
    /// Segment `a1`–`a2` is parallel to segment `b1`–`b2` (平行).
    Parallel {
        a1: PointId,
        a2: PointId,
        b1: PointId,
        b2: PointId,
    },
    /// Segment `a1`–`a2` is perpendicular to segment `b1`–`b2` (直交).
    Perpendicular {
        a1: PointId,
        a2: PointId,
        b1: PointId,
        b2: PointId,
    },
    /// Segment `a1`–`a2` has the same length as segment `b1`–`b2` (等値).
    EqualLength {
        a1: PointId,
        a2: PointId,
        b1: PointId,
        b2: PointId,
    },
    /// Two circles share a centre (同心). Mathematically identical to
    /// [`Constraint::Coincident`] applied to both centres — kept as its own
    /// variant because it names circles, not raw points, matching how F-012
    /// lists it as a distinct constraint kind.
    Concentric { a: CircleId, b: CircleId },
    /// `a` and `b` are mirror images of each other across the line through
    /// `axis1` and `axis2` (対称).
    Symmetric {
        a: PointId,
        b: PointId,
        axis1: PointId,
        axis2: PointId,
    },
    /// Pins one scalar parameter — a point's `x`/`y`, or a circle's
    /// `radius` — to a fixed value (固定). The primitive every other kind of
    /// "fixed" reduces to; [`crate::System::fix_point`] is the
    /// two-parameter convenience for pinning a whole point.
    Fixed { param: ParamId, target: f64 },
    /// Two circles are externally tangent: their centres are exactly
    /// `ra + rb` apart (接線). Internal tangency (one circle inside the
    /// other, centres `|ra − rb|` apart) is a distinct, less common case
    /// this does not cover.
    TangentCircles { a: CircleId, b: CircleId },
    /// The line through `p1`–`p2` is tangent to `circle` (接線).
    TangentLineCircle {
        p1: PointId,
        p2: PointId,
        circle: CircleId,
    },
    /// The distance between two points equals `target` (寸法拘束).
    Distance { a: PointId, b: PointId, target: f64 },
    /// The signed angle swept from segment `a1`–`a2` to segment `b1`–`b2`,
    /// counter-clockwise positive, equals `target` radians (寸法拘束).
    Angle {
        a1: PointId,
        a2: PointId,
        b1: PointId,
        b2: PointId,
        target: f64,
    },
}

impl Constraint {
    /// How many residual rows [`Constraint::residuals`] appends. Every
    /// variant but the three below reduces to a single scalar condition.
    #[must_use]
    pub fn residual_count(&self) -> usize {
        match self {
            Constraint::Coincident { .. }
            | Constraint::Concentric { .. }
            | Constraint::Symmetric { .. } => 2,
            _ => 1,
        }
    }

    /// Appends this constraint's residuals, evaluated at `p` (a `System`'s
    /// full parameter vector), to `out`. `pub(crate)`: residual evaluation is
    /// the solver's business, not a public API — a caller only ever adds
    /// constraints and reads back [`crate::SolveReport`] / [`crate::DofReport`].
    pub(crate) fn residuals(&self, p: &[f64], out: &mut Vec<f64>) {
        match *self {
            Constraint::Coincident { a, b } => {
                out.push(p[a.x.index()] - p[b.x.index()]);
                out.push(p[a.y.index()] - p[b.y.index()]);
            }
            Constraint::Horizontal { a, b } => {
                out.push(p[a.y.index()] - p[b.y.index()]);
            }
            Constraint::Vertical { a, b } => {
                out.push(p[a.x.index()] - p[b.x.index()]);
            }
            Constraint::Parallel { a1, a2, b1, b2 } => {
                let (ax, ay) = dir(p, a1, a2);
                let (bx, by) = dir(p, b1, b2);
                out.push(ax * by - ay * bx);
            }
            Constraint::Perpendicular { a1, a2, b1, b2 } => {
                let (ax, ay) = dir(p, a1, a2);
                let (bx, by) = dir(p, b1, b2);
                out.push(ax * bx + ay * by);
            }
            Constraint::EqualLength { a1, a2, b1, b2 } => {
                out.push(len(p, a1, a2) - len(p, b1, b2));
            }
            Constraint::Concentric { a, b } => {
                out.push(p[a.center.x.index()] - p[b.center.x.index()]);
                out.push(p[a.center.y.index()] - p[b.center.y.index()]);
            }
            Constraint::Symmetric { a, b, axis1, axis2 } => {
                let (dx, dy) = dir(p, axis1, axis2);
                let (abx, aby) = dir(p, a, b);
                // a-to-b is perpendicular to the axis...
                out.push(abx * dx + aby * dy);
                // ...and the a-b midpoint lies on the axis line.
                let mx = (p[a.x.index()] + p[b.x.index()]) / 2.0;
                let my = (p[a.y.index()] + p[b.y.index()]) / 2.0;
                let ox = mx - p[axis1.x.index()];
                let oy = my - p[axis1.y.index()];
                out.push(ox * dy - oy * dx);
            }
            Constraint::Fixed { param, target } => {
                out.push(p[param.index()] - target);
            }
            Constraint::TangentCircles { a, b } => {
                let dx = p[a.center.x.index()] - p[b.center.x.index()];
                let dy = p[a.center.y.index()] - p[b.center.y.index()];
                let dist = dx.hypot(dy);
                out.push(dist - (p[a.radius.index()] + p[b.radius.index()]));
            }
            Constraint::TangentLineCircle { p1, p2, circle } => {
                let d = point_line_distance(p, p1, p2, circle.center);
                out.push(d - p[circle.radius.index()]);
            }
            Constraint::Distance { a, b, target } => {
                out.push(len(p, a, b) - target);
            }
            Constraint::Angle {
                a1,
                a2,
                b1,
                b2,
                target,
            } => {
                let (ax, ay) = dir(p, a1, a2);
                let (bx, by) = dir(p, b1, b2);
                // Want θb − θa = target. sin(x) is smooth and has no branch
                // cut, unlike atan2(θb) − atan2(θa), so the residual is
                // built from the angle-subtraction identity instead:
                //   sin((θb−θa) − target) = sin(θb−θa)cos(target) − cos(θb−θa)sin(target)
                // where sin(θb−θa) = cross(a,b)/(|a||b|) and
                // cos(θb−θa) = dot(a,b)/(|a||b|).
                let cross = ax * by - ay * bx;
                let dot = ax * bx + ay * by;
                // A zero-length input segment has no angle to speak of;
                // clamping the denominator turns that degenerate case into
                // a large-but-finite residual instead of a NaN that would
                // otherwise poison the whole solve's Jacobian.
                let denom = (ax.hypot(ay) * bx.hypot(by)).max(od_geom2d::tol::POINT_EPS);
                out.push((cross * target.cos() - dot * target.sin()) / denom);
            }
        }
    }
}

fn dir(p: &[f64], a: PointId, b: PointId) -> (f64, f64) {
    (
        p[b.x.index()] - p[a.x.index()],
        p[b.y.index()] - p[a.y.index()],
    )
}

fn len(p: &[f64], a: PointId, b: PointId) -> f64 {
    let (dx, dy) = dir(p, a, b);
    dx.hypot(dy)
}

/// Perpendicular distance from `point` to the (infinite) line through `l1`
/// and `l2`.
fn point_line_distance(p: &[f64], l1: PointId, l2: PointId, point: PointId) -> f64 {
    let (dx, dy) = dir(p, l1, l2);
    let length = dx.hypot(dy).max(od_geom2d::tol::POINT_EPS);
    let ox = p[point.x.index()] - p[l1.x.index()];
    let oy = p[point.y.index()] - p[l1.y.index()];
    (ox * dy - oy * dx).abs() / length
}
