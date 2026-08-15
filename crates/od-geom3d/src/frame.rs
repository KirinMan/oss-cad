//! Right-handed orthonormal frames.
//!
//! A frame is how a placement is expressed everywhere in OpenDraft: where a
//! block sits, which way an equipment connector faces, what plane a UCS defines.
//! Storing an origin plus two axes (rather than a 4×4 matrix) keeps the
//! orthonormality invariant checkable and the serialised form readable.

use crate::point::{Point3, Vec3};
use od_geom2d::tol;

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Frame3 {
    pub origin: Point3,
    x_axis: Vec3,
    y_axis: Vec3,
}

impl Frame3 {
    pub const WORLD: Self = Self {
        origin: Point3::ORIGIN,
        x_axis: Vec3::X,
        y_axis: Vec3::Y,
    };

    /// Builds a frame from an origin and a normal, choosing the in-plane axes
    /// arbitrarily but deterministically. Enough for "a port faces this way".
    #[must_use]
    pub fn from_normal(origin: Point3, normal: Vec3) -> Option<Self> {
        let z = normal.normalized()?;
        let x = z.any_perpendicular()?;
        Some(Self {
            origin,
            x_axis: x,
            y_axis: z.cross(x),
        })
    }

    /// Builds a frame from an origin and two axes. `y_hint` is orthogonalised
    /// against `x`, so callers may pass an approximate up direction.
    /// Returns `None` if the axes are parallel or degenerate.
    #[must_use]
    pub fn from_axes(origin: Point3, x: Vec3, y_hint: Vec3) -> Option<Self> {
        let x = x.normalized()?;
        let z = x.cross(y_hint).normalized()?;
        Some(Self {
            origin,
            x_axis: x,
            y_axis: z.cross(x),
        })
    }

    #[inline]
    #[must_use]
    pub fn x_axis(&self) -> Vec3 {
        self.x_axis
    }

    #[inline]
    #[must_use]
    pub fn y_axis(&self) -> Vec3 {
        self.y_axis
    }

    #[inline]
    #[must_use]
    pub fn normal(&self) -> Vec3 {
        self.x_axis.cross(self.y_axis)
    }

    /// Local coordinates to world.
    #[must_use]
    pub fn local_to_world(&self, local: Point3) -> Point3 {
        self.origin + self.x_axis * local.x + self.y_axis * local.y + self.normal() * local.z
    }

    /// World coordinates to local.
    #[must_use]
    pub fn world_to_local(&self, world: Point3) -> Point3 {
        let d = world - self.origin;
        Point3::new(d.dot(self.x_axis), d.dot(self.y_axis), d.dot(self.normal()))
    }

    /// The same frame turned to face the opposite way, which is what connecting
    /// two ports requires: they must be coincident and anti-parallel.
    #[must_use]
    pub fn reversed(&self) -> Self {
        Self {
            origin: self.origin,
            x_axis: -self.x_axis,
            y_axis: self.y_axis,
        }
    }

    /// Checks the invariant this type exists to protect.
    #[must_use]
    pub fn is_orthonormal(&self) -> bool {
        tol::eq_len(self.x_axis.length(), 1.0)
            && tol::eq_len(self.y_axis.length(), 1.0)
            && self.x_axis.dot(self.y_axis).abs() <= tol::POINT_EPS
    }
}

impl Default for Frame3 {
    fn default() -> Self {
        Self::WORLD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_frame_is_identity() {
        let f = Frame3::WORLD;
        let p = Point3::new(1.0, 2.0, 3.0);
        assert!(f.local_to_world(p).coincides_with(p));
        assert!(f.world_to_local(p).coincides_with(p));
        assert!(f.is_orthonormal());
    }

    #[test]
    fn local_and_world_round_trip() {
        let f = Frame3::from_axes(
            Point3::new(100.0, 200.0, 300.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::Z,
        )
        .expect("independent axes");
        assert!(f.is_orthonormal());
        let world = Point3::new(-5.0, 42.0, 7.5);
        assert!(
            f.local_to_world(f.world_to_local(world))
                .coincides_with(world)
        );
    }

    #[test]
    fn parallel_axes_have_no_frame() {
        assert!(Frame3::from_axes(Point3::ORIGIN, Vec3::X, Vec3::X).is_none());
        assert!(Frame3::from_normal(Point3::ORIGIN, Vec3::ZERO).is_none());
    }

    #[test]
    fn reversing_flips_the_normal() {
        let f = Frame3::from_normal(Point3::ORIGIN, Vec3::Z).expect("valid");
        let r = f.reversed();
        assert!(r.is_orthonormal());
        assert!(tol::eq_len(f.normal().dot(r.normal()), -1.0));
    }
}
