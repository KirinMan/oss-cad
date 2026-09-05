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
