//! Output, in a form for a person and a form for a machine.
//!
//! Both come from the same data. A report that a person reads and a report that
//! CI parses drifting apart is how "it passed locally" happens.

use crate::check::{Finding, Severity};
use crate::load::LoadOutcome;
use od_core::Database;
use od_domain_mep::graph::ConnectionGraph;
use od_domain_mep::takeoff::Takeoff;
use od_parts::{Catalog, Instance, Part};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

fn emit<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(e) => eprintln!("failed to render JSON: {e}"),
    }
}

#[derive(Serialize)]
struct Summary<'a> {
    file: String,
    format: &'static str,
    entities: usize,
    layers: usize,
    blocks: usize,
    preserved_entities: usize,
    preserved_sections: usize,
    unsupported_types: &'a [String],
    unsupported_features: &'a [String],
    warnings: usize,
    extents_mm: [f64; 6],
}

fn summarise<'a>(db: &Database, outcome: &'a LoadOutcome, path: &Path) -> Summary<'a> {
    let s = db.stats();
    let b = s.bounds;
    let extents = if b.is_empty() {
        [0.0; 6]
    } else {
        [b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z]
    };
    Summary {
        file: path.display().to_string(),
        format: outcome.format,
        entities: s.entities,
        layers: s.layers,
        blocks: s.blocks,
        preserved_entities: s.unsupported_entities,
        preserved_sections: s.preserved_blobs,
        unsupported_types: &outcome.unsupported_entities,
        unsupported_features: &outcome.unsupported_features,
        warnings: outcome.warnings.len(),
        extents_mm: extents,
    }
}

pub fn inspection(db: &Database, outcome: &LoadOutcome, path: &Path, json: bool) {
    if json {
        #[derive(Serialize)]
        struct Detailed<'a> {
            #[serde(flatten)]
            summary: Summary<'a>,
            entities_by_type: indexmap::IndexMap<String, usize>,
            layer_names: Vec<String>,
        }
        emit(&Detailed {
            summary: summarise(db, outcome, path),
            entities_by_type: db.stats().entities_by_type,
            layer_names: db
                .tables
                .layers
                .iter()
                .map(|(_, l)| l.name.clone())
                .collect(),
        });
        return;
    }

    let s = db.stats();
    println!("{}", path.display());
    println!("  entities        {}", s.entities);
    println!("  layers          {}", s.layers);
    println!("  blocks          {}", s.blocks);
    if !s.bounds.is_empty() {
        let b = s.bounds;
        println!(
            "  extents         {:.1} × {:.1} mm  ({:.1}, {:.1}) to ({:.1}, {:.1})",
            b.size().x,
            b.size().y,
            b.min.x,
            b.min.y,
            b.max.x,
            b.max.y
        );
    }

    if !s.entities_by_type.is_empty() {
        println!("\n  by type");
        for (name, count) in &s.entities_by_type {
            println!("    {name:<14} {count}");
        }
    }

    if !outcome.unsupported_entities.is_empty() {
        println!(
            "\n  preserved verbatim ({} entities): {}",
            s.unsupported_entities,
            outcome.unsupported_entities.join(", ")
        );
    }
    if !outcome.unsupported_features.is_empty() {
        println!(
            "  preserved from a newer build: {}",
            outcome.unsupported_features.join(", ")
        );
    }
    if s.preserved_blobs > 0 {
        println!("  preserved sections: {}", s.preserved_blobs);
    }
    if !outcome.warnings.is_empty() {
        println!("\n  warnings ({})", outcome.warnings.len());
        for w in outcome.warnings.iter().take(10) {
            println!("    {}: {}", w.context, w.message);
        }
        if outcome.warnings.len() > 10 {
            println!("    … and {} more", outcome.warnings.len() - 10);
        }
    }
}

