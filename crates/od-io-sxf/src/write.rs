//! SFC writer.
//!
//! Only model space is written, and only the geometry kinds this crate
//! actually models (see crate docs): `Geometry::Unsupported` and anything
//! else this build cannot represent in SXF is skipped, not replayed —
//! replaying an opaque payload captured from a *different* source format
//! (DXF's own `Unsupported` entities, say) back out as if it were SFC text
//! would risk writing garbage rather than honestly dropping it. A caller
//! that needs to know what did not make it through reports that itself
//! (`od-cli`'s `conversion_losses`, the same mechanism DXF's own writer
//! leans on for `Dimension`/`Viewport`).

use crate::table;
use od_core::{Color, Database, Entity, Geometry, LineWeight, ObjectId};
use std::collections::HashMap;
use std::fmt::Write as _;

/// Writes an SFC document as a string.
#[must_use]
pub fn write_string(db: &Database) -> String {
    let mut out = String::with_capacity(4 * 1024);
    write_header(&mut out);
    out.push_str("DATA;\n");

    let mut number = 10u64;
    let tables = Tables::build(db);
    tables.write_all(&mut out, &mut number);

    for (id, entity) in db.entities_in(db.model_space()) {
        write_shape(&mut out, &mut number, db, &tables, id, entity);
    }

    write_feature(&mut out, &mut number, &default_sheet());

    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    out
}

fn write_header(out: &mut String) {
    out.push_str("ISO-10303-21;\n");
    out.push_str("HEADER;\n");
    out.push_str("FILE_DESCRIPTION(('SCADEC level2 feature_mode'),\n'2;1');\n");
    out.push_str(
        "FILE_NAME('',\n'2000-01-01T00:00:00',\n('' ),\n('' ),\n'OpenDraft',\n'od-io-sxf',\n'');\n",
    );
    out.push_str("FILE_SCHEMA(('ASSOCIATIVE_DRAUGHTING'));\n");
    out.push_str("ENDSEC;\n\n");
}

fn write_feature(out: &mut String, number: &mut u64, body: &str) {
    out.push_str("\n/*SXF\n#");
    let _ = write!(out, "{number}");
    out.push_str(" = ");
    out.push_str(body);
    out.push_str("\nSXF*/\n");
    *number += 10;
}

// ── Parameter formatting ────────────────────────────────────────────────

fn num(v: f64) -> String {
    format!("'{v}'")
}

fn int(v: i64) -> String {
    format!("'{v}'")
}

fn text(v: &str) -> String {
    format!("\\'{v}\\'")
}

fn list(v: &[f64]) -> String {
    let inner = v
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("'({inner})'")
}

fn feature(name: &str, params: &[String]) -> String {
    format!("{name}({})", params.join(","))
}

fn default_sheet() -> String {
    // No `od_core` field carries a sheet size, so every write emits the
    // same fixed A3-landscape default rather than tracking it — a
    // documented, permanent loss for this format (crate docs).
    feature(
        "drawing_sheet_feature",
        &[text(""), int(3), int(1), int(420), int(297)],
    )
}

// ── Tables ───────────────────────────────────────────────────────────────

struct Tables {
    layers: HashMap<ObjectId, u32>,
    colors: HashMap<(u8, u8, u8), u32>,
    linetypes: HashMap<ObjectId, u32>,
    widths: HashMap<i64, u32>,
    fonts: HashMap<ObjectId, u32>,
    records: Vec<String>,
}

impl Tables {
    fn build(db: &Database) -> Self {
        let mut t = Self {
            layers: HashMap::new(),
            colors: HashMap::new(),
            linetypes: HashMap::new(),
            widths: HashMap::new(),
            fonts: HashMap::new(),
            records: Vec::new(),
        };

        // Colours and widths are collected from what is actually used —
        // SXF has no unused-symbol concept, unlike a DXF/`.odc` table.
        for (_, entity) in db.entities_in(db.model_space()) {
            let (color, width_mm, _) = resolve_style(db, entity);
            t.color_code(color);
            t.width_code(width_mm);
        }

        for (id, lt) in db.tables.linetypes.iter() {
            t.linetype_code(db, id, lt);
        }
        for (id, style) in db.tables.text_styles.iter() {
            t.font_code(id, style);
        }
        for (id, layer) in db.tables.layers.iter() {
            t.layer_code(id, layer);
        }

        t
    }

