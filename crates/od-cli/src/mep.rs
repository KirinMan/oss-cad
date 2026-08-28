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
use od_parts::{Catalog, Profile};
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

/// Draws a route on an existing drawing and saves the result — the CLI's own
/// path through `draw_route`, mirroring how `od edit` is the CLI's own path
/// through `od_core::Command`. Kept separate from `edit`: a route is not a
/// single `Command`, and cannot be, without `od-core` learning what a
/// "system" or a "spec" is (rule 1) — the domain's own vocabulary belongs in
/// the domain's own entry point.
#[expect(
    clippy::too_many_arguments,
    reason = "a route's five inputs are all required and none group naturally"
)]
pub fn route(
    input: &Path,
    output: &Path,
    system: &str,
    spec: &str,
    profile: &str,
    path: &str,
    render_svg: Option<&Path>,
    json: bool,
) -> Result<()> {
    let catalog = Catalog::bundled().context("loading the bundled part catalogue")?;
    let (db, _) = crate::load::load(input)?;
    let mut doc = Document::new(db);

    let profile = parse_profile(profile)?;
    let path = parse_path(path)?;

    let (segments, fittings) = doc.edit::<_, _, anyhow::Error>("Route", |tx| {
        let spec_route = RouteSpec {
            system: system.to_owned(),
            spec: spec.to_owned(),
            profile,
            path,
        };
        let result = draw_route(tx, &catalog, &spec_route)?;

        for &id in &result.segments {
            let seg = store::read_route(tx.db(), id)?;
            derive::insert_route(tx, &catalog, &seg, derive::ViewMode::DoubleLine)?;
        }
        for &id in &result.fittings {
            let part = store::read_part(tx.db(), id)?;
            derive::insert_part(tx, &catalog, &part)?;
        }

        Ok((result.segments.len(), result.fittings.len()))
    })?;

    crate::load::save(&doc.db, output)?;

    let rendered = match render_svg {
        Some(svg_out) => {
            let (svg, view_box) =
                od_io_svg::to_svg_with_view_box(&doc.db, &od_io_svg::SvgOptions::default());
            std::fs::write(svg_out, &svg)
                .with_context(|| format!("writing {}", svg_out.display()))?;
            Some((svg_out, view_box))
        }
        None => None,
    };

    crate::report::mep_route(input, output, segments, fittings, rendered, json);
    Ok(())
}

/// Places one piece of equipment on an existing drawing and saves the
/// result — the CLI's own path through [`PlacedPart`], for the same reason
/// [`route`] is its own command rather than an `od_core::Command`: equipment
/// placement needs a part id and a system, vocabulary `od-core` must never
/// learn (rule 1).
///
/// Always [`PlacementKind::Equipment`] — a fitting is something [`route`]
/// inserts automatically, never something placed directly, so there is no
/// ambiguity to ask the caller to resolve.
#[expect(
    clippy::too_many_arguments,
    reason = "a placement's inputs are all required and none group naturally"
)]
pub fn place(
    input: &Path,
    output: &Path,
    part_id: &str,
    position: Point3,
    rotation_deg: f64,
    mirror_y: bool,
    system: Option<&str>,
    set: &[String],
    render_svg: Option<&Path>,
    json: bool,
) -> Result<()> {
    let catalog = Catalog::bundled().context("loading the bundled part catalogue")?;
    let (db, _) = crate::load::load(input)?;
    let mut doc = Document::new(db);

    let params = parse_set_overrides(set)?;

    let part = PlacedPart {
        part_id: part_id.to_owned(),
        position,
        rotation: rotation_deg.to_radians(),
        mirror_y,
        params,
        kind: PlacementKind::Equipment,
        system: system.map(str::to_owned),
    };

    let id = doc.edit::<_, _, anyhow::Error>("Place", |tx| {
        let id = store::add_part(tx, &part)?;
        derive::insert_part(tx, &catalog, &part)?;
        Ok(id)
    })?;

    crate::load::save(&doc.db, output)?;

    let rendered = match render_svg {
        Some(svg_out) => {
            let (svg, view_box) =
                od_io_svg::to_svg_with_view_box(&doc.db, &od_io_svg::SvgOptions::default());
            std::fs::write(svg_out, &svg)
                .with_context(|| format!("writing {}", svg_out.display()))?;
            Some((svg_out, view_box))
        }
        None => None,
    };

    crate::report::mep_place(input, output, id, rendered, json);
    Ok(())
}

