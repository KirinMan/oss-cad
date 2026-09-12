//! `od` — OpenDraft's headless command line.
//!
//! Phase 0 ships this before any GUI on purpose (`docs/06-roadmap.md`): batch
//! conversion, drawing inspection and standards checking are useful on their own,
//! they are what the round-trip guarantee is tested through, and they give the
//! project something working to show long before there is a canvas to draw on.
//!
//! Every command speaks both to a person and to a machine: pass `--json` and the
//! output is structured, which is how the API and CI consume it.

mod check;
mod load;
mod mep;
mod report;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use od_parts::Catalog;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "od",
    version,
    about = "OpenDraft — drawing conversion, inspection and part catalogue",
    long_about = None
)]
struct Cli {
    /// Emit machine-readable JSON instead of a human-readable report.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Creates a blank drawing — no entities, no layers beyond the default
    /// `0`, nothing to open first.
    ///
    /// Every other command needs an existing file to read; this is the one
    /// way to get a first file at all, for the same reason a text editor
    /// needs a "new document" action distinct from "open."
    New {
        output: PathBuf,
        /// Also render the (necessarily empty) result to SVG, so a caller
        /// gets both in one invocation instead of a second load — matches
        /// `edit`/`mep route`'s own `--render`.
        #[arg(long, value_name = "SVG")]
        render: Option<PathBuf>,
    },
    /// Convert a drawing between formats.
    ///
    /// `.odc` is the native container and holds the whole document; `.dxf` is
    /// for exchange and cannot carry everything.
    Convert {
        input: PathBuf,
        output: PathBuf,
        /// Fail instead of warning when data would be preserved-but-unreadable,
        /// or dropped by the target format.
        #[arg(long)]
        strict: bool,
    },
    /// Report what is in a drawing.
    Inspect { input: PathBuf },
    /// Check a drawing against a rule set.
    Check {
        input: PathBuf,
        /// Rule set to apply.
        #[arg(long, value_enum, default_value_t = RuleSet::Basic)]
        rules: RuleSet,
    },
    /// Applies one edit command and saves the result (ADR-006).
    ///
    /// The single path a UI, a script or a service applies a change through —
    /// `apps/api` shells out to this for the editing canvas rather than
    /// reimplementing `od-core`'s edit logic in TypeScript.
    Edit {
        input: PathBuf,
        output: PathBuf,
        /// A `Command`, as JSON — e.g.
        /// `{"kind":"add_line","layer":"0","a":[0,0,0],"b":[1000,0,0]}`.
        #[arg(long)]
        command: String,
        /// Also render the result to SVG, so a caller gets both in one
        /// invocation instead of a second full document load.
        #[arg(long, value_name = "SVG")]
        render: Option<PathBuf>,
    },

    /// Runs a JavaScript file against a drawing and saves the result
    /// (`od-script`).
    ///
    /// The script drives the document through `od.execute(command)` — the
    /// same `Command` JSON shape `--command` above and `POST
    /// /api/drawings/edit` already accept (ADR-006) — plus `od.entities()`/
    /// `od.inspect()` for reading it back, and `console.log` for output.
    Script {
        input: PathBuf,
        output: PathBuf,
        /// The `.js` file to run.
        script: PathBuf,
    },

    /// Read a drawing, write it, read it back, and compare.
    Roundtrip {
        input: PathBuf,
        /// Format to round-trip through. Defaults to the input's own.
        #[arg(long, value_name = "FORMAT")]
        via: Option<String>,
    },
    /// Render a drawing to SVG.
    ///
    /// SVG needs no GPU and opens in anything, which makes it how a drawing
    /// becomes visible before the editing canvas exists.
    Render {
        input: PathBuf,
        output: PathBuf,
        /// Only draw this window: `x1,y1,x2,y2` in drawing millimetres.
        #[arg(long, value_name = "X1,Y1,X2,Y2", allow_hyphen_values = true)]
        window: Option<String>,
        /// Only draw these layers, comma-separated.
        #[arg(long, value_delimiter = ',')]
        layers: Option<Vec<String>>,
        /// Render on a dark canvas rather than white paper.
        #[arg(long)]
        dark: bool,
        /// Pixel width. Without it the SVG scales to its container.
        #[arg(long, value_name = "PX")]
        width: Option<u32>,
        /// Render this paper-space layout instead of model space, by name
        /// (e.g. `*Paper_Space`, the default layout every document has).
        #[arg(long)]
        layout: Option<String>,
    },

