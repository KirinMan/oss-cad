//! Resolving connection points to world space.
//!
//! [`od_parts::Port`] lives in a part's local frame; placing the part turns
//! that into a [`WorldPort`]. The transform is the same one a `BlockRef`
//! applies (`docs/03-data-model.md`), kept here as plain arithmetic rather
//! than going through `Frame3` — placement here is deliberately planar
//! (rotation about +Z, an optional Y-mirror for the elbow-handedness trick;
//! see [`crate::model::PlacedPart`]), and a general 3D frame would let a
//! caller express placements this domain does not yet support without saying
//! so.

use crate::model::{PlacedPart, RouteSegment, WorldPort};
use od_core::{ObjectId, Point3};
use od_geom3d::Vec3;
use od_parts::{Catalog, Instance};

/// `position + Rotate(rotation) · MirrorY(mirror_y) · local`. Matches the
/// order `od-io-svg` composes a block reference's transform in, so a part's
/// ports land exactly where its drawn symbol does.
#[must_use]
pub fn transform_point(local: Point3, position: Point3, rotation: f64, mirror_y: bool) -> Point3 {
    let y = if mirror_y { -local.y } else { local.y };
    let (s, c) = rotation.sin_cos();
    Point3::new(
        position.x + local.x * c - y * s,
        position.y + local.x * s + y * c,
        position.z + local.z,
    )
}

/// The same transform without the translation, for directions.
#[must_use]
pub fn transform_dir(local: Vec3, rotation: f64, mirror_y: bool) -> Vec3 {
    let y = if mirror_y { -local.y } else { local.y };
    let (s, c) = rotation.sin_cos();
    Vec3::new(local.x * c - y * s, local.x * s + y * c, local.z)
}

/// Builds the placed part's instance and its world-space ports together, since
/// every caller that wants one wants the other — the instance for its geometry
/// and bounds, the ports for connections.
///
/// # Errors
/// Whatever [`Catalog::instantiate`] returns: an unknown part id or an
/// expression that fails at this part's parameter values.
pub fn place(
    catalog: &Catalog,
    owner: ObjectId,
    part: &PlacedPart,
) -> od_parts::catalog::Result<(Instance, Vec<WorldPort>)> {
    let instance = catalog.instantiate(&part.part_id, &part.params)?;
    let ports = instance
        .ports
        .iter()
        .map(|p| WorldPort {
            owner,
            name: p.name.clone(),
            position: transform_point(p.frame.origin, part.position, part.rotation, part.mirror_y),
            direction: transform_dir(p.frame.normal(), part.rotation, part.mirror_y)
                .normalized()
                .unwrap_or(Vec3::X),
            profile: p.profile,
            system_kind: p.system_kind,
        })
        .collect();
    Ok((instance, ports))
}

/// A route's two ports: `start`, facing back the way the run came from, and
/// `end`, facing the way it continues. Both carry the same profile — a route
/// segment is one uniform run; a size change is a reducer, a separate part.
///
/// The `system_kind` on these ports comes from the caller, because a
/// `RouteSegment` names its system by a [`od_parts::SystemDef`] id (a string,
/// resolved against a catalogue) rather than carrying the `SystemKind`
/// directly — connection matching needs the kind, so it is looked up once
/// here rather than at every call site.
#[must_use]
pub fn route_ports(
    owner: ObjectId,
    route: &RouteSegment,
    system_kind: od_parts::SystemKind,
) -> [WorldPort; 2] {
    // A degenerate route (start == end) has no direction to report; the
    // direction is undefined but must still be *something*, since a caller
    // must not panic on data that only failed to have positive length.
    let dir = (route.end - route.start).normalized().unwrap_or(Vec3::X);
    [
        WorldPort {
            owner,
            name: "start".into(),
            position: route.start,
            direction: -dir,
            profile: route.profile,
            system_kind,
        },
        WorldPort {
            owner,
            name: "end".into(),
            position: route.end,
            direction: dir,
            profile: route.profile,
            system_kind,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PlacementKind;
    use od_core::tol;
    use std::f64::consts::FRAC_PI_2;

    #[test]
    fn identity_placement_leaves_local_points_alone() {
        let p = transform_point(Point3::new(3.0, 4.0, 5.0), Point3::ORIGIN, 0.0, false);
        assert!(p.coincides_with(Point3::new(3.0, 4.0, 5.0)));
    }

    #[test]
    fn ninety_degree_rotation_turns_x_into_y() {
        let p = transform_point(Point3::new(1.0, 0.0, 0.0), Point3::ORIGIN, FRAC_PI_2, false);
        assert!(p.coincides_with(Point3::new(0.0, 1.0, 0.0)));
    }

    #[test]
    fn mirroring_flips_y_before_rotating() {
        let p = transform_point(Point3::new(0.0, 1.0, 0.0), Point3::ORIGIN, 0.0, true);
        assert!(p.coincides_with(Point3::new(0.0, -1.0, 0.0)));

        // Mirror, then a 90° turn: local +Y (mirrored to -Y) rotates to +X.
        let p2 = transform_point(Point3::new(0.0, 1.0, 0.0), Point3::ORIGIN, FRAC_PI_2, true);
        assert!(p2.coincides_with(Point3::new(1.0, 0.0, 0.0)));
    }

    #[test]
    fn translation_carries_through() {
        let p = transform_point(
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(1000.0, 2000.0, 3000.0),
            0.0,
            false,
        );
        assert!(p.coincides_with(Point3::new(1001.0, 2001.0, 3000.0)));
    }

    #[test]
    fn a_route_faces_its_ports_outward() {
        let owner = od_core::ObjectId::new(od_core::ActorId::SYSTEM, 1);
        let route = RouteSegment {
            system: "sys.air.supply".into(),
            spec: "spec.duct.galvanised.rect".into(),
            profile: od_parts::Profile::Rect { w: 400.0, h: 300.0 },
            start: Point3::ORIGIN,
            end: Point3::new(5000.0, 0.0, 0.0),
        };
        let [start, end] = route_ports(owner, &route, od_parts::SystemKind::Air);
        assert!(tol::eq_len(
            start.direction.dot(Vec3::new(-1.0, 0.0, 0.0)),
            1.0
        ));
        assert!(tol::eq_len(
            end.direction.dot(Vec3::new(1.0, 0.0, 0.0)),
            1.0
        ));
        assert_eq!(start.position, route.start);
        assert_eq!(end.position, route.end);
    }

    #[test]
    fn placing_a_bundled_part_produces_world_ports_at_its_placement() {
        let catalog = Catalog::bundled().expect("loads");
        let owner = od_core::ObjectId::new(od_core::ActorId::SYSTEM, 1);
        let part = PlacedPart {
            part_id: "duct.elbow.rect.90".into(),
            position: Point3::new(10_000.0, 5_000.0, 2800.0),
            rotation: 0.0,
            mirror_y: false,
            params: std::collections::HashMap::new(),
            kind: PlacementKind::Fitting {
                auto_generated: false,
            },
            system: None,
        };
        let (_, world) = place(&catalog, owner, &part).expect("places");
        assert_eq!(world.len(), 2);
        // The inlet sits at the placement origin — see route.rs for why that
        // convention is load-bearing.
        let inlet = world.iter().find(|p| p.name == "in").expect("named 'in'");
        assert!(inlet.position.coincides_with(part.position));
    }
}
