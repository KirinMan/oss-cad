//! Drawing a route (F-101, F-103).
//!
//! This is the idea `docs/04-mep.md` §1 calls out as the one the whole domain
//! should be organised around: draw a centreline, and the fittings a real
//! installation would need appear on their own. A user (or, for now, a caller)
//! supplies a polyline; `draw_route` walks it, and at every corner sharp
//! enough to need a fitting, inserts one and splits the run either side of it.
//!
//! Scope, stated plainly rather than left to be discovered by surprise:
//! automatic insertion only handles **90° bends in a single horizontal
//! plane**. That covers the overwhelming majority of real duct and pipe
//! runs — orthogonal routing in a ceiling void at one elevation — and it is
//! also the only case the bundled catalogue's elbow parts are shaped for (a
//! 90° elbow's local bend axis is +Z, so it turns in plan; there is no 45°
//! reference in [`od_parts::Spec::elbow_part`] to fall back to, and a vertical
//! riser needs a different placement representation than
//! [`crate::model::PlacedPart`] offers). Anything else is a clearly-named
//! [`MepError`] rather than a plausible-looking wrong fitting.

use crate::model::{MepError, PlacedPart, PlacementKind, Result, RouteSegment};
use crate::ports;
use crate::store::{add_part, add_route};
use od_core::{ObjectId, Point3, Transaction, tol};
use od_geom3d::Vec3;
use od_parts::{Catalog, Profile};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct RouteSpec {
    /// A [`od_parts::SystemDef`] id.
    pub system: String,
    /// A [`od_parts::Spec`] id — supplies the elbow part and the minimum bend
    /// radius for it.
    pub spec: String,
    pub profile: Profile,
    /// Centreline vertices. Must all share one Z — see the module
    /// documentation for why.
    pub path: Vec<Point3>,
}

#[derive(Debug, Clone, Default)]
pub struct RouteResult {
    pub segments: Vec<ObjectId>,
    /// Elbows the router inserted, in path order.
    pub fittings: Vec<ObjectId>,
}

/// The angle between two directions is treated as a bend only within this of
/// 90°, and as no bend at all within this of 0°. Real routing input comes from
/// clicked points, not typed angles, so a little slack is unavoidable — but a
/// generous tolerance here is how a 87° corner would quietly become a square
/// one, so it stays tight.
const BEND_TOLERANCE_RAD: f64 = 0.5_f64.to_radians();

pub fn draw_route(
    tx: &mut Transaction<'_>,
    catalog: &Catalog,
    spec: &RouteSpec,
) -> Result<RouteResult> {
    let path = dedup_path(&spec.path);
    if path.len() < 2 {
        return Err(MepError::EmptyPath);
    }
    let z0 = path[0].z;
    if path.iter().any(|p| !tol::eq_len(p.z, z0)) {
        return Err(MepError::NotPlanar);
    }

    let spec_def = catalog
        .spec(&spec.spec)
        .ok_or_else(|| MepError::NoSuchSpec(spec.spec.clone()))?;

    let mut result = RouteResult::default();
    let mut run_start = path[0];

    for window in path.windows(3) {
        let [a, b, c] = window else {
            unreachable!("windows(3) yields exactly 3")
        };
        let Some(d_in) = (*b - *a).normalized() else {
            continue;
        };
        let Some(d_out) = (*c - *b).normalized() else {
            continue;
        };

        let cos_angle = d_in.dot(d_out).clamp(-1.0, 1.0);
        let angle = cos_angle.acos();

        if angle < BEND_TOLERANCE_RAD {
            // Close enough to straight: no fitting, the run continues through
            // b unbroken.
            continue;
        }
        if (angle - std::f64::consts::FRAC_PI_2).abs() > BEND_TOLERANCE_RAD {
            return Err(MepError::UnsupportedBendAngle {
                x: b.x,
                y: b.y,
                z: b.z,
                degrees: angle.to_degrees(),
            });
        }

        let ctx = ElbowContext {
            catalog,
            spec_def,
            profile: spec.profile,
            system: &spec.system,
        };
        let (inlet, outlet, fitting_id) = place_elbow(tx, &ctx, *b, d_in, d_out)?;

        if run_start.distance_to(inlet) > tol::POINT_EPS {
            let seg = RouteSegment {
                system: spec.system.clone(),
                spec: spec.spec.clone(),
                profile: spec.profile,
                start: run_start,
                end: inlet,
            };
            result.segments.push(add_route(tx, &seg)?);
        }
        result.fittings.push(fitting_id);
        run_start = outlet;
    }

    let path_end = path[path.len() - 1];
    if run_start.distance_to(path_end) > tol::POINT_EPS {
        let seg = RouteSegment {
            system: spec.system.clone(),
            spec: spec.spec.clone(),
            profile: spec.profile,
            start: run_start,
            end: path_end,
        };
        result.segments.push(add_route(tx, &seg)?);
    }

    if result.segments.is_empty() && result.fittings.is_empty() {
        return Err(MepError::EmptyPath);
    }
    Ok(result)
}

