//! Exports IFC4 by shelling out to
//! [`od-bridge-ifc`](../../../bridges/od-bridge-ifc) — a standalone Python
//! script that uses [IfcOpenShell](https://ifcopenshell.org/) (LGPL-3.0) to
//! build the actual file. This crate never imports or links IfcOpenShell:
//! it only knows how to serialise a request to JSON and run a subprocess,
//! which is what keeps that LGPL-3.0 dependency out of every Apache-2.0
//! Rust binary this workspace produces (`docs/05-interop-license.md`,
//! `NOTICE`). If the bridge script or a Python interpreter isn't available,
//! [`export`] fails with a clear error — IFC export is an optional
//! capability, never a build requirement, the same way OCCT is meant to be
//! (roadmap risk R-9).
//!
//! # Scope
//!
//! Export only, and narrow even within that:
//!
//! - **Routed centrelines only.** No placed equipment or fittings — those
//!   would need their own IFC entity per catalogue part
//!   ([`od_parts::Part::ifc_class`] already exists for this, but wiring it
//!   through is a separate piece of work).
//! - **Geometry is a pre-tessellated triangle mesh** in world-space
//!   millimetres (exactly [`od_geom3d::MeshData`]'s shape, restated here
//!   without a dependency on that crate so this one stays generic) — the
//!   same mesh `od-io-gltf` already consumes. Every element's IFC
//!   `ObjectPlacement` is therefore identity: there is no per-element
//!   transform to derive, only coordinates a caller already computed.
//! - **One fixed `IfcSite`/`IfcBuilding`/`IfcBuildingStorey`.** There is no
//!   `Level`/`GridAxis` table upstream yet to place elements against
//!   (`docs/04-mep.md` §5's own note on this), so every element lands in
//!   one nominal storey.
//! - **Properties are flat text.** [`IfcElement::properties`] becomes one
//!   `IfcPropertySingleValue` (`IfcText`) per entry — no typed/unit-aware
//!   properties yet, even though [`od_core::xdata`]'s `FieldDef` already
//!   carries a type and a unit for exactly this. Wiring that through is
//!   this crate's most natural next step, not attempted here.
//! - **No reading an IFC file back in, and no IDS self-verification** — both
//!   real parts of F-203, neither attempted in this first slice.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// One IFC product: a class, a name, pre-tessellated world-space geometry,
/// and property sets. See the crate docs for what each field does and does
/// not carry yet.
#[derive(Debug, Clone, Serialize)]
pub struct IfcElement {
    /// An IFC4 entity name, e.g. `"IfcDuctSegment"`. Not validated by this
    /// crate — an unknown class surfaces as a bridge failure
    /// ([`IfcBridgeError::BridgeFailed`]), since IfcOpenShell is the one
    /// place that actually knows the IFC4 schema.
    pub ifc_class: String,
    pub name: String,
    /// World-space millimetres.
    pub positions: Vec<[f64; 3]>,
    /// 0-indexed into `positions`; the bridge converts to IFC's 1-indexed
    /// convention. Empty means "no geometry for this element" — a valid
    /// IFC product with `Representation = $`, not an error (a caller might
    /// have a profile it cannot tessellate, the same "narrow gap, not
    /// fatal" treatment used elsewhere in this workspace).
    pub triangles: Vec<[u32; 3]>,
    /// `Pset name -> {Property name -> value}`.
    pub properties: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IfcExportRequest {
    pub project_name: String,
    pub elements: Vec<IfcElement>,
}

#[derive(Debug, Deserialize)]
struct BridgeResponse {
    elements_written: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum IfcBridgeError {
    #[error("encoding the export request: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("spawning `{python} {bridge}`: {source}")]
    Spawn {
        python: String,
        bridge: String,
        #[source]
        source: std::io::Error,
    },
    /// Unreachable in practice — [`export`] always requests a piped stdin —
    /// but handled as a real error rather than assumed, since nothing
    /// upstream can prove [`std::process::Child::stdin`] stays `Some`.
    #[error("od-bridge-ifc's process has no stdin pipe")]
    NoStdin,
    #[error("writing the request to od-bridge-ifc's stdin: {0}")]
    Write(std::io::Error),
    #[error("waiting for od-bridge-ifc to exit: {0}")]
    Wait(std::io::Error),
    #[error("od-bridge-ifc exited with status {status}: {stderr}")]
    BridgeFailed { status: i32, stderr: String },
    #[error("od-bridge-ifc's response was not the JSON it promises to print: {0}")]
    Decode(serde_json::Error),
}

pub type Result<T> = std::result::Result<T, IfcBridgeError>;

/// Runs `python bridge_script <out>`, feeding `request` as JSON on stdin,
/// and returns the number of elements the bridge reports having written.
///
/// # Errors
/// See [`IfcBridgeError`]'s variants: the process could not be spawned, its
/// stdin could not be written, it exited non-zero (its stderr is included
/// verbatim), or its stdout was not the `{"elements_written": N}` it
/// promises on success.
pub fn export(
    request: &IfcExportRequest,
    python: &Path,
    bridge_script: &Path,
    out: &Path,
) -> Result<usize> {
    let payload = serde_json::to_vec(request)?;

    let mut child = Command::new(python)
        .arg(bridge_script)
        .arg(out)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| IfcBridgeError::Spawn {
            python: python.display().to_string(),
            bridge: bridge_script.display().to_string(),
            source,
        })?;

