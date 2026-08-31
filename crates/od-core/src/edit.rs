//! Commands: the one path every edit goes through (`docs/02-architecture.md`
//! ADR-006). A mouse click, a script, and — eventually — a message from a
//! collaborator all become the same [`Command`], so undo, scripting and
//! collaborative sync share one mechanism instead of three growing apart.
//!
//! This is a closed enum rather than the `Box<dyn Command>` registry ADR-006
//! sketches: with a handful of concrete edits, a registry buys indirection
//! and nothing else. The trait is worth its weight once something outside
//! this crate needs to contribute a command of its own — a plugin, or a
//! domain crate — and turning this into one then is a mechanical change,
//! since every caller already goes through [`Document::execute`] rather than
//! constructing a variant's fields directly into a transaction.

use crate::entity::{Entity, Geometry};
use crate::error::DbError;
use crate::id::ObjectId;
use crate::transaction::Transaction;
use od_geom2d::{Point2, Polyline2, tol};
use od_geom3d::{Point3, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    /// Draws a straight line on `layer` (created if it does not exist yet),
    /// in model space.
    AddLine { layer: String, a: Point3, b: Point3 },
    /// Draws a circle on `layer`, in the XY plane.
    AddCircle {
        layer: String,
        center: Point3,
        radius: f64,
    },
    /// Draws an arc on `layer`, in the XY plane. `start_angle` and `sweep`
    /// are radians, matching [`Geometry::Arc`]'s own convention.
    AddArc {
        layer: String,
        center: Point3,
        radius: f64,
        start_angle: f64,
        sweep: f64,
    },
    /// Draws a straight-segment polyline on `layer`. `points` must all share
    /// one Z — [`Geometry::Polyline`] is planar, at a fixed elevation, the
    /// same constraint `od-domain-mep::route::draw_route` enforces on a
    /// routed centreline.
    AddPolyline {
        layer: String,
        points: Vec<Point3>,
        closed: bool,
    },
    /// Translates every named entity by the same offset.
    MoveEntities { ids: Vec<ObjectId>, delta: Vec3 },
    /// Turns every named entity in place by the same angle, about its own
    /// insertion point. Only a block reference carries an orientation of its
    /// own to turn ([`crate::entity::BlockRef::rotation`]); rotating a line
    /// or a polyline about a pivot is a different, larger feature (it needs
    /// a pivot point, not just an angle) and is not this command.
    RotateEntities { ids: Vec<ObjectId>, radians: f64 },
    /// Removes entities outright.
    DeleteEntities { ids: Vec<ObjectId> },
    /// Duplicates every named entity, offset by `delta`. Unlike
    /// [`Command::MoveEntities`], the originals are untouched — the copies
    /// are new entities, on the same layer and in the same space as their
    /// source.
    CopyEntities { ids: Vec<ObjectId>, delta: Vec3 },
    /// Reflects every named entity across the line through `a` and `b` (in
    /// the XY plane), as new entities. When `keep_original` is `false`, the
    /// sources are deleted too — the same "keep source objects?" choice a
    /// real CAD's mirror command asks.
    MirrorEntities {
        ids: Vec<ObjectId>,
        a: Point3,
        b: Point3,
        keep_original: bool,
    },
    /// Creates a new entity parallel to `id`, offset by `distance`. Positive
    /// is outward: away from the centre for a circle or arc, to the left of
    /// the direction from the line's first point to its second. The source
    /// is never modified.
    OffsetEntity { id: ObjectId, distance: f64 },
}

/// What a [`Command`] did, so a caller — a UI selecting what it just drew, a
/// script checking an id — does not have to re-inspect the document to find
/// out.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CommandOutcome {
    pub created: Vec<ObjectId>,
    pub modified: Vec<ObjectId>,
    pub deleted: Vec<ObjectId>,
}

