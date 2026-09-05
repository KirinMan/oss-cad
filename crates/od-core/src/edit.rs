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

use crate::entity::{BlockRef, DimensionEntity, Entity, Geometry, TextEntity, ViewportEntity};
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
    /// Moves one vertex of `id` — a line's endpoint (`index` 0 or 1), or one
    /// of a polyline's vertices — to `position`, leaving every other vertex
    /// where it is. Grip editing, as opposed to [`Command::MoveEntities`]'s
    /// whole-entity translation. See [`crate::entity::Geometry::editable_vertices`]
    /// for exactly which kinds and indices this accepts.
    SetVertex {
        id: ObjectId,
        index: usize,
        position: Point3,
    },
    /// Draws single-line text on `layer`, in the always-present `standard`
    /// text style (`Database::new` creates it, the same one DXF's own
    /// default points at) — choosing a style is a separate feature
    /// (`docs/06-roadmap.md`'s dimensioning/text work), not part of placing
    /// text at all. `rotation` is radians, counter-clockwise, matching
    /// [`crate::entity::TextEntity`]'s own convention.
    AddText {
        layer: String,
        position: Point3,
        text: String,
        height: f64,
        rotation: f64,
    },
    /// Draws a linear dimension on `layer`, measuring `point_a` to
    /// `point_b`, in the always-present `Standard` dim style — the same
    /// "choosing a style is separate" reasoning as [`Command::AddText`].
    /// `text_override`, when given, replaces the displayed measurement
    /// entirely (e.g. a tolerance note), matching
    /// [`crate::entity::DimensionEntity::text_override`].
    AddDimension {
        layer: String,
        point_a: Point3,
        point_b: Point3,
        offset: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_override: Option<String>,
    },
    /// Groups `ids` into a new block definition named `name`, replacing them
    /// in model space with a single [`crate::entity::BlockRef`] at
    /// `base_point`, on `layer`. Each entity's geometry is stored relative
    /// to `base_point` (the block's own local origin — [`Database::ensure_block`]
    /// always starts one at `Point3::ORIGIN`), so the group renders exactly
    /// where it visually was; only its identity changes, from loose
    /// entities to one reference.
    CreateBlock {
        name: String,
        layer: String,
        base_point: Point3,
        ids: Vec<ObjectId>,
    },
    /// Inserts an existing block definition — one [`CreateBlock`] made, or
    /// one the source drawing already had — as a new
    /// [`crate::entity::BlockRef`] on `layer`. Unlike [`Command::AddLine`]'s
    /// `layer`, `block_name` is never auto-created: inserting a definition
    /// that does not exist would place nothing, silently.
    InsertBlock {
        layer: String,
        block_name: String,
        position: Point3,
        rotation: f64,
        scale: Vec3,
    },
    /// Creates a new paper-space layout named `name`. Unlike
    /// [`Command::CreateBlock`]'s `ensure`-style reuse of an existing name, a
    /// duplicate name here is an error — a layout is a top-level document
    /// object a user names deliberately (like a sheet), not an incidental
    /// grouping that is fine to fall back to when the name happens to
    /// collide.
    CreateLayout { name: String },
    /// Places a viewport on the paper-space layout `layout`, at `position`
    /// with paper-space size `width` x `height`, framing the model-space
    /// point `target` at `scale` paper units per model unit — see
    /// [`crate::entity::ViewportEntity`] for the exact geometry this
    /// produces.
    AddViewport {
        layout: String,
        position: Point3,
        width: f64,
        height: f64,
        target: Point3,
        scale: f64,
    },
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
            Command::SetVertex {
                id,
                index,
                position,
            } => {
                let entity = tx.db().entity(*id).ok_or(DbError::NoSuchObject(*id))?;
                let geometry_name = entity.geom.type_name().to_owned();
                let count = entity.geom.editable_vertices().len();
                if count == 0 {
                    return Err(DbError::UnsupportedEdit {
                        id: *id,
                        geometry: geometry_name,
                    });
                }
                if *index >= count {
                    return Err(DbError::InvalidCommand(format!(
                        "vertex index {index} is out of range — this entity has {count}"
                    )));
                }
                // A polyline is planar at one shared elevation
                // (`Geometry::Polyline`'s own doc comment), so a grip drag
                // that tried to give just one vertex a different Z would be
                // asking for something the representation cannot express —
                // caught here rather than silently moving every other
                // vertex's height along with it, or silently dropping the
                // new Z on the floor.
                if let Geometry::Polyline { elevation, .. } = &entity.geom {
                    if !tol::eq_len(position.z, *elevation) {
                        return Err(DbError::InvalidCommand(format!(
                            "this polyline's vertices all share elevation {elevation}; \
                             {} does not match",
                            position.z
                        )));
                    }
                }
                let index = *index;
                let position = *position;
                let mut handled = false;
                tx.modify_entity(*id, |e| {
                    handled = set_vertex(&mut e.geom, index, position);
                })?;
                if !handled {
                    // Unreachable in practice: `count` and `index` above
                    // already prove `set_vertex` will succeed for this
                    // exact (geom, index) pair. Kept as a real error rather
                    // than an `unreachable!()` so a future edit to either
                    // function that drifts out of sync with the other fails
                    // as a normal `Result`, not a panic (unwrap/expect/panic
                    // are denied in this crate's production code —
                    // clippy.toml).
                    return Err(DbError::UnsupportedEdit {
                        id: *id,
                        geometry: geometry_name,
                    });
                }
                outcome.modified.push(*id);
            }
            Command::AddText {
                layer,
                position,
                text,
                height,
                rotation,
            } => {
                if *height <= 0.0 {
                    return Err(DbError::InvalidCommand(
                        "text height must be positive".into(),
                    ));
                }
                // Always the `standard` style: `Database::new` creates one
                // for every document, so this can never actually miss —
                // `?` here is just how that guarantee stays honest instead
                // of an `unwrap` (denied in this crate's production code).
                let style = tx
                    .db()
                    .tables
                    .text_styles
                    .id_of("standard")
                    .ok_or_else(|| {
                        DbError::InvalidCommand("no `standard` text style in this document".into())
                    })?;
                let layer_id = tx.ensure_layer(layer);
                let space = tx.db().model_space();
                let id = tx.add_entity(Entity::new(
                    layer_id,
                    space,
                    Geometry::Text(Box::new(TextEntity {
                        position: *position,
                        value: text.clone(),
                        height: *height,
                        rotation: *rotation,
                        style,
                        flow: crate::entity::TextFlow::default(),
                        h_align: crate::entity::HAlign::default(),
                        v_align: crate::entity::VAlign::default(),
                        width_factor: 1.0,
                        oblique: 0.0,
                    })),
                ))?;
                outcome.created.push(id);
            }
            Command::AddDimension {
                layer,
                point_a,
                point_b,
                offset,
                text_override,
            } => {
                if point_a.distance_to(*point_b) < tol::POINT_EPS {
                    return Err(DbError::InvalidCommand(
                        "a dimension needs two distinct points".into(),
                    ));
                }
                // Always the `standard` dim style, for the same reason
                // AddText always uses the standard text style.
                let style = tx.db().tables.dim_styles.id_of("standard").ok_or_else(|| {
                    DbError::InvalidCommand("no `standard` dim style in this document".into())
                })?;
                let layer_id = tx.ensure_layer(layer);
                let space = tx.db().model_space();
                let id = tx.add_entity(Entity::new(
                    layer_id,
                    space,
                    Geometry::Dimension(Box::new(DimensionEntity {
                        point_a: *point_a,
                        point_b: *point_b,
                        offset: *offset,
                        text_override: text_override.clone(),
                        style,
                    })),
                ))?;
                outcome.created.push(id);
            }
            Command::CreateBlock {
                name,
                layer,
                base_point,
                ids,
            } => {
                if ids.is_empty() {
                    return Err(DbError::InvalidCommand(
                        "a block needs at least one entity".into(),
                    ));
                }
                // Snapshot first: every id has to resolve and every
                // geometry has to be translatable before anything is
                // touched, so a failure partway through never leaves the
                // selection half-converted (removed from model space with
                // nothing to show for it).
                let mut members = Vec::with_capacity(ids.len());
                for &id in ids {
                    let entity = tx.db().entity(id).ok_or(DbError::NoSuchObject(id))?;
                    let (owner_layer, style, visible) =
                        (entity.layer, entity.style.clone(), entity.visible);
                    let mut geom = entity.geom.clone();
                    let rebase = Vec3::new(-base_point.x, -base_point.y, -base_point.z);
                    if !try_translate(&mut geom, rebase) {
                        return Err(DbError::UnsupportedEdit {
                            id,
                            geometry: geom.type_name().to_owned(),
                        });
                    }
                    members.push((owner_layer, geom, style, visible));
                }

                let block_id = tx.ensure_block(name);
                for &id in ids {
                    tx.remove(id)?;
                }
                for (owner_layer, geom, style, visible) in members {
                    tx.add_entity(Entity {
                        layer: owner_layer,
                        geom,
                        style,
                        visible,
                        owner_space: block_id,
                    })?;
                }

                let layer_id = tx.ensure_layer(layer);
                let space = tx.db().model_space();
                let id = tx.add_entity(Entity::new(
                    layer_id,
                    space,
                    Geometry::BlockRef(Box::new(BlockRef {
                        block: block_id,
                        position: *base_point,
                        scale: Vec3::new(1.0, 1.0, 1.0),
                        rotation: 0.0,
                        attributes: Vec::new(),
                        array: (1, 1),
                        array_spacing: (0.0, 0.0),
                    })),
                ))?;
                outcome.created.push(id);
            }
            Command::InsertBlock {
                layer,
                block_name,
                position,
                rotation,
                scale,
            } => {
                // Never auto-created, unlike ensure_layer's usual behaviour
                // for a drawing entity's own layer: a block definition that
                // does not exist has nothing to insert, so silently
                // creating an empty one would place nothing rather than
                // erroring loudly about it.
                let block_id = tx.db().tables.blocks.id_of(block_name).ok_or_else(|| {
                    DbError::InvalidCommand(format!("no block named `{block_name}`"))
                })?;
                let layer_id = tx.ensure_layer(layer);
                let space = tx.db().model_space();
                let id = tx.add_entity(Entity::new(
                    layer_id,
                    space,
                    Geometry::BlockRef(Box::new(BlockRef {
                        block: block_id,
                        position: *position,
                        scale: *scale,
                        rotation: *rotation,
                        attributes: Vec::new(),
                        array: (1, 1),
                        array_spacing: (0.0, 0.0),
                    })),
                ))?;
                outcome.created.push(id);
            }
            Command::CreateLayout { name } => {
                if tx.db().tables.blocks.id_of(name).is_some() {
                    return Err(DbError::InvalidCommand(format!(
                        "a layout named `{name}` already exists"
                    )));
                }
                let id = tx.ensure_paper_space(name);
                outcome.created.push(id);
            }
            Command::AddViewport {
                layout,
                position,
                width,
                height,
                target,
                scale,
            } => {
                if *width <= 0.0 || *height <= 0.0 {
                    return Err(DbError::InvalidCommand(
                        "a viewport needs a positive width and height".into(),
                    ));
                }
                if *scale <= 0.0 {
                    return Err(DbError::InvalidCommand(
                        "a viewport's scale must be positive".into(),
                    ));
                }
                // Never auto-created, matching InsertBlock's treatment of
                // `block_name`: a layout that does not exist has nowhere to
                // place this viewport, so it errors loudly instead of
                // inventing one under a name the caller did not choose.
                let layout_id = tx.db().tables.blocks.id_of(layout).ok_or_else(|| {
                    DbError::InvalidCommand(format!("no layout named `{layout}`"))
                })?;
                let is_paper_space = tx
                    .db()
                    .tables
                    .blocks
                    .get(layout_id)
                    .is_some_and(|b| b.kind == crate::tables::BlockKind::PaperSpace);
                if !is_paper_space {
                    return Err(DbError::InvalidCommand(format!(
                        "`{layout}` is not a paper-space layout"
                    )));
                }
                let layer_id = tx.ensure_layer("0");
                let id = tx.add_entity(Entity::new(
                    layer_id,
                    layout_id,
                    Geometry::Viewport(Box::new(ViewportEntity {
                        position: *position,
                        width: *width,
                        height: *height,
                        target: *target,
                        scale: *scale,
                    })),
                ))?;
                outcome.created.push(id);
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
        // Only the paper-space placement moves; `target` is a model-space
        // reference and must stay put, or the viewport would silently frame
        // a different part of the model than before.
        Geometry::Viewport(vp) => vp.position = vp.position + delta,
        // `offset` is a relative perpendicular distance, so it needs no
        // change — only the two measured points move.
        Geometry::Dimension(d) => {
            d.point_a = d.point_a + delta;
            d.point_b = d.point_b + delta;
        }
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
/// Moves vertex `index` of `geom` to `position`, and reports whether it
/// knew how to — `false` covers both "this kind has no editable vertices"
/// and "index is out of range for it", which [`Command::apply`] tells apart
/// itself (via [`Geometry::editable_vertices`]'s length) before ever calling
/// this, so it always knows which one a `false` here means.
fn set_vertex(geom: &mut Geometry, index: usize, position: Point3) -> bool {
    match geom {
        Geometry::Line { a, b } => match index {
            0 => *a = position,
            1 => *b = position,
            _ => return false,
        },
        Geometry::Polyline { polyline, .. } => match polyline.vertices.get_mut(index) {
            Some(v) => {
                v.point.x = position.x;
                v.point.y = position.y;
            }
            None => return false,
        },
        Geometry::Polyline3d { points, .. } => match points.get_mut(index) {
            Some(p) => *p = position,
            None => return false,
        },
        _ => return false,
    }
    true
}

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
    fn set_vertex_moves_one_line_endpoint_and_leaves_the_other() {
        let mut d = doc();
        let id = d
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

        d.execute(
            "Grip",
            &Command::SetVertex {
                id,
                index: 0,
                position: Point3::new(-500.0, 200.0, 0.0),
            },
        )
        .expect("commits");
        assert_eq!(
            d.db.entity(id).expect("exists").geom,
            Geometry::Line {
                a: Point3::new(-500.0, 200.0, 0.0),
                b: Point3::new(1000.0, 0.0, 0.0),
            }
        );
    }

    #[test]
    fn set_vertex_moves_one_polyline_vertex_and_leaves_its_neighbours() {
        let mut d = doc();
        let id = d
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
            .expect("commits")
            .created[0];

        d.execute(
            "Grip",
            &Command::SetVertex {
                id,
                index: 1,
                position: Point3::new(1200.0, 500.0, 300.0),
            },
        )
        .expect("commits");
        let Some(Geometry::Polyline { polyline, .. }) = d.db.entity(id).map(|e| e.geom.clone())
        else {
            panic!("expected a Polyline");
        };
        assert!(od_geom2d::tol::eq_len(polyline.vertices[0].point.x, 0.0));
        assert!(od_geom2d::tol::eq_len(polyline.vertices[1].point.x, 1200.0));
        assert!(od_geom2d::tol::eq_len(polyline.vertices[1].point.y, 500.0));
        assert!(od_geom2d::tol::eq_len(polyline.vertices[2].point.x, 1000.0));
        assert!(od_geom2d::tol::eq_len(polyline.vertices[2].point.y, 1000.0));
    }

    #[test]
    fn set_vertex_rejects_an_elevation_that_does_not_match_the_polylines_own() {
        let mut d = doc();
        let id = d
            .execute(
                "Draw",
                &Command::AddPolyline {
                    layer: "0".into(),
                    points: vec![
                        Point3::new(0.0, 0.0, 300.0),
                        Point3::new(1000.0, 0.0, 300.0),
                    ],
                    closed: false,
                },
            )
            .expect("commits")
            .created[0];

        let err = d
            .execute(
                "Grip",
                &Command::SetVertex {
                    id,
                    index: 0,
                    position: Point3::new(0.0, 0.0, 999.0),
                },
            )
            .expect_err("a polyline cannot have one vertex at a different elevation");
        assert!(matches!(err, DbError::InvalidCommand(_)));
    }

    #[test]
    fn set_vertex_rejects_an_out_of_range_index() {
        let mut d = doc();
        let id = d
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

        let err = d
            .execute(
                "Grip",
                &Command::SetVertex {
                    id,
                    index: 2,
                    position: Point3::ORIGIN,
                },
            )
            .expect_err("a line only has 2 vertices");
        assert!(matches!(err, DbError::InvalidCommand(_)));
    }

    #[test]
    fn set_vertex_rejects_a_kind_with_no_editable_vertices() {
        let mut d = doc();
        let id = d
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
                "Grip",
                &Command::SetVertex {
                    id,
                    index: 0,
                    position: Point3::ORIGIN,
                },
            )
            .expect_err("a circle has no editable vertices");
        assert!(matches!(err, DbError::UnsupportedEdit { .. }));
    }

    #[test]
    fn add_text_draws_in_the_standard_style() {
        let mut d = doc();
        let outcome = d
            .execute(
                "Draw",
                &Command::AddText {
                    layer: "A-NOTE".into(),
                    position: Point3::new(1000.0, 2000.0, 0.0),
                    text: "配管平面図".into(),
                    height: 250.0,
                    rotation: 0.0,
                },
            )
            .expect("commits");
        let entity = d.db.entity(outcome.created[0]).expect("exists");
        let Geometry::Text(t) = &entity.geom else {
            panic!("expected Text, got {:?}", entity.geom);
        };
        assert_eq!(t.value, "配管平面図");
        assert_eq!(t.position, Point3::new(1000.0, 2000.0, 0.0));
        assert!(od_geom2d::tol::eq_len(t.height, 250.0));
        assert_eq!(
            d.db.tables
                .text_styles
                .get(t.style)
                .map(|s| s.name.as_str()),
            Some("Standard")
        );
    }

    #[test]
    fn add_text_rejects_a_non_positive_height() {
        let mut d = doc();
        let err = d
            .execute(
                "Draw",
                &Command::AddText {
                    layer: "0".into(),
                    position: Point3::ORIGIN,
                    text: "x".into(),
                    height: 0.0,
                    rotation: 0.0,
                },
            )
            .expect_err("zero height text is degenerate");
        assert!(matches!(err, DbError::InvalidCommand(_)));
    }

    #[test]
    fn add_dimension_measures_two_points_in_the_standard_style() {
        let mut d = doc();
        let outcome = d
            .execute(
                "Draw",
                &Command::AddDimension {
                    layer: "A-DIM".into(),
                    point_a: Point3::ORIGIN,
                    point_b: Point3::new(3600.0, 0.0, 0.0),
                    offset: 500.0,
                    text_override: None,
                },
            )
            .expect("commits");
        let entity = d.db.entity(outcome.created[0]).expect("exists");
        let Geometry::Dimension(dim) = &entity.geom else {
            panic!("expected Dimension, got {:?}", entity.geom);
        };
        assert_eq!(dim.point_a, Point3::ORIGIN);
        assert_eq!(dim.point_b, Point3::new(3600.0, 0.0, 0.0));
        assert!(od_geom2d::tol::eq_len(dim.measured_length(), 3600.0));
        assert!(dim.text_override.is_none());
        assert_eq!(
            d.db.tables
                .dim_styles
                .get(dim.style)
                .map(|s| s.name.as_str()),
            Some("Standard")
        );
    }

    #[test]
    fn add_dimension_rejects_two_coincident_points() {
        let mut d = doc();
        let err = d
            .execute(
                "Draw",
                &Command::AddDimension {
                    layer: "0".into(),
                    point_a: Point3::ORIGIN,
                    point_b: Point3::ORIGIN,
                    offset: 500.0,
                    text_override: None,
                },
            )
            .expect_err("zero-length dimension is degenerate");
        assert!(matches!(err, DbError::InvalidCommand(_)));
    }

    #[test]
    fn dimension_line_offsets_perpendicular_to_the_measured_segment() {
        let dim = DimensionEntity {
            point_a: Point3::ORIGIN,
            point_b: Point3::new(1000.0, 0.0, 0.0),
            offset: 200.0,
            text_override: None,
            style: crate::id::ObjectId::new(crate::id::ActorId::SYSTEM, 1),
        };
        let (line_a, line_b) = dim.dimension_line();
        assert!(od_geom2d::tol::eq_len(line_a.x, 0.0));
        assert!(od_geom2d::tol::eq_len(line_a.y, 200.0));
        assert!(od_geom2d::tol::eq_len(line_b.x, 1000.0));
        assert!(od_geom2d::tol::eq_len(line_b.y, 200.0));
    }

    #[test]
    fn create_block_groups_entities_and_keeps_their_world_position() {
        let mut d = doc();
        let line_id = d
            .execute(
                "Draw",
                &Command::AddLine {
                    layer: "0".into(),
                    a: Point3::new(1000.0, 2000.0, 0.0),
                    b: Point3::new(1500.0, 2000.0, 0.0),
                },
            )
            .expect("commits")
            .created[0];
        let circle_id = d
            .execute(
                "Draw",
                &Command::AddCircle {
                    layer: "0".into(),
                    center: Point3::new(1250.0, 2200.0, 0.0),
                    radius: 100.0,
                },
            )
            .expect("commits")
            .created[0];

        let outcome = d
            .execute(
                "Block",
                &Command::CreateBlock {
                    name: "FAN".into(),
                    layer: "A-EQPT".into(),
                    base_point: Point3::new(1250.0, 2000.0, 0.0),
                    ids: vec![line_id, circle_id],
                },
            )
            .expect("commits");

        // The originals are gone, replaced by one BlockRef.
        assert!(d.db.entity(line_id).is_none());
        assert!(d.db.entity(circle_id).is_none());
        assert_eq!(outcome.created.len(), 1);
        let bref_entity = d.db.entity(outcome.created[0]).expect("exists");
        let Geometry::BlockRef(bref) = &bref_entity.geom else {
            panic!("expected a BlockRef, got {:?}", bref_entity.geom);
        };
        assert_eq!(bref.position, Point3::new(1250.0, 2000.0, 0.0));

        let block = d.db.tables.blocks.get(bref.block).expect("block exists");
        assert_eq!(block.name, "FAN");
        assert_eq!(block.entities.len(), 2);

        // The block's own member geometry is rebased around base_point, but
        // the whole thing still resolves to the same world position it had
        // before — entity_bounds() walks through the BlockRef exactly the
        // way rendering does.
        let world_bounds = d.db.entity_bounds(outcome.created[0]);
        assert!(od_geom2d::tol::eq_len(world_bounds.min.x, 1000.0));
        assert!(od_geom2d::tol::eq_len(world_bounds.max.x, 1500.0));
    }

    #[test]
    fn create_block_rejects_an_empty_selection() {
        let mut d = doc();
        let err = d
            .execute(
                "Block",
                &Command::CreateBlock {
                    name: "EMPTY".into(),
                    layer: "0".into(),
                    base_point: Point3::ORIGIN,
                    ids: vec![],
                },
            )
            .expect_err("a block needs at least one entity");
        assert!(matches!(err, DbError::InvalidCommand(_)));
    }

    #[test]
    fn insert_block_places_a_new_reference_to_an_existing_definition() {
        let mut d = doc();
        let line_id = d
            .execute(
                "Draw",
                &Command::AddLine {
                    layer: "0".into(),
                    a: Point3::ORIGIN,
                    b: Point3::new(500.0, 0.0, 0.0),
                },
            )
            .expect("commits")
            .created[0];
        let first_ref = d
            .execute(
                "Block",
                &Command::CreateBlock {
                    name: "SYMBOL".into(),
                    layer: "0".into(),
                    base_point: Point3::ORIGIN,
                    ids: vec![line_id],
                },
            )
            .expect("commits")
            .created[0];

        let outcome = d
            .execute(
                "Insert",
                &Command::InsertBlock {
                    layer: "0".into(),
                    block_name: "SYMBOL".into(),
                    position: Point3::new(5000.0, 5000.0, 0.0),
                    rotation: 0.0,
                    scale: Vec3::new(1.0, 1.0, 1.0),
                },
            )
            .expect("commits");

        let second_ref = outcome.created[0];
        assert_ne!(first_ref, second_ref);
        let Geometry::BlockRef(a) = &d.db.entity(first_ref).expect("exists").geom else {
            panic!("expected BlockRef");
        };
        let Geometry::BlockRef(b) = &d.db.entity(second_ref).expect("exists").geom else {
            panic!("expected BlockRef");
        };
        assert_eq!(a.block, b.block, "both reference the same definition");
        assert_eq!(b.position, Point3::new(5000.0, 5000.0, 0.0));
    }

    #[test]
    fn insert_block_rejects_a_name_that_does_not_exist() {
        let mut d = doc();
        let err = d
            .execute(
                "Insert",
                &Command::InsertBlock {
                    layer: "0".into(),
                    block_name: "NO-SUCH-BLOCK".into(),
                    position: Point3::ORIGIN,
                    rotation: 0.0,
                    scale: Vec3::new(1.0, 1.0, 1.0),
                },
            )
            .expect_err("cannot insert a block that was never defined");
        assert!(matches!(err, DbError::InvalidCommand(_)));
    }

    #[test]
    fn create_layout_adds_a_new_paper_space_block() {
        let mut d = doc();
        let outcome = d
            .execute(
                "Layout",
                &Command::CreateLayout {
                    name: "Sheet 1".into(),
                },
            )
            .expect("commits");
        let id = outcome.created[0];
        let block = d.db.tables.blocks.get(id).expect("exists");
        assert_eq!(block.name, "Sheet 1");
        assert_eq!(block.kind, crate::tables::BlockKind::PaperSpace);
    }

    #[test]
    fn create_layout_rejects_a_duplicate_name() {
        let mut d = doc();
        d.execute(
            "Layout",
            &Command::CreateLayout {
                name: "Sheet 1".into(),
            },
        )
        .expect("commits");
        let err = d
            .execute(
                "Layout",
                &Command::CreateLayout {
                    name: "Sheet 1".into(),
                },
            )
            .expect_err("the name is already taken");
        assert!(matches!(err, DbError::InvalidCommand(_)));
    }

    #[test]
    fn add_viewport_places_one_on_an_existing_layout() {
        let mut d = doc();
        let outcome = d
            .execute(
                "Viewport",
                &Command::AddViewport {
                    layout: crate::database::PAPER_SPACE.into(),
                    position: Point3::new(100.0, 100.0, 0.0),
                    width: 200.0,
                    height: 150.0,
                    target: Point3::new(5000.0, 5000.0, 0.0),
                    scale: 0.5,
                },
            )
            .expect("commits");
        let id = outcome.created[0];
        let Geometry::Viewport(vp) = &d.db.entity(id).expect("exists").geom else {
            panic!("expected Viewport");
        };
        assert_eq!(vp.position, Point3::new(100.0, 100.0, 0.0));
        assert_eq!(vp.target, Point3::new(5000.0, 5000.0, 0.0));
        assert_eq!(vp.scale, 0.5);
    }

    #[test]
    fn add_viewport_rejects_a_non_positive_size_or_scale() {
        let mut d = doc();
        let bad_width = Command::AddViewport {
            layout: crate::database::PAPER_SPACE.into(),
            position: Point3::ORIGIN,
            width: 0.0,
            height: 150.0,
            target: Point3::ORIGIN,
            scale: 1.0,
        };
        assert!(matches!(
            d.execute("Viewport", &bad_width)
                .expect_err("width must be positive"),
            DbError::InvalidCommand(_)
        ));

        let bad_scale = Command::AddViewport {
            layout: crate::database::PAPER_SPACE.into(),
            position: Point3::ORIGIN,
            width: 200.0,
            height: 150.0,
            target: Point3::ORIGIN,
            scale: -1.0,
        };
        assert!(matches!(
            d.execute("Viewport", &bad_scale)
                .expect_err("scale must be positive"),
            DbError::InvalidCommand(_)
        ));
    }

    #[test]
    fn add_viewport_rejects_an_unknown_layout() {
        let mut d = doc();
        let err = d
            .execute(
                "Viewport",
                &Command::AddViewport {
                    layout: "NO-SUCH-LAYOUT".into(),
                    position: Point3::ORIGIN,
                    width: 200.0,
                    height: 150.0,
                    target: Point3::ORIGIN,
                    scale: 1.0,
                },
            )
            .expect_err("cannot place a viewport on a layout that was never defined");
        assert!(matches!(err, DbError::InvalidCommand(_)));
    }

    #[test]
    fn add_viewport_rejects_model_space_as_the_layout() {
        let mut d = doc();
        let err = d
            .execute(
                "Viewport",
                &Command::AddViewport {
                    layout: crate::database::MODEL_SPACE.into(),
                    position: Point3::ORIGIN,
                    width: 200.0,
                    height: 150.0,
                    target: Point3::ORIGIN,
                    scale: 1.0,
                },
            )
            .expect_err("model space is not a paper-space layout");
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
