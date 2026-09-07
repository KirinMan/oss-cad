//! Opening and saving drawings, whatever the format.
//!
//! Every reader reports different things — DXF has entity types it could not
//! model, `.odc` has container features it did not recognise — so the CLI
//! normalises them into one shape. A user asking "what did you not understand
//! about my file?" should get one answer, not one per format.

use anyhow::{Context, Result};
use od_core::Database;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub context: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct LoadOutcome {
    /// The format the drawing came from, for the report.
    pub format: &'static str,
    pub warnings: Vec<Warning>,
    /// Entity types kept byte-for-byte because this build does not model them.
    /// Derived from the loaded document rather than from the reader, so it is
    /// the same answer whichever format the drawing arrived in.
    pub unsupported_entities: Vec<String>,
    /// Container-level features from a newer build, kept but not understood.
    pub unsupported_features: Vec<String>,
}

/// Formats that can be read.
pub const READABLE: &[&str] = &["dxf", "odc", "sfc"];
/// Formats that can be written.
pub const WRITABLE: &[&str] = &["odc", "dxf", "sfc", "json"];

pub fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase()
}

pub fn load(path: &Path) -> Result<(Database, LoadOutcome)> {
    match extension_of(path).as_str() {
        "dxf" => {
            let (db, outcome) = od_io_dxf::read_file(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let unsupported_entities = unsupported_entity_types(&db);
            Ok((
                db,
                LoadOutcome {
                    format: "dxf",
                    warnings: outcome
                        .warnings
                        .into_iter()
                        .map(|w| Warning {
                            context: w.context,
                            message: w.message,
                        })
                        .collect(),
                    unsupported_entities,
                    unsupported_features: Vec::new(),
                },
            ))
        }

        "odc" => {
            let (db, outcome) = od_io_odc::read_file(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let mut features = outcome.preserved_features;
            features.extend(outcome.undeclared_entries);
            let unsupported_entities = unsupported_entity_types(&db);
            Ok((
                db,
                LoadOutcome {
                    format: "odc",
                    warnings: outcome
                        .warnings
                        .into_iter()
                        .map(|message| Warning {
                            context: "document".into(),
                            message,
                        })
                        .collect(),
                    unsupported_entities,
                    unsupported_features: features,
                },
            ))
        }

        "sfc" => {
            let (db, outcome) = od_io_sxf::read_file(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let unsupported_entities = unsupported_entity_types(&db);
            Ok((
                db,
                LoadOutcome {
                    format: "sfc",
                    warnings: outcome
                        .warnings
                        .into_iter()
                        .map(|w| Warning {
                            context: w.context,
                            message: w.message,
                        })
                        .collect(),
                    unsupported_entities,
                    unsupported_features: Vec::new(),
                },
            ))
        }

        "dwg" => anyhow::bail!(
            "DWG is read through the separate `od-bridge-dwg` component, which is not \
             installed. Convert to DXF first, or see docs/05-interop-license.md."
        ),

        other => anyhow::bail!(
            "cannot read `{other}` files (supported: {})",
            READABLE.join(", ")
        ),
    }
}

/// The distinct source types of entities this build preserved but did not
/// model, in the order they first appear.
fn unsupported_entity_types(db: &Database) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, entity) in db.entities() {
        if let od_core::Geometry::Unsupported { source_type, .. } = &entity.geom
            && !out.iter().any(|t| t == source_type)
        {
            out.push(source_type.clone());
        }
    }
    out
}

pub fn save(db: &Database, path: &Path) -> Result<()> {
    match extension_of(path).as_str() {
        // The native format, and the only one that holds the whole document —
        // extension data, schemas, storeys, grids, and anything preserved from
        // an earlier read.
        "odc" => od_io_odc::write_file(db, path)
            .with_context(|| format!("writing {}", path.display()))?,

        "dxf" => od_io_dxf::write_file(db, path)
            .with_context(|| format!("writing {}", path.display()))?,

        "sfc" => od_io_sxf::write_file(db, path)
            .with_context(|| format!("writing {}", path.display()))?,

        // Not a format so much as a window into the model, for debugging and
        // for tools that would rather not learn a container.
        "json" => {
            let text = serde_json::to_string_pretty(db)?;
            std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
        }

        other => anyhow::bail!(
            "cannot write `{other}` files (supported: {})",
            WRITABLE.join(", ")
        ),
    }
    Ok(())
}

/// What a conversion to `target` will cost, if anything.
///
/// Said before the write rather than after, because "your storey and grid
/// definitions did not fit in the file you just saved" is not useful once the
/// original is closed.
#[must_use]
pub fn conversion_losses(db: &Database, target: &str) -> Vec<String> {
    match target {
        "dxf" => dxf_conversion_losses(db),
        "sfc" => sfc_conversion_losses(db),
        _ => Vec::new(),
    }
}

fn count_kind(db: &Database, matches: impl Fn(&od_core::Geometry) -> bool) -> usize {
    db.entities().filter(|(_, e)| matches(&e.geom)).count()
}

fn dxf_conversion_losses(db: &Database) -> Vec<String> {
    let mut losses = Vec::new();
    if !db.schemas.is_empty() {
        losses.push(format!(
            "{} attribute schema(s) — values are written as XDATA but lose their \
             declared types, units and IFC mappings",
            db.schemas.len()
        ));
    }
    if !db.tables.levels.is_empty() {
        losses.push(format!("{} storey definition(s)", db.tables.levels.len()));
    }
    if !db.tables.grids.is_empty() {
        losses.push(format!("{} grid axis definition(s)", db.tables.grids.len()));
    }
    let custom = db
        .objects()
        .filter(|(_, o)| matches!(o.kind, od_core::ObjectKind::Custom { .. }))
        .count();
    if custom > 0 {
        losses.push(format!("{custom} domain object(s)"));
    }
    let dimensions = count_kind(db, |g| matches!(g, od_core::Geometry::Dimension(_)));
    if dimensions > 0 {
        losses.push(format!(
            "{dimensions} dimension(s) — DXF's DIMENSION entity is not yet written"
        ));
    }
    let viewports = count_kind(db, |g| matches!(g, od_core::Geometry::Viewport(_)));
    if viewports > 0 {
        losses.push(format!(
            "{viewports} viewport(s) — DXF's LAYOUT/VPORT objects are not yet written"
        ));
    }
    let solids = count_kind(db, |g| matches!(g, od_core::Geometry::Solid3d { .. }));
    if solids > 0 {
        losses.push(format!(
            "{solids} solid(s) — DXF's 3DSOLID entity is not yet written"
        ));
    }
    // od-io-dxf only ever writes model space (the ENTITIES section) and
    // ordinary block definitions (the BLOCKS section) — an entity owned by
    // a paper-space layout has nowhere in the file to go at all, unlike
    // Dimension/Viewport (specific geometry kinds it always skips): this is
    // any geometry kind, lost purely because of *where* it was drawn.
    let paper_space_entities = db
        .entities()
        .filter(|(_, e)| {
            db.tables
                .blocks
                .get(e.owner_space)
                .is_some_and(|b| b.kind == od_core::BlockKind::PaperSpace)
        })
        .count();
    if paper_space_entities > 0 {
        losses.push(format!(
            "{paper_space_entities} entities on a paper-space layout — DXF export only writes \
             model space"
        ));
    }
    losses
}

/// `od-io-sxf` writes model-space `Point`/`Line`/`Circle`/`Arc`/`Ellipse`/
/// `Polyline`/`Text` only (its own crate docs) — everything else in this
/// list is reported here rather than silently dropped, including things
/// that DXF itself carries losslessly (schemas, storeys, grids, blocks):
/// SFC's feature set has no attribute mechanism and no symbol-table
/// equivalent for any of them.
fn sfc_conversion_losses(db: &Database) -> Vec<String> {
    let mut losses = Vec::new();
    if !db.schemas.is_empty() {
        losses.push(format!(
            "{} attribute schema(s) — SFC has no attribute mechanism to carry them",
            db.schemas.len()
        ));
    }
    if !db.tables.levels.is_empty() {
        losses.push(format!("{} storey definition(s)", db.tables.levels.len()));
    }
    if !db.tables.grids.is_empty() {
        losses.push(format!("{} grid axis definition(s)", db.tables.grids.len()));
    }
    let custom = db
        .objects()
        .filter(|(_, o)| matches!(o.kind, od_core::ObjectKind::Custom { .. }))
        .count();
    if custom > 0 {
        losses.push(format!("{custom} domain object(s)"));
    }
    let unmodeled = count_kind(db, |g| {
        !matches!(
            g,
            od_core::Geometry::Point(_)
                | od_core::Geometry::Line { .. }
                | od_core::Geometry::Circle { .. }
                | od_core::Geometry::Arc { .. }
                | od_core::Geometry::Ellipse { .. }
                | od_core::Geometry::Polyline { .. }
                | od_core::Geometry::Text(_)
        )
    });
    if unmodeled > 0 {
        losses.push(format!(
            "{unmodeled} entit{} of a kind od-io-sxf does not write (dimensions, blocks, \
             viewports, hatches, splines, solids, and anything already unsupported)",
            if unmodeled == 1 { "y" } else { "ies" }
        ));
    }
    losses
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_core::{ActorId, GridAxis, Level, Point2, Vec2};

    #[test]
    fn dxf_conversion_reports_what_it_cannot_carry() {
        let mut db = Database::new(ActorId::SYSTEM);
        let id = db.reserve_id();
        db.tables.levels.insert(
            id,
            Level {
                name: "3FL".into(),
                elevation: 8400.0,
                height: None,
                order: 3,
            },
        );
        let id = db.reserve_id();
        db.tables.grids.insert(
            id,
            GridAxis {
                name: "X1".into(),
                origin: Point2::ORIGIN,
                direction: Vec2::Y,
                family: "X".into(),
            },
        );

        let losses = conversion_losses(&db, "dxf");
        assert_eq!(losses.len(), 2);
        assert!(losses.iter().any(|l| l.contains("storey")));
        assert!(losses.iter().any(|l| l.contains("grid")));

        assert!(
            conversion_losses(&db, "odc").is_empty(),
            "the native format loses nothing"
        );
    }

    #[test]
    fn dxf_conversion_reports_a_viewport_it_cannot_write() {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.ensure_layer("0");
        let paper_space = db
            .tables
            .blocks
            .id_of(od_core::PAPER_SPACE)
            .expect("Database::new always seeds a default paper space");
        db.insert_entity(od_core::Entity::new(
            layer,
            paper_space,
            od_core::Geometry::Viewport(Box::new(od_core::ViewportEntity {
                position: od_core::Point3::ORIGIN,
                width: 200.0,
                height: 150.0,
                target: od_core::Point3::ORIGIN,
                scale: 1.0,
            })),
        ))
        .expect("inserts");

        let losses = conversion_losses(&db, "dxf");
        assert!(losses.iter().any(|l| l.contains("viewport")));
    }

    #[test]
    fn sfc_conversion_carries_a_plain_line_losslessly() {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.ensure_layer("0");
        let space = db.model_space();
        db.insert_entity(od_core::Entity::new(
            layer,
            space,
            od_core::Geometry::Line {
                a: od_core::Point3::ORIGIN,
                b: od_core::Point3::new(100.0, 0.0, 0.0),
            },
        ))
        .expect("inserts");

        assert!(
            conversion_losses(&db, "sfc").is_empty(),
            "od-io-sxf models plain lines"
        );
    }

    #[test]
    fn sfc_conversion_reports_a_dimension_it_cannot_write() {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.ensure_layer("0");
        let space = db.model_space();
        let style = db
            .tables
            .dim_styles
            .id_of("standard")
            .expect("Database::new always seeds a standard dim style");
        db.insert_entity(od_core::Entity::new(
            layer,
            space,
            od_core::Geometry::Dimension(Box::new(od_core::DimensionEntity {
                point_a: od_core::Point3::ORIGIN,
                point_b: od_core::Point3::new(1000.0, 0.0, 0.0),
                offset: 50.0,
                text_override: None,
                style,
            })),
        ))
        .expect("inserts");

        let losses = conversion_losses(&db, "sfc");
        assert!(
            losses
                .iter()
                .any(|l| l.contains("od-io-sxf does not write"))
        );
    }

    #[test]
    fn dxf_conversion_reports_a_solid3d_it_cannot_write() {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.ensure_layer("0");
        let space = db.model_space();
        db.insert_entity(od_core::Entity::new(
            layer,
            space,
            od_core::Geometry::Solid3d {
                handle: od_core::SolidHandle(1),
                bounds: od_core::Aabb3::from_points([od_core::Point3::ORIGIN]),
            },
        ))
        .expect("inserts");

        // write.rs's Solid3d arm says "skipped ... and reported by the
        // caller" (crates/od-io-dxf/src/write.rs) -- but no caller does:
        // dxf_conversion_losses has no Solid3d check at all, so a solid is
        // dropped from the DXF file with *no* warning of any kind, unlike
        // Dimension/Viewport which are explicitly counted above.
        let losses = conversion_losses(&db, "dxf");
        assert!(
            losses.iter().any(|l| l.contains("solid")),
            "a Solid3d entity vanishes from DXF output with no loss reported at all: {losses:?}"
        );
    }

    #[test]
    fn dxf_conversion_reports_entities_on_a_paper_space_layout() {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.ensure_layer("0");
        let paper_space = db
            .tables
            .blocks
            .id_of(od_core::PAPER_SPACE)
            .expect("Database::new always seeds a default paper space");
        db.insert_entity(od_core::Entity::new(
            layer,
            paper_space,
            od_core::Geometry::Line {
                a: od_core::Point3::ORIGIN,
                b: od_core::Point3::new(100.0, 0.0, 0.0),
            },
        ))
        .expect("inserts");

        // od_io_dxf::write only ever writes model space (ENTITIES) and
        // ordinary block definitions (BLOCKS) -- an entity owned by a
        // paper-space layout has nowhere in the file to go, unlike
        // Dimension/Viewport (skipped by geometry kind, not by space).
        let losses = conversion_losses(&db, "dxf");
        assert!(
            losses.iter().any(|l| l.contains("paper-space")),
            "an ordinary line on a paper-space layout vanishes with no loss reported: {losses:?}"
        );
    }

    #[test]
    fn extensions_are_matched_case_insensitively() {
        assert_eq!(extension_of(Path::new("Plan.DXF")), "dxf");
        assert_eq!(extension_of(Path::new("plan.odc")), "odc");
        assert_eq!(extension_of(Path::new("plan")), "");
    }
}