    /// Find entities by position, through the spatial index.
    Query {
        input: PathBuf,
        /// Entities meeting this window: `x1,y1,x2,y2`.
        #[arg(
            long,
            value_name = "X1,Y1,X2,Y2",
            conflicts_with = "near",
            allow_hyphen_values = true
        )]
        window: Option<String>,
        /// Entities nearest this point: `x,y`.
        #[arg(long, value_name = "X,Y", allow_hyphen_values = true)]
        near: Option<String>,
        /// How many to return for `--near`.
        #[arg(long, default_value_t = 10)]
        count: usize,
    },

    /// Browse the part catalogue.
    Parts {
        #[command(subcommand)]
        command: PartsCommand,
        /// Load additional part definitions from this directory.
        #[arg(long, global = true)]
        library: Option<PathBuf>,
    },

    /// Building services (MEP) routing, take-off and connectivity checks.
    Mep {
        #[command(subcommand)]
        command: MepCommand,
    },
}

#[derive(Subcommand, Debug)]
enum MepCommand {
    /// Generates a small worked example: a fan, a routed duct, and the
    /// automatic elbow the route needs — proof the domain works end to end.
    Demo { output: PathBuf },
    /// Draws a centreline route — auto-inserting the fittings any 90° bends
    /// need — and saves the result.
    Route {
        input: PathBuf,
        output: PathBuf,
        /// A `SystemDef` id, e.g. `sys.air.supply`.
        #[arg(long)]
        system: String,
        /// A `Spec` id, e.g. `spec.duct.galvanised.rect`.
        #[arg(long)]
        spec: String,
        /// `rect:W,H` or `round:D`, in millimetres.
        #[arg(long)]
        profile: String,
        /// Centreline vertices: `x,y,z;x,y,z;…`, at least two, all one Z.
        #[arg(long, allow_hyphen_values = true)]
        path: String,
        /// Also render the result to SVG, so a caller gets both in one
        /// invocation instead of a second full document load.
        #[arg(long, value_name = "SVG")]
        render: Option<PathBuf>,
        /// Also sweep every routed segment into a solid (`od-geom3d-lite`)
        /// and write the combined mesh as binary glTF — the one thing that
        /// makes the sweep externally checkable before OpenDraft has a 3D
        /// view of its own. Fittings are not included (crate docs).
        #[arg(long, value_name = "GLB")]
        render_3d: Option<PathBuf>,
        /// Also export every routed segment as IFC4, through
        /// `od-bridge-ifc` (`od-io-ifc` — LGPL-3.0 kept out of this binary
        /// via process isolation, see NOTICE). Requires `--ifc-bridge`.
        /// Fittings are not included (crate docs).
        #[arg(long, value_name = "IFC", requires = "ifc_bridge")]
        render_ifc: Option<PathBuf>,
        /// Path to `bridges/od-bridge-ifc/od_bridge_ifc.py`.
        #[arg(long, value_name = "PATH")]
        ifc_bridge: Option<PathBuf>,
        /// Python interpreter to run the IFC bridge with — must have
        /// `ifcopenshell` installed.
        #[arg(long, value_name = "PATH", default_value = "python3")]
        ifc_python: PathBuf,
    },
    /// Places one piece of equipment — always `Equipment`, never a fitting,
    /// which `route` inserts automatically.
    Place {
        input: PathBuf,
        output: PathBuf,
        /// A catalogue part id, e.g. `hvac.fan.sirocco`.
        #[arg(long)]
        part: String,
        #[arg(long, allow_hyphen_values = true)]
        x: f64,
        #[arg(long, allow_hyphen_values = true)]
        y: f64,
        #[arg(long, allow_hyphen_values = true, default_value_t = 0.0)]
        z: f64,
        /// Degrees, about +Z.
        #[arg(long, allow_hyphen_values = true, default_value_t = 0.0)]
        rotation: f64,
        /// Mirrors the part's local Y axis before rotating.
        #[arg(long)]
        mirror: bool,
        /// A `SystemDef` id; chooses the layer and colour it draws with.
        #[arg(long)]
        system: Option<String>,
        /// Parameter overrides, as `NAME=VALUE`.
        #[arg(long = "set", value_name = "NAME=VALUE")]
        set: Vec<String>,
        /// Also render the result to SVG, so a caller gets both in one
        /// invocation instead of a second full document load.
        #[arg(long, value_name = "SVG")]
        render: Option<PathBuf>,
    },
    /// Reports routed lengths by system/spec and part counts by id.
    Takeoff {
        input: PathBuf,
        /// Also write the take-off as a single CSV table (routes, fittings
        /// and equipment, distinguished by a `kind` column).
        #[arg(long, value_name = "CSV")]
        csv: Option<PathBuf>,
    },
    /// Reports unconnected ports and parts the catalogue could not resolve.
    Check { input: PathBuf },
    /// Lists every port in the document, connected or not — what an editing
    /// canvas offers up as snap targets so a route lands exactly on one.
    Ports { input: PathBuf },
    /// Checks every pair of routed segments for interference (F-108).
    ///
    /// Every profile is treated as its circumscribing cylinder (crate docs
    /// on `od_domain_mep::clash`) — a conservative approximation, not a
    /// true box/box test. `PlacedPart`s (equipment, fittings) are not
    /// checked. Always runs a hard-clash check (any overlap) and a
    /// duplicate-route check (the same run drawn twice); `--clearance` adds
    /// a minimum-gap check on top.
    Clash {
        input: PathBuf,
        /// Minimum required gap in millimetres between any two runs'
        /// surfaces, checked across every system pair. Omit to skip this
        /// check and report only hard clashes and duplicates.
        #[arg(long)]
        clearance: Option<f64>,
        /// Also write every issue found as a BCF 2.1 topic
        /// (`od-io-bcf` — export only, no viewpoints or snapshots), so a
        /// BIM coordination tool can open the same findings directly.
        #[arg(long, value_name = "BCFZIP")]
        bcf: Option<PathBuf>,
    },
    /// Proposes routes between two points, treating every existing route
    /// as an obstacle (F-107). A proposal only — nothing is drawn; pipe a
    /// chosen candidate's path into `mep route --path`.
    Autoroute {
        input: PathBuf,
        /// Exactly two points: `start;end`, e.g. `0,0,2800;5000,3000,2800`.
        #[arg(long, allow_hyphen_values = true)]
        path: String,
        /// Extra clearance in millimetres added on top of each existing
        /// route's circumscribing radius when treating it as an obstacle.
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        clearance: f64,
    },
}