pub fn conversion(
    db: &Database,
    outcome: &LoadOutcome,
    input: &Path,
    output: &Path,
    losses: &[String],
    json: bool,
) {
    if json {
        #[derive(Serialize)]
        struct Result<'a> {
            #[serde(flatten)]
            summary: Summary<'a>,
            output: String,
            /// What the target format cannot carry. Empty for `.odc`.
            losses: &'a [String],
        }
        emit(&Result {
            summary: summarise(db, outcome, input),
            output: output.display().to_string(),
            losses,
        });
        return;
    }

    let s = db.stats();
    println!(
        "{} → {}  ({} entities, {} layers)",
        input.display(),
        output.display(),
        s.entities,
        s.layers
    );
    if s.unsupported_entities > 0 {
        println!(
            "  {} entity(ies) preserved verbatim: {}",
            s.unsupported_entities,
            outcome.unsupported_entities.join(", ")
        );
    }
    if !losses.is_empty() {
        println!("\n  this format cannot carry:");
        for loss in losses {
            println!("    {loss}");
        }
        println!("    (save as .odc to keep them)");
    }
    if !outcome.warnings.is_empty() {
        println!(
            "  {} warning(s); run `od inspect` for detail",
            outcome.warnings.len()
        );
    }
}

pub fn findings(findings: &[Finding], path: &Path, json: bool) {
    if json {
        #[derive(Serialize)]
        struct Report<'a> {
            file: String,
            passed: bool,
            findings: &'a [Finding],
        }
        emit(&Report {
            file: path.display().to_string(),
            passed: !findings.iter().any(|f| f.severity == Severity::Error),
            findings,
        });
        return;
    }

    if findings.is_empty() {
        println!("{}: no findings", path.display());
        return;
    }
    println!("{}", path.display());
    for f in findings {
        let tag = match f.severity {
            Severity::Error => "error",
            Severity::Warning => "warn ",
            Severity::Info => "info ",
        };
        println!("  {tag}  [{}] {}", f.rule, f.message);
    }
}

/// Compares two reads of the same drawing. Returns true when they differ.
pub fn roundtrip(
    first: &Database,
    second: &Database,
    warnings_on_reread: usize,
    path: &Path,
    format: &str,
    json: bool,
) -> bool {
    let (a, b) = (first.stats(), second.stats());
    let entity_delta = b.entities as i64 - a.entities as i64;
    let layer_delta = b.layers as i64 - a.layers as i64;

    // Extents are the cheapest whole-drawing check that catches a geometry
    // change a count would miss.
    let extent_shift = if a.bounds.is_empty() || b.bounds.is_empty() {
        0.0
    } else {
        (b.bounds.min.x - a.bounds.min.x)
            .abs()
            .max((b.bounds.max.x - a.bounds.max.x).abs())
            .max((b.bounds.min.y - a.bounds.min.y).abs())
            .max((b.bounds.max.y - a.bounds.max.y).abs())
    };
    let differs = entity_delta != 0 || layer_delta != 0 || extent_shift > od_core::tol::POINT_EPS;

    if json {
        #[derive(Serialize)]
        struct Report<'a> {
            file: String,
            format: &'a str,
            identical: bool,
            entities_before: usize,
            entities_after: usize,
            layers_before: usize,
            layers_after: usize,
            max_extent_shift_mm: f64,
            warnings_on_reread: usize,
        }
        emit(&Report {
            file: path.display().to_string(),
            format,
            identical: !differs,
            entities_before: a.entities,
            entities_after: b.entities,
            layers_before: a.layers,
            layers_after: b.layers,
            max_extent_shift_mm: extent_shift,
            warnings_on_reread,
        });
        return differs;
    }

    println!("{} (via {format})", path.display());
    println!("  entities  {} → {}", a.entities, b.entities);
    println!("  layers    {} → {}", a.layers, b.layers);
    println!("  extents   shifted by {extent_shift:.9} mm");
    if differs {
        println!("\n  DIFFERS — the drawing did not survive the round trip intact");
    } else {
        println!("\n  identical");
    }
    differs
}

pub fn part_list(catalog: &Catalog, query: &str, json: bool) {
    let matches: Vec<&Part> = catalog.search(query).collect();
    if json {
        #[derive(Serialize)]
        struct Row<'a> {
            id: &'a str,
            name_ja: &'a str,
            name_en: &'a str,
            category: &'a str,
            ifc_class: &'a str,
            parameters: Vec<&'a str>,
            ports: usize,
        }
        let rows: Vec<Row> = matches
            .iter()
            .map(|p| Row {
                id: &p.id,
                name_ja: &p.name.ja,
                name_en: &p.name.en,
                category: p.category.as_str(),
                ifc_class: &p.ifc_class,
                parameters: p.parameters.iter().map(|x| x.name.as_str()).collect(),
                ports: p.ports.len(),
            })
            .collect();
        emit(&rows);
        return;
    }

    if matches.is_empty() {
        println!("no parts match `{query}`");
        return;
    }
    // Grouped by category rather than shown in catalogue order: the file a part
    // happens to live in is not something a user browsing the library cares
    // about, and interleaved categories print the same heading twice.
    let mut by_category: indexmap::IndexMap<&str, Vec<&Part>> = indexmap::IndexMap::new();
    for p in &matches {
        by_category.entry(p.category.as_str()).or_default().push(p);
    }
    by_category.sort_keys();
    for (category, parts) in &by_category {
        println!("\n{category}");
        for p in parts {
            println!("  {:<32} {}", p.id, p.name.ja);
        }
    }
    println!("\n{} part(s)", matches.len());
}

