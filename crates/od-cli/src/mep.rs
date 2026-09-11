//! `od mep` — building services (MEP) commands.
//!
//! `demo` proves the domain end to end without needing hand-drawn input: it
//! places a fan, routes a duct off its outlet through an automatic 90° elbow,
//! and saves a real drawing that `od render`/`od inspect`/`od roundtrip` can
//! all be pointed at. `takeoff` and `check` work on any drawing built the same
//! way — by hand later, once there is a canvas, or by another tool today.

use anyhow::{Context, Result};
use od_core::{ActorId, Database, Document, Point3};
use od_domain_mep::autoroute;
use od_domain_mep::clash::{self, ClashKind, ClashRule, Severity as ClashSeverity, SystemFilter};
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
/// Where to write an IFC export and how to reach the bridge that does it —
/// grouped into one struct rather than three more loose parameters on
/// [`route`], which already has plenty.
pub struct IfcExportOptions<'a> {
    pub out: &'a Path,
    pub bridge: &'a Path,
    pub python: &'a Path,
}

#[expect(
    clippy::too_many_arguments,
    reason = "a route's five inputs, plus the three optional render outputs, are all required and none group naturally"
)]
pub fn route(
    input: &Path,
    output: &Path,
    system: &str,
    spec: &str,
    profile: &str,
    path: &str,
    render_svg: Option<&Path>,
    render_3d: Option<&Path>,
    render_ifc: Option<IfcExportOptions<'_>>,
    json: bool,
) -> Result<()> {
    let catalog = Catalog::bundled().context("loading the bundled part catalogue")?;
    let (db, _) = crate::load::load(input)?;
    let mut doc = Document::new(db);

    let profile = parse_profile(profile)?;
    let path = parse_path(path)?;

    let (segment_ids, fitting_ids) = doc.edit::<_, _, anyhow::Error>("Route", |tx| {
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

        Ok((result.segments, result.fittings))
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

    if let Some(glb_out) = render_3d {
        render_route_solids(&doc, &segment_ids, glb_out)?;
    }

    if let Some(opts) = render_ifc {
        render_route_ifc(&doc, &catalog, &segment_ids, &opts)?;
    }

    crate::report::mep_route(
        input,
        output,
        segment_ids.len(),
        fitting_ids.len(),
        rendered,
        json,
    );
    Ok(())
}

/// Sweeps every routed segment into a solid through the default kernel
/// (`od-geom3d-lite` — ADR-002; this is the CLI's own choice of kernel, not
/// something `od-domain-mep` hardcodes) and writes the combined mesh to a
/// `.glb` file. Fittings (`fitting_ids`) are not included: they come from
/// the part catalogue as block references, not as `RouteSegment`s, and
/// giving *those* a 3D solid is a separate, catalogue-side feature this
/// does not attempt.
fn render_route_solids(
    doc: &Document,
    segment_ids: &[od_core::ObjectId],
    glb_out: &Path,
) -> Result<()> {
    let mut kernel = od_geom3d_lite::LiteKernel::new();
    let mut meshes = Vec::new();
    for &id in segment_ids {
        let seg =
            store::read_route(&doc.db, id).context("reading back a just-inserted route segment")?;
        let solid = match derive::route_solid(&seg, &mut kernel) {
            Ok(solid) => solid,
            // Profile::Oval/Terminal have no tessellation yet (crate docs) —
            // skipped rather than failing the whole render, the same
            // "narrow gap, not a fatal error" treatment conversion_losses
            // gives an unsupported geometry kind elsewhere in this CLI.
            Err(od_domain_mep::model::MepError::UnsupportedProfile(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        use od_geom3d::SolidKernel as _;
        let mesh_handle = kernel.triangulate(solid, 0.1)?;
        meshes.push(kernel.mesh_data(mesh_handle)?.clone());
    }
    od_io_gltf::write_file(&meshes, glb_out)
        .with_context(|| format!("writing {}", glb_out.display()))?;
    Ok(())
}

/// Exports every routed segment as an IFC4 product through `od-bridge-ifc`
/// (`od-io-ifc`; see that crate's docs for exactly what is and is not
/// carried across yet — geometry only, flat text properties, one fixed
/// nominal storey). Fittings are not included, the same reasoning as
/// [`render_route_solids`]. Sweeps each segment again rather than sharing
/// `render_route_solids`'s meshes: a deliberate simplification, since
/// `--render-3d` and `--render-ifc` are independent, optional outputs and a
/// route drawing's segment count never makes a second sweep pass costly.
fn render_route_ifc(
    doc: &Document,
    catalog: &Catalog,
    segment_ids: &[od_core::ObjectId],
    opts: &IfcExportOptions<'_>,
) -> Result<()> {
    let mut kernel = od_geom3d_lite::LiteKernel::new();
    let mut elements = Vec::new();
    for &id in segment_ids {
        let seg =
            store::read_route(&doc.db, id).context("reading back a just-inserted route segment")?;
        let solid = match derive::route_solid(&seg, &mut kernel) {
            Ok(solid) => solid,
            Err(od_domain_mep::model::MepError::UnsupportedProfile(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        use od_geom3d::SolidKernel as _;
        let mesh_handle = kernel.triangulate(solid, 0.1)?;
        let mesh = kernel.mesh_data(mesh_handle)?;

        let kind = catalog
            .system(&seg.system)
            .map_or(od_parts::SystemKind::Generic, |s| s.kind);
        let mut properties = std::collections::BTreeMap::new();
        properties.insert(
            "Pset_OpenDraftRoute".to_owned(),
            std::collections::BTreeMap::from([
                ("System".to_owned(), seg.system.clone()),
                ("Spec".to_owned(), seg.spec.clone()),
            ]),
        );
        elements.push(od_io_ifc::IfcElement {
            ifc_class: ifc_class_for(kind).to_owned(),
            name: id.to_string(),
            positions: mesh.positions.iter().map(|p| [p.x, p.y, p.z]).collect(),
            triangles: mesh.indices.as_chunks::<3>().0.to_vec(),
            properties,
        });
    }

    let request = od_io_ifc::IfcExportRequest {
        project_name: "OpenDraft export".into(),
        elements,
    };
    od_io_ifc::export(&request, opts.python, opts.bridge, opts.out)
        .with_context(|| format!("writing {}", opts.out.display()))?;
    Ok(())
}

/// `RouteSegment` -> IFC entity, by system kind (`docs/05-interop-license.md`
/// §2.4's own mapping table). `Generic` (supports, sleeves, penetrations)
/// has no distinct IFC distribution-segment class, so it falls back to
/// `IfcPipeSegment` rather than inventing one — a narrowing, not a claim
/// that a support rail is a pipe.
fn ifc_class_for(kind: od_parts::SystemKind) -> &'static str {
    use od_parts::SystemKind;
    match kind {
        SystemKind::Air => "IfcDuctSegment",
        SystemKind::Power | SystemKind::Signal => "IfcCableCarrierSegment",
        SystemKind::Water
        | SystemKind::Drainage
        | SystemKind::Hydronic
        | SystemKind::FireProtection
        | SystemKind::Gas
        | SystemKind::Generic => "IfcPipeSegment",
    }
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

pub fn takeoff(input: &Path, csv_out: Option<&Path>, json: bool) -> Result<()> {
    let catalog = Catalog::bundled().context("loading the bundled part catalogue")?;
    let (db, _) = crate::load::load(input)?;
    let result = takeoff::take_off(&db, &catalog);
    if let Some(path) = csv_out {
        std::fs::write(path, takeoff::to_csv(&result))
            .with_context(|| format!("writing {}", path.display()))?;
    }
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

/// Lists every port in the document — not just the unconnected ones `check`
/// reports. An editing canvas needs this to offer a component's ports up as
/// snap targets (F-104): drawing a route that lands exactly on a port's
/// position, facing the opposite way, on a matching system, is what makes the
/// connection graph link the two without any extra step.
pub fn ports(input: &Path, json: bool) -> Result<()> {
    let catalog = Catalog::bundled().context("loading the bundled part catalogue")?;
    let (db, _) = crate::load::load(input)?;
    let graph = ConnectionGraph::build(&db, &catalog);
    crate::report::mep_ports(&graph, input, json);
    Ok(())
}

/// Two runs this close together were drawn as the same route, not
/// coincidentally adjacent — well below normal drawing precision, far above
/// floating-point noise. Fixed rather than `--clearance`-configurable: a
/// duplicate isn't a distance problem, it is exact or it is not one.
const DUPLICATE_TOLERANCE_MM: f64 = 1.0;

/// Runs the interference check (F-108) over every routed segment. Always
/// checks for hard clashes and duplicate runs; `clearance`, if given, also
/// checks every pair for a minimum required gap. See
/// `od_domain_mep::clash`'s module docs for exactly what "checked" means
/// here (every profile as its circumscribing cylinder, routes only).
pub fn clash(
    input: &Path,
    clearance: Option<f64>,
    bcf_out: Option<&Path>,
    json: bool,
) -> Result<()> {
    let (db, _) = crate::load::load(input)?;

    let mut rules = vec![
        ClashRule {
            a: SystemFilter::any(),
            b: SystemFilter::any(),
            kind: ClashKind::Hard,
            tolerance: 0.0,
            severity: ClashSeverity::Error,
        },
        ClashRule {
            a: SystemFilter::any(),
            b: SystemFilter::any(),
            kind: ClashKind::Duplicate,
            tolerance: DUPLICATE_TOLERANCE_MM,
            severity: ClashSeverity::Warning,
        },
    ];
    if let Some(tolerance) = clearance {
        rules.push(ClashRule {
            a: SystemFilter::any(),
            b: SystemFilter::any(),
            kind: ClashKind::Clearance,
            tolerance,
            severity: ClashSeverity::Warning,
        });
    }

    let issues = clash::find_clashes(&db, &rules);
    let failed = issues.iter().any(|i| i.severity == ClashSeverity::Error);

    if let Some(bcf_path) = bcf_out {
        let topics: Vec<od_io_bcf::BcfTopic> =
            issues.iter().map(clash_issue_to_bcf_topic).collect();
        od_io_bcf::write_file(&topics, bcf_path)
            .with_context(|| format!("writing {}", bcf_path.display()))?;
    }

    crate::report::mep_clash(&issues, input, json);
    if failed {
        // A hard clash is exactly the kind of mistake this check exists to
        // catch before it reaches site, same convention as `check` (F-108).
        std::process::exit(1);
    }
    Ok(())
}

/// One `ClashIssue` -> one BCF topic. `od-io-bcf` is deliberately generic
/// over what a "topic" is (it does not depend on `od-domain-mep`, mirroring
/// how `od-io-gltf` stays generic over `MeshData` rather than naming
/// `RouteSegment`) — this function is where that translation happens, the
/// same role `render_route_solids` plays for the glTF export above.
fn clash_issue_to_bcf_topic(issue: &od_domain_mep::clash::ClashIssue) -> od_io_bcf::BcfTopic {
    use od_domain_mep::clash::ClashKind;
    use std::hash::{Hash, Hasher};

    // Content-derived, not position-derived: the same issue gets the same
    // BCF Guid across repeated runs of the same drawing even if
    // find_clashes's internal iteration order ever shifts, which matters
    // for diffing two exports in version control or a CI baseline.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    issue.a.hash(&mut hasher);
    issue.b.hash(&mut hasher);
    format!("{:?}", issue.kind).hash(&mut hasher);
    let guid = od_io_bcf::guid_from_seed(hasher.finish());

    let priority = match issue.severity {
        od_domain_mep::clash::Severity::Error => "Critical",
        od_domain_mep::clash::Severity::Warning => "Normal",
        od_domain_mep::clash::Severity::Info => "Minor",
    };
    let kind_label = match issue.kind {
        ClashKind::Hard => "Hard clash",
        ClashKind::Clearance => "Clearance violation",
        ClashKind::Duplicate => "Duplicate route",
    };

    od_io_bcf::BcfTopic {
        guid,
        title: format!("{kind_label}: {} x {}", issue.a, issue.b),
        topic_type: "Clash".into(),
        // No workflow state is persisted yet (od_domain_mep::clash's own
        // module docs) -- every export is a fresh, unaddressed finding.
        topic_status: "Open".into(),
        priority: priority.into(),
        description: format!(
            "gap: {:.1}mm at ({:.0}, {:.0}, {:.0})",
            issue.gap_mm, issue.location.x, issue.location.y, issue.location.z
        ),
        creation_author: "OpenDraft".into(),
        creation_date: od_io_bcf::iso8601_utc(std::time::SystemTime::now()),
    }
}

/// Proposes routes between two points, treating every existing route
/// segment as an obstacle (`od_domain_mep::clash::effective_radius`'s
/// circumscribing-cylinder approximation, same as interference checking,
/// inflated by `clearance`). A proposal only — this never writes to the
/// document (F-107: "a person always chooses and can redraw"); pipe a
/// chosen candidate's path into `od mep route --path` to actually draw it.
pub fn autoroute(input: &Path, path: &str, clearance: f64, json: bool) -> Result<()> {
    let (db, _) = crate::load::load(input)?;
    let points = parse_path(path)?;
    let [start, end] = points.as_slice() else {
        anyhow::bail!("--path must have exactly two points: start;end");
    };

    let obstacles: Vec<od_geom3d::Aabb3> = store::all_routes(&db)
        .into_iter()
        .map(|(_, route)| {
            let radius = clash::effective_radius(route.profile) + clearance;
            od_geom3d::Aabb3::new(route.start, route.end).inflated(radius)
        })
        .collect();

    let candidates = autoroute::find_candidates(*start, *end, &obstacles)?;
    crate::report::mep_autoroute(&candidates, input, json);
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
