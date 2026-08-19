//! `od mep` — building services (MEP) commands.
//!
//! `demo` proves the domain end to end without needing hand-drawn input: it
//! places a fan, routes a duct off its outlet through an automatic 90° elbow,
//! and saves a real drawing that `od render`/`od inspect`/`od roundtrip` can
//! all be pointed at. `takeoff` and `check` work on any drawing built the same
//! way — by hand later, once there is a canvas, or by another tool today.

use anyhow::{Context, Result};
use od_core::{ActorId, Database, Document, Point3};
use od_domain_mep::model::{PlacedPart, PlacementKind};
use od_domain_mep::route::{RouteSpec, draw_route};
use od_domain_mep::{derive, graph::ConnectionGraph, ports, store, takeoff};
use od_geom3d::Vec3;
use od_parts::Catalog;
use std::collections::HashMap;
use std::path::Path;

pub fn demo(output: &Path, json: bool) -> Result<()> {
    let catalog = Catalog::bundled().context("loading the bundled part catalogue")?;
    let mut doc = Document::new(Database::new(ActorId::SYSTEM));

    let (fittings, segments) = doc.edit::<_, _, anyhow::Error>("MEP demo", |tx| {
        let fan = PlacedPart {
            part_id: "hvac.fan.sirocco".into(),
            position: Point3::new(0.0, 0.0, 2800.0),
            rotation: 0.0,
            mirror_y: false,
            params: HashMap::new(),
            kind: PlacementKind::Equipment,
            system: Some("sys.air.supply".into()),
        };
        let fan_id = store::add_part(tx, &fan)?;
        derive::insert_part(tx, &catalog, &fan)?;

        let (_, fan_ports) = ports::place(&catalog, fan_id, &fan)?;
        let outlet = fan_ports
            .iter()
            .find(|p| p.name == "out")
            .ok_or_else(|| anyhow::anyhow!("the bundled fan has no port named `out`"))?
            .clone();

        // A short run off the fan, then a left turn — enough to exercise
        // automatic elbow insertion without inventing a whole building.
        let turn = Vec3::new(-outlet.direction.y, outlet.direction.x, 0.0);
        let path = vec![
            outlet.position,
            outlet.position + outlet.direction * 4000.0,
            outlet.position + outlet.direction * 4000.0 + turn * 3000.0,
        ];
        let spec = RouteSpec {
            system: "sys.air.supply".into(),
            spec: "spec.duct.galvanised.rect".into(),
            profile: outlet.profile,
            path,
        };
        let result = draw_route(tx, &catalog, &spec)?;

        for &id in &result.segments {
            let seg = store::read_route(tx.db(), id)?;
            derive::insert_route(tx, &catalog, &seg, derive::ViewMode::DoubleLine)?;
        }
        for &id in &result.fittings {
            let part = store::read_part(tx.db(), id)?;
            derive::insert_part(tx, &catalog, &part)?;
        }

        Ok((result.fittings.len(), result.segments.len()))
    })?;

    crate::load::save(&doc.db, output)?;
    crate::report::mep_demo(&doc.db, output, fittings, segments, json);
    Ok(())
}

pub fn takeoff(input: &Path, json: bool) -> Result<()> {
    let (db, _) = crate::load::load(input)?;
    let result = takeoff::take_off(&db);
    crate::report::mep_takeoff(&result, input, json);
    Ok(())
}

pub fn check(input: &Path, json: bool) -> Result<()> {
    let catalog = Catalog::bundled().context("loading the bundled part catalogue")?;
    let (db, _) = crate::load::load(input)?;
    let graph = ConnectionGraph::build(&db, &catalog);
    let unconnected = graph.unconnected().len();
    let skipped = graph.skipped().len();
    crate::report::mep_check(&graph, input, json);
    if unconnected > 0 || skipped > 0 {
        // A dangling port or an unresolvable part is exactly the kind of
        // mistake this check exists to catch before it reaches site (F-108).
        std::process::exit(1);
    }
    Ok(())
}
