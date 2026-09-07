//! Transactions, commands and undo (ADR-006).
//!
//! Every change to a document goes through a [`Transaction`]. That single rule
//! is what makes undo, scripting, macro recording, audit logging and — later —
//! collaborative editing fall out of one mechanism instead of five. A change
//! made by mutating the database directly is a change that cannot be undone,
//! replayed, sent to a collaborator, or explained to a user.
//!
//! A transaction validates on commit and rolls back as a unit. A document is
//! never left in a half-applied state, which matters more here than in most
//! systems: a partially inserted route or a dangling layer reference is not a
//! visible error, it is a drawing that looks fine and is wrong.

use crate::database::{Database, Object};
use crate::entity::Entity;
use crate::error::{DbError, Result};
use crate::id::ObjectId;

/// One reversible step. Reverse deltas are stored rather than snapshots: a
/// drawing is large and edits are small, and holding a copy of the document per
/// undo step is how a CAD session runs out of memory.
#[derive(Debug, Clone)]
pub enum Change {
    Created {
        id: ObjectId,
    },
    Removed {
        object: Box<Object>,
        /// Position in the owner's draw order, so undo restores order too.
        index: Option<usize>,
    },
    Modified {
        id: ObjectId,
        before: Box<Object>,
    },
}

/// A committed group of changes, undone and redone as one.
#[derive(Debug, Clone)]
pub struct Batch {
    pub name: String,
    pub changes: Vec<Change>,
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Batch>,
    redo: Vec<Batch>,
    /// 0 means unlimited.
    limit: usize,
}

impl History {
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit,
        }
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Name of the next undoable batch, for the UI's "Undo <what>".
    #[must_use]
    pub fn undo_name(&self) -> Option<&str> {
        self.undo.last().map(|b| b.name.as_str())
    }

    #[must_use]
    pub fn redo_name(&self) -> Option<&str> {
        self.redo.last().map(|b| b.name.as_str())
    }

    fn push(&mut self, batch: Batch) {
        self.undo.push(batch);
        // A new edit invalidates the redo branch, as in every editor.
        self.redo.clear();
        if self.limit > 0 && self.undo.len() > self.limit {
            self.undo.remove(0);
        }
    }
}

/// An in-progress edit. Call [`Transaction::commit`] to keep it or
/// [`Transaction::rollback`] to discard it; dropping it without either is a
/// programming error and is reported as a rollback.
#[derive(Debug)]
pub struct Transaction<'db> {
    db: &'db mut Database,
    name: String,
    changes: Vec<Change>,
    finished: bool,
}

impl<'db> Transaction<'db> {
    pub(crate) fn new(db: &'db mut Database, name: impl Into<String>) -> Self {
        Self {
            db,
            name: name.into(),
            changes: Vec::new(),
            finished: false,
        }
    }

    /// Read-only view of the document as it stands mid-transaction.
    #[must_use]
    pub fn db(&self) -> &Database {
        self.db
    }

    /// Finds or creates a layer by name. Not undo-tracked, matching how a bulk
    /// importer already creates layers directly on the `Database` (DXF's
    /// reader does exactly this outside any transaction): a layer coming into
    /// existence is additive and idempotent, not the kind of change a user
    /// expects "undo" to reverse on its own.
    pub fn ensure_layer(&mut self, name: &str) -> ObjectId {
        self.db.ensure_layer(name)
    }

    /// Finds or creates a block definition by name. Not undo-tracked, for the
    /// same reason as [`Transaction::ensure_layer`].
    pub fn ensure_block(&mut self, name: &str) -> ObjectId {
        self.db.ensure_block(name)
    }

    /// Finds or creates a paper-space layout by name. Not undo-tracked, for
    /// the same reason as [`Transaction::ensure_layer`].
    pub fn ensure_paper_space(&mut self, name: &str) -> ObjectId {
        self.db.ensure_paper_space(name)
    }

    pub fn add_entity(&mut self, entity: Entity) -> Result<ObjectId> {
        let id = self.db.insert_entity(entity)?;
        self.changes.push(Change::Created { id });
        Ok(id)
    }

