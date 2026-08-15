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
    /// Convert a drawing between formats.
    Convert {
        input: PathBuf,
        output: PathBuf,
        /// Fail instead of warning when entities are preserved rather than understood.
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
    /// Read a drawing, write it, read it back, and compare.
    Roundtrip { input: PathBuf },
    /// Browse the part catalogue.
    Parts {
        #[command(subcommand)]
        command: PartsCommand,
        /// Load additional part definitions from this directory.
        #[arg(long, global = true)]
        library: Option<PathBuf>,
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
        Command::Convert {
            input,
            output,
            strict,
        } => convert(&input, &output, strict, cli.json),
        Command::Inspect { input } => inspect(&input, cli.json),
        Command::Check { input, rules } => check_drawing(&input, rules, cli.json),
        Command::Roundtrip { input } => roundtrip(&input, cli.json),
        Command::Parts { command, library } => parts(&command, library.as_deref(), cli.json),
    }
}

fn load(path: &std::path::Path) -> Result<(od_core::Database, od_io_dxf::ReadOutcome)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    match ext.as_str() {
        "dxf" => od_io_dxf::read_file(path).with_context(|| format!("reading {}", path.display())),
        "dwg" => anyhow::bail!(
            "DWG is read through the separate `od-bridge-dwg` component, which is not \
             installed. Convert to DXF first, or see docs/05-interop-license.md."
        ),
        other => anyhow::bail!("unsupported input format `{other}` (supported: dxf)"),
    }
}

fn convert(
    input: &std::path::Path,
    output: &std::path::Path,
    strict: bool,
    json: bool,
) -> Result<()> {
    let (db, outcome) = load(input)?;

    if strict && !outcome.unsupported_types.is_empty() {
        anyhow::bail!(
            "--strict: {} entity type(s) were preserved but not understood: {}",
            outcome.unsupported_types.len(),
            outcome.unsupported_types.join(", ")
        );
    }

    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    match ext.as_str() {
        "dxf" => od_io_dxf::write_file(&db, output)
            .with_context(|| format!("writing {}", output.display()))?,
        "json" => {
            let text = serde_json::to_string_pretty(&db)?;
            std::fs::write(output, text)
                .with_context(|| format!("writing {}", output.display()))?;
        }
        other => anyhow::bail!("unsupported output format `{other}` (supported: dxf, json)"),
    }

    report::conversion(&db, &outcome, input, output, json);
    Ok(())
}

fn inspect(input: &std::path::Path, json: bool) -> Result<()> {
    let (db, outcome) = load(input)?;
    report::inspection(&db, &outcome, input, json);
    Ok(())
}

fn check_drawing(input: &std::path::Path, rules: RuleSet, json: bool) -> Result<()> {
    let (db, _) = load(input)?;
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

fn roundtrip(input: &std::path::Path, json: bool) -> Result<()> {
    let (first, _) = load(input)?;
    let written = od_io_dxf::write_string(&first);
    let (second, outcome) = od_io_dxf::read_str(&written).context("re-reading our own output")?;
    let diff = report::roundtrip(&first, &second, &outcome, input, json);
    if diff {
        std::process::exit(1);
    }
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
            let mut overrides = std::collections::HashMap::new();
            for pair in set {
                let (name, value) = pair
                    .split_once('=')
                    .with_context(|| format!("`{pair}` should look like NAME=VALUE"))?;
                let v: f64 = value
                    .trim()
                    .parse()
                    .with_context(|| format!("`{value}` is not a number"))?;
                overrides.insert(name.trim().to_owned(), v);
            }
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
