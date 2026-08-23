//! The document.
//!
//! One drawing is one [`Database`]: header variables, symbol tables, a named
//! object dictionary, and the objects themselves. Entities are owned by block
//! records, and model space is simply the block record named `*Model_Space` —
//! so "which space is this in" is answered by ownership rather than by a flag
//! that can disagree with reality.

use crate::entity::{Entity, Geometry};
use crate::error::{DbError, Result};
use crate::id::{ActorId, IdGenerator, ObjectId};
use crate::tables::{BlockKind, BlockRecord, DimStyle, Layer, LineType, SymbolTables, TextStyle};
use crate::xdata::{AppId, XDataMap, XDataSchema};
use indexmap::IndexMap;
use od_geom3d::{Aabb3, Point3, Vec3};
use serde::{Deserialize, Serialize};

/// The name AutoCAD gives model space. Kept identical so exporters do not have
/// to translate, and so a user reading a layer/block listing sees what they
/// expect.
pub const MODEL_SPACE: &str = "*Model_Space";
pub const PAPER_SPACE: &str = "*Paper_Space";

/// Drawing units, following the DXF `$INSUNITS` enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Units {
    Unitless,
    #[default]
    Millimeters,
    Centimeters,
    Meters,
    Inches,
    Feet,
}

impl Units {
    /// How many millimetres one unit is worth. Storage is always millimetres;
    /// this converts what a user typed.
    #[must_use]
    pub fn to_mm(self) -> f64 {
        match self {
            Units::Unitless | Units::Millimeters => 1.0,
            Units::Centimeters => 10.0,
            Units::Meters => 1000.0,
            Units::Inches => 25.4,
            Units::Feet => 304.8,
        }
    }
}

/// Where this drawing sits on the earth, for GIS and BIM coordination.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeoRef {
    /// EPSG code of the projected system, e.g. 6675 for JGD2011 / Japan Plane
    /// Rectangular CS IX.
    pub epsg: u32,
    /// The projected coordinate that drawing origin corresponds to.
    pub origin_easting: f64,
    pub origin_northing: f64,
    pub origin_elevation: f64,
    /// Rotation from true north to drawing north, in radians.
    pub project_north: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeaderVars {
    pub units: Units,
    pub linear_precision: u8,
    pub angular_precision: u8,
    /// Angle of the +X axis, in radians, for angle input and display.
    pub angle_base: f64,
    /// True when angles increase clockwise.
    pub angle_clockwise: bool,
    pub current_layer: Option<ObjectId>,
    pub insertion_base: Point3,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo: Option<GeoRef>,
}

impl Default for HeaderVars {
    fn default() -> Self {
        Self {
            units: Units::Millimeters,
            linear_precision: 2,
            angular_precision: 2,
            angle_base: 0.0,
            angle_clockwise: false,
            current_layer: None,
            insertion_base: Point3::ORIGIN,
            geo: None,
        }
    }
}

/// A named-object dictionary entry set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Dictionary {
    pub entries: IndexMap<String, ObjectId>,
}

