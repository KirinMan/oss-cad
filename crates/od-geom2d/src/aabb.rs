//! Axis-aligned bounding boxes — the currency of the spatial index.

use crate::point::{Point2, Vec2};
use crate::tol;

/// An axis-aligned bounding box. An empty box is represented by inverted
/// bounds, so `Aabb2::EMPTY.union_point(p)` yields exactly `p`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Aabb2 {
    pub min: Point2,
    pub max: Point2,
}

impl Aabb2 {
    pub const EMPTY: Self = Self {
        min: Point2 {
            x: f64::INFINITY,
            y: f64::INFINITY,
        },
        max: Point2 {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
        },
    };

    #[must_use]
    pub fn new(min: Point2, max: Point2) -> Self {
        Self {
            min: Point2::new(min.x.min(max.x), min.y.min(max.y)),
            max: Point2::new(min.x.max(max.x), min.y.max(max.y)),
        }
    }

    #[must_use]
    pub fn from_points(points: impl IntoIterator<Item = Point2>) -> Self {
        points
            .into_iter()
            .fold(Self::EMPTY, |acc, p| acc.union_point(p))
    }

    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }

    #[must_use]
    pub fn union_point(self, p: Point2) -> Self {
        Self {
            min: Point2::new(self.min.x.min(p.x), self.min.y.min(p.y)),
            max: Point2::new(self.max.x.max(p.x), self.max.y.max(p.y)),
        }
    }

    #[must_use]
    pub fn union(self, o: Self) -> Self {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        self.union_point(o.min).union_point(o.max)
    }

    /// Grown by `d` on every side. Negative values shrink, which is how
    /// clearance checks are expressed.
    #[must_use]
    pub fn inflated(self, d: f64) -> Self {
        if self.is_empty() {
            return self;
        }
        Self {
            min: Point2::new(self.min.x - d, self.min.y - d),
            max: Point2::new(self.max.x + d, self.max.y + d),
        }
    }

    #[must_use]
    pub fn width(&self) -> f64 {
        if self.is_empty() {
            0.0
        } else {
            self.max.x - self.min.x
        }
    }

    #[must_use]
    pub fn height(&self) -> f64 {
        if self.is_empty() {
            0.0
        } else {
            self.max.y - self.min.y
        }
    }

    #[must_use]
    pub fn center(&self) -> Point2 {
        self.min.midpoint(self.max)
    }

    #[must_use]
    pub fn size(&self) -> Vec2 {
        Vec2::new(self.width(), self.height())
    }

    /// Touching boxes count as intersecting: a shared edge is a real adjacency
    /// in a drawing, and the broad phase must not drop it.
    #[must_use]
    pub fn intersects(&self, o: &Self) -> bool {
        !self.is_empty()
            && !o.is_empty()
            && self.min.x <= o.max.x + tol::POINT_EPS
            && o.min.x <= self.max.x + tol::POINT_EPS
            && self.min.y <= o.max.y + tol::POINT_EPS
            && o.min.y <= self.max.y + tol::POINT_EPS
    }

    #[must_use]
    pub fn contains_point(&self, p: Point2) -> bool {
        !self.is_empty()
            && p.x >= self.min.x - tol::POINT_EPS
            && p.x <= self.max.x + tol::POINT_EPS
            && p.y >= self.min.y - tol::POINT_EPS
            && p.y <= self.max.y + tol::POINT_EPS
    }
}

impl Default for Aabb2 {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_box_absorbs_the_first_point() {
        let b = Aabb2::EMPTY.union_point(Point2::new(5.0, -3.0));
        assert!(!b.is_empty());
        assert_eq!(b.min, b.max);
        assert_eq!(b.min, Point2::new(5.0, -3.0));
    }

    #[test]
    fn constructor_orders_the_corners() {
        let b = Aabb2::new(Point2::new(10.0, 10.0), Point2::new(0.0, 0.0));
        assert_eq!(b.min, Point2::ORIGIN);
        assert!(tol::eq_len(b.width(), 10.0));
    }

    #[test]
    fn touching_boxes_intersect() {
        let a = Aabb2::new(Point2::ORIGIN, Point2::new(10.0, 10.0));
        let b = Aabb2::new(Point2::new(10.0, 0.0), Point2::new(20.0, 10.0));
        assert!(a.intersects(&b));
        let far = Aabb2::new(Point2::new(10.1, 0.0), Point2::new(20.0, 10.0));
        assert!(!a.intersects(&far));
    }

    #[test]
    fn empty_never_intersects() {
        let a = Aabb2::new(Point2::ORIGIN, Point2::new(10.0, 10.0));
        assert!(!a.intersects(&Aabb2::EMPTY));
        assert!(!Aabb2::EMPTY.contains_point(Point2::ORIGIN));
    }
}
