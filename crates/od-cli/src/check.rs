//! Drawing standards checking (F-304).
//!
//! Two levels. `basic` checks structural integrity — the things that make a
//! drawing wrong rather than merely unconventional. `jp` adds the naming and
//! annotation conventions that Japanese practice and electronic delivery expect,
//! which are worth flagging but are not corruption.

use od_core::{Database, Geometry};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub rule: &'static str,
    pub severity: Severity,
    pub message: String,
    /// How many objects triggered it.
    pub count: usize,
}

pub fn run(db: &Database, japanese_conventions: bool) -> Vec<Finding> {
    let mut out = Vec::new();

    let problems = db.validate();
    if !problems.is_empty() {
        out.push(Finding {
            rule: "integrity",
            severity: Severity::Error,
            message: format!(
                "{} broken reference(s), e.g. {}",
                problems.len(),
                problems
                    .first()
                    .map(ToString::to_string)
                    .unwrap_or_default()
            ),
            count: problems.len(),
        });
    }

    let empty_layers = db
        .tables
        .layers
        .iter()
        .filter(|(id, _)| !db.entities().any(|(_, e)| e.layer == *id))
        .count();
    if empty_layers > 0 {
        out.push(Finding {
            rule: "empty-layers",
            severity: Severity::Info,
            message: format!("{empty_layers} layer(s) hold no entities"),
            count: empty_layers,
        });
    }

    let unsupported = db
        .entities()
        .filter(|(_, e)| matches!(e.geom, Geometry::Unsupported { .. }))
        .count();
    if unsupported > 0 {
        out.push(Finding {
            rule: "preserved-entities",
            severity: Severity::Warning,
            message: format!(
                "{unsupported} entity(ies) are preserved verbatim rather than understood; \
                 they will survive a save but cannot be edited"
            ),
            count: unsupported,
        });
    }

    // Zero-length geometry is invisible, unselectable and usually the residue of
    // a mis-click. It also breaks downstream offset and trim operations.
    let degenerate = db
        .entities()
        .filter(|(_, e)| match &e.geom {
            Geometry::Line { a, b } => a.coincides_with(*b),
            Geometry::Circle { radius, .. } | Geometry::Arc { radius, .. } => {
                od_core::tol::is_zero_len(*radius)
            }
            Geometry::Polyline { polyline, .. } => polyline.span_count() == 0,
            _ => false,
        })
        .count();
    if degenerate > 0 {
        out.push(Finding {
            rule: "degenerate-geometry",
            severity: Severity::Warning,
            message: format!("{degenerate} zero-length or zero-radius entity(ies)"),
            count: degenerate,
        });
    }

    let on_layer_zero = db
        .entities()
        .filter(|(_, e)| db.tables.layers.get(e.layer).is_some_and(|l| l.name == "0"))
        .count();
    if on_layer_zero > 0 {
        out.push(Finding {
            rule: "layer-zero",
            severity: Severity::Warning,
            message: format!(
                "{on_layer_zero} entity(ies) sit on layer 0, which is reserved for \
                 block definition contents"
            ),
            count: on_layer_zero,
        });
    }

    if japanese_conventions {
        // The electronic delivery requirements expect discipline-prefixed layer
        // names; a drawing full of "Layer1" will be rejected on submission.
        let unconventional: Vec<String> = db
            .tables
            .layers
            .iter()
            .map(|(_, l)| l.name.clone())
            .filter(|n| n != "0" && !looks_like_a_standard_layer(n))
            .collect();
        if !unconventional.is_empty() {
            out.push(Finding {
                rule: "layer-naming",
                severity: Severity::Warning,
                message: format!(
                    "{} layer(s) do not follow the discipline-prefixed convention, e.g. {}",
                    unconventional.len(),
                    unconventional
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                count: unconventional.len(),
            });
        }

        // Text below about 2 mm at plot scale is unreadable, and 2.5 mm is the
        // usual floor in Japanese drawing standards.
        let tiny_text = db
            .entities()
            .filter(|(_, e)| match &e.geom {
                Geometry::Text(t) => t.height < 2.0,
                Geometry::MText(t) => t.height < 2.0,
                _ => false,
            })
            .count();
        if tiny_text > 0 {
            out.push(Finding {
                rule: "text-height",
                severity: Severity::Warning,
                message: format!("{tiny_text} text entity(ies) are under 2 mm tall"),
                count: tiny_text,
            });
        }

        if db.tables.levels.is_empty() {
            out.push(Finding {
                rule: "levels",
                severity: Severity::Info,
                message: "no storeys are defined; MEP elements cannot be filed by floor".into(),
                count: 0,
            });
        }

        if db.tables.grids.is_empty() {
            out.push(Finding {
                rule: "grids",
                severity: Severity::Info,
                message: "no structural grid is defined; elements cannot be positioned \
                          relative to it, so they will not follow an architectural update"
                    .into(),
                count: 0,
            });
        }
    }

    out
}

/// Layer names in Japanese practice carry a discipline prefix (`A-`, `S-`, `M-`,
/// `E-`, `P-`, `F-`, `C-`, `D-`), as the SXF delivery requirements expect.
fn looks_like_a_standard_layer(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    matches!(
        first.to_ascii_uppercase(),
        'A' | 'S' | 'M' | 'E' | 'P' | 'F' | 'C' | 'D' | 'G'
    ) && chars.next() == Some('-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_core::{ActorId, Entity, Point3};

    fn db_with_line_on(layer_name: &str) -> Database {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.ensure_layer(layer_name);
        let space = db.model_space();
        db.insert_entity(Entity::new(
            layer,
            space,
            Geometry::Line {
                a: Point3::ORIGIN,
                b: Point3::new(1000.0, 0.0, 0.0),
            },
        ))
        .expect("inserts");
        db
    }

    fn has(findings: &[Finding], rule: &str) -> bool {
        findings.iter().any(|f| f.rule == rule)
    }

    #[test]
    fn a_clean_drawing_reports_no_errors() {
        let db = db_with_line_on("M-DUCT-SA");
        let findings = run(&db, false);
        assert!(!findings.iter().any(|f| f.severity == Severity::Error));
    }

    #[test]
    fn degenerate_geometry_is_flagged() {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.ensure_layer("M-TEST");
        let space = db.model_space();
        db.insert_entity(Entity::new(
            layer,
            space,
            Geometry::Line {
                a: Point3::ORIGIN,
                b: Point3::ORIGIN,
            },
        ))
        .expect("inserts");
        assert!(has(&run(&db, false), "degenerate-geometry"));
    }

    #[test]
    fn entities_on_layer_zero_are_flagged() {
        let db = db_with_line_on("0");
        assert!(has(&run(&db, false), "layer-zero"));
    }

    #[test]
    fn japanese_rules_only_apply_when_asked() {
        let db = db_with_line_on("Layer1");
        assert!(!has(&run(&db, false), "layer-naming"));
        assert!(has(&run(&db, true), "layer-naming"));
    }

    #[test]
    fn discipline_prefixed_layers_pass_the_naming_rule() {
        for name in ["M-DUCT-SA", "P-PIPE-CW", "E-POWR-LTG", "A-WALL"] {
            let db = db_with_line_on(name);
            assert!(
                !has(&run(&db, true), "layer-naming"),
                "{name} should be accepted"
            );
        }
        for name in ["Layer1", "レイヤ", "MDUCT"] {
            let db = db_with_line_on(name);
            assert!(
                has(&run(&db, true), "layer-naming"),
                "{name} should be flagged"
            );
        }
    }
}
