//! Positions and directions in model space (millimetres, `f64`).

use od_geom2d::tol;
use std::ops::{Add, Div, Mul, Neg, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Point3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);

    #[inline]
    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[inline]
    #[must_use]
    pub fn length(self) -> f64 {
        self.length_sq().sqrt()
    }

    #[inline]
    #[must_use]
    pub fn length_sq(self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    #[inline]
    #[must_use]
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    #[inline]
    #[must_use]
    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    /// `None` below [`tol::POINT_EPS`] — see the 2D counterpart for why a
    /// degenerate direction is never invented.
    #[must_use]
    pub fn normalized(self) -> Option<Self> {
        let len = self.length();
        if tol::is_zero_len(len) {
            None
        } else {
            Some(self / len)
        }
    }

    /// Any unit vector perpendicular to this one. Picks the world axis least
    /// aligned with `self` first, so the result is numerically stable.
    #[must_use]
    pub fn any_perpendicular(self) -> Option<Self> {
        let n = self.normalized()?;
        let seed = if n.x.abs() <= n.y.abs() && n.x.abs() <= n.z.abs() {
            Self::X
        } else if n.y.abs() <= n.z.abs() {
            Self::Y
        } else {
            Self::Z
        };
        n.cross(seed).normalized()
    }

    #[must_use]
    pub fn is_zero(self) -> bool {
        tol::is_zero_len(self.length())
    }
}

impl Point3 {
    pub const ORIGIN: Self = Self::new(0.0, 0.0, 0.0);

    #[inline]
    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[inline]
    #[must_use]
    pub fn from_2d(p: od_geom2d::Point2, z: f64) -> Self {
        Self::new(p.x, p.y, z)
    }

    /// Drops Z. Used when projecting to a plan view, never for storage.
    #[inline]
    #[must_use]
    pub fn to_2d(self) -> od_geom2d::Point2 {
        od_geom2d::Point2::new(self.x, self.y)
    }

    #[inline]
    #[must_use]
    pub fn distance_to(self, o: Self) -> f64 {
        (o - self).length()
    }

    #[inline]
    #[must_use]
    pub fn coincides_with(self, o: Self) -> bool {
        self.distance_to(o) <= tol::POINT_EPS
    }

    #[inline]
    #[must_use]
    pub fn lerp(self, o: Self, t: f64) -> Self {
        Self::new(
            self.x + (o.x - self.x) * t,
            self.y + (o.y - self.y) * t,
            self.z + (o.z - self.z) * t,
        )
    }

    #[inline]
    #[must_use]
    pub fn midpoint(self, o: Self) -> Self {
        self.lerp(o, 0.5)
    }

    #[inline]
    #[must_use]
    pub fn to_vec(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }
}

impl Add<Vec3> for Point3 {
    type Output = Point3;
    #[inline]
    fn add(self, v: Vec3) -> Point3 {
        Point3::new(self.x + v.x, self.y + v.y, self.z + v.z)
    }
}

impl Sub<Vec3> for Point3 {
    type Output = Point3;
    #[inline]
    fn sub(self, v: Vec3) -> Point3 {
        Point3::new(self.x - v.x, self.y - v.y, self.z - v.z)
    }
}

impl Sub<Point3> for Point3 {
    type Output = Vec3;
    #[inline]
    fn sub(self, o: Point3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    #[inline]
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl Sub for Vec3 {
    type Output = Vec3;
    #[inline]
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f64> for Vec3 {
    type Output = Vec3;
    #[inline]
    fn mul(self, s: f64) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Div<f64> for Vec3 {
    type Output = Vec3;
    #[inline]
    fn div(self, s: f64) -> Vec3 {
        Vec3::new(self.x / s, self.y / s, self.z / s)
    }
}

impl Neg for Vec3 {
    type Output = Vec3;
    #[inline]
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_follows_the_right_hand_rule() {
        assert_eq!(Vec3::X.cross(Vec3::Y), Vec3::Z);
        assert_eq!(Vec3::Y.cross(Vec3::Z), Vec3::X);
    }

    #[test]
    fn perpendicular_is_found_for_every_axis() {
        for v in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(1.0, 1.0, 1.0)] {
            let p = v.any_perpendicular().expect("non-degenerate");
            assert!(tol::is_zero_len(p.dot(v.normalized().expect("non-zero"))));
            assert!(tol::eq_len(p.length(), 1.0));
        }
        assert!(Vec3::ZERO.any_perpendicular().is_none());
    }

    #[test]
    fn round_trip_through_2d_keeps_xy() {
        let p = Point3::new(3.0, 4.0, 5.0);
        assert!(Point3::from_2d(p.to_2d(), p.z).coincides_with(p));
    }
}
