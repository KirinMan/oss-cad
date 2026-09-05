//! Errors the document model can produce.

use crate::id::ObjectId;
use crate::xdata::{AppId, SchemaError};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DbError {
    #[error("no object {0}")]
    NoSuchObject(ObjectId),

    #[error("object {from} refers to {to}, which is not a valid {what}")]
    DanglingReference {
        from: ObjectId,
        to: ObjectId,
        what: &'static str,
    },

    #[error("object {0} is not an entity")]
    NotAnEntity(ObjectId),

    #[error(transparent)]
    Schema(#[from] SchemaError),

    #[error("no schema registered for `{0}`, so its data cannot be checked")]
    UnknownSchema(AppId),

    #[error("transaction `{name}` failed validation and was rolled back: {problems:?}")]
    ValidationFailed {
        name: String,
        problems: Vec<DbError>,
    },

    #[error("entity {id} is a {geometry}, which this edit does not yet support")]
    UnsupportedEdit { id: ObjectId, geometry: String },

    /// A [`crate::Command`]'s own arguments fail a check the type system
    /// cannot express — e.g. a polyline whose points do not share one
    /// elevation, which `Geometry::Polyline` requires because it is planar.
    #[error("invalid command: {0}")]
    InvalidCommand(String),
}

pub type Result<T> = std::result::Result<T, DbError>;