/// Data preserved verbatim because this build could not interpret it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreservedBlob {
    /// Which reader produced it, e.g. `"dxf:r2018"`.
    pub source: String,
    pub section: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ObjectKind {
    Entity(Box<Entity>),
    Dictionary(Dictionary),
    /// A domain-defined object. The core stores and round-trips it without
    /// interpreting it; the owning plugin gives it meaning.
    Custom {
        type_id: String,
        data: serde_json::Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: ObjectId,
    /// The block record, dictionary or other object that owns this one.
    pub owner: Option<ObjectId>,
    pub kind: ObjectKind,
    #[serde(default, skip_serializing_if = "XDataMap::is_empty")]
    pub xdata: XDataMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext_dict: Option<ObjectId>,
}

impl Object {
    #[must_use]
    pub fn as_entity(&self) -> Option<&Entity> {
        match &self.kind {
            ObjectKind::Entity(e) => Some(e),
            _ => None,
        }
    }

    pub fn as_entity_mut(&mut self) -> Option<&mut Entity> {
        match &mut self.kind {
            ObjectKind::Entity(e) => Some(e),
            _ => None,
        }
    }
}

/// How deep a block reference chain may nest before it is treated as a cycle.
/// Real drawings rarely exceed four; anything past this is a corrupt file or a
/// self-referencing block, and either way recursing further only makes the
/// eventual failure harder to diagnose.
const MAX_BLOCK_DEPTH: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Database {
    pub header: HeaderVars,
    pub tables: SymbolTables,
    objects: IndexMap<ObjectId, Object>,
    ids: IdGenerator,
    /// Schemas for every application that has stored data in this document.
    /// Written into the file so the attributes stay meaningful without the
    /// plugin that produced them.
    pub schemas: IndexMap<AppId, XDataSchema>,
    pub named_dict: ObjectId,
    model_space: ObjectId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preserved: Vec<PreservedBlob>,
}

impl Database {
    /// A new drawing with the entries every drawing is required to have: layer
    /// `0`, the `Continuous` linetype, the `Standard` text style, model space,
    /// and one paper space layout.
    #[must_use]
    pub fn new(actor: ActorId) -> Self {
        let mut ids = IdGenerator::new(actor);
        let named_dict = ids.next_id();
        let model_space = ids.next_id();

        let mut tables = SymbolTables::default();
        let layer0 = ids.next_id();
        tables.layers.insert(layer0, Layer::zero());
        tables
            .linetypes
            .insert(ids.next_id(), LineType::continuous());
        let text_style = ids.next_id();
        tables.text_styles.insert(text_style, TextStyle::standard());
        tables.dim_styles.insert(
            ids.next_id(),
            DimStyle {
                name: "Standard".into(),
                text_style,
                text_height: 2.5,
                arrow_size: 2.5,
                extension_offset: 0.625,
                extension_beyond: 1.25,
                scale: 1.0,
                decimal_places: 0,
                suppress_trailing_zeros: true,
            },
        );
        tables.blocks.insert(
            model_space,
            BlockRecord {
                name: MODEL_SPACE.into(),
                base_point: Point3::ORIGIN,
                kind: BlockKind::ModelSpace,
                entities: Vec::new(),
                xref: None,
                description: String::new(),
            },
        );
        tables.blocks.insert(
            ids.next_id(),
            BlockRecord {
                name: PAPER_SPACE.into(),
                base_point: Point3::ORIGIN,
                kind: BlockKind::PaperSpace,
                entities: Vec::new(),
                xref: None,
                description: String::new(),
            },
        );

        let mut objects = IndexMap::new();
        objects.insert(
            named_dict,
            Object {
                id: named_dict,
                owner: None,
                kind: ObjectKind::Dictionary(Dictionary::default()),
                xdata: XDataMap::default(),
                ext_dict: None,
            },
        );

        Self {
            header: HeaderVars {
                current_layer: Some(layer0),
                ..HeaderVars::default()
            },
            tables,
            objects,
            ids,
            schemas: IndexMap::new(),
            named_dict,
            model_space,
            preserved: Vec::new(),
        }
    }

    #[must_use]
    pub fn model_space(&self) -> ObjectId {
        self.model_space
    }

    #[must_use]
    pub fn actor(&self) -> ActorId {
        self.ids.actor()
    }

    /// Allocates an id without creating anything, for callers that must know an
    /// id before they can build the object that will use it.
    pub fn reserve_id(&mut self) -> ObjectId {
        self.ids.next_id()
    }

    #[must_use]
    pub fn object(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(&id)
    }

    pub(crate) fn object_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.get_mut(&id)
    }

    #[must_use]
    pub fn entity(&self, id: ObjectId) -> Option<&Entity> {
        self.object(id)?.as_entity()
    }

    #[must_use]
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    pub fn objects(&self) -> impl Iterator<Item = (ObjectId, &Object)> {
        self.objects.iter().map(|(id, o)| (*id, o))
    }

    pub fn entities(&self) -> impl Iterator<Item = (ObjectId, &Entity)> {
        self.objects
            .iter()
            .filter_map(|(id, o)| o.as_entity().map(|e| (*id, e)))
    }

    /// Entities owned by one block record, in draw order.
    pub fn entities_in(&self, space: ObjectId) -> impl Iterator<Item = (ObjectId, &Entity)> {
        self.tables
            .blocks
            .get(space)
            .into_iter()
            .flat_map(|b| b.entities.iter())
            .filter_map(|id| self.entity(*id).map(|e| (*id, e)))
    }

    /// Registers a schema, rejecting one that is internally inconsistent.
    /// Doing this at registration rather than at export is what makes an
    /// unmapped attribute impossible instead of merely unlikely.
    pub fn register_schema(&mut self, schema: XDataSchema) -> Result<()> {
        schema.validate().map_err(DbError::Schema)?;
        self.schemas.insert(schema.app_id.clone(), schema);
        Ok(())
    }

    /// Inserts an entity into a block record. Prefer going through a
    /// [`crate::Transaction`]; this is the primitive it is built on.
    pub fn insert_entity(&mut self, entity: Entity) -> Result<ObjectId> {
        let space = entity.owner_space;
        if self.tables.blocks.get(space).is_none() {
            return Err(DbError::NoSuchObject(space));
        }
        if self.tables.layers.get(entity.layer).is_none() {
            return Err(DbError::NoSuchObject(entity.layer));
        }
        let id = self.ids.next_id();
        self.objects.insert(
            id,
            Object {
                id,
                owner: Some(space),
                kind: ObjectKind::Entity(Box::new(entity)),
                xdata: XDataMap::default(),
                ext_dict: None,
            },
        );
        if let Some(block) = self.tables.blocks.get_mut(space) {
            block.entities.push(id);
        }
        Ok(id)
    }

    /// Inserts a domain-defined object: an opaque `{type_id, data}` bag the
    /// core stores and round-trips without interpreting.
    ///
    /// This is the seam a domain layer builds on instead of touching the
    /// core's internals (`docs/03-data-model.md` §3.2, CLAUDE.md rule 1): the
    /// core knows there is *a* type id and *some* JSON, never what either one
    /// means. Unlike an entity, a custom object is not owned by a block record
    /// — it is data referenced by id, not something drawn directly — so
    /// `owner` is free-form and may be `None`.
    pub fn insert_custom(
        &mut self,
        owner: Option<ObjectId>,
        type_id: impl Into<String>,
        data: serde_json::Value,
    ) -> Result<ObjectId> {
        if let Some(owner) = owner {
            if !self.objects.contains_key(&owner) {
                return Err(DbError::NoSuchObject(owner));
            }
        }
        let id = self.ids.next_id();
        self.objects.insert(
            id,
            Object {
                id,
                owner,
                kind: ObjectKind::Custom {
                    type_id: type_id.into(),
                    data,
                },
                xdata: XDataMap::default(),
                ext_dict: None,
            },
        );
        Ok(id)
    }

    /// Removes an object and detaches it from its owning block record.
    pub fn remove_object(&mut self, id: ObjectId) -> Result<Object> {
        let obj = self
            .objects
            .shift_remove(&id)
            .ok_or(DbError::NoSuchObject(id))?;
        if let Some(owner) = obj.owner {
            if let Some(block) = self.tables.blocks.get_mut(owner) {
                block.entities.retain(|e| *e != id);
            }
        }
        Ok(obj)
    }

    /// Re-inserts a previously removed object under its original id. Used by
    /// undo, which must restore identity and not merely equivalent content —
    /// anything referring to the object by id has to keep working.
    pub fn restore_object(&mut self, obj: Object, index_in_owner: Option<usize>) -> Result<()> {
        let id = obj.id;
        let owner = obj.owner;
        self.ids.observe(id);
        self.objects.insert(id, obj);
        if let Some(owner) = owner {
            if let Some(block) = self.tables.blocks.get_mut(owner) {
                if !block.entities.contains(&id) {
                    let at = index_in_owner.unwrap_or(block.entities.len());
                    block.entities.insert(at.min(block.entities.len()), id);
                }
            }
        }
        Ok(())
    }

    /// The position of an entity within its owner's draw order.
    #[must_use]
    pub fn draw_index(&self, id: ObjectId) -> Option<usize> {
        let owner = self.object(id)?.owner?;
        self.tables
            .blocks
            .get(owner)?
            .entities
            .iter()
            .position(|e| *e == id)
    }

    /// World bounds of one entity, resolving block references through the
    /// database. Returns empty bounds for a reference whose definition is
    /// missing rather than guessing.
    #[must_use]
    pub fn entity_bounds(&self, id: ObjectId) -> Aabb3 {
        self.entity(id)
            .map_or(Aabb3::EMPTY, |e| self.geometry_bounds(&e.geom, 0))
    }

    fn geometry_bounds(&self, geom: &Geometry, depth: usize) -> Aabb3 {
        let Geometry::BlockRef(bref) = geom else {
            return geom.local_bounds();
        };
        if depth >= MAX_BLOCK_DEPTH {
            return Aabb3::from_points([bref.position]);
        }
        let Some(def) = self.tables.blocks.get(bref.block) else {
            return Aabb3::EMPTY;
        };
        let inner = def
            .entities
            .iter()
            .filter_map(|e| self.entity(*e))
            .fold(Aabb3::EMPTY, |acc, e| {
                acc.union(self.geometry_bounds(&e.geom, depth + 1))
            });
        if inner.is_empty() {
            return Aabb3::from_points([bref.position]);
        }
        // Scale about the block's base point, then rotate and place. The corner
        // set is transformed rather than the box, since rotation of an AABB is
        // only correct through its corners.
        let base = def.base_point;
        let (s, c) = bref.rotation.sin_cos();
        let corners = [
            Point3::new(inner.min.x, inner.min.y, inner.min.z),
            Point3::new(inner.max.x, inner.min.y, inner.min.z),
            Point3::new(inner.min.x, inner.max.y, inner.min.z),
            Point3::new(inner.max.x, inner.max.y, inner.min.z),
            Point3::new(inner.min.x, inner.min.y, inner.max.z),
            Point3::new(inner.max.x, inner.min.y, inner.max.z),
            Point3::new(inner.min.x, inner.max.y, inner.max.z),
            Point3::new(inner.max.x, inner.max.y, inner.max.z),
        ];
        let mut out = Aabb3::EMPTY;
        for corner in corners {
            let local = corner - base;
            let scaled = Vec3::new(
                local.x * bref.scale.x,
                local.y * bref.scale.y,
                local.z * bref.scale.z,
            );
            let rotated = Vec3::new(
                scaled.x * c - scaled.y * s,
                scaled.x * s + scaled.y * c,
                scaled.z,
            );
            out = out.union_point(bref.position + rotated);
        }
        out
    }

    /// Bounds of everything in one space.
    #[must_use]
    pub fn space_bounds(&self, space: ObjectId) -> Aabb3 {
        self.entities_in(space).fold(Aabb3::EMPTY, |acc, (id, _)| {
            acc.union(self.entity_bounds(id))
        })
    }

    /// Ensures a layer exists, creating it if needed, and returns its id.
    pub fn ensure_layer(&mut self, name: &str) -> ObjectId {
        if let Some(id) = self.tables.layers.id_of(name) {
            return id;
        }
        let id = self.ids.next_id();
        self.tables.layers.insert(id, Layer::new(name));
        id
    }

    /// Ensures a block definition exists and returns its id.
    pub fn ensure_block(&mut self, name: &str) -> ObjectId {
        if let Some(id) = self.tables.blocks.id_of(name) {
            return id;
        }
        let id = self.ids.next_id();
        self.tables.blocks.insert(
            id,
            BlockRecord {
                name: name.to_owned(),
                base_point: Point3::ORIGIN,
                kind: BlockKind::Definition,
                entities: Vec::new(),
                xref: None,
                description: String::new(),
            },
        );
        id
    }

    /// Restores derived state after loading: name indexes, and the id counter,
    /// which must not hand out an id the file already uses.
    pub fn rehydrate(&mut self) {
        self.tables.reindex();
        let ids: Vec<ObjectId> = self.objects.keys().copied().collect();
        for id in ids {
            self.ids.observe(id);
        }
        for (id, _) in self.tables.layers.iter() {
            self.ids.observe(id);
        }
        for (id, _) in self.tables.blocks.iter() {
            self.ids.observe(id);
        }
    }

    /// Checks the invariants the rest of the system relies on. Cheap enough to
    /// run after every import and in every test.
    pub fn validate(&self) -> Vec<DbError> {
        let mut problems = Vec::new();
        for (id, obj) in self.objects() {
            if let Some(e) = obj.as_entity() {
                if self.tables.layers.get(e.layer).is_none() {
                    problems.push(DbError::DanglingReference {
                        from: id,
                        to: e.layer,
                        what: "layer",
                    });
                }
                if self.tables.blocks.get(e.owner_space).is_none() {
                    problems.push(DbError::DanglingReference {
                        from: id,
                        to: e.owner_space,
                        what: "owner space",
                    });
                }
                if let Geometry::BlockRef(b) = &e.geom {
                    if self.tables.blocks.get(b.block).is_none() {
                        problems.push(DbError::DanglingReference {
                            from: id,
                            to: b.block,
                            what: "block definition",
                        });
                    }
                }
            }
            for (app, rec) in &obj.xdata.records {
                if let Some(schema) = self.schemas.get(app) {
                    if let Err(e) = schema.validate_record(rec) {
                        problems.push(DbError::Schema(e));
                    }
                } else {
                    problems.push(DbError::UnknownSchema(app.clone()));
                }
            }
        }
        for (block_id, block) in self.tables.blocks.iter() {
            for e in &block.entities {
                if !self.objects.contains_key(e) {
                    problems.push(DbError::DanglingReference {
                        from: block_id,
                        to: *e,
                        what: "block member",
                    });
                }
            }
        }
        problems
    }

    /// Summary used by `od inspect` and by the API's drawing report.
    #[must_use]
    pub fn stats(&self) -> DbStats {
        let mut by_type: IndexMap<String, usize> = IndexMap::new();
        let mut unsupported = 0usize;
        for (_, e) in self.entities() {
            *by_type.entry(e.geom.type_name().to_owned()).or_default() += 1;
            if matches!(e.geom, Geometry::Unsupported { .. }) {
                unsupported += 1;
            }
        }
        by_type.sort_keys();
        DbStats {
            objects: self.objects.len(),
            entities: self.entities().count(),
            layers: self.tables.layers.len(),
            blocks: self.tables.blocks.len(),
            unsupported_entities: unsupported,
            preserved_blobs: self.preserved.len(),
            entities_by_type: by_type,
            bounds: self.space_bounds(self.model_space),
        }
    }

    /// Takes the document apart into the pieces a container format stores
    /// separately.
    ///
    /// A `.odc` file is not one blob — objects are chunked so a large drawing
    /// can be read incrementally, and tables and schemas live in their own
    /// entries so a tool can read a drawing's layer list without parsing its
    /// geometry (`docs/03-data-model.md` §5). That requires reaching the parts
    /// individually, and this is the seam for it: writers get the pieces, and
    /// the invariants stay inside this module rather than being re-derived by
    /// every format.
    #[must_use]
    pub fn to_snapshot(&self) -> DatabaseSnapshot {
        DatabaseSnapshot {
            header: self.header.clone(),
            tables: self.tables.clone(),
            objects: self.objects.values().cloned().collect(),
            ids: self.ids.clone(),
            schemas: self.schemas.clone(),
            named_dict: self.named_dict,
            model_space: self.model_space,
            preserved: self.preserved.clone(),
        }
    }

    /// Rebuilds a document from its parts, restoring the derived state
    /// [`Database::rehydrate`] owns — name indexes and the id counter.
    ///
    /// Object order is preserved, because it is the draw order.
    #[must_use]
    pub fn from_snapshot(snapshot: DatabaseSnapshot) -> Self {
        let mut objects = IndexMap::with_capacity(snapshot.objects.len());
        for object in snapshot.objects {
            objects.insert(object.id, object);
        }
        let mut db = Self {
            header: snapshot.header,
            tables: snapshot.tables,
            objects,
            ids: snapshot.ids,
            schemas: snapshot.schemas,
            named_dict: snapshot.named_dict,
            model_space: snapshot.model_space,
            preserved: snapshot.preserved,
        };
        db.rehydrate();
        db
    }
}