    fn color_code(&mut self, c: Color) -> u32 {
        let rgb = resolve_rgb(c);
        if let Some(&code) = self.colors.get(&rgb) {
            return code;
        }
        let code = if let Some(i) = table::PREDEFINED_COLORS
            .iter()
            .position(|&(_, r, g, b)| (r, g, b) == rgb)
        {
            u32::try_from(i).unwrap_or(0) + 1
        } else {
            let code = table::FIRST_USER_COLOR + u32::try_from(self.colors.len()).unwrap_or(0);
            let (r, g, b) = rgb;
            self.records.push(write_call(
                "user_defined_colour_feature",
                &[int(i64::from(r)), int(i64::from(g)), int(i64::from(b))],
            ));
            code
        };
        if table::PREDEFINED_COLORS
            .iter()
            .any(|&(_, r, g, b)| (r, g, b) == rgb)
        {
            let name = table::PREDEFINED_COLORS[(code - 1) as usize].0;
            self.records
                .push(write_call("pre_defined_colour_feature", &[text(name)]));
        }
        self.colors.insert(rgb, code);
        code
    }

    fn width_code(&mut self, mm: f64) -> u32 {
        let key = mm_to_hundredths(mm);
        if let Some(&code) = self.widths.get(&key) {
            return code;
        }
        let code = if let Some(i) = table::PREDEFINED_WIDTHS
            .iter()
            .position(|&w| mm_to_hundredths(w) == key)
        {
            u32::try_from(i).unwrap_or(0) + 1
        } else {
            table::FIRST_USER_WIDTH + u32::try_from(self.widths.len()).unwrap_or(0)
        };
        self.records.push(write_call("width_feature", &[num(mm)]));
        self.widths.insert(key, code);
        code
    }

    fn linetype_code(&mut self, db: &Database, id: ObjectId, lt: &od_core::LineType) {
        let predefined = table::PREDEFINED_LINETYPES
            .iter()
            .position(|(n, _)| n.eq_ignore_ascii_case(&lt.name));
        let code = if let Some(i) = predefined {
            let (name, _) = table::PREDEFINED_LINETYPES[i];
            self.records
                .push(write_call("pre_defined_font_feature", &[text(name)]));
            u32::try_from(i).unwrap_or(0) + 1
        } else {
            let code =
                table::FIRST_USER_LINETYPE + u32::try_from(self.linetypes.len()).unwrap_or(0);
            let pitch = table::from_od_pattern(&lt.pattern);
            self.records.push(write_call(
                "user_defined_font_feature",
                &[
                    text(&lt.name),
                    int(i64::try_from(pitch.len()).unwrap_or(0)),
                    list(&pitch),
                ],
            ));
            code
        };
        let _ = db;
        self.linetypes.insert(id, code);
    }

    fn font_code(&mut self, id: ObjectId, style: &od_core::TextStyle) {
        let code = u32::try_from(self.fonts.len()).unwrap_or(0) + 1;
        self.records
            .push(write_call("text_font_feature", &[text(&style.name)]));
        self.fonts.insert(id, code);
    }

    fn layer_code(&mut self, id: ObjectId, layer: &od_core::Layer) {
        let code = u32::try_from(self.layers.len()).unwrap_or(0) + 1;
        self.records.push(write_call(
            "layer_feature",
            &[text(&layer.name), int(i64::from(layer.visible))],
        ));
        self.layers.insert(id, code);
    }

    fn write_all(&self, out: &mut String, number: &mut u64) {
        for r in &self.records {
            write_feature(out, number, r);
        }
    }
}

fn write_call(name: &str, params: &[String]) -> String {
    feature(name, params)
}

/// Drawing coordinates stay within `od_geom2d::tol`'s own documented
/// ±10^9 mm range, so a width nowhere near that is malformed input, not a
/// value this needs to represent exactly — clamped rather than panicking.
const MAX_HUNDREDTHS_MM: f64 = 1e11;

