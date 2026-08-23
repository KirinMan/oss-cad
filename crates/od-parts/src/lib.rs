//! The MEP part library.
//!
//! The hardest thing to compete with in this market is not a feature — it is a
//! vendor's catalogue of tens of thousands of modelled products (R-1 in
//! `docs/06-roadmap.md`). The answer here is to be parametric rather than
//! enumerated: one definition of a rectangular elbow, written in terms of `W`,
//! `H` and `R`, covers every size that will ever be drawn, and a starter
//! catalogue covering the common trades ships inside the binary.
//!
//! That does not replace manufacturer data — a specified product still needs
//! its real dimensions. It does mean a drawing can be produced on day one,
//! offline, by someone who has signed nothing.
//!
//! ```
//! use od_parts::Catalog;
//! use std::collections::HashMap;
//!
//! let catalog = Catalog::bundled().expect("the bundled catalogue is valid");
//!
//! let mut params = HashMap::new();
//! params.insert("W".to_owned(), 500.0);
//! params.insert("H".to_owned(), 300.0);
//! let elbow = catalog.instantiate("duct.elbow.rect.90", &params).expect("builds");
//!
//! assert_eq!(elbow.ports.len(), 2);
//! assert!(!elbow.bounds().is_empty());
//! ```

pub mod catalog;
pub mod expr;
pub mod model;

pub use catalog::{Catalog, CatalogError, CatalogFile, Instance, Port, Profile, Solid};
pub use expr::{Expr, ExprError, Value};
pub use model::{
    Body3d, Category, Localized, Parameter, Part, PortDef, ProfileDef, PropertyDef, Shape2d,
    SizeEntry, Spec, SystemDef, SystemKind,
};