impl Command {
    pub(crate) fn apply(&self, tx: &mut Transaction<'_>) -> Result<CommandOutcome, DbError> {
        let mut outcome = CommandOutcome::default();
        match self {
            Command::AddLine { layer, a, b } => {
                let layer_id = tx.ensure_layer(layer);
                let space = tx.db().model_space();
                let id = tx.add_entity(Entity::new(
                    layer_id,
                    space,
                    Geometry::Line { a: *a, b: *b },
                ))?;
                outcome.created.push(id);
            }
            Command::AddCircle {
                layer,
                center,
                radius,
            } => {
                let layer_id = tx.ensure_layer(layer);
                let space = tx.db().model_space();
                let id = tx.add_entity(Entity::new(
                    layer_id,
                    space,
                    Geometry::Circle {
                        center: *center,
                        radius: *radius,
                        normal: Vec3::Z,
                    },
                ))?;
                outcome.created.push(id);
            }
            Command::AddArc {
                layer,
                center,
                radius,
                start_angle,
                sweep,
            } => {
                let layer_id = tx.ensure_layer(layer);
                let space = tx.db().model_space();
                let id = tx.add_entity(Entity::new(
                    layer_id,
                    space,
                    Geometry::Arc {
                        center: *center,
                        radius: *radius,
                        start_angle: *start_angle,
                        sweep: *sweep,
                        normal: Vec3::Z,
                    },
                ))?;
                outcome.created.push(id);
            }
            Command::AddPolyline {
                layer,
                points,
                closed,
            } => {
                if points.len() < 2 {
                    return Err(DbError::InvalidCommand(
                        "a polyline needs at least two points".into(),
                    ));
                }
                let elevation = points[0].z;
                if points.iter().any(|p| !tol::eq_len(p.z, elevation)) {
                    return Err(DbError::InvalidCommand(
                        "a polyline's points must all share one elevation".into(),
                    ));
                }
                let vertices = points.iter().map(|p| Point2::new(p.x, p.y));
                let layer_id = tx.ensure_layer(layer);
                let space = tx.db().model_space();
                let id = tx.add_entity(Entity::new(
                    layer_id,
                    space,
                    Geometry::Polyline {
                        polyline: Polyline2::from_points(vertices, *closed),
                        elevation,
                        normal: Vec3::Z,
                        width: 0.0,
                    },
                ))?;
                outcome.created.push(id);
            }
            Command::MoveEntities { ids, delta } => {
                for &id in ids {
                    let mut handled = false;
                    tx.modify_entity(id, |e| handled = try_translate(&mut e.geom, *delta))?;
                    if !handled {
                        let geometry = tx
                            .db()
                            .entity(id)
                            .map_or_else(|| "?".to_owned(), |e| e.geom.type_name().to_owned());
                        return Err(DbError::UnsupportedEdit { id, geometry });
                    }
                    outcome.modified.push(id);
                }
            }
            Command::RotateEntities { ids, radians } => {
                for &id in ids {
                    let mut handled = false;
                    tx.modify_entity(id, |e| handled = try_rotate(&mut e.geom, *radians))?;
                    if !handled {
                        let geometry = tx
                            .db()
                            .entity(id)
                            .map_or_else(|| "?".to_owned(), |e| e.geom.type_name().to_owned());
                        return Err(DbError::UnsupportedEdit { id, geometry });
                    }
                    outcome.modified.push(id);
                }
            }
            Command::DeleteEntities { ids } => {
                for &id in ids {
                    tx.remove(id)?;
                    outcome.deleted.push(id);
                }
            }
            Command::CopyEntities { ids, delta } => {
                for &id in ids {
                    let entity = tx.db().entity(id).ok_or(DbError::NoSuchObject(id))?;
                    let (layer, owner_space, style, visible) = (
                        entity.layer,
                        entity.owner_space,
                        entity.style.clone(),
                        entity.visible,
                    );
                    let mut geom = entity.geom.clone();
                    if !try_translate(&mut geom, *delta) {
                        return Err(DbError::UnsupportedEdit {
                            id,
                            geometry: geom.type_name().to_owned(),
                        });
                    }
                    let new_id = tx.add_entity(Entity {
                        layer,
                        geom,
                        style,
                        visible,
                        owner_space,
                    })?;
                    outcome.created.push(new_id);
                }
            }
            Command::MirrorEntities {
                ids,
                a,
                b,
                keep_original,
            } => {
                if a.distance_to(*b) < od_geom2d::tol::POINT_EPS {
                    return Err(DbError::InvalidCommand(
                        "a mirror line needs two distinct points".into(),
                    ));
                }
                for &id in ids {
                    let entity = tx.db().entity(id).ok_or(DbError::NoSuchObject(id))?;
                    let (layer, owner_space, style, visible) = (
                        entity.layer,
                        entity.owner_space,
                        entity.style.clone(),
                        entity.visible,
                    );
                    let mut geom = entity.geom.clone();
                    if !try_mirror(&mut geom, *a, *b) {
                        return Err(DbError::UnsupportedEdit {
                            id,
                            geometry: geom.type_name().to_owned(),
                        });
                    }
                    let new_id = tx.add_entity(Entity {
                        layer,
                        geom,
                        style,
                        visible,
                        owner_space,
                    })?;
                    outcome.created.push(new_id);
                    if !keep_original {
                        tx.remove(id)?;
                        outcome.deleted.push(id);
                    }
                }
            }
            Command::OffsetEntity { id, distance } => {
                let entity = tx.db().entity(*id).ok_or(DbError::NoSuchObject(*id))?;
                let (layer, owner_space, style, visible) = (
                    entity.layer,
                    entity.owner_space,
                    entity.style.clone(),
                    entity.visible,
                );
                let mut geom = entity.geom.clone();
                match &mut geom {
                    Geometry::Line { a, b } => {
                        let dx = b.x - a.x;
                        let dy = b.y - a.y;
                        let len = (dx * dx + dy * dy).sqrt();
                        if len < tol::POINT_EPS {
                            return Err(DbError::InvalidCommand(
                                "cannot offset a zero-length line".into(),
                            ));
                        }
                        let shift = Vec3::new(-dy / len * *distance, dx / len * *distance, 0.0);
                        *a = *a + shift;
                        *b = *b + shift;
                    }
                    Geometry::Circle { radius, .. } | Geometry::Arc { radius, .. } => {
                        let new_radius = *radius + *distance;
                        if new_radius <= 0.0 {
                            return Err(DbError::InvalidCommand(format!(
                                "offsetting by {distance} would leave a radius of \
                                 {new_radius}, which is not positive"
                            )));
                        }
                        *radius = new_radius;
                    }
                    _ => {
                        return Err(DbError::UnsupportedEdit {
                            id: *id,
                            geometry: geom.type_name().to_owned(),
                        });
                    }
                }
                let new_id = tx.add_entity(Entity {
                    layer,
                    geom,
                    style,
                    visible,
                    owner_space,
                })?;
                outcome.created.push(new_id);
            }
        }
        Ok(outcome)
    }
}

