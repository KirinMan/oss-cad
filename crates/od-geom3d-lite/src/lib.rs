//! A lightweight [`SolidKernel`] (ADR-002): sweep and triangulate, nothing
//! else.
//!
//! MEP ductwork and piping is almost entirely a closed profile carried along
//! a centreline — a duct run, a pipe run, most fittings — and none of that
//! needs a general boolean solid modeller. This kernel covers exactly that
//! case and refuses (`KernelError::Unsupported`, not a panic) anything a
//! real B-rep kernel would be needed for, on the understanding that a caller
//! falls back to one rather than treating the refusal as fatal — the whole
//! point of the split in ADR-002 is that most work never reaches that
//! fallback. No OCCT, no FFI, no C++ toolchain: this is what keeps a WASM
//! build of OpenDraft possible at all.
//!
//! ```
//! use od_geom3d::{Frame3, Point3, SolidKernel, SweepRequest, Vec3};
//! use od_geom3d_lite::LiteKernel;
//!
//! let profile = vec![
//!     Point3::new(-50.0, -25.0, 0.0),
//!     Point3::new(50.0, -25.0, 0.0),
//!     Point3::new(50.0, 25.0, 0.0),
//!     Point3::new(-50.0, 25.0, 0.0),
//! ];
//! let path = vec![
//!     Frame3::from_axes(Point3::ORIGIN, Vec3::X, Vec3::Z).expect("valid axes"),
//!     Frame3::from_axes(Point3::new(0.0, 1000.0, 0.0), Vec3::X, Vec3::Z).expect("valid axes"),
//! ];
//!
//! let mut kernel = LiteKernel::new();
//! let solid = kernel
//!     .sweep(&SweepRequest {
//!         profile,
//!         path_frames: path,
//!         capped: true,
//!     })
//!     .expect("a straight duct run sweeps");
//! let mesh = kernel.triangulate(solid, 0.1).expect("triangulates");
//! let data = kernel.mesh_data(mesh).expect("just created");
//! assert!(data.is_well_formed());
//! ```

use od_geom3d::{Aabb3, KernelError, MeshData, MeshHandle, Point3, Result, SolidHandle};

/// A swept solid, as this kernel actually represents one: the world-space
/// vertex ring at every path frame. Enough to derive bounds and a mesh from;
/// nothing here claims to be a real boundary representation (no shared
/// topology, no half-edges) — a caller that needs one uses the OCCT-backed
/// kernel instead.
#[derive(Debug, Clone)]
struct LiteSolid {
    rings: Vec<Vec<Point3>>,
    capped: bool,
}

/// The default [`SolidKernel`] (ADR-002): sweep and triangulate a closed,
/// convex profile along a path. See the module docs for what it deliberately
/// does not do.
#[derive(Debug, Default)]
pub struct LiteKernel {
    solids: std::collections::HashMap<SolidHandle, LiteSolid>,
    meshes: std::collections::HashMap<MeshHandle, MeshData>,
    next_solid: u64,
    next_mesh: u64,
}

impl LiteKernel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl od_geom3d::SolidKernel for LiteKernel {
    fn name(&self) -> &'static str {
        "od-geom3d-lite"
    }

    fn sweep(&mut self, req: &od_geom3d::SweepRequest) -> Result<SolidHandle> {
        if req.profile.len() < 3 {
            return Err(KernelError::Degenerate(
                "a swept profile needs at least 3 points".into(),
            ));
        }
        if req.path_frames.len() < 2 {
            return Err(KernelError::Degenerate(
                "a sweep needs at least 2 path frames".into(),
            ));
        }
        let rings: Vec<Vec<Point3>> = req
            .path_frames
            .iter()
            .map(|frame| {
                req.profile
                    .iter()
                    .map(|p| frame.local_to_world(*p))
                    .collect()
            })
            .collect();

        let id = SolidHandle(self.next_solid);
        self.next_solid += 1;
        self.solids.insert(
            id,
            LiteSolid {
                rings,
                capped: req.capped,
            },
        );
        Ok(id)
    }

    fn boolean(
        &mut self,
        _op: od_geom3d::BooleanOp,
        _a: SolidHandle,
        _b: SolidHandle,
    ) -> Result<SolidHandle> {
        Err(KernelError::Unsupported(
            "boolean operations need a full B-rep kernel",
        ))
    }

    fn bounds(&self, solid: SolidHandle) -> Result<Aabb3> {
        let s = self
            .solids
            .get(&solid)
            .ok_or(KernelError::UnknownHandle(solid))?;
        Ok(Aabb3::from_points(s.rings.iter().flatten().copied()))
    }

    /// `sag` is accepted (the trait's own contract) but unused: a swept
    /// profile's rings are already exactly where the caller placed them, so
    /// there is no curved surface here to approximate more or less finely —
    /// the sweep itself is the only place curvature enters (a round pipe's
    /// own profile, tessellated by the caller before it ever reaches this
    /// kernel), and that choice is the caller's, not this kernel's to
    /// second-guess.
    fn triangulate(&mut self, solid: SolidHandle, _sag: f64) -> Result<MeshHandle> {
        let s = self
            .solids
            .get(&solid)
            .ok_or(KernelError::UnknownHandle(solid))?;
        let mesh = triangulate_swept(&s.rings, s.capped);
        let id = MeshHandle(self.next_mesh);
        self.next_mesh += 1;
        self.meshes.insert(id, mesh);
        Ok(id)
    }

    fn mesh_data(&self, mesh: MeshHandle) -> Result<&MeshData> {
        self.meshes.get(&mesh).ok_or(KernelError::UnknownMesh(mesh))
    }

    fn free(&mut self, solid: SolidHandle) {
        self.solids.remove(&solid);
    }
}

