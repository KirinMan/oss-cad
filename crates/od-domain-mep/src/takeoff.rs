//! Quantity take-off (F-111).
//!
//! Walking the source objects is enough — no rendering, no graph traversal.
//! Length is the sum of `RouteSegment::length`, grouped by system and spec;
//! fitting counts are grouped by part id.
//!
//! Stock-length-aware piece and joint counts (docs/04-mep.md §4.7's
//! "直管長（定尺考慮）") use [`od_parts::Spec::stock_length`]: each
//! `RouteSegment` is one physical run that has to be cut from whole stock
//! lengths, so a 6500mm run against 5500mm stock needs two pieces and the
//! one joint between them — never a plain `length / stock_length` averaged
//! across every run in the group, which would hide exactly the joins a
//! fabricator needs to plan for. A spec with no stock length (an
//! `od_parts::Profile` fabricated to length, not cut from stock — none in
//! the bundled catalogue today, but nothing requires one) reports `None`
//! rather than a nonsensical zero.
//!
//! Deliberately still not here: support/sleeve counts and insulation/paint
//! area (docs/04-mep.md §4.7's remaining columns) — both need data this
//! crate's model does not carry yet (support spacing rules, insulation
//! thickness), so they are left for whenever that data exists to compute
//! them against, rather than guessed at.

use crate::store::{all_parts, all_routes};
use od_core::Database;
use od_parts::Catalog;
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteTotal {
    pub length_mm: f64,
    pub count: usize,
    /// Stock lengths needed to cut every run in this group, each run costed
    /// independently (module docs). `None` when the spec has no
    /// [`od_parts::Spec::stock_length`], or could not be found in the
    /// catalogue.
    pub stock_pieces: Option<usize>,
}

impl RouteTotal {
    /// Joints from butting stock lengths together within a run — every
    /// piece after a run's first needs one. Exactly `stock_pieces - count`,
    /// since each of the group's `count` runs contributes zero joints for
    /// its own first piece.
    #[must_use]
    pub fn joints(&self) -> Option<usize> {
        self.stock_pieces.map(|p| p.saturating_sub(self.count))
    }
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
pub fn take_off(db: &Database, catalog: &Catalog) -> Takeoff {
    let mut out = Takeoff::default();

    for (_, route) in all_routes(db) {
        let stock_length = catalog.spec(&route.spec).map(|s| s.stock_length);
        let entry = out
            .routes
            .entry((route.system.clone(), route.spec.clone()))
            .or_default();
        entry.length_mm += route.length();
        entry.count += 1;
        if let Some(len) = stock_length
            && len > 0.0
        {
            let pieces = (route.length() / len).ceil().max(1.0);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a run's piece count stays tiny (single digits to low hundreds) next to usize::MAX"
            )]
            let pieces = pieces as usize;
            *entry.stock_pieces.get_or_insert(0) += pieces;
        }
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

/// One importable table: every route group, then every fitting, then every
/// equipment count, distinguished by a `kind` column rather than split into
/// three files — `docs/04-mep.md` §4.7 asks for CSV/XLSX output, and a
/// single sheet is what a spreadsheet import expects. Rows are sorted by
/// their key so the output is stable across calls, which matters for both
/// the tests here and for diffing two take-offs of the same drawing.
#[must_use]
pub fn to_csv(t: &Takeoff) -> String {
    let mut out = String::from("kind,system,spec,part_id,length_mm,count,stock_pieces,joints\n");

    let mut routes: Vec<(&(String, String), &RouteTotal)> = t.routes.iter().collect();
    routes.sort_by(|a, b| a.0.cmp(b.0));
    for ((system, spec), total) in routes {
        write_csv_row(
            &mut out,
            [
                "route",
                &csv_field(system),
                &csv_field(spec),
                "",
                &total.length_mm.to_string(),
                &total.count.to_string(),
                &opt_to_string(total.stock_pieces),
                &opt_to_string(total.joints()),
            ],
        );
    }

    let mut fittings: Vec<(&String, &usize)> = t.fittings.iter().collect();
    fittings.sort_by_key(|(id, _)| id.as_str());
    for (part_id, count) in fittings {
        write_csv_row(
            &mut out,
            [
                "fitting",
                "",
                "",
                &csv_field(part_id),
                "",
                &count.to_string(),
                "",
                "",
            ],
        );
    }

    let mut equipment: Vec<(&String, &usize)> = t.equipment.iter().collect();
    equipment.sort_by_key(|(id, _)| id.as_str());
    for (part_id, count) in equipment {
        write_csv_row(
            &mut out,
            [
                "equipment",
                "",
                "",
                &csv_field(part_id),
                "",
                &count.to_string(),
                "",
                "",
            ],
        );
    }

    out
}

fn write_csv_row(out: &mut String, fields: [&str; 8]) {
    let _ = writeln!(out, "{}", fields.join(","));
}

fn opt_to_string(v: Option<usize>) -> String {
    v.map_or_else(String::new, |n| n.to_string())
}

/// RFC 4180 quoting: a field containing a comma, a quote or a newline is
/// wrapped in quotes, with any quote inside doubled. Every field this module
/// passes through here is a catalogue id today (no commas or quotes in
/// practice), but an id is user-authorable data in principle
/// (`parts/README.md` does not forbid it), so this does not assume that
/// stays true.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PlacedPart, PlacementKind, RouteSegment};
    use crate::store::{add_part, add_route};
    use od_core::{ActorId, Document, Point3};
    use od_parts::Profile;
    use std::collections::HashMap as Map;