#[derive(Subcommand, Debug)]
enum PartsCommand {
    /// List parts, optionally filtered by a search term.
    List { query: Option<String> },
    /// Show one part in full.
    Show {
        id: String,
        /// Parameter overrides, as `NAME=VALUE`.
        #[arg(long = "set", value_name = "NAME=VALUE")]
        set: Vec<String>,
    },
    /// List the systems in the catalogue.
    Systems,
    /// List specifications, or show one specification's size table.
    Specs { id: Option<String> },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum RuleSet {
    /// Structural integrity only: references resolve, nothing is orphaned.
    Basic,
    /// Adds layer naming and annotation conventions.
    Jp,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::New { output, render } => new_drawing(&output, render.as_deref(), cli.json),
        Command::Convert {
            input,
            output,
            strict,
        } => convert(&input, &output, strict, cli.json),
        Command::Inspect { input } => inspect(&input, cli.json),
        Command::Edit {
            input,
            output,
            command,
            render,
        } => edit_drawing(&input, &output, &command, render.as_deref(), cli.json),
        Command::Script {
            input,
            output,
            script,
        } => run_script(&input, &output, &script, cli.json),
        Command::Check { input, rules } => check_drawing(&input, rules, cli.json),
        Command::Roundtrip { input, via } => roundtrip(&input, via, cli.json),
        Command::Render {
            input,
            output,
            window,
            layers,
            dark,
            width,
            layout,
        } => render(
            &input,
            &output,
            window.as_deref(),
            layers,
            dark,
            width,
            layout.as_deref(),
            cli.json,
        ),
        Command::Query {
            input,
            window,
            near,
            count,
        } => query(&input, window.as_deref(), near.as_deref(), count, cli.json),
        Command::Parts { command, library } => parts(&command, library.as_deref(), cli.json),
        Command::Mep { command } => match command {
            MepCommand::Demo { output } => mep::demo(&output, cli.json),
            MepCommand::Route {
                input,
                output,
                system,
                spec,
                profile,
                path,
                render,
                render_3d,
                render_ifc,
                ifc_bridge,
                ifc_python,
            } => {
                let ifc_opts = match (&render_ifc, &ifc_bridge) {
                    (Some(out), Some(bridge)) => Some(mep::IfcExportOptions {
                        out,
                        bridge,
                        python: &ifc_python,
                    }),
                    _ => None,
                };
                mep::route(
                    &input,
                    &output,
                    &system,
                    &spec,
                    &profile,
                    &path,
                    render.as_deref(),
                    render_3d.as_deref(),
                    ifc_opts,
                    cli.json,
                )
            }
            MepCommand::Place {
                input,
                output,
                part,
                x,
                y,
                z,
                rotation,
                mirror,
                system,
                set,
                render,
            } => mep::place(
                &input,
                &output,
                &part,
                od_core::Point3::new(x, y, z),
                rotation,
                mirror,
                system.as_deref(),
                &set,
                render.as_deref(),
                cli.json,
            ),
            MepCommand::Takeoff { input, csv } => mep::takeoff(&input, csv.as_deref(), cli.json),
            MepCommand::Check { input } => mep::check(&input, cli.json),
            MepCommand::Ports { input } => mep::ports(&input, cli.json),
            MepCommand::Clash {
                input,
                clearance,
                bcf,
            } => mep::clash(&input, clearance, bcf.as_deref(), cli.json),
            MepCommand::Autoroute {
                input,
                path,
                clearance,
            } => mep::autoroute(&input, &path, clearance, cli.json),
        },
    }
}

