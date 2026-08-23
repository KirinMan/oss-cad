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
    /// Read a drawing, write it, read it back, and compare.
    Roundtrip {
        input: PathBuf,
        /// Format to round-trip through. Defaults to the input's own.
        #[arg(long, value_name = "FORMAT")]
        via: Option<String>,
    },
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
        Command::Roundtrip { input, via } => roundtrip(&input, via, cli.json),
        Command::Parts { command, library } => parts(&command, library.as_deref(), cli.json),
    }
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
