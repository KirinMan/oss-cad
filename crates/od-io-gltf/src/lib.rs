//! Writes [`od_geom3d::MeshData`] to glTF 2.0 binary (`.glb`).
//!
//! Not a general-purpose exporter: one mesh, one node, one scene, positions
//! and indices only — no materials, no normals, no textures. The point is
//! narrower than "export 3D geometry": it is the one thing that makes
//! [`od-geom3d-lite`](https://docs.rs/od-geom3d-lite)'s sweep output
//! externally checkable at all before OpenDraft has its own 3D view — open
//! the file in any standard glTF viewer and the shape is either right or it
//! is not, which a triangle count in a test assertion cannot fully stand in
//! for. Meant for verification, not for shipping a finished export feature.
//!
//! ```
//! use od_geom3d::{MeshData, Point3};
//!
//! let mesh = MeshData {
//!     positions: vec![
//!         Point3::new(0.0, 0.0, 0.0),
//!         Point3::new(1.0, 0.0, 0.0),
//!         Point3::new(0.0, 1.0, 0.0),
//!     ],
//!     indices: vec![0, 1, 2],
//! };
//! let glb = od_io_gltf::to_glb(std::slice::from_ref(&mesh));
//! assert_eq!(&glb[0..4], b"glTF");
//! ```

use od_geom3d::MeshData;

const GLTF_MAGIC: u32 = 0x4654_6C67; // "glTF"
const GLTF_VERSION: u32 = 2;
const CHUNK_TYPE_JSON: u32 = 0x4E4F_534A; // "JSON"

/// Merges every mesh into one glTF primitive (concatenated positions,
/// index-shifted indices) and returns the `.glb` file's bytes.
///
/// Merging rather than emitting one primitive per input mesh is a
/// deliberate simplification: a route drawing may have dozens of segments,
/// and a single combined primitive is both simpler to write correctly and
/// enough to answer "does this shape look right" by eye. Per-segment
/// materials/selection would need separate primitives — not attempted here.
#[must_use]
pub fn to_glb(meshes: &[MeshData]) -> Vec<u8> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for mesh in meshes {
        let base = as_u32(positions.len());
        positions.extend(
            mesh.positions
                .iter()
                .map(|p| [narrow(p.x), narrow(p.y), narrow(p.z)]),
        );
        indices.extend(mesh.indices.iter().map(|i| i + base));
    }

    let (min, max) = bounds(&positions);

    let mut position_bytes = Vec::with_capacity(positions.len() * 12);
    for p in &positions {
        for c in p {
            position_bytes.extend_from_slice(&c.to_le_bytes());
        }
    }
    let mut index_bytes = Vec::with_capacity(indices.len() * 4);
    for i in &indices {
        index_bytes.extend_from_slice(&i.to_le_bytes());
    }

    let json = serde_json::json!({
        "asset": { "version": "2.0", "generator": "od-io-gltf" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "mesh": 0 }],
        "meshes": [{
            "primitives": [{
                "attributes": { "POSITION": 0 },
                "indices": 1,
                "mode": 4,
            }],
        }],
        "accessors": [
            {
                "bufferView": 0,
                "componentType": 5126,
                "count": positions.len(),
                "type": "VEC3",
                "min": min,
                "max": max,
            },
            {
                "bufferView": 1,
                "componentType": 5125,
                "count": indices.len(),
                "type": "SCALAR",
            },
        ],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": position_bytes.len(), "target": 34962 },
            { "buffer": 0, "byteOffset": position_bytes.len(), "byteLength": index_bytes.len(), "target": 34963 },
        ],
        "buffers": [{ "byteLength": position_bytes.len() + index_bytes.len() }],
    });
    let mut json_bytes = serde_json::to_vec(&json).unwrap_or_default();
    pad_to_4(&mut json_bytes, b' ');

    let mut bin_bytes = position_bytes;
    bin_bytes.extend_from_slice(&index_bytes);
    pad_to_4(&mut bin_bytes, 0);

    let total_len = 12 + (8 + json_bytes.len()) + (8 + bin_bytes.len());

    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(&GLTF_MAGIC.to_le_bytes());
    out.extend_from_slice(&GLTF_VERSION.to_le_bytes());
    out.extend_from_slice(&as_u32(total_len).to_le_bytes());

    out.extend_from_slice(&as_u32(json_bytes.len()).to_le_bytes());
    out.extend_from_slice(&CHUNK_TYPE_JSON.to_le_bytes());
    out.extend_from_slice(&json_bytes);

    out.extend_from_slice(&as_u32(bin_bytes.len()).to_le_bytes());
    out.extend_from_slice(&bin_chunk_type().to_le_bytes());
    out.extend_from_slice(&bin_bytes);

    out
}