fn new_drawing(
    output: &std::path::Path,
    render_svg: Option<&std::path::Path>,
    json: bool,
) -> Result<()> {
    let db = od_core::Database::new(od_core::ActorId::SYSTEM);
    load::save(&db, output)?;

    let rendered = match render_svg {
        Some(svg_out) => {
            let (svg, view_box) =
                od_io_svg::to_svg_with_view_box(&db, &od_io_svg::SvgOptions::default());
            std::fs::write(svg_out, &svg)
                .with_context(|| format!("writing {}", svg_out.display()))?;
            Some((svg_out, view_box))
        }
        None => None,
    };

    report::new_drawing(output, rendered, json);
    Ok(())
}

fn convert(
    input: &std::path::Path,
    output: &std::path::Path,
    strict: bool,
    json: bool,
) -> Result<()> {
    let (db, outcome) = load::load(input)?;

    if strict && !outcome.unsupported_entities.is_empty() {
        anyhow::bail!(
            "--strict: {} entity type(s) were preserved but not understood: {}",
            outcome.unsupported_entities.len(),
            outcome.unsupported_entities.join(", ")
        );
    }

    let target = load::extension_of(output);
    let losses = load::conversion_losses(&db, &target);
    if strict && !losses.is_empty() {
        anyhow::bail!(
            "--strict: converting to {target} would lose {}",
            losses.join("; ")
        );
    }

    load::save(&db, output)?;
    report::conversion(&db, &outcome, input, output, &losses, json);
    Ok(())
}

fn inspect(input: &std::path::Path, json: bool) -> Result<()> {
    let (db, outcome) = load::load(input)?;
    report::inspection(&db, &outcome, input, json);
    Ok(())
}