/// `NAME=VALUE` pairs, as `--set` takes them for both `parts show` and
/// `mep place` — pulled out once rather than duplicated between `main.rs`
/// and here.
pub fn parse_set_overrides(set: &[String]) -> Result<HashMap<String, f64>> {
    let mut params = HashMap::new();
    for pair in set {
        let (name, value) = pair
            .split_once('=')
            .with_context(|| format!("`{pair}` should look like NAME=VALUE"))?;
        let v: f64 = value
            .trim()
            .parse()
            .with_context(|| format!("`{value}` is not a number"))?;
        params.insert(name.trim().to_owned(), v);
    }
    Ok(params)
}

/// `rect:W,H` or `round:D`, in millimetres.
fn parse_profile(text: &str) -> Result<Profile> {
    let (kind, dims) = text
        .split_once(':')
        .with_context(|| format!("`{text}` should look like `rect:W,H` or `round:D`"))?;
    match kind {
        "rect" => {
            let (w, h) = dims
                .split_once(',')
                .with_context(|| format!("`{text}` should look like `rect:W,H`"))?;
            Ok(Profile::Rect {
                w: w.trim().parse().context("width is not a number")?,
                h: h.trim().parse().context("height is not a number")?,
            })
        }
        "round" => Ok(Profile::Round {
            d: dims.trim().parse().context("diameter is not a number")?,
        }),
        other => anyhow::bail!("`{other}` should be `rect` or `round`"),
    }
}

/// Centreline vertices: `x,y,z;x,y,z;…`.
fn parse_path(text: &str) -> Result<Vec<Point3>> {
    text.split(';')
        .map(|point| {
            let parts: Vec<f64> = point
                .split(',')
                .map(|v| v.trim().parse::<f64>())
                .collect::<std::result::Result<_, _>>()
                .with_context(|| format!("`{point}` should be three numbers: x,y,z"))?;
            let [x, y, z] = parts.as_slice() else {
                anyhow::bail!("`{point}` should be three numbers: x,y,z");
            };
            Ok(Point3::new(*x, *y, *z))
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_rectangular_profile() {
        let p = parse_profile("rect:400,300").expect("parses");
        assert_eq!(p, Profile::Rect { w: 400.0, h: 300.0 });
    }

    #[test]
    fn parses_a_round_profile() {
        let p = parse_profile("round:50").expect("parses");
        assert_eq!(p, Profile::Round { d: 50.0 });
    }

    #[test]
    fn rejects_an_unknown_profile_kind() {
        assert!(parse_profile("hex:10").is_err());
        assert!(parse_profile("no-colon-here").is_err());
        assert!(parse_profile("rect:400").is_err());
    }

    #[test]
    fn parses_a_multi_point_path_with_negative_coordinates() {
        let path = parse_path("0,0,500;5000,0,500;5000,-3000,500").expect("parses");
        assert_eq!(
            path,
            vec![
                Point3::new(0.0, 0.0, 500.0),
                Point3::new(5000.0, 0.0, 500.0),
                Point3::new(5000.0, -3000.0, 500.0),
            ]
        );
    }

    #[test]
    fn rejects_a_malformed_path_point() {
        assert!(parse_path("0,0,0;not,a,point").is_err());
        assert!(parse_path("0,0").is_err());
    }

    #[test]
    fn parses_set_overrides() {
        let overrides =
            parse_set_overrides(&["W=500".to_owned(), "H=300".to_owned()]).expect("parses");
        assert_eq!(overrides.get("W"), Some(&500.0));
        assert_eq!(overrides.get("H"), Some(&300.0));
    }

    #[test]
    fn rejects_a_malformed_set_override() {
        assert!(parse_set_overrides(&["no-equals-sign".to_owned()]).is_err());
        assert!(parse_set_overrides(&["W=wide".to_owned()]).is_err());
    }
}
