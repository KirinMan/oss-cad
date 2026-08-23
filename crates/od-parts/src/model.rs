//! Part, system and spec definitions.

use crate::expr::{Expr, Value};
use od_core::{AppId, FieldType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Text shown to a user, in the languages the catalogue carries.
///
/// Japanese first because that is the working language of the trades this
/// library is for; English is the fallback so the catalogue is still usable
/// outside Japan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Localized {
    pub ja: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub en: String,
}

impl Localized {
    #[must_use]
    pub fn get(&self, lang: &str) -> &str {
        match lang {
            "en" if !self.en.is_empty() => &self.en,
            _ => &self.ja,
        }
    }
}

/// What a port may connect to. Connections are refused across kinds, which
/// catches the most common modelling mistake — a supply duct joined to a drain
/// — at the moment it is drawn rather than at the clash review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemKind {
    /// 空調（給気・還気・排気・外気）
    Air,
    /// 給水・給湯
    Water,
    /// 排水・通気
    Drainage,
    /// 冷温水・冷媒
    Hydronic,
    /// 消火
    FireProtection,
    /// ガス
    Gas,
    /// 電力
    Power,
    /// 弱電・通信
    Signal,
    /// Fits anything — supports, sleeves, penetrations.
    Generic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileDef {
    Rect {
        w: Value,
        h: Value,
    },
    Round {
        d: Value,
    },
    Oval {
        w: Value,
        h: Value,
    },
    /// A port that carries no duct or pipe, e.g. an electrical terminal.
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortDef {
    pub name: String,
    /// Position in the part's local frame.
    pub origin: [Value; 3],
    /// Outward direction — the way a connecting run leaves the part.
    pub direction: [Value; 3],
    pub profile: ProfileDef,
    pub system_kind: SystemKind,
}

/// A 2D symbol primitive, drawn in the part's local frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Shape2d {
    Line {
        a: [Value; 2],
        b: [Value; 2],
    },
    Circle {
        c: [Value; 2],
        r: Value,
    },
    Arc {
        c: [Value; 2],
        r: Value,
        /// Degrees, counter-clockwise.
        start: Value,
        sweep: Value,
    },
    Polyline {
        points: Vec<[Value; 2]>,
        #[serde(default)]
        closed: bool,
    },
    Text {
        at: [Value; 2],
        value: String,
        height: Value,
    },
}

/// A 3D body primitive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body3d {
    Box {
        /// Corner with the lowest coordinates.
        min: [Value; 3],
        max: [Value; 3],
    },
    Cylinder {
        base: [Value; 3],
        axis: [Value; 3],
        radius: Value,
    },
    /// A rectangular or round elbow: a profile swept through an arc.
    Bend {
        center: [Value; 3],
        /// Rotation axis of the bend.
        axis: [Value; 3],
        radius: Value,
        /// Degrees.
        angle: Value,
        profile: ProfileDef,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub label: Localized,
    pub default: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    pub unit: String,
    /// When present, the parameter may only take these values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<f64>,
}

/// A property carried into the drawing and out to IFC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertyDef {
    pub name: String,
    pub label: Localized,
    pub ty: FieldType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// `PropertySet.Property`, required for the same reason it is required on a
    /// schema field: an attribute with nowhere to go in IFC is an attribute
    /// that will be lost.
    pub ifc_property: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    DuctFitting,
    DuctTerminal,
    DuctEquipment,
    PipeFitting,
    Valve,
    Sanitary,
    HvacEquipment,
    PlumbingEquipment,
    ElectricalFixture,
    ElectricalEquipment,
    Support,
    Penetration,
}

impl Category {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Category::DuctFitting => "duct-fitting",
            Category::DuctTerminal => "duct-terminal",
            Category::DuctEquipment => "duct-equipment",
            Category::PipeFitting => "pipe-fitting",
            Category::Valve => "valve",
            Category::Sanitary => "sanitary",
            Category::HvacEquipment => "hvac-equipment",
            Category::PlumbingEquipment => "plumbing-equipment",
            Category::ElectricalFixture => "electrical-fixture",
            Category::ElectricalEquipment => "electrical-equipment",
            Category::Support => "support",
            Category::Penetration => "penetration",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub id: String,
    pub name: Localized,
    pub category: Category,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The IFC entity this part becomes on export.
    pub ifc_class: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<PortDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbol2d: Vec<Shape2d>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body3d: Vec<Body3d>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<PropertyDef>,
    /// Free-text source note: a standard, a public catalogue, or "generic".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

impl Part {
    /// Default values for every parameter.
    #[must_use]
    pub fn default_params(&self) -> HashMap<String, f64> {
        let mut out: HashMap<String, f64> = HashMap::new();
        // Two passes so a default may refer to an earlier parameter, which is
        // how "radius defaults to one width" is expressed.
        for _ in 0..2 {
            for p in &self.parameters {
                if let Ok(v) = p.default.eval(&out) {
                    out.insert(p.name.clone(), v);
                }
            }
        }
        out
    }

