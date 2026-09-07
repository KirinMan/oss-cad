//! Spatial indexing.
//!
//! One data structure answers the four questions a CAD session asks constantly:
//! what is on screen, what is under the cursor, what should the cursor snap to,
//! and which parts might be clashing. All four are "what is near here", and all
//! four are unusable at drawing scale without an index (`docs/02-architecture.md`
//! §6).
//!
//! This crate is deliberately independent of the document model: it indexes
//! bounds and hands back whatever value was stored with them. That keeps the
//! index out of [`od_core::Database`] — it is derived data, rebuildable from the
//! document, and something that must not be part of what a document *is*.
//!
//! ```
//! use od_index::{Entry, RTree};
//! use od_geom3d::{Aabb3, Point3};
//!
//! let tree = RTree::bulk_load((0..1000).map(|i| Entry {
//!     bounds: Aabb3::new(
//!         Point3::new(f64::from(i) * 100.0, 0.0, 0.0),
//!         Point3::new(f64::from(i) * 100.0 + 50.0, 50.0, 0.0),
//!     ),
//!     value: i,
//! }));
//!
//! let onscreen = tree.query(Aabb3::new(
//!     Point3::new(0.0, 0.0, -1.0),
//!     Point3::new(500.0, 100.0, 1.0),
//! ));
//! assert_eq!(onscreen.len(), 6);
//! ```

pub mod drawing;
pub mod rtree;

pub use drawing::DrawingIndex;
pub use rtree::{Entry, RTree};
