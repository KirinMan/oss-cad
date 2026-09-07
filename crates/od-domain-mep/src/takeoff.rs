//! Quantity take-off (F-111).
//!
//! Walking the source objects is enough — no rendering, no graph traversal.
//! Length is the sum of `RouteSegment::length`, grouped by system and spec;
//! fitting counts are grouped by part id. This is deliberately the simplest
//! version of F-111: a real take-off also wants stock-length-aware joint
//! counts and support/sleeve counts, which need [`od_parts::Spec::stock_length`]
//! and the support/penetration categories respectively — both straightforward
//! additions once there is a real drawing to check the numbers against.

use crate::store::{all_parts, all_routes};
use od_core::Database;
use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteTotal {
    pub length_mm: f64,
    pub count: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Takeoff {
    /// Keyed by (system id, spec id).
    pub routes: HashMap<(String, String), RouteTotal>,
    /// Keyed by part id.
    pub fittings: HashMap<String, usize>,
    pub equipment: HashMap<String, usize>,
}

impl Takeoff {
    #[must_use]
    pub fn total_length_mm(&self) -> f64 {
        self.routes.values().map(|t| t.length_mm).sum()
    }
}

#[must_use]
pub fn take_off(db: &Database) -> Takeoff {
    let mut out = Takeoff::default();

    for (_, route) in all_routes(db) {
        let entry = out
            .routes
            .entry((route.system.clone(), route.spec.clone()))
            .or_default();
        entry.length_mm += route.length();
        entry.count += 1;
    }

    for (_, part) in all_parts(db) {
        let bucket = match part.kind {
            crate::model::PlacementKind::Equipment => &mut out.equipment,
            crate::model::PlacementKind::Fitting { .. } => &mut out.fittings,
        };
        *bucket.entry(part.part_id).or_insert(0) += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PlacedPart, PlacementKind, RouteSegment};
    use crate::store::{add_part, add_route};
    use od_core::{ActorId, Document, Point3};
    use od_parts::Profile;
    use std::collections::HashMap as Map;

    #[test]
    fn lengths_are_summed_per_system_and_spec() {
        let mut d = Document::new(Database::new(ActorId::SYSTEM));
        let a = RouteSegment {
            system: "sys.air.supply".into(),
            spec: "spec.duct.galvanised.rect".into(),
            profile: Profile::Rect { w: 400.0, h: 300.0 },
            start: Point3::ORIGIN,
            end: Point3::new(4000.0, 0.0, 0.0),
        };
        let b = RouteSegment {
            start: Point3::new(4000.0, 0.0, 0.0),
            end: Point3::new(6500.0, 0.0, 0.0),
            ..a.clone()
        };
        d.edit("Route", |tx| {
            add_route(tx, &a)?;
            add_route(tx, &b)
        })
        .expect("commits");

        let t = take_off(&d.db);
        let total = &t.routes[&(
            "sys.air.supply".to_owned(),
            "spec.duct.galvanised.rect".to_owned(),
        )];
        assert!(od_core::tol::eq_len(total.length_mm, 6500.0));
        assert_eq!(total.count, 2);
        assert!(od_core::tol::eq_len(t.total_length_mm(), 6500.0));
    }

    #[test]
    fn fittings_and_equipment_are_counted_separately() {
        let mut d = Document::new(Database::new(ActorId::SYSTEM));
        let equip = PlacedPart {
            part_id: "hvac.fan.sirocco".into(),
            position: Point3::ORIGIN,
            rotation: 0.0,
            mirror_y: false,
            params: Map::new(),
            kind: PlacementKind::Equipment,
            system: None,
        };
        let elbow = PlacedPart {
            part_id: "duct.elbow.rect.90".into(),
            position: Point3::new(1000.0, 0.0, 0.0),
            rotation: 0.0,
            mirror_y: false,
            params: Map::new(),
            kind: PlacementKind::Fitting {
                auto_generated: true,
            },
            system: None,
        };
        d.edit("Place", |tx| {
            add_part(tx, &equip)?;
            add_part(tx, &elbow)
        })
        .expect("commits");

        let t = take_off(&d.db);
        assert_eq!(t.equipment.get("hvac.fan.sirocco"), Some(&1));
        assert_eq!(t.fittings.get("duct.elbow.rect.90"), Some(&1));
        assert!(!t.fittings.contains_key("hvac.fan.sirocco"));
    }

    #[test]
    fn an_empty_document_takes_off_to_nothing() {
        let d = Document::new(Database::new(ActorId::SYSTEM));
        let t = take_off(&d.db);
        assert!(t.routes.is_empty());
        assert!(od_core::tol::eq_len(t.total_length_mm(), 0.0));
    }
}