/// A document taken apart for storage. See [`Database::to_snapshot`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSnapshot {
    pub header: HeaderVars,
    pub tables: SymbolTables,
    /// In draw order.
    pub objects: Vec<Object>,
    pub ids: IdGenerator,
    pub schemas: IndexMap<AppId, XDataSchema>,
    pub named_dict: ObjectId,
    pub model_space: ObjectId,
    pub preserved: Vec<PreservedBlob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbStats {
    pub objects: usize,
    pub entities: usize,
    pub layers: usize,
    pub blocks: usize,
    pub unsupported_entities: usize,
    pub preserved_blobs: usize,
    pub entities_by_type: IndexMap<String, usize>,
    pub bounds: Aabb3,
}

impl Default for Database {
    fn default() -> Self {
        Self::new(ActorId::SYSTEM)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::BlockRef;

    fn line(db: &Database, a: Point3, b: Point3) -> Entity {
        Entity::new(
            db.header.current_layer.expect("layer 0 exists"),
            db.model_space(),
            Geometry::Line { a, b },
        )
    }

    #[test]
    fn a_new_drawing_has_the_mandatory_entries() {
        let db = Database::new(ActorId::SYSTEM);
        assert!(db.tables.layers.by_name("0").is_some());
        assert!(db.tables.linetypes.by_name("continuous").is_some());
        assert!(db.tables.text_styles.by_name("standard").is_some());
        assert!(db.tables.blocks.by_name(MODEL_SPACE).is_some());
        assert!(db.tables.blocks.by_name(PAPER_SPACE).is_some());
        assert!(db.validate().is_empty());
    }

    #[test]
    fn inserting_an_entity_puts_it_in_the_owning_space() {
        let mut db = Database::new(ActorId::SYSTEM);
        let e = line(&db, Point3::ORIGIN, Point3::new(1000.0, 0.0, 0.0));
        let id = db.insert_entity(e).expect("inserts");
        assert_eq!(db.entities_in(db.model_space()).count(), 1);
        assert_eq!(db.draw_index(id), Some(0));
        assert!(db.validate().is_empty());
    }

    #[test]
    fn a_custom_object_round_trips_without_the_core_interpreting_it() {
        let mut db = Database::new(ActorId::SYSTEM);
        let payload = serde_json::json!({"kind": "example", "value": 42});
        let id = db
            .insert_custom(None, "org.example.thing", payload.clone())
            .expect("inserts");

        let obj = db.object(id).expect("exists");
        assert_eq!(obj.owner, None, "unowned, since it draws nothing itself");
        match &obj.kind {
            ObjectKind::Custom { type_id, data } => {
                assert_eq!(type_id, "org.example.thing");
                assert_eq!(data, &payload);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
        assert!(db.validate().is_empty());

        let json = serde_json::to_string(&db).expect("serialises");
        let back: Database = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back.object(id).map(|o| &o.kind), Some(&obj.kind));
    }

    #[test]
    fn a_custom_object_can_be_owned_by_another_object() {
        let mut db = Database::new(ActorId::SYSTEM);
        let owner = db
            .insert_custom(None, "org.example.group", serde_json::json!({}))
            .expect("inserts");
        let child = db
            .insert_custom(Some(owner), "org.example.member", serde_json::json!({}))
            .expect("inserts");
        assert_eq!(db.object(child).and_then(|o| o.owner), Some(owner));
    }

    #[test]
    fn a_custom_object_cannot_claim_a_nonexistent_owner() {
        let mut db = Database::new(ActorId::SYSTEM);
        let bogus = ObjectId::new(ActorId(77), 77);
        assert!(matches!(
            db.insert_custom(Some(bogus), "org.example.thing", serde_json::json!({})),
            Err(DbError::NoSuchObject(_))
        ));
    }

    #[test]
    fn inserting_onto_a_missing_layer_is_refused() {
        let mut db = Database::new(ActorId::SYSTEM);
        let bogus = ObjectId::new(ActorId(99), 99);
        let e = Entity::new(bogus, db.model_space(), Geometry::Point(Point3::ORIGIN));
        assert!(matches!(db.insert_entity(e), Err(DbError::NoSuchObject(_))));
    }

    #[test]
    fn removing_and_restoring_preserves_identity_and_order() {
        let mut db = Database::new(ActorId::SYSTEM);
        let a = db
            .insert_entity(line(&db, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0)))
            .expect("inserts");
        let b = db
            .insert_entity(line(&db, Point3::ORIGIN, Point3::new(2.0, 0.0, 0.0)))
            .expect("inserts");
        let c = db
            .insert_entity(line(&db, Point3::ORIGIN, Point3::new(3.0, 0.0, 0.0)))
            .expect("inserts");

        let index = db.draw_index(b).expect("has an index");
        let obj = db.remove_object(b).expect("removes");
        assert_eq!(db.entities_in(db.model_space()).count(), 2);

        db.restore_object(obj, Some(index)).expect("restores");
        let order: Vec<ObjectId> = db
            .tables
            .blocks
            .get(db.model_space())
            .expect("model space")
            .entities
            .clone();
        assert_eq!(order, vec![a, b, c], "draw order must be restored exactly");
    }

    #[test]
    fn block_reference_bounds_account_for_placement() {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.header.current_layer.expect("layer 0");
        let block = db.ensure_block("DESK");
        db.insert_entity(Entity::new(
            layer,
            block,
            Geometry::Line {
                a: Point3::ORIGIN,
                b: Point3::new(1000.0, 500.0, 0.0),
            },
        ))
        .expect("inserts into the block");

        let ins = db
            .insert_entity(Entity::new(
                layer,
                db.model_space(),
                Geometry::BlockRef(Box::new(BlockRef {
                    block,
                    position: Point3::new(10_000.0, 0.0, 0.0),
                    scale: Vec3::new(2.0, 2.0, 1.0),
                    rotation: 0.0,
                    attributes: Vec::new(),
                    array: (1, 1),
                    array_spacing: (0.0, 0.0),
                })),
            ))
            .expect("inserts");

        let b = db.entity_bounds(ins);
        assert!(od_geom2d::tol::eq_len(b.min.x, 10_000.0));
        assert!(od_geom2d::tol::eq_len(b.max.x, 12_000.0), "scaled by 2");
        assert!(od_geom2d::tol::eq_len(b.max.y, 1_000.0));
    }

    #[test]
    fn a_self_referencing_block_does_not_recurse_forever() {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.header.current_layer.expect("layer 0");
        let block = db.ensure_block("LOOP");
        db.insert_entity(Entity::new(
            layer,
            block,
            Geometry::BlockRef(Box::new(BlockRef {
                block,
                position: Point3::new(1.0, 0.0, 0.0),
                scale: Vec3::new(1.0, 1.0, 1.0),
                rotation: 0.0,
                attributes: Vec::new(),
                array: (1, 1),
                array_spacing: (0.0, 0.0),
            })),
        ))
        .expect("inserts");
        let ins = db
            .insert_entity(Entity::new(
                layer,
                db.model_space(),
                Geometry::BlockRef(Box::new(BlockRef {
                    block,
                    position: Point3::ORIGIN,
                    scale: Vec3::new(1.0, 1.0, 1.0),
                    rotation: 0.0,
                    attributes: Vec::new(),
                    array: (1, 1),
                    array_spacing: (0.0, 0.0),
                })),
            ))
            .expect("inserts");
        // Terminates, and reports something finite.
        let b = db.entity_bounds(ins);
        assert!(!b.is_empty());
    }

    #[test]
    fn validate_reports_dangling_references() {
        let mut db = Database::new(ActorId::SYSTEM);
        let id = db
            .insert_entity(line(&db, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0)))
            .expect("inserts");
        // Break the layer reference behind the database's back.
        let bogus = ObjectId::new(ActorId(42), 42);
        if let Some(e) = db.object_mut(id).and_then(Object::as_entity_mut) {
            e.layer = bogus;
        }
        let problems = db.validate();
        assert_eq!(problems.len(), 1);
        assert!(matches!(
            problems[0],
            DbError::DanglingReference { what: "layer", .. }
        ));
    }