    fn catalog() -> Catalog {
        Catalog::bundled().expect("bundled catalogue is valid")
    }

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

        let t = take_off(&d.db, &catalog());
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

        let t = take_off(&d.db, &catalog());
        assert_eq!(t.equipment.get("hvac.fan.sirocco"), Some(&1));
        assert_eq!(t.fittings.get("duct.elbow.rect.90"), Some(&1));
        assert!(!t.fittings.contains_key("hvac.fan.sirocco"));
    }

    #[test]
    fn an_empty_document_takes_off_to_nothing() {
        let d = Document::new(Database::new(ActorId::SYSTEM));
        let t = take_off(&d.db, &catalog());
        assert!(t.routes.is_empty());
        assert!(od_core::tol::eq_len(t.total_length_mm(), 0.0));
    }

    #[test]
    fn a_run_longer_than_stock_needs_extra_pieces_and_a_joint() {
        // spec.pipe.sgp's stock length is 5500mm (parts/specs/pipe-sgp.json).
        let mut d = Document::new(Database::new(ActorId::SYSTEM));
        let run = RouteSegment {
            system: "sys.water.cold".into(),
            spec: "spec.pipe.sgp".into(),
            profile: Profile::Round { d: 50.0 },
            start: Point3::ORIGIN,
            end: Point3::new(6500.0, 0.0, 0.0),
        };
        d.edit("Route", |tx| add_route(tx, &run)).expect("commits");

        let t = take_off(&d.db, &catalog());
        let total = &t.routes[&("sys.water.cold".to_owned(), "spec.pipe.sgp".to_owned())];
        assert_eq!(
            total.stock_pieces,
            Some(2),
            "6500mm needs two 5500mm pieces"
        );
        assert_eq!(total.joints(), Some(1));
    }

    #[test]
    fn two_short_runs_need_no_joints_between_them() {
        // Two 2000mm runs, each well under 5500mm stock: one piece each,
        // zero joints -- summing lengths first would wrongly imply a joint
        // once the *combined* 4000mm still fits one piece, but these are two
        // physically separate cuts either way.
        let mut d = Document::new(Database::new(ActorId::SYSTEM));
        let a = RouteSegment {
            system: "sys.water.cold".into(),
            spec: "spec.pipe.sgp".into(),
            profile: Profile::Round { d: 50.0 },
            start: Point3::ORIGIN,
            end: Point3::new(2000.0, 0.0, 0.0),
        };
        let b = RouteSegment {
            start: Point3::new(0.0, 1000.0, 0.0),
            end: Point3::new(2000.0, 1000.0, 0.0),
            ..a.clone()
        };
        d.edit("Route", |tx| {
            add_route(tx, &a)?;
            add_route(tx, &b)
        })
        .expect("commits");

        let t = take_off(&d.db, &catalog());
        let total = &t.routes[&("sys.water.cold".to_owned(), "spec.pipe.sgp".to_owned())];
        assert_eq!(total.stock_pieces, Some(2), "one piece per run");
        assert_eq!(total.joints(), Some(0));
    }

    #[test]
    fn an_unknown_spec_reports_no_stock_piece_count() {
        let mut d = Document::new(Database::new(ActorId::SYSTEM));
        let run = RouteSegment {
            system: "sys.water.cold".into(),
            spec: "spec.no.such.thing".into(),
            profile: Profile::Round { d: 50.0 },
            start: Point3::ORIGIN,
            end: Point3::new(2000.0, 0.0, 0.0),
        };
        d.edit("Route", |tx| add_route(tx, &run)).expect("commits");

        let t = take_off(&d.db, &catalog());
        let total = &t.routes[&("sys.water.cold".to_owned(), "spec.no.such.thing".to_owned())];
        assert_eq!(total.stock_pieces, None);
        assert_eq!(total.joints(), None);
    }

    #[test]
    fn csv_export_covers_routes_fittings_and_equipment() {
        let mut d = Document::new(Database::new(ActorId::SYSTEM));
        let run = RouteSegment {
            system: "sys.water.cold".into(),
            spec: "spec.pipe.sgp".into(),
            profile: Profile::Round { d: 50.0 },
            start: Point3::ORIGIN,
            end: Point3::new(6500.0, 0.0, 0.0),
        };
        let equip = PlacedPart {
            part_id: "hvac.fan.sirocco".into(),
            position: Point3::ORIGIN,
            rotation: 0.0,
            mirror_y: false,
            params: Map::new(),
            kind: PlacementKind::Equipment,
            system: None,
        };
        d.edit("Build", |tx| {
            add_route(tx, &run)?;
            add_part(tx, &equip)
        })
        .expect("commits");

        let csv = to_csv(&take_off(&d.db, &catalog()));
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "kind,system,spec,part_id,length_mm,count,stock_pieces,joints"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("route,sys.water.cold,spec.pipe.sgp,,6500,1,2,1"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("equipment,,,hvac.fan.sirocco,,1,,"))
        );
    }

    #[test]
    fn csv_quotes_a_field_containing_a_comma() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
    }
}