/// No string in a drawing has anywhere near `u32::MAX` characters, so the
/// fallback never actually triggers — going through `u32` just makes that a
/// checked fact rather than an assumption (the same pattern `od-solver`'s
/// `as_f64` uses).
fn char_count_f64(s: &str) -> f64 {
    u32::try_from(s.chars().count()).map_or(f64::MAX, f64::from)
}

fn mm_to_hundredths(mm: f64) -> i64 {
    let hundredths = (mm * 100.0).round();
    if hundredths >= MAX_HUNDREDTHS_MM {
        10_000_000_000
    } else if hundredths <= -MAX_HUNDREDTHS_MM {
        -10_000_000_000
    } else {
        // Range-checked above, so this narrowing cannot truncate.
        #[allow(clippy::cast_possible_truncation)]
        {
            hundredths as i64
        }
    }
}

/// Resolves an entity's colour/width(mm)/linetype id, following ByLayer
/// against its own layer — SXF shapes carry style directly, with no
/// ByLayer/ByBlock indirection of their own to preserve.
fn resolve_style(db: &Database, entity: &Entity) -> (Color, f64, Option<ObjectId>) {
    let layer = db.tables.layers.get(entity.layer);
    let color = match entity.style.color {
        Color::ByLayer | Color::ByBlock => layer.map_or(Color::FOREGROUND, |l| l.color),
        c => c,
    };
    let mm = match entity.style.lineweight {
        LineWeight::Hundredths(h) => f64::from(h) / 100.0,
        _ => layer
            .and_then(|l| l.lineweight.millimetres())
            .unwrap_or(0.13),
    };
    let linetype = entity
        .style
        .linetype
        .or_else(|| layer.and_then(|l| l.linetype));
    (color, mm, linetype)
}

/// `od_core::Color::Index` beyond the seven named ACI constants has no
/// table this crate carries (a full 256-entry AutoCAD Color Index would be
/// its own, separate undertaking) — approximated as mid-grey, a documented,
/// narrow simplification rather than an attempt at the full ACI table.
fn resolve_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb { r, g, b } => (r, g, b),
        Color::Index(1) => (255, 0, 0),
        Color::Index(2) => (255, 255, 0),
        Color::Index(3) => (0, 255, 0),
        Color::Index(4) => (0, 255, 255),
        Color::Index(5) => (0, 0, 255),
        Color::Index(6) => (255, 0, 255),
        Color::Index(7) => (255, 255, 255),
        Color::Index(_) | Color::ByLayer | Color::ByBlock => (128, 128, 128),
    }
}

// ── Geometry ─────────────────────────────────────────────────────────────