/// Translates geometry in place, and reports whether it knew how to. Deliberately
/// narrow rather than a catch-all `_ => true`: an entity kind this does not yet
/// handle must fail the move loudly ([`DbError::UnsupportedEdit`]) rather than
/// silently stay put, which would look like the command succeeded while the
/// entity never moved.
///
/// [`Geometry::Solid3d`] and [`Geometry::Unsupported`] are the two kinds this
/// deliberately never will handle: the former is an opaque kernel handle
/// `od-core` has no business reaching into, and the latter is preserved
/// verbatim precisely because this build does not understand its shape
/// (rule 2 — never destroy what you cannot read) — translating it by
/// guesswork would be exactly that.
fn try_translate(geom: &mut Geometry, delta: Vec3) -> bool {
    match geom {
        Geometry::Point(p) => *p = *p + delta,
        Geometry::Line { a, b } => {
            *a = *a + delta;
            *b = *b + delta;
        }
        Geometry::Circle { center, .. }
        | Geometry::Arc { center, .. }
        | Geometry::Ellipse { center, .. } => *center = *center + delta,
        // Equipment and fittings are placed as block references
        // (`docs/04-mep.md`), so moving one is the common case of moving a
        // piece of MEP content, not a rare one.
        Geometry::BlockRef(block_ref) => block_ref.position = block_ref.position + delta,
        Geometry::Text(t) => t.position = t.position + delta,
        Geometry::MText(t) => t.position = t.position + delta,
        Geometry::Polyline {
            polyline,
            elevation,
            ..
        } => {
            for v in &mut polyline.vertices {
                v.point.x += delta.x;
                v.point.y += delta.y;
            }
            *elevation += delta.z;
        }
        Geometry::Polyline3d { points, .. } => {
            for p in points {
                *p = *p + delta;
            }
        }
        Geometry::Spline { control_points, .. } => {
            for p in control_points {
                *p = *p + delta;
            }
        }
        Geometry::Hatch(hatch) => {
            for l in &mut hatch.loops {
                for v in &mut l.vertices {
                    v.point.x += delta.x;
                    v.point.y += delta.y;
                }
            }
            hatch.elevation += delta.z;
        }
        _ => return false,
    }
    true
}

/// Turns geometry in place, and reports whether it knew how to — the
/// rotational counterpart of [`try_translate`], narrow for the same reason.
fn try_rotate(geom: &mut Geometry, radians: f64) -> bool {
    match geom {
        Geometry::BlockRef(block_ref) => block_ref.rotation += radians,
        _ => return false,
    }
    true
}