fn check_drawing(input: &std::path::Path, rules: RuleSet, json: bool) -> Result<()> {
    let (db, _) = load::load(input)?;
    let findings = check::run(&db, rules == RuleSet::Jp);
    let failed = findings
        .iter()
        .any(|f| f.severity == check::Severity::Error);
    report::findings(&findings, input, json);
    if failed {
        // A non-zero exit is what makes this usable as a CI gate.
        std::process::exit(1);
    }
    Ok(())
}

/// Reads, writes and reads again — in the file's own format unless told
/// otherwise. Round-tripping DXF through .odc would test the wrong thing:
/// what matters is that a format does not lose data to itself.
fn roundtrip(input: &std::path::Path, via: Option<String>, json: bool) -> Result<()> {
    let (first, _) = load::load(input)?;
    let format = via.unwrap_or_else(|| load::extension_of(input));

    let (second, warnings) = match format.as_str() {
        "dxf" => {
            let written = od_io_dxf::write_string(&first);
            let (db, outcome) =
                od_io_dxf::read_str(&written).context("re-reading our own DXF output")?;
            (db, outcome.warnings.len())
        }
        "odc" => {
            let written = od_io_odc::write_bytes(&first).context("writing .odc")?;
            let (db, outcome) =
                od_io_odc::read_bytes(&written).context("re-reading our own .odc output")?;
            (db, outcome.warnings.len())
        }
        other => anyhow::bail!("cannot round-trip through `{other}` (supported: dxf, odc)"),
    };

    let diff = report::roundtrip(&first, &second, warnings, input, &format, json);
    if diff {
        std::process::exit(1);
    }
    Ok(())
}

/// `x1,y1,x2,y2` in drawing millimetres, as a window of any height — plan views
/// are how these are always specified.
fn parse_window(text: &str) -> Result<od_geom3d::Aabb3> {
    let parts: Vec<f64> = text
        .split(',')
        .map(|v| v.trim().parse::<f64>())
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("`{text}` should be four numbers: x1,y1,x2,y2"))?;
    let [x1, y1, x2, y2] = parts.as_slice() else {
        anyhow::bail!("`{text}` should be four numbers: x1,y1,x2,y2");
    };
    Ok(od_geom3d::Aabb3::new(
        od_core::Point3::new(*x1, *y1, f64::NEG_INFINITY),
        od_core::Point3::new(*x2, *y2, f64::INFINITY),
    ))
}

fn parse_point(text: &str) -> Result<od_core::Point3> {
    let parts: Vec<f64> = text
        .split(',')
        .map(|v| v.trim().parse::<f64>())
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("`{text}` should be two or three numbers"))?;
    match parts.as_slice() {
        [x, y] => Ok(od_core::Point3::new(*x, *y, 0.0)),
        [x, y, z] => Ok(od_core::Point3::new(*x, *y, *z)),
        _ => anyhow::bail!("`{text}` should be two or three numbers"),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one CLI subcommand's inputs, passed straight through from its \
              flags; none group naturally"
)]
fn render(
    input: &std::path::Path,
    output: &std::path::Path,
    window: Option<&str>,
    layers: Option<Vec<String>>,
    dark: bool,
    width: Option<u32>,
    layout: Option<&str>,
    json: bool,
) -> Result<()> {
    let (db, _) = load::load(input)?;

    let mut options = od_io_svg::SvgOptions {
        background: if dark {
            od_io_svg::Background::Dark
        } else {
            od_io_svg::Background::Paper
        },
        width_px: width,
        layers,
        ..Default::default()
    };
    if let Some(text) = window {
        options.window = Some(parse_window(text)?);
    }
    if let Some(name) = layout {
        let id = db
            .tables
            .blocks
            .id_of(name)
            .with_context(|| format!("no layout named `{name}`"))?;
        let is_paper_space = db
            .tables
            .blocks
            .get(id)
            .is_some_and(|b| b.kind == od_core::BlockKind::PaperSpace);
        anyhow::ensure!(is_paper_space, "`{name}` is not a paper-space layout");
        options.space = Some(id);
    }

    let (svg, view_box) = od_io_svg::to_svg_with_view_box(&db, &options);
    std::fs::write(output, &svg).with_context(|| format!("writing {}", output.display()))?;

    report::render(&db, input, output, svg.len(), view_box, json);
    Ok(())
}