    /// Adds a domain-defined object. See [`Database::insert_custom`].
    pub fn add_custom(
        &mut self,
        owner: Option<ObjectId>,
        type_id: impl Into<String>,
        data: serde_json::Value,
    ) -> Result<ObjectId> {
        let id = self.db.insert_custom(owner, type_id, data)?;
        self.changes.push(Change::Created { id });
        Ok(id)
    }

    pub fn remove(&mut self, id: ObjectId) -> Result<()> {
        let index = self.db.draw_index(id);
        let object = self.db.remove_object(id)?;
        self.changes.push(Change::Removed {
            object: Box::new(object),
            index,
        });
        Ok(())
    }

    /// Applies `f` to an entity, recording enough to undo it.
    pub fn modify_entity<F>(&mut self, id: ObjectId, f: F) -> Result<()>
    where
        F: FnOnce(&mut Entity),
    {
        let before = self
            .db
            .object(id)
            .cloned()
            .ok_or(DbError::NoSuchObject(id))?;
        let obj = self.db.object_mut(id).ok_or(DbError::NoSuchObject(id))?;
        let entity = obj.as_entity_mut().ok_or(DbError::NotAnEntity(id))?;
        f(entity);
        self.changes.push(Change::Modified {
            id,
            before: Box::new(before),
        });
        Ok(())
    }

    /// Applies `f` to any object, recording enough to undo it. The general form
    /// of [`Transaction::modify_entity`], used for extension data and custom
    /// objects.
    pub fn modify_object<F>(&mut self, id: ObjectId, f: F) -> Result<()>
    where
        F: FnOnce(&mut Object),
    {
        let before = self
            .db
            .object(id)
            .cloned()
            .ok_or(DbError::NoSuchObject(id))?;
        let obj = self.db.object_mut(id).ok_or(DbError::NoSuchObject(id))?;
        f(obj);
        self.changes.push(Change::Modified {
            id,
            before: Box::new(before),
        });
        Ok(())
    }

    /// Validates and keeps the changes. On failure everything is undone, so the
    /// caller never has to reason about a partially applied edit.
    pub fn commit(mut self, history: &mut History) -> Result<usize> {
        let problems = self.db.validate();
        if !problems.is_empty() {
            let name = self.name.clone();
            self.undo_changes();
            self.finished = true;
            return Err(DbError::ValidationFailed { name, problems });
        }
        let count = self.changes.len();
        self.finished = true;
        if count > 0 {
            history.push(Batch {
                name: std::mem::take(&mut self.name),
                changes: std::mem::take(&mut self.changes),
            });
        }
        Ok(count)
    }

    pub fn rollback(mut self) {
        self.undo_changes();
        self.finished = true;
    }

    fn undo_changes(&mut self) {
        let changes = std::mem::take(&mut self.changes);
        undo_batch(self.db, &changes);
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.finished {
            // Neither committed nor rolled back — most likely an early return on
            // an error path. Undo rather than leave the edit half-applied.
            self.undo_changes();
        }
    }
}

/// Reverses a batch, last change first.
fn undo_batch(db: &mut Database, changes: &[Change]) -> Vec<Change> {
    let mut inverse = Vec::with_capacity(changes.len());
    for change in changes.iter().rev() {
        match change {
            Change::Created { id } => {
                let index = db.draw_index(*id);
                if let Ok(object) = db.remove_object(*id) {
                    inverse.push(Change::Removed {
                        object: Box::new(object),
                        index,
                    });
                }
            }
            Change::Removed { object, index } => {
                let id = object.id;
                if db.restore_object((**object).clone(), *index).is_ok() {
                    inverse.push(Change::Created { id });
                }
            }
            Change::Modified { id, before } => {
                if let Some(current) = db.object(*id).cloned() {
                    if let Some(slot) = db.object_mut(*id) {
                        *slot = (**before).clone();
                    }
                    inverse.push(Change::Modified {
                        id: *id,
                        before: Box::new(current),
                    });
                }
            }
        }
    }
    inverse
}

/// A document: the database plus its edit history.
///
/// Separating them keeps [`Database`] serialisable on its own — history lives
/// in its own part of the `.odc` container and can be trimmed or dropped
/// without touching the drawing.
#[derive(Debug, Default)]
pub struct Document {
    pub db: Database,
    pub history: History,
}

