//! The mechanical/electrical/plumbing domain.
//!
//! `docs/04-mep.md` states the idea this crate exists to carry out: **the
//! truth is centreline + profile + system; every view is derived from it.**
//! Everything here — [`model::RouteSegment`], [`model::PlacedPart`], the
//! connection graph, the router — is built entirely on `od-core`'s public
//! extension mechanism (`Database::insert_custom`,
//! `Database::entity`/`insert_entity`). `od-core` itself is never touched to
//! add a domain concept, and CI checks that it stays that way
//! (CLAUDE.md rule 1).
//!
//! # What this crate does not do yet
//!
//! - **Placement is planar.** A [`model::PlacedPart`] turns about +Z only. A
//!   riser, or a fitting that changes elevation, needs a full 3D orientation
//!   this does not express. Every route in one call is required to lie in a
//!   single horizontal plane for the same reason.
//! - **Automatic fitting insertion handles 90° bends only.** Any other angle
//!   is a [`model::MepError::UnsupportedBendAngle`] rather than a silently
//!   wrong elbow — place the fitting by hand with [`model::PlacedPart`]
//!   instead.
//! - **No regeneration pass.** Derived geometry is produced once, alongside
//!   its source, by the same call that creates the source
//!   ([`derive::derive_route`], [`derive::derive_part`]). There is no editor
//!   yet to make anything dirty, so there is nothing to regenerate against —
//!   this is a deliberate simplification, not an oversight, and the source
//!   objects hold everything needed to add it later.
//!
//! ```
//! use od_core::{ActorId, Database, Document};
//! use od_parts::Catalog;
//! use od_domain_mep::{model::RouteSegment, route};
//!
//! let catalog = Catalog::bundled()?;
//! let mut doc = Document::new(Database::new(ActorId::SYSTEM));
//!
//! let route = route::RouteSpec {
//!     system: "sys.air.supply".into(),
//!     spec: "spec.duct.galvanised.rect".into(),
//!     profile: od_parts::Profile::Rect { w: 400.0, h: 300.0 },
//!     path: vec![
//!         od_core::Point3::new(0.0, 0.0, 2800.0),
//!         od_core::Point3::new(5000.0, 0.0, 2800.0),
//!         od_core::Point3::new(5000.0, 3000.0, 2800.0),
//!     ],
//! };
//!
//! let result = doc.edit("Draw duct run", |tx| {
//!     route::draw_route(tx, &catalog, &route)
//! })?;
//!
//! assert_eq!(result.segments.len(), 2, "one straight run either side of the bend");
//! assert_eq!(result.fittings.len(), 1, "the router inserted the elbow");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod autoroute;
pub mod clash;
pub mod derive;
pub mod graph;
pub mod model;
pub mod ports;
pub mod route;
pub mod store;
pub mod takeoff;

pub use model::{MepError, PlacedPart, PlacementKind, Result, RouteSegment, WorldPort};
