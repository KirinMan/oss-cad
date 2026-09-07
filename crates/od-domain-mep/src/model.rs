//! The domain model.
//!
//! Per `docs/04-mep.md` §1: **the truth is centreline + profile + system**.
//! Everything else — the single-line view, the double-line view, the block
//! reference that makes a fitting visible — is derived from these two structs
//! and never edited directly. Neither type appears anywhere in `od-core`; both
//! are carried as an opaque `{type_id, data}` bag through
//! [`Database::insert_custom`](od_core::Database::insert_custom), which is the
//! whole point — the core does not know a duct from a dictionary entry.

use od_core::Point3;
use od_parts::{Profile, SystemKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A straight run of duct, pipe or conduit between two points.
///
/// Deliberately just two points, not a polyline: a bend is a
/// [`PlacedPart`] in its own right (an elbow occupies real space and has its
/// own footprint), so a routed path becomes a *chain* of straight segments
/// joined by fittings rather than one segment with corners. See
/// [`crate::route::draw_route`] for how that chain gets built.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteSegment {
    /// A [`od_parts::SystemDef`] id — what this run carries.
    pub system: String,
    /// A [`od_parts::Spec`] id — how it is fabricated.
    pub spec: String,
    pub profile: Profile,
    pub start: Point3,
    pub end: Point3,
}

impl RouteSegment {
    #[must_use]
    pub fn length(&self) -> f64 {
        self.start.distance_to(self.end)
    }
}

/// Why a [`PlacedPart`] exists: placed by a person, or inserted by the router
/// to make a bend physically buildable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementKind {
    Equipment,
    Fitting { auto_generated: bool },
}

/// A catalogue part placed in the drawing — equipment and fittings alike; the
/// document does not need to tell them apart to place, connect or draw them.
///
/// Placement is planar on purpose: `rotation` turns about +Z only, and
/// `mirror_y` — reflecting the local Y axis before rotating — is what lets one
/// stocked 90° elbow definition serve both left and right turns without a
/// second part (docs/04-mep.md's fittings are physically symmetric that way).
/// A vertical riser or a bend out of plane needs a full orientation this
/// cannot express; that is out of scope here and noted in the crate's module
/// documentation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacedPart {
    pub part_id: String,
    pub position: Point3,
    /// Radians, about +Z.
    pub rotation: f64,
    #[serde(default)]
    pub mirror_y: bool,
    /// Overrides passed to [`od_parts::Catalog::instantiate`]; anything not
    /// listed here takes the part's own default.
    #[serde(default)]
    pub params: HashMap<String, f64>,
    pub kind: PlacementKind,
    /// A [`od_parts::SystemDef`] id, used only to choose the layer and colour
    /// this part is drawn with. An auto-generated fitting inherits its route's
    /// system; a standalone piece of equipment may leave this unset and draws
    /// on layer `0` until someone assigns it one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
}

/// A connection point, resolved to world space.
///
/// This is what the connection graph matches on. It is deliberately not the
/// same type as [`od_parts::Port`] — that one lives in a part's local frame,
/// and mixing the two up is exactly the kind of bug that only shows once two
/// parts are placed at different rotations.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldPort {
    pub owner: od_core::ObjectId,
    pub name: String,
    pub position: Point3,
    /// Outward — the direction a connecting run leaves along.
    pub direction: od_geom3d::Vec3,
    pub profile: Profile,
    pub system_kind: SystemKind,
}

#[derive(Debug, thiserror::Error)]
pub enum MepError {
    #[error(transparent)]
    Db(#[from] od_core::DbError),
    #[error(transparent)]
    Catalog(#[from] od_parts::CatalogError),
    #[error("no MEP object at {0}")]
    NotFound(od_core::ObjectId),
    #[error("object {0} is not a {1}")]
    WrongType(od_core::ObjectId, &'static str),
    #[error("malformed MEP data at {0}: {1}")]
    Corrupt(od_core::ObjectId, serde_json::Error),
    /// A `RouteSegment` or `PlacedPart` failed to serialise — practically
    /// unreachable for the well-typed structs this crate builds, but a
    /// non-finite coordinate (`NaN`, `Infinity`) is real input a caller could
    /// construct, and `serde_json` correctly refuses to encode one rather
    /// than writing a `null` that would silently corrupt the document.
    #[error("could not encode MEP data: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("no spec `{0}` in the catalogue")]
    NoSuchSpec(String),
    #[error("no system `{0}` in the catalogue")]
    NoSuchSystem(String),
    #[error(
        "only 90° bends can be routed automatically; the turn at ({x:.0}, {y:.0}, {z:.0}) is {degrees:.1}°. \
         Place the fitting yourself with PlacedPart for any other angle."
    )]
    UnsupportedBendAngle {
        x: f64,
        y: f64,
        z: f64,
        degrees: f64,
    },
    #[error("a route needs at least two points")]
    EmptyPath,
    #[error(
        "route path must lie in one horizontal plane (all points at the same Z); MEP routing does not yet place risers"
    )]
    NotPlanar,
    /// A catalogue elbow part does not have the shape the router assumes —
    /// not a malformed document, a part definition that cannot be routed
    /// through automatically.
    #[error("part `{part_id}` (placed as {id}) cannot be used as an automatic elbow: {reason}")]
    UnroutableFitting {
        id: od_core::ObjectId,
        part_id: String,
        reason: String,
    },
}

pub type Result<T> = std::result::Result<T, MepError>;