/// What kind of elbow to place — everything about the *run*, as opposed to
/// `place_elbow`'s other parameters, which are about the *bend*. Grouped into
/// one struct because all four travel together for the length of a route:
/// splitting them into separate arguments would just be the same four values
/// threaded through by hand.
#[derive(Clone, Copy)]
struct ElbowContext<'a> {
    catalog: &'a Catalog,
    spec_def: &'a od_parts::Spec,
    profile: Profile,
    system: &'a str,
}

/// Places one 90° elbow at `at`, oriented so its two ports face back along
/// `d_in` and forward along `d_out`.
///
/// `at` is the nominal corner — where the two straight legs, extended, would
/// meet — not the elbow's own placement origin: a real elbow has a bend
/// radius, so its inlet sits `R` back from the corner along the incoming leg
/// and its outlet sits `R` forward along the outgoing one. Placing it any
/// other way would leave the connecting segments not quite collinear with the
/// elbow's ports, which is a route that looks connected but is not.
///
/// Returns the elbow's inlet and outlet world positions — read back from the
/// instantiated part rather than trusted from the nominal calculation, so a
/// catalogue definition shaped differently than assumed fails loudly here.
fn place_elbow(
    tx: &mut Transaction<'_>,
    ctx: &ElbowContext<'_>,
    at: Point3,
    d_in: Vec3,
    d_out: Vec3,
) -> Result<(Point3, Point3, ObjectId)> {
    let ElbowContext {
        catalog,
        spec_def,
        profile,
        system,
    } = *ctx;

    let mut params: HashMap<String, f64> = HashMap::new();
    match profile {
        Profile::Rect { w, h } | Profile::Oval { w, h } => {
            params.insert("W".into(), w);
            params.insert("H".into(), h);
        }
        Profile::Round { d } => {
            params.insert("D".into(), d);
        }
        Profile::Terminal => {}
    }
    let radius = spec_def.min_bend_radius(profile.extent());
    params.insert("R".into(), radius);

    // Local convention (parts/parts/duct-fittings.json): the inlet port sits
    // at the local origin facing -X (the connecting run extends that way),
    // and the fixed local turn carries it to the outlet facing +Y — a left
    // (CCW) turn entered travelling local +X. Aligning local +X with `d_in`
    // (the direction the run actually travels into the bend) is what puts
    // the inlet's local -X — its outward-facing direction — onto `-d_in`, so
    // it faces back the way the run came, matching a mating port there. The
    // placement origin (the local inlet) then sits `radius` back from the
    // corner along `-d_in`, not at the corner itself.
    let rotation = d_in.y.atan2(d_in.x);
    // A right turn is this same part mirrored across its own inlet axis — the
    // physical fact that one stocked 90° elbow serves both hands depending on
    // which way it is installed.
    let turn_is_left = d_in.cross(d_out).z > 0.0;
    let mirror_y = !turn_is_left;

    let placed = PlacedPart {
        part_id: spec_def.elbow_part.clone(),
        position: at - d_in * radius,
        rotation,
        mirror_y,
        params,
        system: Some(system.to_owned()),
        kind: PlacementKind::Fitting {
            auto_generated: true,
        },
    };
    let id = add_part(tx, &placed)?;
    let (_, world) = ports::place(catalog, id, &placed)?;
    if world.len() != 2 {
        return Err(MepError::UnroutableFitting {
            id,
            part_id: placed.part_id,
            reason: format!("has {} ports; the router requires exactly two", world.len()),
        });
    }
    let inlet =
        world
            .iter()
            .find(|p| p.name == "in")
            .ok_or_else(|| MepError::UnroutableFitting {
                id,
                part_id: placed.part_id.clone(),
                reason: "has no port named `in`, which the router requires".to_owned(),
            })?;
    let outlet =
        world
            .iter()
            .find(|p| p.name == "out")
            .ok_or_else(|| MepError::UnroutableFitting {
                id,
                part_id: placed.part_id.clone(),
                reason: "has no port named `out`, which the router requires".to_owned(),
            })?;
    let expected_inlet = at - d_in * radius;
    if inlet.position.distance_to(expected_inlet) > tol::POINT_EPS {
        return Err(MepError::UnroutableFitting {
            id,
            part_id: placed.part_id,
            reason: "its `in` port is not at its local origin, which the router requires"
                .to_owned(),
        });
    }

    Ok((inlet.position, outlet.position, id))
}