/// Builds a side wall (one quad, split into two triangles, per span per
/// profile edge) between consecutive rings, and — when `capped` — flat fan
/// caps on the first and last ring.
///
/// Every ring gets its own vertex block rather than sharing vertices across
/// rings: simpler to build correctly, at the cost of some duplication a
/// renderer or exporter is free to weld back down later if it cares to. Fan
/// triangulation for the caps assumes a convex profile in the order given
/// (true for the rectangular and polygon-tessellated-round profiles MEP
/// ducts and pipes actually use); a concave profile would triangulate
/// incorrectly, and this kernel has no way to detect that case honestly, so
/// it is not attempted.
fn triangulate_swept(rings: &[Vec<Point3>], capped: bool) -> MeshData {
    let n = rings[0].len();
    let mut positions = Vec::with_capacity(rings.len() * n);
    for ring in rings {
        positions.extend_from_slice(ring);
    }

    let mut indices = Vec::new();
    for span in 0..rings.len() - 1 {
        let base0 = idx(span, 0, n);
        let base1 = idx(span + 1, 0, n);
        for j in 0..n {
            let j2 = (j + 1) % n;
            let a = base0 + as_u32(j);
            let b = base0 + as_u32(j2);
            let c = base1 + as_u32(j2);
            let d = base1 + as_u32(j);
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }

    if capped {
        // Start cap faces "backward" along the path, so its fan winds the
        // opposite way from the end cap's for both to face outward.
        let base_start = idx(0, 0, n);
        for j in 1..n - 1 {
            indices.extend_from_slice(&[
                base_start,
                base_start + as_u32(j + 1),
                base_start + as_u32(j),
            ]);
        }
        let base_end = idx(rings.len() - 1, 0, n);
        for j in 1..n - 1 {
            indices.extend_from_slice(&[base_end, base_end + as_u32(j), base_end + as_u32(j + 1)]);
        }
    }

    MeshData { positions, indices }
}

fn idx(ring: usize, vertex: usize, ring_size: usize) -> u32 {
    as_u32(ring * ring_size + vertex)
}

/// Mesh index counts stay in the thousands even for an elaborate fitting, so
/// this never actually saturates — going through `u32::try_from` makes that
/// a checked fact rather than an assumption, the same pattern used
/// throughout this workspace for narrowing conversions clippy would
/// otherwise deny outright.
fn as_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_geom3d::{Frame3, SolidKernel, SweepRequest, Vec3};

    fn rect_profile() -> Vec<Point3> {
        vec![
            Point3::new(-50.0, -25.0, 0.0),
            Point3::new(50.0, -25.0, 0.0),
            Point3::new(50.0, 25.0, 0.0),
            Point3::new(-50.0, 25.0, 0.0),
        ]
    }

    fn straight_path(length: f64) -> Vec<Frame3> {
        vec![
            Frame3::from_axes(Point3::ORIGIN, Vec3::X, Vec3::Z).expect("valid axes"),
            Frame3::from_axes(Point3::new(0.0, length, 0.0), Vec3::X, Vec3::Z).expect("valid axes"),
        ]
    }

    #[test]
    fn a_straight_duct_sweeps_and_triangulates() {
        let mut k = LiteKernel::new();
        let solid = k
            .sweep(&SweepRequest {
                profile: rect_profile(),
                path_frames: straight_path(1000.0),
                capped: true,
            })
            .expect("sweeps");

        let bounds = k.bounds(solid).expect("known handle");
        assert!((bounds.size().x - 100.0).abs() < 1e-9);
        assert!((bounds.size().y - 1000.0).abs() < 1e-9);
        assert!((bounds.size().z - 50.0).abs() < 1e-9);

        let mesh = k.triangulate(solid, 0.1).expect("triangulates");
        let data = k.mesh_data(mesh).expect("just created");
        assert!(data.is_well_formed());
        // 4 side walls x 2 triangles, plus 2 triangles per cap x 2 caps.
        assert_eq!(data.triangle_count(), 4 * 2 + 2 * 2);
        assert_eq!(data.positions.len(), 4 * 2);
    }

    #[test]
    fn an_uncapped_sweep_has_no_end_faces() {
        let mut k = LiteKernel::new();
        let solid = k
            .sweep(&SweepRequest {
                profile: rect_profile(),
                path_frames: straight_path(1000.0),
                capped: false,
            })
            .expect("sweeps");
        let mesh = k.triangulate(solid, 0.1).expect("triangulates");
        let data = k.mesh_data(mesh).expect("just created");
        assert_eq!(data.triangle_count(), 4 * 2);
    }

    #[test]
    fn a_multi_span_path_sweeps_every_span() {
        let mut k = LiteKernel::new();
        let path = vec![
            Frame3::from_axes(Point3::ORIGIN, Vec3::X, Vec3::Z).expect("valid axes"),
            Frame3::from_axes(Point3::new(0.0, 500.0, 0.0), Vec3::X, Vec3::Z).expect("valid axes"),
            Frame3::from_axes(Point3::new(0.0, 1000.0, 0.0), Vec3::X, Vec3::Z).expect("valid axes"),
        ];
        let solid = k
            .sweep(&SweepRequest {
                profile: rect_profile(),
                path_frames: path,
                capped: false,
            })
            .expect("sweeps");
        let mesh = k.triangulate(solid, 0.1).expect("triangulates");
        let data = k.mesh_data(mesh).expect("just created");
        // 2 spans x 4 walls x 2 triangles.
        assert_eq!(data.triangle_count(), 2 * 4 * 2);
    }

    #[test]
    fn a_short_profile_is_rejected_as_degenerate() {
        let mut k = LiteKernel::new();
        let err = k
            .sweep(&SweepRequest {
                profile: vec![Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0)],
                path_frames: straight_path(1000.0),
                capped: false,
            })
            .expect_err("a 2-point profile is not a closed shape");
        assert!(matches!(err, KernelError::Degenerate(_)));
    }

    #[test]
    fn a_single_frame_path_is_rejected_as_degenerate() {
        let mut k = LiteKernel::new();
        let err = k
            .sweep(&SweepRequest {
                profile: rect_profile(),
                path_frames: vec![Frame3::WORLD],
                capped: false,
            })
            .expect_err("a sweep needs somewhere to go");
        assert!(matches!(err, KernelError::Degenerate(_)));
    }

    #[test]
    fn boolean_operations_are_explicitly_unsupported_not_a_panic() {
        let mut k = LiteKernel::new();
        let a = k
            .sweep(&SweepRequest {
                profile: rect_profile(),
                path_frames: straight_path(1000.0),
                capped: true,
            })
            .expect("sweeps");
        let b = k
            .sweep(&SweepRequest {
                profile: rect_profile(),
                path_frames: straight_path(500.0),
                capped: true,
            })
            .expect("sweeps");
        let err = k
            .boolean(od_geom3d::BooleanOp::Union, a, b)
            .expect_err("this kernel never supports booleans");
        assert!(matches!(err, KernelError::Unsupported(_)));
    }

    #[test]
    fn freeing_a_solid_makes_it_an_unknown_handle() {
        let mut k = LiteKernel::new();
        let solid = k
            .sweep(&SweepRequest {
                profile: rect_profile(),
                path_frames: straight_path(1000.0),
                capped: true,
            })
            .expect("sweeps");
        k.free(solid);
        assert!(matches!(
            k.bounds(solid),
            Err(KernelError::UnknownHandle(_))
        ));
    }

    #[test]
    fn an_unknown_mesh_handle_is_a_named_error() {
        let k = LiteKernel::new();
        let err = k.mesh_data(MeshHandle(999)).expect_err("never created");
        assert!(matches!(err, KernelError::UnknownMesh(_)));
    }
}