fn write_shape(
    out: &mut String,
    number: &mut u64,
    db: &Database,
    t: &Tables,
    id: ObjectId,
    entity: &Entity,
) {
    let Some(&layer_code) = t.layers.get(&entity.layer) else {
        return;
    };
    let (color, width_mm, linetype_id) = resolve_style(db, entity);
    let color_code = *t.colors.get(&resolve_rgb(color)).unwrap_or(&1);
    let width_code = *t.widths.get(&mm_to_hundredths(width_mm)).unwrap_or(&1);
    let linetype_code = linetype_id
        .and_then(|id| t.linetypes.get(&id))
        .copied()
        .unwrap_or(1);

    let body = match &entity.geom {
        Geometry::Line { a, b } => Some(feature(
            "line_feature",
            &[
                int(i64::from(layer_code)),
                int(i64::from(color_code)),
                int(i64::from(linetype_code)),
                int(i64::from(width_code)),
                num(a.x),
                num(a.y),
                num(b.x),
                num(b.y),
            ],
        )),
        Geometry::Circle { center, radius, .. } => Some(feature(
            "circle_feature",
            &[
                int(i64::from(layer_code)),
                int(i64::from(color_code)),
                int(i64::from(linetype_code)),
                int(i64::from(width_code)),
                num(center.x),
                num(center.y),
                num(*radius),
            ],
        )),
        Geometry::Arc {
            center,
            radius,
            start_angle,
            sweep,
            ..
        } => {
            let start = start_angle.to_degrees();
            let end = (start_angle + sweep).to_degrees();
            Some(feature(
                "arc_feature",
                &[
                    int(i64::from(layer_code)),
                    int(i64::from(color_code)),
                    int(i64::from(linetype_code)),
                    int(i64::from(width_code)),
                    num(center.x),
                    num(center.y),
                    num(*radius),
                    int(0),
                    num(start),
                    num(end),
                ],
            ))
        }
        Geometry::Ellipse {
            center,
            major_axis,
            ratio,
            start_param,
            end_param,
            ..
        } => {
            let rx = major_axis.length();
            let ry = rx * ratio;
            let rot = major_axis.y.atan2(major_axis.x).to_degrees();
            let full = (end_param - start_param - std::f64::consts::TAU).abs() < 1e-6;
            if full {
                Some(feature(
                    "ellipse_feature",
                    &[
                        int(i64::from(layer_code)),
                        int(i64::from(color_code)),
                        int(i64::from(linetype_code)),
                        int(i64::from(width_code)),
                        num(center.x),
                        num(center.y),
                        num(rx),
                        num(ry),
                        num(rot),
                    ],
                ))
            } else {
                Some(feature(
                    "ellipse_arc_feature",
                    &[
                        int(i64::from(layer_code)),
                        int(i64::from(color_code)),
                        int(i64::from(linetype_code)),
                        int(i64::from(width_code)),
                        num(center.x),
                        num(center.y),
                        num(rx),
                        num(ry),
                        int(0),
                        num(rot),
                        num(start_param.to_degrees()),
                        num(end_param.to_degrees()),
                    ],
                ))
            }
        }
        Geometry::Polyline { polyline, .. } => {
            if polyline.vertices.len() < 2 {
                None
            } else {
                let mut xs: Vec<f64> = polyline.vertices.iter().map(|v| v.point.x).collect();
                let mut ys: Vec<f64> = polyline.vertices.iter().map(|v| v.point.y).collect();
                if polyline.closed {
                    xs.push(xs[0]);
                    ys.push(ys[0]);
                }
                let n = xs.len();
                Some(feature(
                    "polyline_feature",
                    &[
                        int(i64::from(layer_code)),
                        int(i64::from(color_code)),
                        int(i64::from(linetype_code)),
                        int(i64::from(width_code)),
                        int(i64::try_from(n).unwrap_or(0)),
                        list(&xs),
                        list(&ys),
                    ],
                ))
            }
        }
        Geometry::Point(p) => Some(feature(
            "point_marker_feature",
            &[
                int(i64::from(layer_code)),
                int(i64::from(color_code)),
                num(p.x),
                num(p.y),
                int(1),
                num(0.0),
                num(1.0),
            ],
        )),
        Geometry::Text(text_entity) => {
            let font_code = db
                .tables
                .text_styles
                .get(text_entity.style)
                .and_then(|_| t.fonts.get(&text_entity.style))
                .copied()
                .unwrap_or(1);
            let bpnt = bpnt_for(text_entity.h_align, text_entity.v_align);
            let direct = if matches!(text_entity.flow, od_core::TextFlow::Vertical) {
                2
            } else {
                1
            };
            Some(feature(
                "text_string_feature",
                &[
                    int(i64::from(layer_code)),
                    int(i64::from(color_code)),
                    int(i64::from(font_code)),
                    text(&text_entity.value),
                    num(text_entity.position.x),
                    num(text_entity.position.y),
                    num(text_entity.height),
                    num(text_entity.height * char_count_f64(&text_entity.value)),
                    num(0.0),
                    num(text_entity.rotation.to_degrees()),
                    num(text_entity.oblique.to_degrees()),
                    int(bpnt),
                    int(direct),
                ],
            ))
        }
        _ => None,
    };

    if let Some(body) = body {
        write_feature(out, number, &body);
    }
    let _ = id;
}

/// SXF's 9-way anchor grid has no "stretched to fit" concept
/// (`HAlign::Fit`) — approximated as centred, the nearest reading.
fn bpnt_for(h: od_core::HAlign, v: od_core::VAlign) -> i64 {
    use od_core::{HAlign, VAlign};
    let col = match h {
        HAlign::Left => 0,
        HAlign::Center | HAlign::Fit => 1,
        HAlign::Right => 2,
    };
    let row = match v {
        VAlign::Bottom | VAlign::Baseline => 0,
        VAlign::Middle => 1,
        VAlign::Top => 2,
    };
    1 + row * 3 + col
}
