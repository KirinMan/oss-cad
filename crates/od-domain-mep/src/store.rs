//! Reading and writing MEP objects in a [`Database`].
//!
//! A [`RouteSegment`] or [`PlacedPart`] is stored as
//! [`ObjectKind::Custom`](od_core::ObjectKind::Custom) — `{type_id, data}`,
//! where `data` is that struct serialised to JSON. This module is the only
//! place that knows the type ids, so a typo in one is a compile error waiting
//! to happen in exactly one file instead of scattered through the crate.

use crate::model::{MepError, PlacedPart, Result, RouteSegment};
use od_core::{Database, ObjectId, ObjectKind, Transaction};

pub const ROUTE_TYPE: &str = "org.opendraft.mep.route";
pub const PART_TYPE: &str = "org.opendraft.mep.part";

/// Adds a route segment to the document.
pub fn add_route(tx: &mut Transaction<'_>, route: &RouteSegment) -> Result<ObjectId> {
    let data = serde_json::to_value(route)?;
    Ok(tx.add_custom(None, ROUTE_TYPE, data)?)
}

/// Adds a placed part (equipment or fitting) to the document.
pub fn add_part(tx: &mut Transaction<'_>, part: &PlacedPart) -> Result<ObjectId> {
    let data = serde_json::to_value(part)?;
    Ok(tx.add_custom(None, PART_TYPE, data)?)
}

/// Reads one route segment.
pub fn read_route(db: &Database, id: ObjectId) -> Result<RouteSegment> {
    let obj = db.object(id).ok_or(MepError::NotFound(id))?;
    match &obj.kind {
        ObjectKind::Custom { type_id, data } if type_id == ROUTE_TYPE => {
            serde_json::from_value(data.clone()).map_err(|e| MepError::Corrupt(id, e))
        }
        _ => Err(MepError::WrongType(id, "route")),
    }
}

/// Reads one placed part.
pub fn read_part(db: &Database, id: ObjectId) -> Result<PlacedPart> {
    let obj = db.object(id).ok_or(MepError::NotFound(id))?;
    match &obj.kind {
        ObjectKind::Custom { type_id, data } if type_id == PART_TYPE => {
            serde_json::from_value(data.clone()).map_err(|e| MepError::Corrupt(id, e))
        }
        _ => Err(MepError::WrongType(id, "part")),
    }
}

/// Every route segment in the document, in no particular order — a document
/// has no inherent MEP draw order the way a block record has an entity order.
pub fn all_routes(db: &Database) -> Vec<(ObjectId, RouteSegment)> {
    db.objects()
        .filter_map(|(id, obj)| match &obj.kind {
            ObjectKind::Custom { type_id, data } if type_id == ROUTE_TYPE => {
                serde_json::from_value(data.clone()).ok().map(|r| (id, r))
            }
            _ => None,
        })
        .collect()
}

/// Every placed part in the document.
pub fn all_parts(db: &Database) -> Vec<(ObjectId, PlacedPart)> {
    db.objects()
        .filter_map(|(id, obj)| match &obj.kind {
            ObjectKind::Custom { type_id, data } if type_id == PART_TYPE => {
                serde_json::from_value(data.clone()).ok().map(|p| (id, p))
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PlacementKind;
    use od_core::{ActorId, Document, Point3};
    use od_parts::Profile;

    fn doc() -> Document {
        Document::new(Database::new(ActorId::SYSTEM))
    }

    fn route() -> RouteSegment {
        RouteSegment {
            system: "sys.air.supply".into(),
            spec: "spec.duct.galvanised.rect".into(),
            profile: Profile::Rect { w: 400.0, h: 300.0 },
            start: Point3::ORIGIN,
            end: Point3::new(5000.0, 0.0, 0.0),
        }
    }

    #[test]
    fn a_route_round_trips_through_the_document() {
        let mut d = doc();
        let r = route();
        let id = d
            .edit("Add route", |tx| add_route(tx, &r))
            .expect("commits");

        let back = read_route(&d.db, id).expect("reads");
        assert_eq!(back, r);
        assert_eq!(all_routes(&d.db), vec![(id, r)]);
        assert!(d.db.validate().is_empty());
    }

    #[test]
    fn a_part_round_trips_through_the_document() {
        let mut d = doc();
        let p = PlacedPart {
            part_id: "hvac.ahu".into(),
            position: Point3::new(1000.0, 2000.0, 3000.0),
            rotation: std::f64::consts::FRAC_PI_2,
            mirror_y: false,
            params: std::collections::HashMap::new(),
            kind: PlacementKind::Equipment,
            system: None,
        };
        let id = d.edit("Add part", |tx| add_part(tx, &p)).expect("commits");
        assert_eq!(read_part(&d.db, id).expect("reads"), p);
        assert_eq!(all_parts(&d.db).len(), 1);
    }

    #[test]
    fn reading_the_wrong_type_is_an_error_not_a_panic() {
        let mut d = doc();
        let id = d
            .edit("Add route", |tx| add_route(tx, &route()))
            .expect("commits");
        assert!(matches!(
            read_part(&d.db, id),
            Err(MepError::WrongType(_, "part"))
        ));
    }

    #[test]
    fn objects_survive_a_json_round_trip_of_the_whole_document() {
        let mut d = doc();
        let id = d
            .edit("Add route", |tx| add_route(tx, &route()))
            .expect("commits");
        let text = serde_json::to_string(&d.db).expect("serialises");
        let back: Database = serde_json::from_str(&text).expect("deserialises");
        assert_eq!(read_route(&back, id).expect("reads"), route());
    }
}
