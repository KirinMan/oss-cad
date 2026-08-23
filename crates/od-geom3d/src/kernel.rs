//! The solid kernel boundary (ADR-002).
//!
//! Nothing above this crate may name OCCT. Callers hold opaque handles and go
//! through [`SolidKernel`]; the implementation behind it can be the light sweep
//! kernel (enough for ducts, pipes and fittings, and small enough to ship to a
//! browser) or an OCCT bridge, chosen at runtime. Keeping the boundary here is
//! what makes the WASM build possible and what leaves room to replace OCCT
//! later without touching the document model.

use crate::aabb::Aabb3;
use crate::frame::Frame3;
use crate::point::Point3;

/// An opaque reference to a solid owned by a kernel. Handles are only
/// meaningful to the kernel that issued them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SolidHandle(pub u64);

/// An opaque reference to a triangulated mesh owned by a kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MeshHandle(pub u64);

/// A closed profile swept along a path — the shape of essentially every duct,
/// pipe and cable tray in a building.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepRequest {
    /// Cross-section, in the profile plane (local XY of `path_frames[0]`).
    pub profile: Vec<Point3>,
    /// Frames along the path. Consecutive frames define one swept span each.
    pub path_frames: Vec<Frame3>,
    pub capped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanOp {
    Union,
    Difference,
    Intersection,
}

#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("solid handle {0:?} is not known to this kernel")]
    UnknownHandle(SolidHandle),
    #[error("degenerate input: {0}")]
    Degenerate(String),
    /// Returned by the light kernel for operations only a full B-rep can do.
    /// Callers are expected to fall back rather than fail: the point of the
    /// split is that most work never needs the heavy kernel.
    #[error("operation not supported by this kernel: {0}")]
    Unsupported(&'static str),
}

pub type Result<T> = std::result::Result<T, KernelError>;

/// Operations the document model may ask of a solid modeller.
pub trait SolidKernel: std::fmt::Debug + Send + Sync {
    /// Human-readable identity, for diagnostics and file provenance.
    fn name(&self) -> &'static str;

    fn sweep(&mut self, req: &SweepRequest) -> Result<SolidHandle>;

    fn boolean(&mut self, op: BooleanOp, a: SolidHandle, b: SolidHandle) -> Result<SolidHandle>;

    fn bounds(&self, solid: SolidHandle) -> Result<Aabb3>;

    /// Triangulates for display. `sag` is the maximum deviation in millimetres.
    fn triangulate(&mut self, solid: SolidHandle, sag: f64) -> Result<MeshHandle>;

    fn mesh_data(&self, mesh: MeshHandle) -> Result<&MeshData>;

    fn free(&mut self, solid: SolidHandle);
}

/// Triangles ready for the renderer or an exporter.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshData {
    pub positions: Vec<Point3>,
    /// Triangle indices into `positions`, three per face.
    pub indices: Vec<u32>,
}

impl MeshData {
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    #[must_use]
    pub fn bounds(&self) -> Aabb3 {
        Aabb3::from_points(self.positions.iter().copied())
    }

    /// True when every index addresses a real vertex and the count is a
    /// multiple of three. Exporters rely on this; a kernel bridge that returns
    /// otherwise is a bug worth catching at the boundary.
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        self.indices.len() % 3 == 0
            && self
                .indices
                .iter()
                .all(|i| (*i as usize) < self.positions.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_rejects_dangling_indices() {
        let mut m = MeshData {
            positions: vec![
                Point3::ORIGIN,
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(0.0, 1.0, 0.0),
            ],
            indices: vec![0, 1, 2],
        };
        assert!(m.is_well_formed());
        assert_eq!(m.triangle_count(), 1);

        m.indices.push(3);
        assert!(
            !m.is_well_formed(),
            "index count is no longer a multiple of 3"
        );

        m.indices = vec![0, 1, 9];
        assert!(!m.is_well_formed(), "index 9 has no vertex");
    }
}