pub fn part_detail(part: &Part, instance: &Instance, json: bool) {
    if json {
        #[derive(Serialize)]
        struct PortRow<'a> {
            name: &'a str,
            origin: [f64; 3],
            direction: [f64; 3],
            profile: od_parts::Profile,
            system_kind: od_parts::SystemKind,
        }
        #[derive(Serialize)]
        struct Detail<'a> {
            part: &'a Part,
            parameters: &'a std::collections::HashMap<String, f64>,
            ports: Vec<PortRow<'a>>,
            /// The resolved plan symbol, so a client can draw the part without
            /// re-implementing the expression evaluator.
            symbol: &'a [od_core::Geometry],
            bounds_mm: [f64; 6],
        }
        let b = instance.bounds();
        emit(&Detail {
            part,
            parameters: &instance.params,
            ports: instance
                .ports
                .iter()
                .map(|p| PortRow {
                    name: &p.name,
                    origin: [p.frame.origin.x, p.frame.origin.y, p.frame.origin.z],
                    direction: {
                        let n = p.frame.normal();
                        [n.x, n.y, n.z]
                    },
                    profile: p.profile,
                    system_kind: p.system_kind,
                })
                .collect(),
            symbol: &instance.symbol,
            bounds_mm: if b.is_empty() {
                [0.0; 6]
            } else {
                [b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z]
            },
        });
        return;
    }

    println!("{}  {}", part.id, part.name.ja);
    if !part.name.en.is_empty() {
        println!("  {}", part.name.en);
    }
    println!("  category   {}", part.category.as_str());
    println!("  IFC        {}", part.ifc_class);
    if !part.source.is_empty() {
        println!("  source     {}", part.source);
    }

    println!("\n  parameters");
    for p in &part.parameters {
        let value = instance.params.get(&p.name).copied().unwrap_or_default();
        let range = match (p.min, p.max) {
            (Some(a), Some(b)) => format!(" [{a} … {b}]"),
            (Some(a), None) => format!(" [{a} …]"),
            (None, Some(b)) => format!(" [… {b}]"),
            (None, None) => String::new(),
        };
        println!(
            "    {:<6} {:>10.1} {:<6} {}{}",
            p.name, value, p.unit, p.label.ja, range
        );
    }

    println!("\n  ports");
    for p in &instance.ports {
        let o = p.frame.origin;
        let n = p.frame.normal();
        println!(
            "    {:<20} at ({:.0}, {:.0}, {:.0})  facing ({:.2}, {:.2}, {:.2})  {:?}",
            p.name, o.x, o.y, o.z, n.x, n.y, n.z, p.profile
        );
    }

    if !part.properties.is_empty() {
        println!("\n  properties → IFC");
        for p in &part.properties {
            println!(
                "    {:<20} {:<8} {}",
                p.name,
                p.unit.as_deref().unwrap_or("-"),
                p.ifc_property
            );
        }
    }

    let b = instance.bounds();
    if !b.is_empty() {
        println!(
            "\n  bounds     {:.0} × {:.0} × {:.0} mm",
            b.size().x,
            b.size().y,
            b.size().z
        );
    }
}

pub fn systems(catalog: &Catalog, json: bool) {
    let mut list: Vec<&od_parts::SystemDef> = catalog.systems().collect();
    list.sort_by(|a, b| a.id.cmp(&b.id));
    if json {
        emit(&list);
        return;
    }
    println!("{:<26} {:<10} {:<16} name", "id", "abbrev", "layer");
    for s in list {
        println!(
            "{:<26} {:<10} {:<16} {}",
            s.id, s.abbreviation, s.layer, s.name.ja
        );
    }
}