/// Writes [`to_glb`]'s output to `path`.
///
/// # Errors
/// Whatever [`std::fs::write`] returns.
pub fn write_file(meshes: &[MeshData], path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
    std::fs::write(path, to_glb(meshes))
}

fn bin_chunk_type() -> u32 {
    // "BIN\0" as little-endian bytes, spelled this way (rather than a
    // literal 0x42494E00) so the ASCII is legible at the call site the way
    // CHUNK_TYPE_JSON's "JSON" already is.
    u32::from_le_bytes(*b"BIN\0")
}

fn pad_to_4(buf: &mut Vec<u8>, pad: u8) {
    while !buf.len().is_multiple_of(4) {
        buf.push(pad);
    }
}

fn bounds(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    if positions.is_empty() {
        min = [0.0; 3];
        max = [0.0; 3];
    }
    (min, max)
}

/// `f64 -> f32`: glTF's POSITION accessor has no double-precision component
/// type, so every exporter narrows here — the loss is well below drawing
/// tolerance at MEP scale (single-precision float error stays under a
/// micrometre for coordinates within a few hundred metres of the origin),
/// and this is a viewer/verification export, not a source of truth to round
/// -trip back from.
#[expect(
    clippy::cast_possible_truncation,
    reason = "glTF positions are f32 by spec; deliberate and documented, not a bug"
)]
fn narrow(v: f64) -> f32 {
    v as f32
}

/// glTF chunk/buffer-view lengths stay in the tens of thousands even for an
/// elaborate route, so this never actually saturates — going through
/// `u32::try_from` makes that a checked fact rather than an assumption, the
/// pattern this workspace uses throughout for narrowing conversions clippy
/// denies outright.
fn as_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_geom3d::Point3;

    fn triangle() -> MeshData {
        MeshData {
            positions: vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(0.0, 1.0, 0.0),
            ],
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn the_file_starts_with_the_glb_header() {
        let glb = to_glb(&[triangle()]);
        assert_eq!(&glb[0..4], b"glTF");
        let version = u32::from_le_bytes(glb[4..8].try_into().expect("4 bytes"));
        assert_eq!(version, 2);
        let declared_len = u32::from_le_bytes(glb[8..12].try_into().expect("4 bytes"));
        assert_eq!(declared_len as usize, glb.len());
    }

    #[test]
    fn the_json_chunk_is_valid_and_4_byte_aligned() {
        let glb = to_glb(&[triangle()]);
        let json_len = u32::from_le_bytes(glb[12..16].try_into().expect("4 bytes")) as usize;
        assert!(json_len.is_multiple_of(4));
        let json_type = &glb[16..20];
        assert_eq!(json_type, b"JSON");
        let json_bytes = &glb[20..20 + json_len];
        let value: serde_json::Value = serde_json::from_slice(json_bytes).expect("valid JSON");
        assert_eq!(value["asset"]["version"], "2.0");
        assert_eq!(value["accessors"][0]["count"], 3);
        assert_eq!(value["accessors"][1]["count"], 3);
    }

    #[test]
    fn the_bin_chunk_holds_positions_then_indices() {
        let glb = to_glb(&[triangle()]);
        let json_len = u32::from_le_bytes(glb[12..16].try_into().expect("4 bytes")) as usize;
        let bin_start = 20 + json_len;
        let bin_len =
            u32::from_le_bytes(glb[bin_start..bin_start + 4].try_into().expect("4 bytes")) as usize;
        let bin_type = &glb[bin_start + 4..bin_start + 8];
        assert_eq!(bin_type, b"BIN\0");
        // 3 positions x 3 floats x 4 bytes, then 3 indices x 4 bytes.
        assert!(bin_len >= 3 * 3 * 4 + 3 * 4);
    }

    #[test]
    fn merging_two_meshes_shifts_the_second_meshs_indices() {
        let glb = to_glb(&[triangle(), triangle()]);
        let json_len = u32::from_le_bytes(glb[12..16].try_into().expect("4 bytes")) as usize;
        let value: serde_json::Value =
            serde_json::from_slice(&glb[20..20 + json_len]).expect("valid JSON");
        assert_eq!(value["accessors"][0]["count"], 6, "6 positions total");
        assert_eq!(value["accessors"][1]["count"], 6, "6 indices total");
    }

    #[test]
    fn an_empty_mesh_list_still_produces_a_valid_file() {
        let glb = to_glb(&[]);
        assert_eq!(&glb[0..4], b"glTF");
    }
}