/// Reflects `p`'s X and Y across the line through `a` and `b`; Z passes
/// through unchanged, so this is really a reflection across the vertical
/// plane containing that line — the natural reading for a 2D editor working
/// in the XY plane, and consistent with [`try_translate`]'s own treatment of
/// a `Point3`'s Z on entities that carry one.
fn reflect_xy(p: Point3, a: Point3, b: Point3) -> Point3 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;
    let ap_x = p.x - a.x;
    let ap_y = p.y - a.y;
    let t = (ap_x * dx + ap_y * dy) / len_sq;
    let proj_x = a.x + t * dx;
    let proj_y = a.y + t * dy;
    Point3::new(2.0 * proj_x - p.x, 2.0 * proj_y - p.y, p.z)
}

/// Reflects geometry across the line through `a` and `b`, and reports
/// whether it knew how to — the same narrow, fail-loudly contract as
/// [`try_translate`] and [`try_rotate`]. Deliberately covers only the kinds
/// this session's draw commands just added creation for (`Point`, `Line`,
/// `Circle`, `Arc`): mirroring a `BlockRef` correctly needs a full
/// transform-decomposition treatment (reflect its local basis, then recover
/// rotation/scale from the result) rather than just moving its insertion
/// point, and mirroring a curved (bulged) polyline span needs its bulge sign
/// and vertex order both reversed together — both are real features, just
/// not ones a caller should get a plausible-looking wrong answer from today.
fn try_mirror(geom: &mut Geometry, a: Point3, b: Point3) -> bool {
    match geom {
        Geometry::Point(p) => *p = reflect_xy(*p, a, b),
        Geometry::Line { a: la, b: lb } => {
            *la = reflect_xy(*la, a, b);
            *lb = reflect_xy(*lb, a, b);
        }
        Geometry::Circle { center, .. } => {
            *center = reflect_xy(*center, a, b);
        }
        Geometry::Arc {
            center,
            radius,
            start_angle,
            sweep,
            ..
        } => {
            let start_pt = *center
                + Vec3::new(
                    *radius * start_angle.cos(),
                    *radius * start_angle.sin(),
                    0.0,
                );
            let end_angle = *start_angle + *sweep;
            let end_pt =
                *center + Vec3::new(*radius * end_angle.cos(), *radius * end_angle.sin(), 0.0);
            let new_center = reflect_xy(*center, a, b);
            let reflected_start = reflect_xy(start_pt, a, b);
            let reflected_end = reflect_xy(end_pt, a, b);
            // Mirroring reverses the sense of travel around the arc, so the
            // new start is the reflection of the old *end* — reusing the
            // old start/end labels as-is would silently swap which side of
            // the arc gets drawn.
            let new_start_angle =
                (reflected_end.y - new_center.y).atan2(reflected_end.x - new_center.x);
            let new_end_angle =
                (reflected_start.y - new_center.y).atan2(reflected_start.x - new_center.x);
            let two_pi = std::f64::consts::TAU;
            let raw_sweep = ((new_end_angle - new_start_angle) % two_pi + two_pi) % two_pi;
            *center = new_center;
            *start_angle = new_start_angle;
            *sweep = if raw_sweep < tol::ANGLE_EPS {
                two_pi
            } else {
                raw_sweep
            };
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::id::ActorId;
    use crate::transaction::Document;

    fn doc() -> Document {
        Document::new(Database::new(ActorId::SYSTEM))
    }

    #[test]
    fn add_line_draws_on_the_named_layer() {
        let mut d = doc();
        let outcome = d
            .execute(
                "Draw",
                &Command::AddLine {
                    layer: "A-Wall".into(),
                    a: Point3::ORIGIN,
                    b: Point3::new(3600.0, 0.0, 0.0),
                },
            )
            .expect("commits");
        assert_eq!(outcome.created.len(), 1);
        let id = outcome.created[0];
        let entity = d.db.entity(id).expect("exists");
        assert_eq!(
            entity.geom,
            Geometry::Line {
                a: Point3::ORIGIN,
                b: Point3::new(3600.0, 0.0, 0.0),
            }
        );
        assert_eq!(
            d.db.tables
                .layers
                .get(entity.layer)
                .expect("layer exists")
                .name,
            "A-Wall"
        );
    }

    #[test]
    fn add_circle_draws_in_the_xy_plane() {
        let mut d = doc();
        let outcome = d
            .execute(
                "Draw",
                &Command::AddCircle {
                    layer: "0".into(),
                    center: Point3::new(1000.0, 2000.0, 0.0),
                    radius: 500.0,
                },
            )
            .expect("commits");
        let entity = d.db.entity(outcome.created[0]).expect("exists");
        assert_eq!(
            entity.geom,
            Geometry::Circle {
                center: Point3::new(1000.0, 2000.0, 0.0),
                radius: 500.0,
                normal: Vec3::Z,
            }
        );
    }

    #[test]
    fn add_arc_draws_in_the_xy_plane() {
        let mut d = doc();
        let outcome = d
            .execute(
                "Draw",
                &Command::AddArc {
                    layer: "0".into(),
                    center: Point3::ORIGIN,
                    radius: 500.0,
                    start_angle: 0.0,
                    sweep: std::f64::consts::FRAC_PI_2,
                },
            )
            .expect("commits");
        let entity = d.db.entity(outcome.created[0]).expect("exists");
        assert_eq!(
            entity.geom,
            Geometry::Arc {
                center: Point3::ORIGIN,
                radius: 500.0,
                start_angle: 0.0,
                sweep: std::f64::consts::FRAC_PI_2,
                normal: Vec3::Z,
            }
        );
    }

    #[test]
    fn add_polyline_draws_a_straight_segment_chain() {
        let mut d = doc();
        let outcome = d
            .execute(
                "Draw",
                &Command::AddPolyline {
                    layer: "0".into(),
                    points: vec![
                        Point3::new(0.0, 0.0, 300.0),
                        Point3::new(1000.0, 0.0, 300.0),
                        Point3::new(1000.0, 1000.0, 300.0),
                    ],
                    closed: false,
                },
            )
            .expect("commits");
        let entity = d.db.entity(outcome.created[0]).expect("exists");
        let Geometry::Polyline {
            polyline,
            elevation,
            ..
        } = &entity.geom
        else {
            panic!("expected a Polyline, got {:?}", entity.geom);
        };
        assert!(od_geom2d::tol::eq_len(*elevation, 300.0));
        assert_eq!(polyline.vertices.len(), 3);
        assert!(!polyline.closed);
    }

    #[test]
    fn add_polyline_rejects_a_non_planar_path() {
        let mut d = doc();
        let err = d
            .execute(
                "Draw",
                &Command::AddPolyline {
                    layer: "0".into(),
                    points: vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 0.0, 500.0)],
                    closed: false,
                },
            )
            .expect_err("differing Z must be rejected");
        assert!(matches!(err, DbError::InvalidCommand(_)));
    }

    #[test]
    fn add_polyline_rejects_a_single_point() {
        let mut d = doc();
        let err = d
            .execute(
                "Draw",
                &Command::AddPolyline {
                    layer: "0".into(),
                    points: vec![Point3::ORIGIN],
                    closed: false,
                },
            )
            .expect_err("one point is not a polyline");
        assert!(matches!(err, DbError::InvalidCommand(_)));
    }

    #[test]
    fn copy_entities_creates_a_translated_duplicate_and_keeps_the_original() {
        let mut d = doc();
        let source = d
            .execute(
                "Draw",
                &Command::AddLine {
                    layer: "0".into(),
                    a: Point3::ORIGIN,
                    b: Point3::new(1000.0, 0.0, 0.0),
                },
            )
            .expect("commits")
            .created[0];

        let outcome = d
            .execute(
                "Copy",
                &Command::CopyEntities {
                    ids: vec![source],
                    delta: Vec3::new(0.0, 500.0, 0.0),
                },
            )
            .expect("commits");
        assert_eq!(outcome.created.len(), 1);
        let copy_id = outcome.created[0];
        assert_ne!(copy_id, source);

        assert_eq!(
            d.db.entity(source).expect("original untouched").geom,
            Geometry::Line {
                a: Point3::ORIGIN,
                b: Point3::new(1000.0, 0.0, 0.0),
            }
        );
        assert_eq!(
            d.db.entity(copy_id).expect("copy exists").geom,
            Geometry::Line {
                a: Point3::new(0.0, 500.0, 0.0),
                b: Point3::new(1000.0, 500.0, 0.0),
            }
        );
    }

    #[test]
    fn mirror_entities_reflects_a_line_and_keeps_the_original_by_default() {
        let mut d = doc();
        let source = d
            .execute(
                "Draw",
                &Command::AddLine {
                    layer: "0".into(),
                    a: Point3::new(100.0, 0.0, 0.0),
                    b: Point3::new(100.0, 500.0, 0.0),
                },
            )
            .expect("commits")
            .created[0];

        // Mirror across the Y axis.
        let outcome = d
            .execute(
                "Mirror",
                &Command::MirrorEntities {
                    ids: vec![source],
                    a: Point3::ORIGIN,
                    b: Point3::new(0.0, 1.0, 0.0),
                    keep_original: true,
                },
            )
            .expect("commits");
        let mirrored_id = outcome.created[0];

        assert!(d.db.entity(source).is_some(), "kept by default");
        assert_eq!(
            d.db.entity(mirrored_id).expect("mirrored exists").geom,
            Geometry::Line {
                a: Point3::new(-100.0, 0.0, 0.0),
                b: Point3::new(-100.0, 500.0, 0.0),
            }
        );
    }

    #[test]
    fn mirror_entities_deletes_the_source_when_not_kept() {
        let mut d = doc();
        let source = d
            .execute(
                "Draw",
                &Command::AddCircle {
                    layer: "0".into(),
                    center: Point3::new(100.0, 0.0, 0.0),
                    radius: 50.0,
                },
            )
            .expect("commits")
            .created[0];

        let outcome = d
            .execute(
                "Mirror",
                &Command::MirrorEntities {
                    ids: vec![source],
                    a: Point3::ORIGIN,
                    b: Point3::new(0.0, 1.0, 0.0),
                    keep_original: false,
                },
            )
            .expect("commits");

        assert!(d.db.entity(source).is_none(), "source deleted");
        assert_eq!(outcome.deleted, vec![source]);
        assert_eq!(outcome.created.len(), 1);
    }

    #[test]
    fn mirror_entities_rejects_a_degenerate_line() {
        let mut d = doc();
        let source = d
            .execute(
                "Draw",
                &Command::AddLine {
                    layer: "0".into(),
                    a: Point3::ORIGIN,
                    b: Point3::new(100.0, 0.0, 0.0),
                },
            )
            .expect("commits")
            .created[0];

        let err = d
            .execute(
                "Mirror",
                &Command::MirrorEntities {
                    ids: vec![source],
                    a: Point3::ORIGIN,
                    b: Point3::ORIGIN,
                    keep_original: true,
                },
            )
            .expect_err("two coincident points do not define a line");
        assert!(matches!(err, DbError::InvalidCommand(_)));
    }

    #[test]
    fn mirror_arc_reverses_its_sweep_direction() {
        let mut d = doc();
        // A quarter circle from 0° to 90°.
        let source = d
            .execute(
                "Draw",
                &Command::AddArc {
                    layer: "0".into(),
                    center: Point3::ORIGIN,
                    radius: 500.0,
                    start_angle: 0.0,
                    sweep: std::f64::consts::FRAC_PI_2,
                },
            )
            .expect("commits")
            .created[0];

        // Mirror across the X axis.
        let outcome = d
            .execute(
                "Mirror",
                &Command::MirrorEntities {
                    ids: vec![source],
                    a: Point3::ORIGIN,
                    b: Point3::new(1.0, 0.0, 0.0),
                    keep_original: false,
                },
            )
            .expect("commits");
        let Some(Geometry::Arc {
            center,
            radius,
            start_angle,
            sweep,
            ..
        }) = d.db.entity(outcome.created[0]).map(|e| e.geom.clone())
        else {
            panic!("expected a mirrored Arc");
        };
        assert!(od_geom2d::tol::eq_len(center.x, 0.0));
        assert!(od_geom2d::tol::eq_len(center.y, 0.0));
        assert!(od_geom2d::tol::eq_len(radius, 500.0));
        // The mirrored quarter circle spans -90° to 0°.
        assert!(
            od_geom2d::tol::eq_len(start_angle, -std::f64::consts::FRAC_PI_2),
            "start_angle was {start_angle}"
        );
        assert!(
            od_geom2d::tol::eq_len(sweep, std::f64::consts::FRAC_PI_2),
            "sweep was {sweep}"
        );
    }

    #[test]
    fn offset_line_shifts_perpendicular_to_its_own_direction() {
        let mut d = doc();
        let source = d
            .execute(
                "Draw",
                &Command::AddLine {
                    layer: "0".into(),
                    a: Point3::ORIGIN,
                    b: Point3::new(1000.0, 0.0, 0.0),
                },
            )
            .expect("commits")
            .created[0];

        let outcome = d
            .execute(
                "Offset",
                &Command::OffsetEntity {
                    id: source,
                    distance: 200.0,
                },
            )
            .expect("commits");

        assert!(d.db.entity(source).is_some(), "source untouched");
        assert_eq!(
            d.db.entity(outcome.created[0]).expect("exists").geom,
            Geometry::Line {
                a: Point3::new(0.0, 200.0, 0.0),
                b: Point3::new(1000.0, 200.0, 0.0),
            }
        );
    }

    #[test]
    fn offset_circle_grows_or_shrinks_its_radius() {
        let mut d = doc();
        let source = d
            .execute(
                "Draw",
                &Command::AddCircle {
                    layer: "0".into(),
                    center: Point3::ORIGIN,
                    radius: 500.0,
                },
            )
            .expect("commits")
            .created[0];

        let outcome = d
            .execute(
                "Offset",
                &Command::OffsetEntity {
                    id: source,
                    distance: 100.0,
                },
            )
            .expect("commits");
        let Some(Geometry::Circle { radius, .. }) =
            d.db.entity(outcome.created[0]).map(|e| e.geom.clone())
        else {
            panic!("expected a Circle");
        };
        assert!(od_geom2d::tol::eq_len(radius, 600.0));
    }

    #[test]
    fn offset_rejects_a_distance_that_collapses_the_radius() {
        let mut d = doc();
        let source = d
            .execute(
                "Draw",
                &Command::AddCircle {
                    layer: "0".into(),
                    center: Point3::ORIGIN,
                    radius: 500.0,
                },
            )
            .expect("commits")
            .created[0];

        let err = d
            .execute(
                "Offset",
                &Command::OffsetEntity {
                    id: source,
                    distance: -600.0,
                },
            )
            .expect_err("a negative radius is not a circle");
        assert!(matches!(err, DbError::InvalidCommand(_)));
    }

    #[test]
    fn offset_rejects_an_unsupported_kind() {
        let mut d = doc();
        let source = d
            .execute(
                "Draw",
                &Command::AddPolyline {
                    layer: "0".into(),
                    points: vec![Point3::ORIGIN, Point3::new(100.0, 0.0, 0.0)],
                    closed: false,
                },
            )
            .expect("commits")
            .created[0];

        let err = d
            .execute(
                "Offset",
                &Command::OffsetEntity {
                    id: source,
                    distance: 10.0,
                },
            )
            .expect_err("polyline offset is not implemented yet");
        assert!(matches!(err, DbError::UnsupportedEdit { .. }));
    }

    #[test]
    fn move_entities_translates_and_undo_restores() {
        let mut d = doc();
        let outcome = d
            .execute(
                "Draw",
                &Command::AddLine {
                    layer: "0".into(),
                    a: Point3::ORIGIN,
                    b: Point3::new(1000.0, 0.0, 0.0),
                },
            )
            .expect("commits");
        let id = outcome.created[0];

        d.execute(
            "Move",
            &Command::MoveEntities {
                ids: vec![id],
                delta: Vec3::new(0.0, 500.0, 0.0),
            },
        )
        .expect("commits");
        assert_eq!(
            d.db.entity(id).expect("exists").geom,
            Geometry::Line {
                a: Point3::new(0.0, 500.0, 0.0),
                b: Point3::new(1000.0, 500.0, 0.0),
            }
        );

        assert_eq!(d.undo().as_deref(), Some("Move"));
        assert_eq!(
            d.db.entity(id).expect("exists").geom,
            Geometry::Line {
                a: Point3::ORIGIN,
                b: Point3::new(1000.0, 0.0, 0.0),
            }
        );
    }

    #[test]
    fn moving_an_unsupported_geometry_kind_fails_loudly_rather_than_silently() {
        let mut d = doc();
        let layer = d.db.ensure_layer("0");
        let space = d.db.model_space();
        // Preserved-verbatim data is the one kind this must never touch:
        // translating it by guesswork is exactly what preserving it verbatim
        // exists to prevent (rule 2 — never destroy what you cannot read).
        let id =
            d.db.insert_entity(Entity::new(
                layer,
                space,
                Geometry::Unsupported {
                    source_type: "ACAD_TABLE".into(),
                    payload: vec![1, 2, 3],
                    proxy: vec![],
                },
            ))
            .expect("inserts");

        let result = d.execute(
            "Move",
            &Command::MoveEntities {
                ids: vec![id],
                delta: Vec3::new(1.0, 0.0, 0.0),
            },
        );
        assert!(matches!(result, Err(DbError::UnsupportedEdit { .. })));
    }

    #[test]
    fn moving_a_block_reference_translates_its_position() {
        use crate::entity::BlockRef;

        let mut d = doc();
        let layer = d.db.ensure_layer("0");
        let space = d.db.model_space();
        let block = d.db.ensure_block("hvac.fan.sirocco");
        let id =
            d.db.insert_entity(Entity::new(
                layer,
                space,
                Geometry::BlockRef(Box::new(BlockRef {
                    block,
                    position: Point3::new(1000.0, 2000.0, 0.0),
                    scale: Vec3::new(1.0, 1.0, 1.0),
                    rotation: 0.0,
                    attributes: vec![],
                    array: (1, 1),
                    array_spacing: (0.0, 0.0),
                })),
            ))
            .expect("inserts");

        d.execute(
            "Move",
            &Command::MoveEntities {
                ids: vec![id],
                delta: Vec3::new(500.0, -500.0, 0.0),
            },
        )
        .expect("commits");

        let Geometry::BlockRef(block_ref) = &d.db.entity(id).expect("exists").geom else {
            panic!("still a block reference");
        };
        assert_eq!(block_ref.position, Point3::new(1500.0, 1500.0, 0.0));
    }

    #[test]
    fn moving_a_polyline_shifts_its_vertices_and_elevation() {
        use od_geom2d::{Point2, Polyline2, Vertex};

        let mut d = doc();
        let layer = d.db.ensure_layer("0");
        let space = d.db.model_space();
        let id =
            d.db.insert_entity(Entity::new(
                layer,
                space,
                Geometry::Polyline {
                    polyline: Polyline2::from_points(
                        [Point2::new(0.0, 0.0), Point2::new(100.0, 0.0)],
                        false,
                    ),
                    elevation: 500.0,
                    normal: Vec3::Z,
                    width: 0.0,
                },
            ))
            .expect("inserts");

        d.execute(
            "Move",
            &Command::MoveEntities {
                ids: vec![id],
                delta: Vec3::new(10.0, 20.0, 30.0),
            },
        )
        .expect("commits");

        let Geometry::Polyline {
            polyline,
            elevation,
            ..
        } = &d.db.entity(id).expect("exists").geom
        else {
            panic!("still a polyline");
        };
        assert_eq!(
            polyline.vertices,
            vec![
                Vertex::straight(Point2::new(10.0, 20.0)),
                Vertex::straight(Point2::new(110.0, 20.0)),
            ]
        );
        assert!(od_geom2d::tol::eq_len(*elevation, 530.0));
    }

    #[test]
    fn rotate_entities_turns_a_block_reference_in_place_and_undo_restores() {
        use crate::entity::BlockRef;

        let mut d = doc();
        let layer = d.db.ensure_layer("0");
        let space = d.db.model_space();
        let block = d.db.ensure_block("hvac.fan.sirocco");
        let id =
            d.db.insert_entity(Entity::new(
                layer,
                space,
                Geometry::BlockRef(Box::new(BlockRef {
                    block,
                    position: Point3::new(1000.0, 2000.0, 0.0),
                    scale: Vec3::new(1.0, 1.0, 1.0),
                    rotation: 0.0,
                    attributes: vec![],
                    array: (1, 1),
                    array_spacing: (0.0, 0.0),
                })),
            ))
            .expect("inserts");

        d.execute(
            "Rotate",
            &Command::RotateEntities {
                ids: vec![id],
                radians: std::f64::consts::FRAC_PI_2,
            },
        )
        .expect("commits");

        let Geometry::BlockRef(block_ref) = &d.db.entity(id).expect("exists").geom else {
            panic!("still a block reference");
        };
        assert!(od_geom2d::tol::eq_len(
            block_ref.rotation,
            std::f64::consts::FRAC_PI_2
        ));
        // Rotating in place must not move it.
        assert_eq!(block_ref.position, Point3::new(1000.0, 2000.0, 0.0));

        assert_eq!(d.undo().as_deref(), Some("Rotate"));
        let Geometry::BlockRef(block_ref) = &d.db.entity(id).expect("exists").geom else {
            panic!("still a block reference");
        };
        assert!(od_geom2d::tol::eq_len(block_ref.rotation, 0.0));
    }

    #[test]
    fn rotating_a_line_fails_loudly_rather_than_silently() {
        let mut d = doc();
        let outcome = d
            .execute(
                "Draw",
                &Command::AddLine {
                    layer: "0".into(),
                    a: Point3::ORIGIN,
                    b: Point3::new(1000.0, 0.0, 0.0),
                },
            )
            .expect("commits");
        let id = outcome.created[0];

        let result = d.execute(
            "Rotate",
            &Command::RotateEntities {
                ids: vec![id],
                radians: 1.0,
            },
        );
        assert!(matches!(result, Err(DbError::UnsupportedEdit { .. })));
    }

    #[test]
    fn delete_entities_removes_and_undo_restores() {
        let mut d = doc();
        let outcome = d
            .execute(
                "Draw",
                &Command::AddLine {
                    layer: "0".into(),
                    a: Point3::ORIGIN,
                    b: Point3::new(1000.0, 0.0, 0.0),
                },
            )
            .expect("commits");
        let id = outcome.created[0];

        d.execute("Delete", &Command::DeleteEntities { ids: vec![id] })
            .expect("commits");
        assert!(d.db.entity(id).is_none());

        assert_eq!(d.undo().as_deref(), Some("Delete"));
        assert!(d.db.entity(id).is_some());
    }
}
