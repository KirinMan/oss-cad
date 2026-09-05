use crate::constraint::Constraint;
use crate::solve::{self, DofReport, SolveReport};
use od_geom2d::Point2;

/// Index into a [`System`]'s flat parameter vector. Never constructed
/// directly — [`System::add_param`] and the handles built from it
/// (`add_point`, `add_circle`) are the only way to get one, so a `ParamId`
/// always names a real slot in the `System` it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParamId(usize);

impl ParamId {
    pub(crate) fn index(self) -> usize {
        self.0
    }
}

/// A 2D point: two free parameters, `x` and `y`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointId {
    pub x: ParamId,
    pub y: ParamId,
}

/// A circle: a centre point plus one free radius parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CircleId {
    pub center: PointId,
    pub radius: ParamId,
}

/// A constraint problem: a flat vector of scalar parameters, plus the
/// constraints relating them (ADR-003).
///
/// [`System::solve`] adjusts every parameter in place to satisfy them, as
/// closely as it can; [`System::analyze`] reports how many degrees of
/// freedom are left and whether any constraint is redundant or conflicting,
/// without changing anything. Points and circles are not entities — this
/// crate has no document model — they are just names for groups of
/// parameters, so a caller (`od-core`, eventually) decides what a `PointId`
/// corresponds to in a real drawing.
#[derive(Debug, Clone, Default)]
pub struct System {
    pub(crate) params: Vec<f64>,
    pub(crate) constraints: Vec<Constraint>,
}

impl System {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one free scalar parameter and returns a handle to it.
    pub fn add_param(&mut self, value: f64) -> ParamId {
        let id = ParamId(self.params.len());
        self.params.push(value);
        id
    }

    /// Adds a point: two free parameters, `x` and `y`.
    pub fn add_point(&mut self, x: f64, y: f64) -> PointId {
        PointId {
            x: self.add_param(x),
            y: self.add_param(y),
        }
    }

    /// Adds a circle: `center` is an existing point (often one already
    /// under other constraints), `radius` is a new free parameter.
    pub fn add_circle(&mut self, center: PointId, radius: f64) -> CircleId {
        CircleId {
            center,
            radius: self.add_param(radius),
        }
    }

    #[must_use]
    pub fn value(&self, p: ParamId) -> f64 {
        self.params[p.index()]
    }

    #[must_use]
    pub fn point(&self, p: PointId) -> Point2 {
        Point2::new(self.value(p.x), self.value(p.y))
    }

    pub fn add_constraint(&mut self, c: Constraint) {
        self.constraints.push(c);
    }

    /// [`Constraint::Fixed`] on both of a point's parameters — the common
    /// case of pinning a whole point in place, spelled out because "固定"
    /// most often means a point, not a lone scalar.
    pub fn fix_point(&mut self, p: PointId, target_x: f64, target_y: f64) {
        self.add_constraint(Constraint::Fixed {
            param: p.x,
            target: target_x,
        });
        self.add_constraint(Constraint::Fixed {
            param: p.y,
            target: target_y,
        });
    }

    /// Drives every constraint's residual toward zero by adjusting the
    /// parameters in place. See [`crate::solve`] for the algorithm.
    pub fn solve(&mut self) -> SolveReport {
        solve::solve(self)
    }

    /// Reports degrees of freedom, redundancy and conflict without changing
    /// any parameter. See [`crate::solve`].
    #[must_use]
    pub fn analyze(&self) -> DofReport {
        solve::analyze(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_are_indexed_in_allocation_order() {
        let mut sys = System::new();
        let a = sys.add_param(1.0);
        let b = sys.add_param(2.0);
        assert_eq!(sys.value(a), 1.0);
        assert_eq!(sys.value(b), 2.0);
    }

    #[test]
    fn a_point_is_two_params() {
        let mut sys = System::new();
        let p = sys.add_point(3.0, 4.0);
        assert_eq!(sys.point(p), Point2::new(3.0, 4.0));
    }
}
