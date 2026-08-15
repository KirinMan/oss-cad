//! Symbol tables — the named things a drawing refers to by name.
//!
//! Layers, linetypes, text styles, blocks and so on, following the structure
//! AutoCAD's database settled on. Two entries are additions rather than
//! inheritance: [`Level`] and [`GridAxis`]. Storeys and structural grids are
//! what every element in a building drawing is positioned against, and leaving
//! them to extension data means each domain reinvents them incompatibly
//! (`docs/03-data-model.md`).

use crate::id::ObjectId;
use crate::style::{Color, LineWeight};
use indexmap::IndexMap;
use od_geom2d::Point2;
use serde::{Deserialize, Serialize};

/// A name-indexed collection of records. Names are compared case-insensitively,
/// as every DWG-family format does, but the original casing is preserved for
/// display — an importer that upper-cases every layer name produces drawings
/// that no longer match the client's standard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table<T> {
    records: IndexMap<ObjectId, T>,
    #[serde(skip)]
    by_name: IndexMap<String, ObjectId>,
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Self {
            records: IndexMap::new(),
            by_name: IndexMap::new(),
        }
    }
}

impl<T: Named> Table<T> {
    pub fn insert(&mut self, id: ObjectId, record: T) -> Option<T> {
        self.by_name.insert(record.name().to_lowercase(), id);
        self.records.insert(id, record)
    }

    #[must_use]
    pub fn get(&self, id: ObjectId) -> Option<&T> {
        self.records.get(&id)
    }

    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut T> {
        self.records.get_mut(&id)
    }

    #[must_use]
    pub fn id_of(&self, name: &str) -> Option<ObjectId> {
        self.by_name.get(&name.to_lowercase()).copied()
    }

    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<&T> {
        self.get(self.id_of(name)?)
    }

    pub fn remove(&mut self, id: ObjectId) -> Option<T> {
        let rec = self.records.shift_remove(&id)?;
        self.by_name.shift_remove(&rec.name().to_lowercase());
        Some(rec)
    }

    pub fn iter(&self) -> impl Iterator<Item = (ObjectId, &T)> {
        self.records.iter().map(|(id, r)| (*id, r))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Rebuilds the name index. Called after deserialisation, where the index is
    /// skipped rather than stored — a name index in the file is one more thing
    /// that can disagree with the records it points at.
    pub fn reindex(&mut self) {
        self.by_name = self
            .records
            .iter()
            .map(|(id, r)| (r.name().to_lowercase(), *id))
            .collect();
    }
}

/// Records in a symbol table are addressable by name.
pub trait Named {
    fn name(&self) -> &str;
}

macro_rules! named {
    ($t:ty) => {
        impl Named for $t {
            fn name(&self) -> &str {
                &self.name
            }
        }
    };
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub color: Color,
    pub lineweight: LineWeight,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linetype: Option<ObjectId>,
    pub visible: bool,
    /// Frozen layers are not drawn and not regenerated; off layers are only not
    /// drawn. The distinction matters for plotting and for xref layer state.
    pub frozen: bool,
    pub locked: bool,
    pub plottable: bool,
    pub transparency: u8,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}
named!(Layer);

impl Layer {
    /// Layer "0" — the one every drawing has and no drawing may delete. Entities
    /// on it inherit the style of the block they are nested in, which is why
    /// block definitions are conventionally drawn there.
    #[must_use]
    pub fn zero() -> Self {
        Self::new("0")
    }

    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            color: Color::FOREGROUND,
            lineweight: LineWeight::Default,
            linetype: None,
            visible: true,
            frozen: false,
            locked: false,
            plottable: true,
            transparency: 0,
            description: String::new(),
        }
    }

    #[must_use]
    pub fn with_color(mut self, c: Color) -> Self {
        self.color = c;
        self
    }

    /// Whether entities on this layer appear on screen.
    #[must_use]
    pub fn is_drawable(&self) -> bool {
        self.visible && !self.frozen
    }
}

/// A dash pattern. Positive lengths draw, negative lengths gap, zero is a dot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineType {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub pattern: Vec<f64>,
}
named!(LineType);

impl LineType {
    #[must_use]
    pub fn continuous() -> Self {
        Self {
            name: "Continuous".into(),
            description: "Solid line".into(),
            pattern: Vec::new(),
        }
    }

    #[must_use]
    pub fn is_continuous(&self) -> bool {
        self.pattern.is_empty()
    }