/// Removes consecutive duplicate points, which a clicked-in path can easily
/// contain and which would otherwise read as a zero-length, direction-free
/// "bend".
fn dedup_path(path: &[Point3]) -> Vec<Point3> {
    let mut out: Vec<Point3> = Vec::with_capacity(path.len());
    for &p in path {
        if out
            .last()
            .is_none_or(|last: &Point3| last.distance_to(p) > tol::POINT_EPS)
        {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{all_parts, all_routes};
    use od_core::{ActorId, Database, Document};

    fn catalog() -> Catalog {
        Catalog::bundled().expect("bundled catalogue is valid")
    }

    fn doc() -> Document {
        Document::new(Database::new(ActorId::SYSTEM))
    }

    fn spec(path: Vec<Point3>) -> RouteSpec {
        RouteSpec {
            system: "sys.air.supply".into(),
            spec: "spec.duct.galvanised.rect".into(),
            profile: Profile::Rect { w: 400.0, h: 300.0 },
            path,
        }
    }

    #[test]
    fn a_straight_run_needs_no_fitting() {
        let mut d = doc();
        let s = spec(vec![Point3::ORIGIN, Point3::new(5000.0, 0.0, 0.0)]);
        let result = d
            .edit("Route", |tx| draw_route(tx, &catalog(), &s))
            .expect("commits");
        assert_eq!(result.segments.len(), 1);
        assert!(result.fittings.is_empty());
        let (_, seg) = &all_routes(&d.db)[0];
        assert!(seg.start.coincides_with(Point3::ORIGIN));
        assert!(seg.end.coincides_with(Point3::new(5000.0, 0.0, 0.0)));
    }

    #[test]
    fn a_left_turn_inserts_one_elbow_and_two_segments() {
        let mut d = doc();
        let s = spec(vec![
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(5000.0, 0.0, 2800.0),
            Point3::new(5000.0, 3000.0, 2800.0),
        ]);
        let result = d
            .edit("Route", |tx| draw_route(tx, &catalog(), &s))
            .expect("commits");
        assert_eq!(result.segments.len(), 2);
        assert_eq!(result.fittings.len(), 1);
        assert_eq!(all_parts(&d.db).len(), 1);
        assert!(d.db.validate().is_empty());

        let graph = crate::graph::ConnectionGraph::build(&d.db, &catalog());
        assert!(graph.skipped().is_empty());
        // Two internal joints (segment↔elbow on each side); two free ends.
        assert_eq!(graph.connections().count(), 2);
        assert_eq!(graph.unconnected().len(), 2);
    }

    #[test]
    fn a_right_turn_mirrors_the_same_elbow_part() {
        let mut d = doc();
        let s = spec(vec![
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(5000.0, 0.0, 2800.0),
            Point3::new(5000.0, -3000.0, 2800.0),
        ]);
        let result = d
            .edit("Route", |tx| draw_route(tx, &catalog(), &s))
            .expect("commits");
        assert_eq!(result.fittings.len(), 1);
        let (_, part) = all_parts(&d.db).into_iter().next().expect("one fitting");
        assert!(part.mirror_y, "a right turn must mirror the left-turn part");

        let graph = crate::graph::ConnectionGraph::build(&d.db, &catalog());
        assert_eq!(graph.connections().count(), 2);
        assert_eq!(graph.unconnected().len(), 2);
    }

    #[test]
    fn an_s_bend_chains_two_elbows_with_a_short_connecting_run() {
        let mut d = doc();
        let s = spec(vec![
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(4000.0, 0.0, 2800.0),
            Point3::new(4000.0, 2000.0, 2800.0),
            Point3::new(8000.0, 2000.0, 2800.0),
        ]);
        let result = d
            .edit("Route", |tx| draw_route(tx, &catalog(), &s))
            .expect("commits");
        assert_eq!(result.fittings.len(), 2);
        assert_eq!(result.segments.len(), 3);
        assert!(d.db.validate().is_empty());

        let graph = crate::graph::ConnectionGraph::build(&d.db, &catalog());
        assert_eq!(graph.unconnected().len(), 2, "only the two open ends");
    }

    #[test]
    fn a_colinear_extra_point_produces_no_fitting() {
        let mut d = doc();
        // Three points on one line: the middle one is not a real bend.
        let s = spec(vec![
            Point3::ORIGIN,
            Point3::new(2500.0, 0.0, 0.0),
            Point3::new(5000.0, 0.0, 0.0),
        ]);
        let result = d
            .edit("Route", |tx| draw_route(tx, &catalog(), &s))
            .expect("commits");
        assert!(result.fittings.is_empty());
        assert_eq!(
            result.segments.len(),
            1,
            "one continuous run, not split at the extra point"
        );
    }

    #[test]
    fn a_45_degree_bend_is_refused_rather_than_faked() {
        let mut d = doc();
        let s = spec(vec![
            Point3::ORIGIN,
            Point3::new(5000.0, 0.0, 0.0),
            Point3::new(8535.0, 3535.0, 0.0),
        ]);
        let result = d.edit("Route", |tx| draw_route(tx, &catalog(), &s));
        assert!(matches!(result, Err(MepError::UnsupportedBendAngle { .. })));
        assert_eq!(
            all_routes(&d.db).len(),
            0,
            "the failed edit must leave no trace"
        );
    }

    #[test]
    fn a_riser_out_of_plane_is_refused() {
        let mut d = doc();
        let s = spec(vec![
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(5000.0, 0.0, 2800.0),
            Point3::new(5000.0, 0.0, 3400.0),
        ]);
        let result = d.edit("Route", |tx| draw_route(tx, &catalog(), &s));
        assert!(matches!(result, Err(MepError::NotPlanar)));
    }

    #[test]
    fn a_single_point_path_is_refused() {
        let mut d = doc();
        let s = spec(vec![Point3::ORIGIN]);
        assert!(matches!(
            d.edit("Route", |tx| draw_route(tx, &catalog(), &s)),
            Err(MepError::EmptyPath)
        ));
    }

    #[test]
    fn duplicate_consecutive_points_are_absorbed_not_treated_as_a_bend() {
        let mut d = doc();
        let s = spec(vec![
            Point3::ORIGIN,
            Point3::new(2000.0, 0.0, 0.0),
            Point3::new(2000.0, 0.0, 0.0),
            Point3::new(2000.0, 3000.0, 0.0),
        ]);
        let result = d
            .edit("Route", |tx| draw_route(tx, &catalog(), &s))
            .expect("commits");
        assert_eq!(
            result.fittings.len(),
            1,
            "the duplicate must not read as a second bend"
        );
    }

    #[test]
    fn round_pipe_routes_through_a_bend_too() {
        let mut d = doc();
        let s = RouteSpec {
            system: "sys.water.cold".into(),
            spec: "spec.pipe.sgp".into(),
            profile: Profile::Round { d: 50.0 },
            path: vec![
                Point3::new(0.0, 0.0, 500.0),
                Point3::new(2000.0, 0.0, 500.0),
                Point3::new(2000.0, 1500.0, 500.0),
            ],
        };
        let result = d
            .edit("Route", |tx| draw_route(tx, &catalog(), &s))
            .expect("commits");
        assert_eq!(result.fittings.len(), 1);
        let (_, part) = all_parts(&d.db).into_iter().next().expect("one fitting");
        assert_eq!(part.part_id, "pipe.elbow.90");
    }
}