/// Runs a JavaScript file against a drawing and saves the result
/// (`od-script`).
fn run_script(
    input: &std::path::Path,
    output: &std::path::Path,
    script_path: &std::path::Path,
    json: bool,
) -> Result<()> {
    let (db, _) = load::load(input)?;
    let doc = od_core::Document::new(db);
    let script = std::fs::read_to_string(script_path)
        .with_context(|| format!("reading {}", script_path.display()))?;

    let (doc, outcome) = od_script::run(doc, &script).context("running the script")?;

    load::save(&doc.db, output)?;
    report::script(&outcome, input, output, json);
    Ok(())
}

/// Applies a [`od_core::Command`] and saves the result — the one path a UI,
/// script or service edits a drawing through (ADR-006).
fn edit_drawing(
    input: &std::path::Path,
    output: &std::path::Path,
    command_json: &str,
    render_svg: Option<&std::path::Path>,
    json: bool,
) -> Result<()> {
    let (db, _) = load::load(input)?;
    let mut doc = od_core::Document::new(db);

    let command: od_core::Command =
        serde_json::from_str(command_json).context("--command is not a valid edit command")?;
    let outcome = doc
        .execute("Edit", &command)
        .context("applying the command")?;

    load::save(&doc.db, output)?;

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

    report::edit(&outcome, input, output, rendered, json);
    Ok(())
}

fn query(
    input: &std::path::Path,
    window: Option<&str>,
    near: Option<&str>,
    count: usize,
    json: bool,
) -> Result<()> {
    let (db, _) = load::load(input)?;
    let index = od_index::DrawingIndex::of_model_space(&db);

    let found = match (window, near) {
        (Some(text), _) => index.query(&db, parse_window(text)?),
        (None, Some(text)) => index.nearest(parse_point(text)?, count),
        (None, None) => anyhow::bail!("give either --window or --near"),
    };

    report::query(&db, &found, index.len(), json);
    Ok(())
}

fn parts(command: &PartsCommand, library: Option<&std::path::Path>, json: bool) -> Result<()> {
    let mut catalog = Catalog::bundled().context("loading the bundled catalogue")?;
    if let Some(dir) = library {
        catalog
            .load_dir(dir)
            .with_context(|| format!("loading parts from {}", dir.display()))?;
        catalog
            .validate()
            .context("validating the merged catalogue")?;
    }

    match command {
        PartsCommand::List { query } => {
            report::part_list(&catalog, query.as_deref().unwrap_or(""), json);
        }
        PartsCommand::Show { id, set } => {
            let overrides = mep::parse_set_overrides(set)?;
            let part = catalog
                .part(id)
                .with_context(|| format!("no part `{id}` in the catalogue"))?;
            let instance = catalog.instantiate(id, &overrides)?;
            report::part_detail(part, &instance, json);
        }
        PartsCommand::Systems => report::systems(&catalog, json),
        PartsCommand::Specs { id } => report::specs(&catalog, id.as_deref(), json),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Without `allow_hyphen_values`, clap reads a leading `-` in `--near`'s
    // own value as the start of a new (unknown) flag — a real bug an editing
    // canvas hits immediately, since half of everything left of the origin
    // has a negative X.
    #[test]
    fn near_and_window_accept_negative_coordinates() {
        let cli = Cli::try_parse_from([
            "od",
            "query",
            "plan.dxf",
            "--near",
            "-137.8,182.7",
            "--count",
            "1",
        ])
        .expect("a negative --near parses");
        assert!(matches!(
            cli.command,
            Command::Query { near: Some(n), .. } if n == "-137.8,182.7"
        ));

        let cli = Cli::try_parse_from([
            "od",
            "query",
            "plan.dxf",
            "--window",
            "-1000,-2000,1000,2000",
        ])
        .expect("a negative --window parses");
        assert!(matches!(
            cli.command,
            Command::Query { window: Some(w), .. } if w == "-1000,-2000,1000,2000"
        ));
    }
}
