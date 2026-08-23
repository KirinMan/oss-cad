//! The OpenDraft document model.
//!
//! This crate knows nothing about rendering, file formats, or any engineering
//! domain. It holds the drawing and the rules that keep it consistent; every
//! other layer is built on top of it and none of them are named here. That
//! constraint is enforced in CI, because it is the property that decides
//! whether a second domain can ever be added (`docs/02-architecture.md`).
//!
//! ```
//! use od_core::{ActorId, Database, Document, Entity, Geometry, Point3};
//!
//! let mut doc = Document::new(Database::new(ActorId::SYSTEM));
//! let layer = doc.db.ensure_layer("A-Wall");
//! let space = doc.db.model_space();
//!
//! let id = doc
//!     .edit("Draw wall", |tx| {
//!         tx.add_entity(Entity::new(
//!             layer,
//!             space,
//!             Geometry::Line {
//!                 a: Point3::ORIGIN,
//!                 b: Point3::new(3600.0, 0.0, 0.0),
//!             },
//!         ))
//!     })
//!     .expect("the edit is valid");
//!
//! assert_eq!(doc.db.entities().count(), 1);
//! assert_eq!(doc.undo().as_deref(), Some("Draw wall"));
//! assert_eq!(doc.db.entities().count(), 0);
//! # let _ = id;
//! ```

pub mod database;
pub mod edit;
pub mod entity;
pub mod error;
pub mod id;
pub mod style;
pub mod tables;
pub mod transaction;
pub mod xdata;

pub use database::{
    Database, DatabaseSnapshot, DbStats, Dictionary, GeoRef, HeaderVars, MODEL_SPACE, Object,
    ObjectKind, PAPER_SPACE, PreservedBlob, Units,
};
pub use edit::{Command, CommandOutcome};
pub use entity::{
    AttributeValue, BlockRef, Entity, Geometry, HAlign, Hatch, HatchPattern, MTextEntity,
    ProxyGraphic, TextEntity, TextFlow, VAlign,
};
pub use error::{DbError, Result};
pub use id::{ActorId, IdGenerator, ObjectId};
pub use style::{Color, GraphicStyle, LineWeight, ResolvedStyle};
pub use tables::{
    BlockKind, BlockRecord, DimStyle, GridAxis, Layer, Level, LineType, SymbolTables, Table,
    TextStyle, XrefSpec,
};
pub use transaction::{Batch, Change, Document, History, Transaction};
pub use xdata::{AppId, FieldDef, FieldType, Record, SchemaError, Value, XDataMap, XDataSchema};

// Re-exported so downstream crates get one consistent geometry vocabulary
// rather than each picking a version of the geometry crates.
pub use od_geom2d::{Aabb2, Arc2, Circle2, Point2, Polyline2, Segment2, Span, Vec2, Vertex, tol};
pub use od_geom3d::{Aabb3, Frame3, Point3, SolidHandle, Vec3};