    #[test]
    fn rehydrate_stops_ids_from_being_reissued() {
        let mut db = Database::new(ActorId(5));
        let id = db
            .insert_entity(line(&db, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0)))
            .expect("inserts");
        let json = serde_json::to_string(&db).expect("serialises");
        let mut back: Database = serde_json::from_str(&json).expect("deserialises");
        back.rehydrate();
        assert!(back.reserve_id().seq > id.seq);
        assert!(back.tables.layers.by_name("0").is_some());
        assert!(back.validate().is_empty());
    }

    #[test]
    fn a_snapshot_round_trips_with_order_and_identity_intact() {
        let mut db = Database::new(ActorId(5));
        let layer = db.ensure_layer("M-DUCT-SA");
        let ids: Vec<ObjectId> = (1..=3)
            .map(|i| {
                db.insert_entity(Entity::new(
                    layer,
                    db.model_space(),
                    Geometry::Line {
                        a: Point3::ORIGIN,
                        b: Point3::new(f64::from(i) * 1000.0, 0.0, 0.0),
                    },
                ))
                .expect("inserts")
            })
            .collect();

        let back = Database::from_snapshot(db.to_snapshot());

        assert_eq!(back.entities().count(), 3);
        assert_eq!(
            back.tables
                .blocks
                .get(back.model_space())
                .expect("model space")
                .entities,
            ids,
            "draw order must survive"
        );
        assert!(back.tables.layers.by_name("m-duct-sa").is_some());
        assert!(back.validate().is_empty());
        // The id counter came with it, so a reload cannot reissue a live id.
        let mut back = back;
        assert!(back.reserve_id().seq > ids.last().expect("ids").seq);
    }

    #[test]
    fn stats_counts_by_geometry_type() {
        let mut db = Database::new(ActorId::SYSTEM);
        db.insert_entity(line(&db, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0)))
            .expect("inserts");
        db.insert_entity(line(&db, Point3::ORIGIN, Point3::new(2.0, 0.0, 0.0)))
            .expect("inserts");
        let layer = db.header.current_layer.expect("layer 0");
        db.insert_entity(Entity::new(
            layer,
            db.model_space(),
            Geometry::Point(Point3::ORIGIN),
        ))
        .expect("inserts");

        let s = db.stats();
        assert_eq!(s.entities, 3);
        assert_eq!(s.entities_by_type.get("line"), Some(&2));
        assert_eq!(s.entities_by_type.get("point"), Some(&1));
        assert_eq!(s.unsupported_entities, 0);
    }
}