    /// Merges caller-supplied values over the defaults, clamping to declared
    /// ranges. Out-of-range input is clamped rather than refused: a user
    /// dragging a size handle should hit a limit, not an error dialog.
    #[must_use]
    pub fn resolve_params(&self, overrides: &HashMap<String, f64>) -> HashMap<String, f64> {
        let mut params = self.default_params();
        for p in &self.parameters {
            if let Some(v) = overrides.get(&p.name) {
                let mut v = *v;
                if let Some(min) = p.min {
                    v = v.max(min);
                }
                if let Some(max) = p.max {
                    v = v.min(max);
                }
                params.insert(p.name.clone(), v);
            }
        }
        // Re-evaluate derived defaults against the overrides.
        for p in &self.parameters {
            if overrides.contains_key(&p.name) {
                continue;
            }
            if matches!(p.default.0, Expr::Number(_)) {
                continue;
            }
            if let Ok(v) = p.default.eval(&params) {
                params.insert(p.name.clone(), v);
            }
        }
        params
    }

    /// The extension-data schema this part contributes, so that a drawing using
    /// it stays self-describing.
    #[must_use]
    pub fn xdata_schema(&self, app: &AppId) -> od_core::XDataSchema {
        od_core::XDataSchema::new(
            app.clone(),
            "0.1.0",
            self.properties
                .iter()
                .map(|p| {
                    let mut f = od_core::FieldDef::new(&p.name, p.ty, &p.ifc_property)
                        .described(p.label.ja.clone());
                    if let Some(u) = &p.unit {
                        f = f.with_unit(u.clone());
                    }
                    f
                })
                .collect(),
        )
    }
}

/// A named system: what it carries, how it is drawn, and where it sits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemDef {
    pub id: String,
    pub name: Localized,
    pub kind: SystemKind,
    /// AutoCAD colour index, so system colours survive a DXF trip.
    pub color: u8,
    /// Layer these elements are drawn on by default.
    pub layer: String,
    /// Default centreline height above the storey's floor level, in mm.
    #[serde(default)]
    pub default_elevation: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub abbreviation: String,
}

/// One stocked size within a spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SizeEntry {
    /// Trade designation: `"50A"`, `"400x300"`, `"φ200"`.
    pub designation: String,
    /// Nominal size used for selection, in mm.
    pub nominal: f64,
    /// Outside diameter or the larger side, in mm.
    pub outside: f64,
    /// Wall thickness, in mm.
    #[serde(default)]
    pub thickness: f64,
    /// Mass per metre, in kg — what a quantity take-off needs.
    #[serde(default)]
    pub mass_per_m: f64,
}

/// How a run of duct or pipe is fabricated: what fittings it uses, how tightly
/// it may bend, and what it is made of. Editable by users, because a hard-coded
/// specification is unusable outside the practice it was written for
/// (`docs/04-mep.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    pub id: String,
    pub name: Localized,
    pub system_kind: SystemKind,
    pub material: Localized,
    /// Part id used for a change of direction.
    pub elbow_part: String,
    /// Part id used for a branch.
    pub branch_part: String,
    /// Part id used for a change of size.
    pub reducer_part: String,
    /// Minimum bend radius as a multiple of the nominal size.
    pub min_bend_radius_ratio: f64,
    /// Straight lengths as supplied, in mm. Take-off counts joints from this.
    #[serde(default)]
    pub stock_length: f64,
    pub joint: Localized,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sizes: Vec<SizeEntry>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub standard: String,
}

impl Spec {
    /// The smallest stocked size that meets `required_mm`, or the largest if
    /// nothing is big enough. Returning the largest rather than `None` matches
    /// what an engineer does next — pick the biggest and note that it is short.
    #[must_use]
    pub fn select_size(&self, required_mm: f64) -> Option<&SizeEntry> {
        self.sizes
            .iter()
            .filter(|s| s.nominal >= required_mm)
            .min_by(|a, b| a.nominal.total_cmp(&b.nominal))
            .or_else(|| {
                self.sizes
                    .iter()
                    .max_by(|a, b| a.nominal.total_cmp(&b.nominal))
            })
    }