pub fn specs(catalog: &Catalog, id: Option<&str>, json: bool) {
    match id {
        None => {
            let mut list: Vec<&od_parts::Spec> = catalog.specs().collect();
            list.sort_by(|a, b| a.id.cmp(&b.id));
            if json {
                emit(&list);
                return;
            }
            println!("{:<32} {:<10} name", "id", "sizes");
            for s in list {
                println!("{:<32} {:<10} {}", s.id, s.sizes.len(), s.name.ja);
            }
        }
        Some(id) => {
            let Some(spec) = catalog.spec(id) else {
                eprintln!("no spec `{id}`");
                return;
            };
            if json {
                emit(spec);
                return;
            }
            println!("{}  {}", spec.id, spec.name.ja);
            println!("  material     {}", spec.material.ja);
            println!("  standard     {}", spec.standard);
            println!("  joint        {}", spec.joint.ja);
            println!("  bend radius  {}× nominal", spec.min_bend_radius_ratio);
            println!("  stock length {} mm", spec.stock_length);
            println!(
                "\n  {:<12} {:>10} {:>10} {:>10} {:>10}",
                "designation", "nominal", "outside", "thick", "kg/m"
            );
            for s in &spec.sizes {
                println!(
                    "  {:<12} {:>10.1} {:>10.1} {:>10.1} {:>10.2}",
                    s.designation, s.nominal, s.outside, s.thickness, s.mass_per_m
                );
            }
        }
    }
}

pub fn render(db: &Database, input: &Path, output: &Path, bytes: usize, json: bool) {
    let s = db.stats();
    if json {
        #[derive(Serialize)]
        struct Report {
            input: String,
            output: String,
            entities: usize,
            svg_bytes: usize,
        }
        emit(&Report {
            input: input.display().to_string(),
            output: output.display().to_string(),
            entities: s.entities,
            svg_bytes: bytes,
        });
        return;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "an SVG report is well under a terabyte; the display value only needs to be close"
    )]
    let kb = bytes as f64 / 1024.0;
    println!(
        "{} → {}  ({} entities, {kb:.1} KB)",
        input.display(),
        output.display(),
        s.entities,
    );
}

pub fn mep_demo(db: &Database, output: &Path, fittings: usize, segments: usize, json: bool) {
    let s = db.stats();
    if json {
        #[derive(Serialize)]
        struct Report {
            output: String,
            entities: usize,
            fittings_placed: usize,
            route_segments: usize,
        }
        emit(&Report {
            output: output.display().to_string(),
            entities: s.entities,
            fittings_placed: fittings,
            route_segments: segments,
        });
        return;
    }
    println!(
        "{}  ({} entities, {segments} route segment(s), {fittings} auto-inserted fitting(s))",
        output.display(),
        s.entities
    );
}

pub fn mep_takeoff(t: &Takeoff, path: &Path, json: bool) {
    if json {
        #[derive(Serialize)]
        struct RouteRow<'a> {
            system: &'a str,
            spec: &'a str,
            length_mm: f64,
            count: usize,
        }
        #[derive(Serialize)]
        struct Report<'a> {
            file: String,
            total_length_mm: f64,
            routes: Vec<RouteRow<'a>>,
            fittings: &'a HashMap<String, usize>,
            equipment: &'a HashMap<String, usize>,
        }
        let mut routes: Vec<RouteRow> = t
            .routes
            .iter()
            .map(|((system, spec), total)| RouteRow {
                system,
                spec,
                length_mm: total.length_mm,
                count: total.count,
            })
            .collect();
        routes.sort_by(|a, b| a.system.cmp(b.system).then(a.spec.cmp(b.spec)));
        emit(&Report {
            file: path.display().to_string(),
            total_length_mm: t.total_length_mm(),
            routes,
            fittings: &t.fittings,
            equipment: &t.equipment,
        });
        return;
    }

    println!("{}", path.display());
    if t.routes.is_empty() {
        println!("  no routed runs");
    } else {
        println!(
            "\n  {:<24} {:<28} {:>12} {:>8}",
            "system", "spec", "length (mm)", "runs"
        );
        let mut routes: Vec<_> = t.routes.iter().collect();
        routes.sort_by(|a, b| a.0.cmp(b.0));
        for ((system, spec), total) in routes {
            println!(
                "  {system:<24} {spec:<28} {:>12.0} {:>8}",
                total.length_mm, total.count
            );
        }
        println!("\n  total length  {:.0} mm", t.total_length_mm());
    }

    if !t.equipment.is_empty() {
        println!("\n  equipment");
        let mut equipment: Vec<_> = t.equipment.iter().collect();
        equipment.sort_by(|a, b| a.0.cmp(b.0));
        for (part_id, count) in equipment {
            println!("    {part_id:<32} {count}");
        }
    }
    if !t.fittings.is_empty() {
        println!("\n  fittings");
        let mut fittings: Vec<_> = t.fittings.iter().collect();
        fittings.sort_by(|a, b| a.0.cmp(b.0));
        for (part_id, count) in fittings {
            println!("    {part_id:<32} {count}");
        }
    }
}

