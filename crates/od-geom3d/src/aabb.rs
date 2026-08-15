//! 3D bounds — the broad phase for clash detection and view culling.

use crate::point::{Point3, Vec3};
use od_geom2d::tol;

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Aabb3 {
    pub min: Point3,
    pub max: Point3,
}

impl Aabb3 {
    pub const EMPTY: Self = Self {
        min: Point3 {
            x: f64::INFINITY,
            y: f64::INFINITY,
            z: f64::INFINITY,
        },
        max: Point3 {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
            z: f64::NEG_INFINITY,
        },
    };

    #[must_use]
    pub fn new(a: Point3, b: Point3) -> Self {
        Self {
            min: Point3::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)),
            max: Point3::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)),
        }
    }

    #[must_use]
    pub fn from_points(points: impl IntoIterator<Item = Point3>) -> Self {
        points
            .into_iter()
            .fold(Self::EMPTY, |acc, p| acc.union_point(p))
    }

    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    #[must_use]
    pub fn union_point(self, p: Point3) -> Self {
        Self {
            min: Point3::new(
                self.min.x.min(p.x),
                self.min.y.min(p.y),
                self.min.z.min(p.z),
            ),
            max: Point3::new(
                self.max.x.max(p.x),
                self.max.y.max(p.y),
                self.max.z.max(p.z),
            ),
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

    /// Grown on every side. Clash rules express insulation thickness and
    /// installation clearance by inflating one side's bounds.
    #[must_use]
    pub fn inflated(self, d: f64) -> Self {
        if self.is_empty() {
            return self;
        }
        Self {
            min: Point3::new(self.min.x - d, self.min.y - d, self.min.z - d),
            max: Point3::new(self.max.x + d, self.max.y + d, self.max.z + d),
        }
    }

    #[must_use]
    pub fn size(&self) -> Vec3 {
        if self.is_empty() {
            Vec3::ZERO
        } else {
            self.max - self.min
        }
    }

    #[must_use]
    pub fn center(&self) -> Point3 {
        self.min.midpoint(self.max)
    }

    #[must_use]
    pub fn intersects(&self, o: &Self) -> bool {
        !self.is_empty()
            && !o.is_empty()
            && self.min.x <= o.max.x + tol::POINT_EPS
            && o.min.x <= self.max.x + tol::POINT_EPS
            && self.min.y <= o.max.y + tol::POINT_EPS
            && o.min.y <= self.max.y + tol::POINT_EPS
            && self.min.z <= o.max.z + tol::POINT_EPS
            && o.min.z <= self.max.z + tol::POINT_EPS
    }

    #[must_use]
    pub fn contains_point(&self, p: Point3) -> bool {
        !self.is_empty()
            && p.x >= self.min.x - tol::POINT_EPS
            && p.x <= self.max.x + tol::POINT_EPS
            && p.y >= self.min.y - tol::POINT_EPS
            && p.y <= self.max.y + tol::POINT_EPS
            && p.z >= self.min.z - tol::POINT_EPS
            && p.z <= self.max.z + tol::POINT_EPS
    }
}

impl Default for Aabb3 {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_separated_in_z_alone_do_not_clash() {
        let duct = Aabb3::new(
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(5000.0, 400.0, 3200.0),
        );
        let beam = Aabb3::new(
            Point3::new(0.0, 0.0, 3300.0),
            Point3::new(5000.0, 400.0, 3900.0),
        );
        assert!(!duct.intersects(&beam));
        // 100 mm of insulation closes the gap.
        assert!(duct.inflated(100.0).intersects(&beam));
    }

    #[test]
    fn empty_is_absorbing() {
        assert!(Aabb3::EMPTY.is_empty());
        assert!(!Aabb3::EMPTY.intersects(&Aabb3::EMPTY));
        let b = Aabb3::from_points([Point3::ORIGIN, Point3::new(1.0, 2.0, 3.0)]);
        assert_eq!(Aabb3::EMPTY.union(b), b);
    }
}
