//! Extension data — how a domain layer stores what it knows on a core object.
//!
//! Unlike AutoCAD's XDATA, records here carry a schema. That is the whole point:
//! the recurring complaint about MEP interoperability is that attributes survive
//! a format conversion as bytes but lose their meaning. A schema travels with the
//! document (`schemas/` in the `.odc` container), so a reader without the plugin
//! still knows a field is "design airflow, m³/h, required" rather than "a number
//! called Q".

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Namespaced owner of a record, e.g. `org.opendraft.mep`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AppId(pub String);

impl AppId {
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A value in an extension record.
///
/// The set is deliberately small and closed. Every variant must have an obvious
/// representation in DXF, IFC property sets and JSON, or round-tripping becomes
/// a per-field negotiation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum Value {
    Bool(bool),
    Int(i64),
    /// A real number in the field's declared unit.
    Real(f64),
    Text(String),
    /// A reference to another object in the same document.
    Ref(crate::id::ObjectId),
    List(Vec<Value>),
}

impl Value {
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Real(_) => "real",
            Value::Text(_) => "text",
            Value::Ref(_) => "ref",
            Value::List(_) => "list",
        }
    }

    #[must_use]
    pub fn as_real(&self) -> Option<f64> {
        match self {
            Value::Real(v) => Some(*v),
            #[expect(
                clippy::cast_precision_loss,
                reason = "an integer field read as a real is an explicit widening the caller asked for"
            )]
            Value::Int(v) => Some(*v as f64),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
}

/// The declared type of a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Bool,
    Int,
    Real,
    Text,
    Ref,
    List,
}

