//! An index over a drawing.
//!
//! [`RTree`] knows nothing about documents; this is the thin layer that puts a
//! drawing into one. It lives here rather than in `od-core` because an index is
//! derived data — rebuildable from the document, and never part of what the
//! document *is*. Keeping it out of `Database` means a document cannot be
//! loaded with a stale index attached, which is a class of bug that is very
//! hard to see: the drawing is right, and only the answers about it are wrong.

use crate::{Entry, RTree};
use od_core::{Database, ObjectId, Point2, Point3};
use od_geom3d::Aabb3;

#[derive(Debug)]
pub struct DrawingIndex {
    tree: RTree<ObjectId>,
    space: ObjectId,
}

impl DrawingIndex {
    /// Indexes one space — model space, or a paper space layout.
    #[must_use]
    pub fn build(db: &Database, space: ObjectId) -> Self {
        let entries = db.entities_in(space).filter_map(|(id, _)| {
            let bounds = db.entity_bounds(id);
            (!bounds.is_empty()).then_some(Entry { bounds, value: id })
        });
        Self {
            tree: RTree::bulk_load(entries),
            space,
        }
    }

    #[must_use]
    pub fn of_model_space(db: &Database) -> Self {
        Self::build(db, db.model_space())
    }

    #[must_use]
    pub fn space(&self) -> ObjectId {
        self.space
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tree.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    /// Bounds of everything indexed — the drawing's extents.
    #[must_use]
    pub fn bounds(&self) -> Aabb3 {
        self.tree.bounds()
    }

    /// Entities whose bounds meet `window`, in the document's draw order.
    ///
    /// Draw order matters: a renderer that returns them in tree order paints
    /// hatches over the lines they were meant to sit behind.
    #[must_use]
    pub fn query(&self, db: &Database, window: Aabb3) -> Vec<ObjectId> {
        let hits: Vec<ObjectId> = self
            .tree
            .query(window)
            .into_iter()
            .map(|e| e.value)
            .collect();
        in_draw_order(db, self.space, hits)
    }

    /// The plan-view form: a screen rectangle, any height.
    #[must_use]
    pub fn query_window(&self, db: &Database, min: Point2, max: Point2) -> Vec<ObjectId> {
        let hits: Vec<ObjectId> = self
            .tree
            .query_planar(min, max)
            .into_iter()
            .map(|e| e.value)
            .collect();
        in_draw_order(db, self.space, hits)
    }

    /// The `count` entities nearest `point`, nearest first.
    ///
    /// This is the broad phase for picking and snapping: it narrows a million
    /// entities to a handful worth testing precisely against the cursor.
    #[must_use]
    pub fn nearest(&self, point: Point3, count: usize) -> Vec<ObjectId> {
        self.tree
            .nearest(point, count)
            .into_iter()
            .map(|e| e.value)
            .collect()
    }
}

/// Restores the owning block record's ordering.
fn in_draw_order(db: &Database, space: ObjectId, mut ids: Vec<ObjectId>) -> Vec<ObjectId> {
    let Some(order) = db.tables.blocks.get(space) else {
        return ids;
    };
    // A position lookup per hit beats sorting by a linear search through the
    // order for each comparison, which is what the obvious version does.
    let position: std::collections::HashMap<ObjectId, usize> = order
        .entities
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i))
        .collect();
    ids.sort_by_key(|id| position.get(id).copied().unwrap_or(usize::MAX));
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_core::{ActorId, Entity, Geometry};

    /// A row of lines, 1 m apart, as a plan drawing's grid lines would be.
    fn drawing(n: u32) -> (Database, Vec<ObjectId>) {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.ensure_layer("M-DUCT-SA");
        let space = db.model_space();
        let ids = (0..n)
            .map(|i| {
                let x = f64::from(i) * 1000.0;
                db.insert_entity(Entity::new(
                    layer,
                    space,
                    Geometry::Line {
                        a: Point3::new(x, 0.0, 0.0),
                        b: Point3::new(x, 5000.0, 0.0),
                    },
                ))
                .expect("inserts")
            })
            .collect();
        (db, ids)
    }

    #[test]
    fn an_empty_drawing_indexes_to_nothing() {
        let db = Database::default();
        let index = DrawingIndex::of_model_space(&db);
        assert!(index.is_empty());
        assert!(index.bounds().is_empty());
        assert!(index.nearest(Point3::ORIGIN, 4).is_empty());
    }

    #[test]
    fn a_window_finds_the_entities_it_crosses() {
        let (db, ids) = drawing(20);
        let index = DrawingIndex::of_model_space(&db);
        assert_eq!(index.len(), 20);

        // A window over x = 2500..5500 crosses the lines at 3000, 4000, 5000.
        let found = index.query_window(
            &db,
            Point2::new(2500.0, -100.0),
            Point2::new(5500.0, 5100.0),
        );
        assert_eq!(found, vec![ids[3], ids[4], ids[5]]);
    }

    #[test]
    fn results_come_back_in_draw_order() {
        let (db, ids) = drawing(50);
        let index = DrawingIndex::of_model_space(&db);
        let found = index.query(&db, index.bounds());
        assert_eq!(found, ids, "a renderer depends on this ordering");
    }

    #[test]
    fn nearest_narrows_to_the_lines_around_a_point() {
        let (db, ids) = drawing(30);
        let index = DrawingIndex::of_model_space(&db);

        // Just right of the line at x = 10000.
        let found = index.nearest(Point3::new(10_100.0, 2500.0, 0.0), 2);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], ids[10]);
        assert_eq!(
            found[1], ids[11],
            "the next nearest is the line to the right"
        );
    }

    #[test]
    fn the_index_covers_the_same_extents_as_the_document() {
        let (db, _) = drawing(10);
        let index = DrawingIndex::of_model_space(&db);
        let from_db = db.space_bounds(db.model_space());
        assert!(od_core::tol::eq_len(index.bounds().min.x, from_db.min.x));
        assert!(od_core::tol::eq_len(index.bounds().max.x, from_db.max.x));
    }

    #[test]
    fn a_block_reference_is_indexed_where_it_is_placed_not_where_it_is_defined() {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.ensure_layer("M-EQUIP");
        let block = db.ensure_block("AHU");
        db.insert_entity(Entity::new(
            layer,
            block,
            Geometry::Line {
                a: Point3::ORIGIN,
                b: Point3::new(2400.0, 1200.0, 0.0),
            },
        ))
        .expect("inserts");

        let space = db.model_space();
        let placed = db
            .insert_entity(Entity::new(
                layer,
                space,
                Geometry::BlockRef(Box::new(od_core::BlockRef {
                    block,
                    position: Point3::new(50_000.0, 20_000.0, 0.0),
                    scale: od_core::Vec3::new(1.0, 1.0, 1.0),
                    rotation: 0.0,
                    attributes: Vec::new(),
                    array: (1, 1),
                    array_spacing: (0.0, 0.0),
                })),
            ))
            .expect("inserts");

        let index = DrawingIndex::of_model_space(&db);
        let found = index.query_window(
            &db,
            Point2::new(49_000.0, 19_000.0),
            Point2::new(53_000.0, 22_000.0),
        );
        assert_eq!(found, vec![placed]);

        // And nothing is indexed back at the block's own origin.
        assert!(
            index
                .query_window(&db, Point2::new(-100.0, -100.0), Point2::new(100.0, 100.0))
                .is_empty()
        );
    }
}
