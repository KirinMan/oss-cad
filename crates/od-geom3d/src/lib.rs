//! 3D primitives for OpenDraft.
//!
//! Deliberately small. Per ADR-002 the B-rep kernel is not written here — it is
//! reached through [`SolidKernel`], whose default implementation will bridge to
//! OCCT and whose light implementation covers the sweeps that MEP routing needs.
//! What lives in this crate is the vocabulary every layer shares: positions,
//! directions, frames and bounds.

pub mod aabb;
pub mod frame;
pub mod kernel;
pub mod point;

pub use aabb::Aabb3;
pub use frame::Frame3;
pub use kernel::{
    BooleanOp, KernelError, MeshData, MeshHandle, Result, SolidHandle, SolidKernel, SweepRequest,
};
pub use point::{Point3, Vec3};