    // Taken and dropped in its own scope so stdin closes before
    // `wait_with_output` reads stdout/stderr to completion -- the bridge
    // reads stdin to EOF before writing anything back, and a pipe left open
    // here is exactly how that would deadlock.
    {
        let mut stdin = child.stdin.take().ok_or(IfcBridgeError::NoStdin)?;
        stdin.write_all(&payload).map_err(IfcBridgeError::Write)?;
    }

    let output = child.wait_with_output().map_err(IfcBridgeError::Wait)?;
    if !output.status.success() {
        return Err(IfcBridgeError::BridgeFailed {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let response: BridgeResponse =
        serde_json::from_slice(&output.stdout).map_err(IfcBridgeError::Decode)?;
    Ok(response.elements_written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> IfcExportRequest {
        let mut props = BTreeMap::new();
        props.insert(
            "Pset_OpenDraftRoute".to_owned(),
            BTreeMap::from([
                ("System".to_owned(), "sys.air.supply".to_owned()),
                ("Spec".to_owned(), "spec.duct.galvanised.rect".to_owned()),
            ]),
        );
        IfcExportRequest {
            project_name: "test".into(),
            elements: vec![IfcElement {
                ifc_class: "IfcDuctSegment".into(),
                name: "Route-0".into(),
                positions: vec![[0.0, 0.0, 0.0], [400.0, 0.0, 0.0], [400.0, 300.0, 0.0]],
                triangles: vec![[0, 1, 2]],
                properties: props,
            }],
        }
    }

    #[test]
    fn a_missing_python_interpreter_is_a_clear_error_not_a_panic() {
        let err = export(
            &request(),
            Path::new("/no/such/python-interpreter"),
            Path::new("/no/such/bridge.py"),
            Path::new("/tmp/wont-be-written.ifc"),
        )
        .expect_err("no such interpreter");
        assert!(matches!(err, IfcBridgeError::Spawn { .. }));
    }

    #[test]
    fn the_request_serialises_with_the_shape_the_bridge_expects() {
        let json = serde_json::to_value(request()).expect("serialises");
        assert_eq!(json["project_name"], "test");
        assert_eq!(json["elements"][0]["ifc_class"], "IfcDuctSegment");
        assert_eq!(json["elements"][0]["positions"][1][0], 400.0);
        assert_eq!(json["elements"][0]["triangles"][0][2], 2);
        assert_eq!(
            json["elements"][0]["properties"]["Pset_OpenDraftRoute"]["System"],
            "sys.air.supply"
        );
    }

    #[test]
    #[ignore = "one-off external verification harness against the real bridge, see PR description"]
    fn write_smoke_file_for_external_verification() {
        // Set OD_IFC_PYTHON and OD_IFC_BRIDGE to a real interpreter with
        // ifcopenshell installed and bridges/od-bridge-ifc/od_bridge_ifc.py.
        let python = std::env::var("OD_IFC_PYTHON").expect("OD_IFC_PYTHON set");
        let bridge = std::env::var("OD_IFC_BRIDGE").expect("OD_IFC_BRIDGE set");
        let written = export(
            &request(),
            Path::new(&python),
            Path::new(&bridge),
            Path::new("/tmp/od-io-ifc-smoke.ifc"),
        )
        .expect("the real bridge succeeds");
        assert_eq!(written, 1);
    }

    #[test]
    fn a_bridge_that_exits_nonzero_surfaces_its_stderr() {
        // `false` always exits 1 and needs no stdin -- a real stand-in for
        // "the bridge ran but failed", independent of Python or the bridge
        // script actually being present.
        let err = export(
            &request(),
            Path::new("/usr/bin/env"),
            Path::new("false"),
            Path::new("/tmp/wont-be-written.ifc"),
        );
        // `env false` exits 1 with empty stderr; either BridgeFailed (the
        // intended path) or a Decode error (if stdout happened to be
        // non-empty) proves the non-zero exit was not silently accepted.
        assert!(matches!(
            err,
            Err(IfcBridgeError::BridgeFailed { .. } | IfcBridgeError::Decode(_))
        ));
    }
}
