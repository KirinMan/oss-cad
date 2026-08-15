//! 2D geometry for OpenDraft.
//!
//! Pure functions over plain data: no document model, no I/O, no global state.
//! Everything here is in drawing space — millimetres stored as `f64` — and every
//! comparison goes through [`tol`].
//!
//! ```
//! use od_geom2d::{Point2, Segment2, curve};
//!
//! let a = Segment2::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
//! let b = Segment2::new(Point2::new(0.0, 10.0), Point2::new(10.0, 0.0));
//! let hits = curve::segment_segment(&a, &b);
//! assert!(hits[0].coincides_with(Point2::new(5.0, 5.0)));
//! ```

pub mod aabb;
pub mod curve;
pub mod point;
pub mod polyline;
pub mod tol;

pub use aabb::Aabb2;
pub use curve::{Arc2, Circle2, Segment2};
pub use point::{Point2, Vec2};
pub use polyline::{Polyline2, Span, Vertex};