pub fn mep_check(graph: &ConnectionGraph, path: &Path, json: bool) {
    let unconnected = graph.unconnected();
    if json {
        #[derive(Serialize)]
        struct PortRow {
            owner: String,
            name: String,
            position_mm: [f64; 3],
        }
        #[derive(Serialize)]
        struct Report {
            file: String,
            passed: bool,
            ports: usize,
            connections: usize,
            unconnected: Vec<PortRow>,
            skipped: Vec<String>,
        }
        emit(&Report {
            file: path.display().to_string(),
            passed: unconnected.is_empty() && graph.skipped().is_empty(),
            ports: graph.ports().len(),
            connections: graph.connections().count(),
            unconnected: unconnected
                .iter()
                .map(|p| PortRow {
                    owner: p.owner.to_string(),
                    name: p.name.clone(),
                    position_mm: [p.position.x, p.position.y, p.position.z],
                })
                .collect(),
            skipped: graph.skipped().iter().map(ToString::to_string).collect(),
        });
        return;
    }

    println!("{}", path.display());
    println!(
        "  {} port(s), {} connection(s)",
        graph.ports().len(),
        graph.connections().count()
    );
    if unconnected.is_empty() {
        println!("  no unconnected ports");
    } else {
        println!("\n  unconnected ({})", unconnected.len());
        for p in &unconnected {
            println!(
                "    {}  {:<8} at ({:.0}, {:.0}, {:.0})",
                p.owner, p.name, p.position.x, p.position.y, p.position.z
            );
        }
    }
    if !graph.skipped().is_empty() {
        println!("\n  could not resolve ({})", graph.skipped().len());
        for id in graph.skipped() {
            println!("    {id}");
        }
    }
}

/// Reports the result of a spatial query.
///
/// The layer and type of each hit, not just its id: an id alone tells the
/// reader nothing about whether the index found what they meant.
pub fn query(db: &Database, found: &[od_core::ObjectId], indexed: usize, json: bool) {
    #[derive(Serialize)]
    struct Hit {
        id: String,
        kind: String,
        layer: String,
        bounds_mm: [f64; 6],
    }

    let hits: Vec<Hit> = found
        .iter()
        .filter_map(|id| {
            let entity = db.entity(*id)?;
            let b = db.entity_bounds(*id);
            Some(Hit {
                id: id.to_string(),
                kind: entity.geom.type_name().to_owned(),
                layer: db
                    .tables
                    .layers
                    .get(entity.layer)
                    .map_or_else(|| "?".to_owned(), |l| l.name.clone()),
                bounds_mm: if b.is_empty() {
                    [0.0; 6]
                } else {
                    [b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z]
                },
            })
        })
        .collect();

    if json {
        #[derive(Serialize)]
        struct Report<'a> {
            matched: usize,
            indexed: usize,
            hits: &'a [Hit],
        }
        emit(&Report {
            matched: hits.len(),
            indexed,
            hits: &hits,
        });
        return;
    }

    if hits.is_empty() {
        println!("nothing found ({indexed} entities indexed)");
        return;
    }
    println!("{:<14} {:<12} {:<20} position", "id", "type", "layer");
    for hit in &hits {
        println!(
            "{:<14} {:<12} {:<20} ({:.0}, {:.0})",
            hit.id, hit.kind, hit.layer, hit.bounds_mm[0], hit.bounds_mm[1]
        );
    }
    println!("\n{} of {indexed} entities", hits.len());
}
