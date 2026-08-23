//! Points and vectors in drawing space (millimetres, `f64`).
//!
//! Points and vectors are separate types on purpose. Adding two positions is
//! meaningless; adding a displacement to a position is not. Keeping them apart
//! makes that distinction a compile error rather than a subtle offset bug.

use crate::tol;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// A displacement in drawing space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

/// A position in drawing space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };
    pub const X: Self = Self { x: 1.0, y: 0.0 };
    pub const Y: Self = Self { x: 0.0, y: 1.0 };

    #[inline]
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// The unit vector at `angle` radians from the +X axis.
    #[inline]
    #[must_use]
    pub fn from_angle(angle: f64) -> Self {
        Self::new(angle.cos(), angle.sin())
    }

    #[inline]
    #[must_use]
    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    #[inline]
    #[must_use]
    pub fn length_sq(self) -> f64 {
        self.x * self.x + self.y * self.y
    }

    #[inline]
    #[must_use]
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y
    }

    /// The 2D cross product (the Z component of the 3D cross product).
    /// Positive when `o` is counter-clockwise from `self`.
    #[inline]
    #[must_use]
    pub fn cross(self, o: Self) -> f64 {
        self.x * o.y - self.y * o.x
    }

    /// Rotated 90° counter-clockwise.
    #[inline]
    #[must_use]
    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    #[inline]
    #[must_use]
    pub fn angle(self) -> f64 {
        self.y.atan2(self.x)
    }

    /// Returns `None` for a vector shorter than [`tol::POINT_EPS`]: there is no
    /// meaningful direction to report, and returning a garbage direction here is
    /// how degenerate geometry propagates.
    #[must_use]
    pub fn normalized(self) -> Option<Self> {
        let len = self.length();
        if tol::is_zero_len(len) {
            None
        } else {
            Some(Self::new(self.x / len, self.y / len))
        }
    }

    #[inline]
    #[must_use]
    pub fn rotated(self, angle: f64) -> Self {
        let (s, c) = angle.sin_cos();
        Self::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }

    #[inline]
    #[must_use]
    pub fn is_zero(self) -> bool {
        tol::is_zero_len(self.length())
    }

    /// True when the two directions are parallel or antiparallel.
    #[must_use]
    pub fn is_parallel_to(self, o: Self) -> bool {
        match (self.normalized(), o.normalized()) {
            (Some(a), Some(b)) => a.cross(b).abs() <= tol::ANGLE_EPS,
            _ => false,
        }
    }
}

impl Point2 {
    pub const ORIGIN: Self = Self { x: 0.0, y: 0.0 };

    #[inline]
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[inline]
    #[must_use]
    pub fn distance_to(self, o: Self) -> f64 {
        (o - self).length()
    }

    #[inline]
    #[must_use]
    pub fn distance_sq_to(self, o: Self) -> f64 {
        (o - self).length_sq()
    }

    /// Coincidence within [`tol::POINT_EPS`]. Use this, never `==`.
    #[inline]
    #[must_use]
    pub fn coincides_with(self, o: Self) -> bool {
        self.distance_to(o) <= tol::POINT_EPS
    }

    #[inline]
    #[must_use]
    pub fn lerp(self, o: Self, t: f64) -> Self {
        Self::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }

    #[inline]
    #[must_use]
    pub fn midpoint(self, o: Self) -> Self {
        self.lerp(o, 0.5)
    }

    #[inline]
    #[must_use]
    pub fn to_vec(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    #[must_use]
    pub fn rotated_about(self, pivot: Self, angle: f64) -> Self {
        pivot + (self - pivot).rotated(angle)
    }

    /// Twice the signed area of triangle `(a, b, c)`. Positive when the points
    /// turn counter-clockwise. The building block for every orientation test.
    #[inline]
    #[must_use]
    pub fn orient2d(a: Self, b: Self, c: Self) -> f64 {
        (b - a).cross(c - a)
    }
}

impl Add<Vec2> for Point2 {
    type Output = Point2;
    #[inline]
    fn add(self, v: Vec2) -> Point2 {
        Point2::new(self.x + v.x, self.y + v.y)
    }
}

impl Sub<Vec2> for Point2 {
    type Output = Point2;
    #[inline]
    fn sub(self, v: Vec2) -> Point2 {
        Point2::new(self.x - v.x, self.y - v.y)
    }
}

impl Sub<Point2> for Point2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, o: Point2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl AddAssign<Vec2> for Point2 {
    #[inline]
    fn add_assign(&mut self, v: Vec2) {
        self.x += v.x;
        self.y += v.y;
    }
}

impl SubAssign<Vec2> for Point2 {
    #[inline]
    fn sub_assign(&mut self, v: Vec2) {
        self.x -= v.x;
        self.y -= v.y;
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn mul(self, s: f64) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
}

impl Mul<Vec2> for f64 {
    type Output = Vec2;
    #[inline]
    fn mul(self, v: Vec2) -> Vec2 {
        v * self
    }
}

impl Div<f64> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn div(self, s: f64) -> Vec2 {
        Vec2::new(self.x / s, self.y / s)
    }
}

impl Neg for Vec2 {
    type Output = Vec2;
    #[inline]
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_minus_point_is_a_displacement() {
        let a = Point2::new(10.0, 20.0);
        let b = Point2::new(13.0, 24.0);
        assert_eq!(b - a, Vec2::new(3.0, 4.0));
        assert!(tol::eq_len((b - a).length(), 5.0));
    }

    #[test]
    fn degenerate_vectors_have_no_direction() {
        assert!(Vec2::new(0.0, 0.0).normalized().is_none());
        assert!(Vec2::new(1e-9, 0.0).normalized().is_none());
        assert!(Vec2::new(1e-3, 0.0).normalized().is_some());
    }

    #[test]
    fn rotation_about_a_pivot_preserves_radius() {
        let pivot = Point2::new(100.0, 100.0);
        let p = Point2::new(150.0, 100.0);
        let r = p.rotated_about(pivot, std::f64::consts::FRAC_PI_2);
        assert!(r.coincides_with(Point2::new(100.0, 150.0)));
        assert!(tol::eq_len(r.distance_to(pivot), 50.0));
    }

    #[test]
    fn orientation_sign_follows_the_turn() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(10.0, 0.0);
        assert!(Point2::orient2d(a, b, Point2::new(5.0, 5.0)) > 0.0);
        assert!(Point2::orient2d(a, b, Point2::new(5.0, -5.0)) < 0.0);
        assert!(Point2::orient2d(a, b, Point2::new(5.0, 0.0)).abs() < tol::AREA_EPS);
    }

    #[test]
    fn parallel_detects_both_senses() {
        let a = Vec2::new(3.0, 4.0);
        assert!(a.is_parallel_to(Vec2::new(6.0, 8.0)));
        assert!(a.is_parallel_to(Vec2::new(-3.0, -4.0)));
        assert!(!a.is_parallel_to(Vec2::new(4.0, 3.0)));
        // A zero vector is parallel to nothing, including itself.
        assert!(!a.is_parallel_to(Vec2::ZERO));
    }
}