impl FieldType {
    #[must_use]
    pub fn accepts(self, v: &Value) -> bool {
        matches!(
            (self, v),
            (FieldType::Bool, Value::Bool(_))
                | (FieldType::Int, Value::Int(_))
                | (FieldType::Real, Value::Real(_) | Value::Int(_))
                | (FieldType::Text, Value::Text(_))
                | (FieldType::Ref, Value::Ref(_))
                | (FieldType::List, Value::List(_))
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDef {
    pub name: String,
    pub ty: FieldType,
    /// Unit symbol for numeric fields (`"mm"`, `"m3/h"`, `"Pa"`, `"A"`).
    /// Required for `Real`: a number without a unit is how conversions go wrong.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Where this field lands in IFC, as `PropertySet.Property`.
    ///
    /// Not optional by accident: [`XDataSchema::validate`] rejects a schema
    /// whose fields have no IFC mapping, which is how "the exporter forgot this
    /// attribute" becomes impossible rather than merely unlikely.
    pub ifc_property: String,
}

impl FieldDef {
    #[must_use]
    pub fn new(name: impl Into<String>, ty: FieldType, ifc_property: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty,
            unit: None,
            required: false,
            default: None,
            description: String::new(),
            ifc_property: ifc_property.into(),
        }
    }

    #[must_use]
    pub fn with_unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    #[must_use]
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    #[must_use]
    pub fn with_default(mut self, v: Value) -> Self {
        self.default = Some(v);
        self
    }

    #[must_use]
    pub fn described(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct XDataSchema {
    pub app_id: AppId,
    pub version: String,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SchemaError {
    #[error("field `{0}` declares no IFC property mapping")]
    MissingIfcMapping(String),
    #[error("numeric field `{0}` declares no unit")]
    MissingUnit(String),
    #[error("duplicate field `{0}`")]
    DuplicateField(String),
    #[error("field `{field}` expects {expected:?} but got {actual}")]
    TypeMismatch {
        field: String,
        expected: FieldType,
        actual: &'static str,
    },
    #[error("required field `{0}` is absent")]
    MissingRequired(String),
    #[error("field `{0}` is not declared by schema {1}")]
    UnknownField(String, AppId),
}

impl XDataSchema {
    #[must_use]
    pub fn new(app_id: AppId, version: impl Into<String>, fields: Vec<FieldDef>) -> Self {
        Self {
            app_id,
            version: version.into(),
            fields,
        }
    }

    /// Checks the schema itself, before it is ever used to store anything.
    pub fn validate(&self) -> Result<(), SchemaError> {
        let mut seen = std::collections::HashSet::new();
        for f in &self.fields {
            if !seen.insert(&f.name) {
                return Err(SchemaError::DuplicateField(f.name.clone()));
            }
            if f.ifc_property.trim().is_empty() {
                return Err(SchemaError::MissingIfcMapping(f.name.clone()));
            }
            if matches!(f.ty, FieldType::Real | FieldType::Int) && f.unit.is_none() {
                return Err(SchemaError::MissingUnit(f.name.clone()));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Checks one record against this schema.
    pub fn validate_record(&self, rec: &Record) -> Result<(), SchemaError> {
        for (name, value) in &rec.fields {
            let Some(def) = self.field(name) else {
                return Err(SchemaError::UnknownField(name.clone(), self.app_id.clone()));
            };
            if !def.ty.accepts(value) {
                return Err(SchemaError::TypeMismatch {
                    field: name.clone(),
                    expected: def.ty,
                    actual: value.type_name(),
                });
            }
        }
        for def in self.fields.iter().filter(|f| f.required) {
            if !rec.fields.contains_key(&def.name) && def.default.is_none() {
                return Err(SchemaError::MissingRequired(def.name.clone()));
            }
        }
        Ok(())
    }

    /// Builds a record pre-filled with declared defaults.
    #[must_use]
    pub fn new_record(&self) -> Record {
        let mut rec = Record::default();
        for f in &self.fields {
            if let Some(d) = &f.default {
                rec.fields.insert(f.name.clone(), d.clone());
            }
        }
        rec
    }
}

/// One application's data on one object. Insertion-ordered so that saving a
/// document twice produces byte-identical output.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Record {
    pub fields: IndexMap<String, Value>,
}

impl Record {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }

    pub fn set(&mut self, name: impl Into<String>, v: Value) -> Option<Value> {
        self.fields.insert(name.into(), v)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// All extension data on one object, keyed by owning application.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct XDataMap {
    pub records: IndexMap<AppId, Record>,
}

impl XDataMap {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[must_use]
    pub fn get(&self, app: &AppId) -> Option<&Record> {
        self.records.get(app)
    }

    pub fn get_mut(&mut self, app: &AppId) -> Option<&mut Record> {
        self.records.get_mut(app)
    }

    pub fn entry(&mut self, app: AppId) -> &mut Record {
        self.records.entry(app).or_default()
    }

    pub fn set(&mut self, app: AppId, rec: Record) -> Option<Record> {
        self.records.insert(app, rec)
    }

    pub fn remove(&mut self, app: &AppId) -> Option<Record> {
        self.records.shift_remove(app)
    }

    /// Reads a field without caring which application owns it — the shortcut
    /// exporters and inspectors want.
    #[must_use]
    pub fn field(&self, app: &AppId, name: &str) -> Option<&Value> {
        self.get(app)?.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schema shaped like one a domain plugin would register. Deliberately
    /// not named after a discipline: the core has no business knowing one.
    fn example_schema() -> XDataSchema {
        XDataSchema::new(
            AppId::new("org.example.plugin"),
            "0.1.0",
            vec![
                FieldDef::new(
                    "airflow",
                    FieldType::Real,
                    "Pset_DuctSegmentTypeCommon.NominalAirflow",
                )
                .with_unit("m3/h")
                .required()
                .described("Design airflow"),
                FieldDef::new("system", FieldType::Text, "Pset_DistributionSystem.Name"),
                FieldDef::new(
                    "insulated",
                    FieldType::Bool,
                    "Pset_DuctSegmentOccurrence.Insulated",
                )
                .with_default(Value::Bool(false)),
            ],
        )
    }

    #[test]
    fn a_schema_without_ifc_mapping_is_rejected() {
        let bad = XDataSchema::new(
            AppId::new("x"),
            "1",
            vec![FieldDef::new("q", FieldType::Text, "")],
        );
        assert_eq!(
            bad.validate(),
            Err(SchemaError::MissingIfcMapping("q".into()))
        );
    }

    #[test]
    fn a_numeric_field_without_a_unit_is_rejected() {
        let bad = XDataSchema::new(
            AppId::new("x"),
            "1",
            vec![FieldDef::new("q", FieldType::Real, "Pset.Q")],
        );
        assert_eq!(bad.validate(), Err(SchemaError::MissingUnit("q".into())));
    }

    #[test]
    fn a_good_schema_validates() {
        assert_eq!(example_schema().validate(), Ok(()));
    }

    #[test]
    fn records_are_checked_against_their_schema() {
        let s = example_schema();
        let mut rec = s.new_record();
        // The default came through.
        assert_eq!(rec.get("insulated"), Some(&Value::Bool(false)));
        // Required field still missing.
        assert_eq!(
            s.validate_record(&rec),
            Err(SchemaError::MissingRequired("airflow".into()))
        );

        rec.set("airflow", Value::Real(1200.0));
        assert_eq!(s.validate_record(&rec), Ok(()));

        rec.set("airflow", Value::Text("lots".into()));
        assert!(matches!(
            s.validate_record(&rec),
            Err(SchemaError::TypeMismatch { .. })
        ));

        rec.set("airflow", Value::Real(1200.0));
        rec.set("undeclared", Value::Int(1));
        assert!(matches!(
            s.validate_record(&rec),
            Err(SchemaError::UnknownField(..))
        ));
    }

    #[test]
    fn an_int_satisfies_a_real_field() {
        let s = example_schema();
        let mut rec = s.new_record();
        rec.set("airflow", Value::Int(1200));
        assert_eq!(s.validate_record(&rec), Ok(()));
        assert_eq!(rec.get("airflow").and_then(Value::as_real), Some(1200.0));
    }

    #[test]
    fn xdata_keeps_applications_separate() {
        let mut x = XDataMap::default();
        x.entry(AppId::new("a")).set("k", Value::Int(1));
        x.entry(AppId::new("b")).set("k", Value::Int(2));
        assert_eq!(x.field(&AppId::new("a"), "k"), Some(&Value::Int(1)));
        assert_eq!(x.field(&AppId::new("b"), "k"), Some(&Value::Int(2)));
        assert_eq!(x.field(&AppId::new("c"), "k"), None);
    }
}