    #[must_use]
    pub fn size(&self, designation: &str) -> Option<&SizeEntry> {
        self.sizes.iter().find(|s| s.designation == designation)
    }

    /// Minimum centreline radius for a bend at this size.
    #[must_use]
    pub fn min_bend_radius(&self, nominal_mm: f64) -> f64 {
        nominal_mm * self.min_bend_radius_ratio
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Spec {
        Spec {
            id: "test".into(),
            name: Localized {
                ja: "試験".into(),
                en: "Test".into(),
            },
            system_kind: SystemKind::Water,
            material: Localized {
                ja: "鋼".into(),
                en: "Steel".into(),
            },
            elbow_part: "e".into(),
            branch_part: "b".into(),
            reducer_part: "r".into(),
            min_bend_radius_ratio: 1.5,
            stock_length: 5500.0,
            joint: Localized {
                ja: "ねじ込み".into(),
                en: "Threaded".into(),
            },
            standard: "JIS G 3452".into(),
            sizes: vec![
                SizeEntry {
                    designation: "15A".into(),
                    nominal: 15.0,
                    outside: 21.7,
                    thickness: 2.8,
                    mass_per_m: 1.31,
                },
                SizeEntry {
                    designation: "25A".into(),
                    nominal: 25.0,
                    outside: 34.0,
                    thickness: 3.2,
                    mass_per_m: 2.43,
                },
                SizeEntry {
                    designation: "50A".into(),
                    nominal: 50.0,
                    outside: 60.5,
                    thickness: 3.8,
                    mass_per_m: 5.31,
                },
            ],
        }
    }

    #[test]
    fn size_selection_rounds_up_to_a_stocked_size() {
        let s = spec();
        assert_eq!(
            s.select_size(20.0).map(|e| e.designation.as_str()),
            Some("25A")
        );
        assert_eq!(
            s.select_size(25.0).map(|e| e.designation.as_str()),
            Some("25A")
        );
        assert_eq!(
            s.select_size(0.0).map(|e| e.designation.as_str()),
            Some("15A")
        );
    }

    #[test]
    fn asking_for_more_than_stocked_returns_the_largest() {
        let s = spec();
        assert_eq!(
            s.select_size(500.0).map(|e| e.designation.as_str()),
            Some("50A"),
            "an engineer needs the closest available, not an empty answer"
        );
    }

    #[test]
    fn bend_radius_follows_the_spec_ratio() {
        assert!(od_core::tol::eq_len(spec().min_bend_radius(50.0), 75.0));
    }

    #[test]
    fn derived_defaults_track_their_inputs() {
        let part = Part {
            id: "duct.elbow".into(),
            name: Localized {
                ja: "エルボ".into(),
                en: "Elbow".into(),
            },
            category: Category::DuctFitting,
            tags: vec![],
            ifc_class: "IfcDuctFitting".into(),
            parameters: vec![
                Parameter {
                    name: "W".into(),
                    label: Localized {
                        ja: "幅".into(),
                        en: "Width".into(),
                    },
                    default: Value::constant(400.0),
                    min: Some(50.0),
                    max: Some(2000.0),
                    unit: "mm".into(),
                    choices: vec![],
                },
                Parameter {
                    name: "R".into(),
                    label: Localized {
                        ja: "曲げ半径".into(),
                        en: "Radius".into(),
                    },
                    default: Value(Expr::parse("max(W, 150)").expect("parses")),
                    min: None,
                    max: None,
                    unit: "mm".into(),
                    choices: vec![],
                },
            ],
            ports: vec![],
            symbol2d: vec![],
            body3d: vec![],
            properties: vec![],
            source: "generic".into(),
        };

        let defaults = part.default_params();
        assert!(od_core::tol::eq_len(defaults["W"], 400.0));
        assert!(od_core::tol::eq_len(defaults["R"], 400.0));

        // Overriding the width moves the derived radius with it.
        let mut over = HashMap::new();
        over.insert("W".to_owned(), 100.0);
        let resolved = part.resolve_params(&over);
        assert!(od_core::tol::eq_len(resolved["W"], 100.0));
        assert!(
            od_core::tol::eq_len(resolved["R"], 150.0),
            "the floor applies"
        );

        // Out-of-range input is clamped, not rejected.
        let mut too_big = HashMap::new();
        too_big.insert("W".to_owned(), 99_999.0);
        assert!(od_core::tol::eq_len(
            part.resolve_params(&too_big)["W"],
            2000.0
        ));
    }
}