impl Document {
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self {
            db,
            history: History::default(),
        }
    }

    /// Starts an edit. The name is what the UI shows next to "Undo".
    pub fn transaction(&mut self, name: impl Into<String>) -> Transaction<'_> {
        Transaction::new(&mut self.db, name)
    }

    /// Convenience for the common shape: run a closure in a transaction,
    /// committing on success and rolling back on error.
    ///
    /// Generic in the error type rather than fixed to [`DbError`], so a
    /// domain crate built on `od-core` (`docs/03-data-model.md` §3.2) can use
    /// its own error type here directly instead of laundering every failure
    /// through `DbError` — which would mean either core naming domain
    /// concepts, or a domain crate's real errors (an unknown catalogue
    /// reference, a routing failure) getting silently reduced to something
    /// core already knows how to spell. The only requirement is that the
    /// caller's error type can represent *this* function's own failure mode
    /// (commit validation), which `E: From<DbError>` states directly.
    pub fn edit<F, T, E>(&mut self, name: impl Into<String>, f: F) -> std::result::Result<T, E>
    where
        F: FnOnce(&mut Transaction<'_>) -> std::result::Result<T, E>,
        E: From<DbError>,
    {
        let mut tx = Transaction::new(&mut self.db, name);
        match f(&mut tx) {
            Ok(value) => {
                tx.commit(&mut self.history).map_err(E::from)?;
                Ok(value)
            }
            Err(e) => {
                tx.rollback();
                Err(e)
            }
        }
    }

    /// Runs one [`crate::edit::Command`] as a single named, undoable edit
    /// (ADR-006, `docs/02-architecture.md`) — the entry point a UI, a script
    /// or a network message all go through, in preference to building a
    /// transaction closure by hand for edits simple enough to describe as
    /// data.
    pub fn execute(
        &mut self,
        name: impl Into<String>,
        command: &crate::edit::Command,
    ) -> Result<crate::edit::CommandOutcome> {
        self.edit(name, |tx| command.apply(tx))
    }

    pub fn undo(&mut self) -> Option<String> {
        let batch = self.history.undo.pop()?;
        let inverse = undo_batch(&mut self.db, &batch.changes);
        let name = batch.name.clone();
        self.history.redo.push(Batch {
            name: batch.name,
            changes: inverse,
        });
        Some(name)
    }

    pub fn redo(&mut self) -> Option<String> {
        let batch = self.history.redo.pop()?;
        let inverse = undo_batch(&mut self.db, &batch.changes);
        let name = batch.name.clone();
        self.history.undo.push(Batch {
            name: batch.name,
            changes: inverse,
        });
        Some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::MODEL_SPACE;
    use crate::entity::Geometry;
    use crate::id::{ActorId, ObjectId};
    use od_geom3d::Point3;

    fn doc() -> Document {
        Document::new(Database::new(ActorId::SYSTEM))
    }

    fn line(db: &Database, x: f64) -> Entity {
        Entity::new(
            db.header.current_layer.expect("layer 0"),
            db.model_space(),
            Geometry::Line {
                a: Point3::ORIGIN,
                b: Point3::new(x, 0.0, 0.0),
            },
        )
    }

    #[test]
    fn commit_keeps_changes_and_names_the_undo_step() {
        let mut d = doc();
        let e = line(&d.db, 100.0);
        d.edit("Draw line", |tx| tx.add_entity(e)).expect("commits");
        assert_eq!(d.db.entities().count(), 1);
        assert_eq!(d.history.undo_name(), Some("Draw line"));
    }

    #[test]
    fn a_custom_object_is_undoable_like_any_other_change() {
        let mut d = doc();
        let id = d
            .edit("Add thing", |tx| {
                tx.add_custom(None, "org.example.thing", serde_json::json!({"n": 1}))
            })
            .expect("commits");
        assert!(d.db.object(id).is_some());

        d.undo().expect("undoes");
        assert!(d.db.object(id).is_none());

        d.redo().expect("redoes");
        assert!(d.db.object(id).is_some());
    }

    #[test]
    fn rollback_leaves_no_trace() {
        let mut d = doc();
        let e = line(&d.db, 100.0);
        let mut tx = d.transaction("Draw line");
        tx.add_entity(e).expect("adds");
        tx.rollback();
        assert_eq!(d.db.entities().count(), 0);
        assert!(!d.history.can_undo());
    }

    #[test]
    fn a_dropped_transaction_does_not_leak_a_half_edit() {
        let mut d = doc();
        {
            let e = line(&d.db, 100.0);
            let mut tx = d.transaction("Abandoned");
            tx.add_entity(e).expect("adds");
            // No commit, no rollback — just dropped.
        }
        assert_eq!(d.db.entities().count(), 0);
    }

    #[test]
    fn undo_and_redo_restore_exactly() {
        let mut d = doc();
        let e1 = line(&d.db, 100.0);
        let e2 = line(&d.db, 200.0);
        d.edit("First", |tx| tx.add_entity(e1)).expect("commits");
        d.edit("Second", |tx| tx.add_entity(e2)).expect("commits");
        assert_eq!(d.db.entities().count(), 2);

        assert_eq!(d.undo().as_deref(), Some("Second"));
        assert_eq!(d.db.entities().count(), 1);
        assert_eq!(d.undo().as_deref(), Some("First"));
        assert_eq!(d.db.entities().count(), 0);
        assert!(d.undo().is_none());

        assert_eq!(d.redo().as_deref(), Some("First"));
        assert_eq!(d.redo().as_deref(), Some("Second"));
        assert_eq!(d.db.entities().count(), 2);
        assert!(d.db.validate().is_empty());
    }

    #[test]
    fn undo_restores_draw_order_after_a_delete() {
        let mut d = doc();
        let mut ids = Vec::new();
        for x in [1.0, 2.0, 3.0] {
            let e = line(&d.db, x);
            ids.push(d.edit("Draw", |tx| tx.add_entity(e)).expect("commits"));
        }
        d.edit("Erase", |tx| tx.remove(ids[1])).expect("commits");
        d.undo().expect("undoes the erase");

        let order =
            d.db.tables
                .blocks
                .by_name(MODEL_SPACE)
                .expect("model space")
                .entities
                .clone();
        assert_eq!(order, ids, "the entity must return to its original slot");
    }

    #[test]
    fn modify_is_undoable() {
        let mut d = doc();
        let e = line(&d.db, 100.0);
        let id = d.edit("Draw", |tx| tx.add_entity(e)).expect("commits");
        d.edit("Stretch", |tx| {
            tx.modify_entity(id, |e| {
                if let Geometry::Line { b, .. } = &mut e.geom {
                    b.x = 999.0;
                }
            })
        })
        .expect("commits");

        assert!(matches!(
            d.db.entity(id).map(|e| &e.geom),
            Some(Geometry::Line { b, .. }) if od_geom2d::tol::eq_len(b.x, 999.0)
        ));
        d.undo().expect("undoes");
        assert!(matches!(
            d.db.entity(id).map(|e| &e.geom),
            Some(Geometry::Line { b, .. }) if od_geom2d::tol::eq_len(b.x, 100.0)
        ));
    }

    #[test]
    fn a_new_edit_clears_the_redo_branch() {
        let mut d = doc();
        let e1 = line(&d.db, 1.0);
        d.edit("First", |tx| tx.add_entity(e1)).expect("commits");
        d.undo().expect("undoes");
        assert!(d.history.can_redo());

        let e2 = line(&d.db, 2.0);
        d.edit("Different", |tx| tx.add_entity(e2))
            .expect("commits");
        assert!(!d.history.can_redo(), "the redo branch is gone");
    }

    #[test]
    fn a_failing_edit_rolls_back_the_whole_group() {
        let mut d = doc();
        let good = line(&d.db, 1.0);
        let bogus_layer = ObjectId::new(ActorId(77), 77);
        let bad = Entity::new(
            bogus_layer,
            d.db.model_space(),
            Geometry::Point(Point3::ORIGIN),
        );

        let result: Result<()> = d.edit("Two things", |tx| {
            tx.add_entity(good)?;
            tx.add_entity(bad)?;
            Ok(())
        });

        assert!(result.is_err());
        assert_eq!(
            d.db.entities().count(),
            0,
            "the first insert must not survive the second's failure"
        );
        assert!(!d.history.can_undo());
    }

    #[test]
    fn an_empty_transaction_does_not_pollute_the_undo_stack() {
        let mut d = doc();
        d.edit("Nothing", |_| Ok::<(), DbError>(()))
            .expect("commits");
        assert!(!d.history.can_undo());
    }
}
