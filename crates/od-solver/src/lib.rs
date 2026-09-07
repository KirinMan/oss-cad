//! 2D geometric constraint solving for OpenDraft (F-012, ADR-003).
//!
//! Pure numerics over a flat parameter vector: no document model, no
//! entities, no I/O. A [`System`] is built from [`ParamId`]/[`PointId`]/
//! [`CircleId`] handles and [`Constraint`]s; [`System::solve`] adjusts every
//! parameter to satisfy them (Levenberg-Marquardt), and [`System::analyze`]
//! reports the remaining degrees of freedom plus any redundant or
//! conflicting constraint, from the solved Jacobian's numerical rank —
//! PlaneGCS's own approach (ADR-003), reimplemented rather than linked in,
//! to keep the LGPL dependency and its WASM weight out of the workspace.
//!
//! Wiring a `System` to real drawing entities — so dragging a point in the
//! editor re-solves a sketch live — is `od-core`/`od-app` work this crate
//! does not do; nothing in the workspace depends on `od-solver` yet
//! (`docs/02-architecture.md`'s own dependency table only allows
//! `od-geom2d` and `nalgebra` the other way).
//!
//! ```
//! use od_solver::{Constraint, System};
//!
//! let mut sys = System::new();
//! let a = sys.add_point(0.0, 0.0);
//! let b = sys.add_point(3.0, 4.0);
//! sys.fix_point(a, 0.0, 0.0);
//! // Pin the length to 5, keep it a straight horizontal run.
//! sys.add_constraint(Constraint::Distance { a, b, target: 5.0 });
//! sys.add_constraint(Constraint::Horizontal { a, b });
//!
//! let report = sys.solve();
//! assert!(report.converged);
//! let p = sys.point(b);
//! assert!((p.x - 5.0).abs() < 1e-6);
//! assert!(p.y.abs() < 1e-6);
//! ```

mod constraint;
mod solve;
mod system;

pub use constraint::Constraint;
pub use solve::{DofReport, SolveReport};
pub use system::{CircleId, ParamId, PointId, System};