    /// One full cycle of the pattern, in drawing units.
    #[must_use]
    pub fn cycle_length(&self) -> f64 {
        self.pattern.iter().map(|d| d.abs()).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub name: String,
    /// Font family name, resolved against the document's embedded fonts first.
    pub font: String,
    /// Fallback used when `font` is unavailable — the mechanism that keeps a
    /// drawing legible on a machine without the original CAD fonts installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub big_font: Option<String>,
    /// 0 means "set per entity".
    pub height: f64,
    pub width_factor: f64,
    pub oblique: f64,
}
named!(TextStyle);

impl TextStyle {
    #[must_use]
    pub fn standard() -> Self {
        Self {
            name: "Standard".into(),
            // A CJK-capable default: a drawing with Japanese annotation must not
            // render as tofu on a machine that never had a CAD font installed.
            font: "Noto Sans JP".into(),
            big_font: None,
            height: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimStyle {
    pub name: String,
    pub text_style: ObjectId,
    pub text_height: f64,
    pub arrow_size: f64,
    pub extension_offset: f64,
    pub extension_beyond: f64,
    /// Overall scale applied to every size in this style.
    pub scale: f64,
    pub decimal_places: u8,
    /// Suppress trailing zeros, as JIS drawings conventionally do.
    pub suppress_trailing_zeros: bool,
}
named!(DimStyle);

/// A block definition. Model space and every paper space layout are block
/// records too, which is what lets a layout hold entities without a second
/// mechanism.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockRecord {
    pub name: String,
    pub base_point: od_geom3d::Point3,
    pub kind: BlockKind,
    /// Entities owned by this record, in draw order.
    #[serde(default)]
    pub entities: Vec<ObjectId>,
    /// External reference, when this block is an xref.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xref: Option<XrefSpec>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}
named!(BlockRecord);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    ModelSpace,
    PaperSpace,
    /// An ordinary, insertable block definition.
    Definition,
}

/// An externally referenced drawing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct XrefSpec {
    /// Path as stored, kept verbatim so a relative path stays relative.
    pub path: String,
    /// Overlays are not carried into a drawing that references *this* one,
    /// which is how circular building/services references are avoided.
    pub overlay: bool,
    /// False while the reference is unloaded or missing.
    pub resolved: bool,
}

/// A storey. Elevation is the finished floor level in drawing units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Level {
    pub name: String,
    pub elevation: f64,
    /// Distance to the next level up, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Sort key for level lists; lower is further down the building.
    pub order: i32,
}
named!(Level);

/// One structural grid line, given as an infinite line through `origin` along
/// `direction`. Curved grids are represented by their own records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridAxis {
    /// Label as drawn: `X1`, `Y3`, `A`, `通り1`.
    pub name: String,
    pub origin: Point2,
    pub direction: od_geom2d::Vec2,
    /// Which family this axis belongs to, so `X1` and `Y1` do not collide.
    pub family: String,
}
named!(GridAxis);

/// Everything a drawing refers to by name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SymbolTables {
    pub layers: Table<Layer>,
    pub linetypes: Table<LineType>,
    pub text_styles: Table<TextStyle>,
    pub dim_styles: Table<DimStyle>,
    pub blocks: Table<BlockRecord>,
    pub levels: Table<Level>,
    pub grids: Table<GridAxis>,
}

impl SymbolTables {
    /// Rebuilds every name index after loading.
    pub fn reindex(&mut self) {
        self.layers.reindex();
        self.linetypes.reindex();
        self.text_styles.reindex();
        self.dim_styles.reindex();
        self.blocks.reindex();
        self.levels.reindex();
        self.grids.reindex();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{ActorId, IdGenerator};

    #[test]
    fn names_match_case_insensitively_but_keep_their_casing() {
        let mut g = IdGenerator::new(ActorId::SYSTEM);
        let mut t: Table<Layer> = Table::default();
        let id = g.next_id();
        t.insert(id, Layer::new("A-Duct-Supply"));

        assert_eq!(t.id_of("a-duct-supply"), Some(id));
        assert_eq!(t.id_of("A-DUCT-SUPPLY"), Some(id));
        assert_eq!(
            t.by_name("a-duct-supply").map(|l| l.name.as_str()),
            Some("A-Duct-Supply"),
            "display casing must survive"
        );
    }

    #[test]
    fn removing_a_record_clears_its_name() {
        let mut g = IdGenerator::new(ActorId::SYSTEM);
        let mut t: Table<Layer> = Table::default();
        let id = g.next_id();
        t.insert(id, Layer::new("TEMP"));
        assert!(t.remove(id).is_some());
        assert_eq!(t.id_of("temp"), None);
        assert!(t.is_empty());
    }

    #[test]
    fn reindex_restores_lookup_after_a_load() {
        let mut g = IdGenerator::new(ActorId::SYSTEM);
        let mut t: Table<Layer> = Table::default();
        t.insert(g.next_id(), Layer::new("S-Beam"));

        let json = serde_json::to_string(&t).expect("serialises");
        let mut back: Table<Layer> = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back.id_of("s-beam"), None, "index is not stored");
        back.reindex();
        assert!(back.id_of("s-beam").is_some());
    }

    #[test]
    fn frozen_and_off_layers_are_both_undrawable() {
        let mut l = Layer::new("X");
        assert!(l.is_drawable());
        l.visible = false;
        assert!(!l.is_drawable());
        l.visible = true;
        l.frozen = true;
        assert!(!l.is_drawable());
    }

    #[test]
    fn linetype_cycle_length_ignores_gap_signs() {
        let lt = LineType {
            name: "DASHED".into(),
            description: String::new(),
            pattern: vec![12.7, -6.35],
        };
        assert!(!lt.is_continuous());
        assert!(od_geom2d::tol::eq_len(lt.cycle_length(), 19.05));
        assert!(LineType::continuous().is_continuous());
    }
}
