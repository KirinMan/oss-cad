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
use od_geom3d::{Point3, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    /// Draws a straight line on `layer` (created if it does not exist yet),
    /// in model space.
    AddLine { layer: String, a: Point3, b: Point3 },
    /// Translates every named entity by the same offset.
    MoveEntities { ids: Vec<ObjectId>, delta: Vec3 },
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
fn try_translate(geom: &mut Geometry, delta: Vec3) -> bool {
    match geom {
        Geometry::Point(p) => *p = *p + delta,
        Geometry::Line { a, b } => {
            *a = *a + delta;
            *b = *b + delta;
        }
        Geometry::Circle { center, .. } | Geometry::Arc { center, .. } => *center = *center + delta,
        // Equipment and fittings are placed as block references
        // (`docs/04-mep.md`), so moving one is the common case of moving a
        // piece of MEP content, not a rare one.
        Geometry::BlockRef(block_ref) => block_ref.position = block_ref.position + delta,
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
        let id =
            d.db.insert_entity(Entity::new(
                layer,
                space,
                Geometry::Polyline3d {
                    points: vec![Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0)],
                    closed: false,
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
