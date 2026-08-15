//! DXF reader.
//!
//! Two rules govern everything here.
//!
//! 1. **Never drop what you cannot read.** An unrecognised entity becomes
//!    [`Geometry::Unsupported`] carrying its original group pairs plus a proxy
//!    outline, so it survives a load-and-save cycle and still draws. An
//!    unrecognised section is kept as a [`PreservedBlob`].
//! 2. **Never fail the whole file for one bad entity.** A malformed entity is
//!    recorded as a warning and skipped. A drawing that opens with a note about
//!    three broken entities is worth more than an error message.

use crate::pair::Pair;
use crate::{DxfError, Result};
use od_core::{
    ActorId, BlockRef, Color, Database, Entity, Geometry, GraphicStyle, LineWeight, MTextEntity,
    ObjectId, Point3, Polyline2, PreservedBlob, ProxyGraphic, TextEntity, Units, Vec3,
};

/// A problem that did not stop the read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub context: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct ReadOutcome {
    pub warnings: Vec<Warning>,
    /// Entity types kept verbatim because this build does not model them.
    pub unsupported_types: Vec<String>,
}

/// Reads an ASCII DXF document.
pub fn read_str(text: &str) -> Result<(Database, ReadOutcome)> {
    let pairs = lex(text)?;
    let mut db = Database::new(ActorId::IMPORT);
    let mut outcome = ReadOutcome::default();
    let mut i = 0usize;

    while i < pairs.len() {
        let p = &pairs[i];
        if p.code == 0 && p.value == "EOF" {
            break;
        }
        if p.code == 0 && p.value == "SECTION" {
            let name = pairs
                .get(i + 1)
                .filter(|n| n.code == 2)
                .map(|n| n.value.clone())
                .unwrap_or_default();
            let end = find_endsec(&pairs, i + 2);
            let body = &pairs[i + 2..end];
            match name.as_str() {
                "HEADER" => read_header(&mut db, body),
                "TABLES" => read_tables(&mut db, body, &mut outcome),
                "BLOCKS" => read_blocks(&mut db, body, &mut outcome),
                "ENTITIES" => read_entities(&mut db, body, None, &mut outcome),
                // CLASSES, OBJECTS, THUMBNAILIMAGE and anything else: preserved
                // verbatim so a save can put them back.
                other => db.preserved.push(PreservedBlob {
                    source: "dxf".into(),
                    section: other.to_owned(),
                    payload: serialize_pairs(body).into_bytes(),
                }),
            }
            i = end + 1;
            continue;
        }
        i += 1;
    }

    db.rehydrate();
    Ok((db, outcome))
}

/// Splits the raw text into group pairs.
fn lex(text: &str) -> Result<Vec<Pair>> {
    let mut out = Vec::new();
    let mut lines = text.lines().enumerate();
    while let Some((n, code_line)) = lines.next() {
        let code_text = code_line.trim();
        if code_text.is_empty() {
            continue;
        }
        let code: i32 = code_text.parse().map_err(|_| DxfError::BadGroupCode {
            line: n + 1,
            text: code_text.to_owned(),
        })?;
        let Some((_, value_line)) = lines.next() else {
            return Err(DxfError::TruncatedPair { line: n + 1 });
        };
        // Only the trailing newline is stripped: leading spaces inside a text
        // value are part of the drawing's content.
        out.push(Pair::new(code, value_line.trim_end_matches(['\r', '\n'])));
    }
    Ok(out)
}

fn find_endsec(pairs: &[Pair], from: usize) -> usize {
    pairs
        .iter()
        .enumerate()
        .skip(from)
        .find(|(_, p)| p.code == 0 && p.value == "ENDSEC")
        .map_or(pairs.len(), |(i, _)| i)
}

fn serialize_pairs(pairs: &[Pair]) -> String {
    let mut s = String::new();
    for p in pairs {
        s.push_str(&p.code.to_string());
        s.push('\n');
        s.push_str(&p.value);
        s.push('\n');
    }
    s
}

fn read_header(db: &mut Database, body: &[Pair]) {
    let mut i = 0;
    while i < body.len() {
        if body[i].code != 9 {
            i += 1;
            continue;
        }
        let var = body[i].value.clone();
        let value = body.get(i + 1);
        match (var.as_str(), value) {
            ("$INSUNITS", Some(v)) => {
                if let Some(code) = v.as_i64() {
                    db.header.units = units_from_dxf(code);
                }
            }
            ("$LUPREC", Some(v)) => {
                if let Some(n) = v.as_i64() {
                    db.header.linear_precision = n.clamp(0, 8) as u8;
                }
            }
            ("$AUPREC", Some(v)) => {
                if let Some(n) = v.as_i64() {
                    db.header.angular_precision = n.clamp(0, 8) as u8;
                }
            }
            ("$ANGBASE", Some(v)) => {
                if let Some(a) = v.as_f64() {
                    db.header.angle_base = a.to_radians();
                }
            }
            ("$ANGDIR", Some(v)) => db.header.angle_clockwise = v.as_i64() == Some(1),
            _ => {}
        }
        i += 1;
    }
}

fn units_from_dxf(code: i64) -> Units {
    match code {
        1 => Units::Inches,
        2 => Units::Feet,
        5 => Units::Centimeters,
        6 => Units::Meters,
        4 => Units::Millimeters,
        _ => Units::Unitless,
    }
}

fn read_tables(db: &mut Database, body: &[Pair], outcome: &mut ReadOutcome) {
    let mut i = 0;
    let mut current_table = String::new();
    while i < body.len() {
        let p = &body[i];
        if p.code == 0 && p.value == "TABLE" {
            current_table = body
                .get(i + 1)
                .filter(|n| n.code == 2)
                .map(|n| n.value.clone())
                .unwrap_or_default();
            i += 1;
            continue;
        }
        if p.code == 0 && p.value == current_table {
            let end = next_record(body, i + 1);
            let record = &body[i + 1..end];
            if current_table == "LAYER" {
                read_layer_record(db, record, outcome);
            }
            i = end;
            continue;
        }
        i += 1;
    }
}

fn next_record(body: &[Pair], from: usize) -> usize {
    body.iter()
        .enumerate()
        .skip(from)
        .find(|(_, p)| p.code == 0)
        .map_or(body.len(), |(i, _)| i)
}

fn read_layer_record(db: &mut Database, record: &[Pair], outcome: &mut ReadOutcome) {
    let Some(name) = record.iter().find(|p| p.code == 2).map(|p| p.value.clone()) else {
        outcome.warnings.push(Warning {
            context: "LAYER".into(),
            message: "record has no name and was skipped".into(),
        });
        return;
    };
    let id = db.ensure_layer(&name);
    let Some(layer) = db.tables.layers.get_mut(id) else {
        return;
    };
    for p in record {
        match p.code {
            62 => {
                if let Some(c) = p.as_i64() {
                    // A negative colour number is how DXF marks a layer as off,
                    // with the absolute value still carrying the colour.
                    layer.visible = c >= 0;
                    if let Ok(index) = u8::try_from(c.unsigned_abs()) {
                        layer.color = Color::Index(index);
                    }
                }
            }
            70 => {
                if let Some(flags) = p.as_i64() {
                    layer.frozen = flags & 1 != 0;
                    layer.locked = flags & 4 != 0;
                }
            }
            290 => layer.plottable = p.as_i64() != Some(0),
            370 => {
                if let Some(w) = p.as_i64() {
                    layer.lineweight = match u16::try_from(w) {
                        Ok(v) => LineWeight::Hundredths(v),
                        Err(_) => LineWeight::Default,
                    };
                }
            }
            _ => {}
        }
    }
}

fn read_blocks(db: &mut Database, body: &[Pair], outcome: &mut ReadOutcome) {
    let mut i = 0;
    while i < body.len() {
        if body[i].code == 0 && body[i].value == "BLOCK" {
            let header_end = next_record(body, i + 1);
            let header = &body[i + 1..header_end];
            let name = header
                .iter()
                .find(|p| p.code == 2)
                .map(|p| p.value.clone())
                .unwrap_or_default();
            let base = read_point(header, 10).unwrap_or(Point3::ORIGIN);

            // Body runs to ENDBLK.
            let end = body
                .iter()
                .enumerate()
                .skip(header_end)
                .find(|(_, p)| p.code == 0 && p.value == "ENDBLK")
                .map_or(body.len(), |(i, _)| i);

            if name.is_empty() {
                outcome.warnings.push(Warning {
                    context: "BLOCK".into(),
                    message: "block has no name and was skipped".into(),
                });
            } else if !name.starts_with('*') {
                // *Model_Space and *Paper_Space already exist; their contents
                // arrive in the ENTITIES section.
                let block_id = db.ensure_block(&name);
                if let Some(rec) = db.tables.blocks.get_mut(block_id) {
                    rec.base_point = base;
                }
                read_entities(db, &body[header_end..end], Some(block_id), outcome);
            }
            i = end + 1;
            continue;
        }
        i += 1;
    }
}

fn read_entities(
    db: &mut Database,
    body: &[Pair],
    space: Option<ObjectId>,
    outcome: &mut ReadOutcome,
) {
    let space = space.unwrap_or_else(|| db.model_space());
    let mut i = 0;
    while i < body.len() {
        if body[i].code != 0 {
            i += 1;
            continue;
        }
        let kind = body[i].value.clone();
        if kind == "ENDBLK" || kind == "SEQEND" {
            i += 1;
            continue;
        }
        // POLYLINE owns the VERTEX records that follow it, up to SEQEND.
        let end = if kind == "POLYLINE" {
            body.iter()
                .enumerate()
                .skip(i + 1)
                .find(|(_, p)| p.code == 0 && p.value == "SEQEND")
                .map_or(body.len(), |(i, _)| i)
        } else {
            next_record(body, i + 1)
        };
        let record = &body[i..end];
        match build_entity(db, &kind, record, space, outcome) {
            Ok(Some(entity)) => {
                if let Err(e) = db.insert_entity(entity) {
                    outcome.warnings.push(Warning {
                        context: kind.clone(),
                        message: e.to_string(),
                    });
                }
            }
            Ok(None) => {}
            Err(e) => outcome.warnings.push(Warning {
                context: kind.clone(),
                message: e.to_string(),
            }),
        }
        i = end;
    }
}

fn read_point(record: &[Pair], base_code: i32) -> Option<Point3> {
    let x = record.iter().find(|p| p.code == base_code)?.as_f64()?;
    let y = record
        .iter()
        .find(|p| p.code == base_code + 10)
        .and_then(Pair::as_f64)
        .unwrap_or(0.0);
    let z = record
        .iter()
        .find(|p| p.code == base_code + 20)
        .and_then(Pair::as_f64)
        .unwrap_or(0.0);
    Some(Point3::new(x, y, z))
}

fn read_f64(record: &[Pair], code: i32) -> Option<f64> {
    record
        .iter()
        .find(|p| p.code == code)
        .and_then(Pair::as_f64)
}

fn read_i64(record: &[Pair], code: i32) -> Option<i64> {
    record
        .iter()
        .find(|p| p.code == code)
        .and_then(Pair::as_i64)
}

fn read_text(record: &[Pair], code: i32) -> Option<String> {
    record
        .iter()
        .find(|p| p.code == code)
        .map(|p| p.value.clone())
}

fn read_normal(record: &[Pair]) -> Vec3 {
    let x = read_f64(record, 210).unwrap_or(0.0);
    let y = read_f64(record, 220).unwrap_or(0.0);
    let z = read_f64(record, 230).unwrap_or(1.0);
    let v = Vec3::new(x, y, z);
    if v.is_zero() { Vec3::Z } else { v }
}

fn build_entity(
    db: &mut Database,
    kind: &str,
    record: &[Pair],
    space: ObjectId,
    outcome: &mut ReadOutcome,
) -> Result<Option<Entity>> {
    let layer_name = read_text(record, 8).unwrap_or_else(|| "0".to_owned());
    let layer = db.ensure_layer(&layer_name);

    let geom = match kind {
        "LINE" => Geometry::Line {
            a: read_point(record, 10).ok_or(DxfError::MissingCode {
                entity: "LINE",
                code: 10,
            })?,
            b: read_point(record, 11).ok_or(DxfError::MissingCode {
                entity: "LINE",
                code: 11,
            })?,
        },
        "POINT" => Geometry::Point(read_point(record, 10).ok_or(DxfError::MissingCode {
            entity: "POINT",
            code: 10,
        })?),
        "CIRCLE" => Geometry::Circle {
            center: read_point(record, 10).ok_or(DxfError::MissingCode {
                entity: "CIRCLE",
                code: 10,
            })?,
            radius: read_f64(record, 40).ok_or(DxfError::MissingCode {
                entity: "CIRCLE",
                code: 40,
            })?,
            normal: read_normal(record),
        },
        "ARC" => {
            let center = read_point(record, 10).ok_or(DxfError::MissingCode {
                entity: "ARC",
                code: 10,
            })?;
            let radius = read_f64(record, 40).ok_or(DxfError::MissingCode {
                entity: "ARC",
                code: 40,
            })?;
            let start = read_f64(record, 50).unwrap_or(0.0).to_radians();
            let end = read_f64(record, 51).unwrap_or(0.0).to_radians();
            // DXF arcs are always counter-clockwise from start to end.
            let arc = od_core::Arc2::from_start_end_ccw(center.to_2d(), radius, start, end);
            Geometry::Arc {
                center,
                radius,
                start_angle: arc.start_angle,
                sweep: arc.sweep,
                normal: read_normal(record),
            }
        }
        "ELLIPSE" => {
            let center = read_point(record, 10).ok_or(DxfError::MissingCode {
                entity: "ELLIPSE",
                code: 10,
            })?;
            let major = read_point(record, 11).ok_or(DxfError::MissingCode {
                entity: "ELLIPSE",
                code: 11,
            })?;
            Geometry::Ellipse {
                center,
                major_axis: Vec3::new(major.x, major.y, major.z),
                ratio: read_f64(record, 40).unwrap_or(1.0),
                start_param: read_f64(record, 41).unwrap_or(0.0),
                end_param: read_f64(record, 42).unwrap_or(std::f64::consts::TAU),
                normal: read_normal(record),
            }
        }
        "LWPOLYLINE" => build_lwpolyline(record),
        "POLYLINE" => build_polyline(record),
        "TEXT" => {
            let style_name = read_text(record, 7).unwrap_or_else(|| "Standard".to_owned());
            let style = db
                .tables
                .text_styles
                .id_of(&style_name)
                .or_else(|| db.tables.text_styles.id_of("Standard"))
                .ok_or(DxfError::MissingCode {
                    entity: "TEXT",
                    code: 7,
                })?;
            Geometry::Text(Box::new(TextEntity {
                position: read_point(record, 10).unwrap_or(Point3::ORIGIN),
                value: read_text(record, 1).unwrap_or_default(),
                height: read_f64(record, 40).unwrap_or(2.5),
                rotation: read_f64(record, 50).unwrap_or(0.0).to_radians(),
                style,
                flow: od_core::TextFlow::Horizontal,
                h_align: match read_i64(record, 72) {
                    Some(1) => od_core::HAlign::Center,
                    Some(2) => od_core::HAlign::Right,
                    Some(5) => od_core::HAlign::Fit,
                    _ => od_core::HAlign::Left,
                },
                v_align: match read_i64(record, 73) {
                    Some(1) => od_core::VAlign::Bottom,
                    Some(2) => od_core::VAlign::Middle,
                    Some(3) => od_core::VAlign::Top,
                    _ => od_core::VAlign::Baseline,
                },
                width_factor: read_f64(record, 41).unwrap_or(1.0),
                oblique: read_f64(record, 51).unwrap_or(0.0).to_radians(),
            }))
        }
        "MTEXT" => {
            let style_name = read_text(record, 7).unwrap_or_else(|| "Standard".to_owned());
            let style = db
                .tables
                .text_styles
                .id_of(&style_name)
                .or_else(|| db.tables.text_styles.id_of("Standard"))
                .ok_or(DxfError::MissingCode {
                    entity: "MTEXT",
                    code: 7,
                })?;
            // Long MTEXT arrives split across repeated code 3 chunks with the
            // tail in code 1; concatenating in file order is the only way to
            // recover the original string.
            let mut value: String = record
                .iter()
                .filter(|p| p.code == 3)
                .map(|p| p.value.as_str())
                .collect();
            value.push_str(&read_text(record, 1).unwrap_or_default());
            Geometry::MText(Box::new(MTextEntity {
                position: read_point(record, 10).unwrap_or(Point3::ORIGIN),
                value,
                height: read_f64(record, 40).unwrap_or(2.5),
                rotation: read_f64(record, 50).unwrap_or(0.0).to_radians(),
                style,
                width: read_f64(record, 41).unwrap_or(0.0),
                line_spacing: read_f64(record, 44).unwrap_or(1.0),
                flow: od_core::TextFlow::Horizontal,
            }))
        }
        "INSERT" => {
            let name = read_text(record, 2).ok_or(DxfError::MissingCode {
                entity: "INSERT",
                code: 2,
            })?;
            let block = db.ensure_block(&name);
            Geometry::BlockRef(Box::new(BlockRef {
                block,
                position: read_point(record, 10).unwrap_or(Point3::ORIGIN),
                scale: Vec3::new(
                    read_f64(record, 41).unwrap_or(1.0),
                    read_f64(record, 42).unwrap_or(1.0),
                    read_f64(record, 43).unwrap_or(1.0),
                ),
                rotation: read_f64(record, 50).unwrap_or(0.0).to_radians(),
                attributes: Vec::new(),
                array: (
                    u32::try_from(read_i64(record, 70).unwrap_or(1).max(1)).unwrap_or(1),
                    u32::try_from(read_i64(record, 71).unwrap_or(1).max(1)).unwrap_or(1),
                ),
                array_spacing: (
                    read_f64(record, 44).unwrap_or(0.0),
                    read_f64(record, 45).unwrap_or(0.0),
                ),
            }))
        }
        other => {
            if !outcome.unsupported_types.iter().any(|t| t == other) {
                outcome.unsupported_types.push(other.to_owned());
            }
            Geometry::Unsupported {
                source_type: other.to_owned(),
                payload: serialize_pairs(record).into_bytes(),
                proxy: proxy_from_record(record),
            }
        }
    };

    let mut entity = Entity::new(layer, space, geom);
    entity.style = read_style(record);
    entity.visible = read_i64(record, 60) != Some(1);
    Ok(Some(entity))
}

/// Best-effort outline for something we are preserving but not modelling: every
/// point-like code in the record, joined up. Crude, but it means the entity has
/// a location on screen and can be selected, which is the difference between
/// "preserved" and "invisible".
fn proxy_from_record(record: &[Pair]) -> Vec<ProxyGraphic> {
    let mut points = Vec::new();
    for p in record.iter().filter(|p| p.code == 10 || p.code == 11) {
        if let Some(x) = p.as_f64() {
            let y = record
                .iter()
                .find(|q| q.code == p.code + 10)
                .and_then(Pair::as_f64)
                .unwrap_or(0.0);
            let z = record
                .iter()
                .find(|q| q.code == p.code + 20)
                .and_then(Pair::as_f64)
                .unwrap_or(0.0);
            points.push(Point3::new(x, y, z));
        }
    }
    if points.is_empty() {
        Vec::new()
    } else {
        vec![ProxyGraphic::Polyline {
            points,
            closed: false,
        }]
    }
}

fn read_style(record: &[Pair]) -> GraphicStyle {
    let mut style = GraphicStyle::default();
    if let Some(c) = read_i64(record, 62) {
        style.color = match c {
            0 => Color::ByBlock,
            256 => Color::ByLayer,
            n => u8::try_from(n).map_or(Color::ByLayer, Color::Index),
        };
    }
    if let Some(rgb) = read_i64(record, 420) {
        let v = u32::try_from(rgb).unwrap_or(0);
        style.color = Color::Rgb {
            r: ((v >> 16) & 0xff) as u8,
            g: ((v >> 8) & 0xff) as u8,
            b: (v & 0xff) as u8,
        };
    }
    if let Some(w) = read_i64(record, 370) {
        style.lineweight = match w {
            -1 => LineWeight::ByLayer,
            -2 => LineWeight::ByBlock,
            -3 => LineWeight::Default,
            n => u16::try_from(n).map_or(LineWeight::Default, LineWeight::Hundredths),
        };
    }
    if let Some(s) = read_f64(record, 48) {
        style.linetype_scale = s;
    }
    if let Some(t) = read_i64(record, 440) {
        // DXF stores transparency as 0x02000000 | (255 - alpha), where alpha
        // 255 is opaque. Our scale is 0..=90, matching the UI's percentage.
        let alpha = u32::try_from(t).unwrap_or(0) & 0xff;
        let percent = 255u32.saturating_sub(alpha) * 90 / 255;
        style.transparency = u8::try_from(percent).unwrap_or(90);
    }
    style
}

fn build_lwpolyline(record: &[Pair]) -> Geometry {
    let closed = read_i64(record, 70).unwrap_or(0) & 1 != 0;
    let elevation = read_f64(record, 38).unwrap_or(0.0);
    let mut vertices: Vec<od_core::Vertex> = Vec::new();

    // Vertices arrive as a flat run: each 10 starts one, and a 42 that follows
    // belongs to the vertex before it.
    for p in record {
        match p.code {
            10 => {
                if let Some(x) = p.as_f64() {
                    vertices.push(od_core::Vertex::straight(od_core::Point2::new(x, 0.0)));
                }
            }
            20 => {
                if let (Some(y), Some(v)) = (p.as_f64(), vertices.last_mut()) {
                    v.point.y = y;
                }
            }
            42 => {
                if let (Some(b), Some(v)) = (p.as_f64(), vertices.last_mut()) {
                    v.bulge = b;
                }
            }
            _ => {}
        }
    }

    Geometry::Polyline {
        polyline: Polyline2::new(vertices, closed),
        elevation,
        normal: read_normal(record),
        width: read_f64(record, 43).unwrap_or(0.0),
    }
}

fn build_polyline(record: &[Pair]) -> Geometry {
    let flags = read_i64(record, 70).unwrap_or(0);
    let closed = flags & 1 != 0;
    let is_3d = flags & 8 != 0;

    // Vertices are separate records following the header; walk them in order.
    let mut points: Vec<Point3> = Vec::new();
    let mut bulges: Vec<f64> = Vec::new();
    let mut i = 0;
    while i < record.len() {
        if record[i].code == 0 && record[i].value == "VERTEX" {
            let end = next_record(record, i + 1);
            let v = &record[i..end];
            if let Some(p) = read_point(v, 10) {
                points.push(p);
                bulges.push(read_f64(v, 42).unwrap_or(0.0));
            }
            i = end;
            continue;
        }
        i += 1;
    }

    if is_3d {
        Geometry::Polyline3d { points, closed }
    } else {
        let elevation = points.first().map_or(0.0, |p| p.z);
        let vertices = points
            .iter()
            .zip(bulges.iter())
            .map(|(p, b)| od_core::Vertex::with_bulge(p.to_2d(), *b))
            .collect();
        Geometry::Polyline {
            polyline: Polyline2::new(vertices, closed),
            elevation,
            normal: read_normal(record),
            width: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n30\n0.0\n11\n100.0\n21\n50.0\n31\n0.0\n0\nENDSEC\n0\nEOF\n";

    #[test]
    fn reads_a_minimal_drawing() {
        let (db, outcome) = read_str(MINIMAL).expect("reads");
        assert_eq!(db.entities().count(), 1);
        assert!(outcome.warnings.is_empty());
        let (_, e) = db.entities().next().expect("one entity");
        match &e.geom {
            Geometry::Line { a, b } => {
                assert!(a.coincides_with(Point3::ORIGIN));
                assert!(b.coincides_with(Point3::new(100.0, 50.0, 0.0)));
            }
            other => panic!("expected a line, got {}", other.type_name()),
        }
    }

    #[test]
    fn an_odd_number_of_lines_is_an_error_not_a_panic() {
        let truncated = "0\nSECTION\n2\nENTITIES\n0\n";
        assert!(matches!(
            read_str(truncated),
            Err(DxfError::TruncatedPair { .. })
        ));
    }

    #[test]
    fn a_non_numeric_group_code_is_reported_with_its_line() {
        let bad = "0\nSECTION\nnot-a-code\nvalue\n";
        match read_str(bad) {
            Err(DxfError::BadGroupCode { line, .. }) => assert_eq!(line, 3),
            other => panic!("expected a group code error, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_entity_is_preserved_with_a_proxy() {
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nACAD_TABLE\n8\n0\n10\n5.0\n20\n7.0\n30\n0.0\n0\nENDSEC\n0\nEOF\n";
        let (db, outcome) = read_str(dxf).expect("reads");
        assert_eq!(outcome.unsupported_types, vec!["ACAD_TABLE"]);
        let (_, e) = db.entities().next().expect("kept the entity");
        match &e.geom {
            Geometry::Unsupported {
                source_type,
                payload,
                proxy,
            } => {
                assert_eq!(source_type, "ACAD_TABLE");
                assert!(!payload.is_empty(), "the original pairs are kept");
                assert!(!proxy.is_empty(), "it must still be visible");
            }
            other => panic!("expected preserved geometry, got {}", other.type_name()),
        }
    }

    #[test]
    fn an_unknown_section_is_preserved_whole() {
        let dxf = "0\nSECTION\n2\nCLASSES\n0\nCLASS\n1\nSOMETHING\n0\nENDSEC\n0\nEOF\n";
        let (db, _) = read_str(dxf).expect("reads");
        assert_eq!(db.preserved.len(), 1);
        assert_eq!(db.preserved[0].section, "CLASSES");
    }

    #[test]
    fn a_broken_entity_does_not_fail_the_file() {
        // A LINE with no start point, followed by a good one.
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n11\n1.0\n21\n1.0\n0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n11\n5.0\n21\n5.0\n0\nENDSEC\n0\nEOF\n";
        let (db, outcome) = read_str(dxf).expect("reads");
        assert_eq!(db.entities().count(), 1, "the good line survives");
        assert_eq!(outcome.warnings.len(), 1, "the bad one is reported");
    }

    #[test]
    fn layers_carry_their_colour_and_state() {
        let dxf = "0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n0\nLAYER\n2\nA-Duct\n62\n-3\n70\n4\n0\nENDTAB\n0\nENDSEC\n0\nEOF\n";
        let (db, _) = read_str(dxf).expect("reads");
        let layer = db.tables.layers.by_name("a-duct").expect("layer exists");
        assert_eq!(layer.name, "A-Duct", "original casing is kept");
        assert_eq!(layer.color, Color::Index(3));
        assert!(!layer.visible, "a negative colour means the layer is off");
        assert!(layer.locked);
    }

    #[test]
    fn lwpolyline_vertices_and_bulges_line_up() {
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nLWPOLYLINE\n8\n0\n90\n3\n70\n1\n10\n0.0\n20\n0.0\n42\n1.0\n10\n100.0\n20\n0.0\n10\n100.0\n20\n100.0\n0\nENDSEC\n0\nEOF\n";
        let (db, _) = read_str(dxf).expect("reads");
        let (_, e) = db.entities().next().expect("one entity");
        match &e.geom {
            Geometry::Polyline { polyline, .. } => {
                assert_eq!(polyline.vertices.len(), 3);
                assert!(polyline.closed);
                assert!(od_core::tol::eq_len(polyline.vertices[0].bulge, 1.0));
                assert!(od_core::tol::eq_len(polyline.vertices[1].bulge, 0.0));
                assert!(
                    polyline.vertices[1]
                        .point
                        .coincides_with(od_core::Point2::new(100.0, 0.0))
                );
            }
            other => panic!("expected a polyline, got {}", other.type_name()),
        }
    }

    #[test]
    fn an_arc_crossing_zero_keeps_its_short_sweep() {
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nARC\n8\n0\n10\n0.0\n20\n0.0\n40\n10.0\n50\n315.0\n51\n45.0\n0\nENDSEC\n0\nEOF\n";
        let (db, _) = read_str(dxf).expect("reads");
        let (_, e) = db.entities().next().expect("one entity");
        match &e.geom {
            Geometry::Arc { sweep, .. } => {
                assert!(
                    od_core::tol::eq_angle(*sweep, std::f64::consts::FRAC_PI_2),
                    "315°→45° is a 90° arc, not a 270° one"
                );
            }
            other => panic!("expected an arc, got {}", other.type_name()),
        }
    }

    #[test]
    fn mtext_chunks_are_concatenated_in_order() {
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nMTEXT\n8\n0\n10\n0.0\n20\n0.0\n40\n2.5\n3\nfirst \n3\nsecond \n1\nthird\n0\nENDSEC\n0\nEOF\n";
        let (db, _) = read_str(dxf).expect("reads");
        let (_, e) = db.entities().next().expect("one entity");
        match &e.geom {
            Geometry::MText(t) => assert_eq!(t.value, "first second third"),
            other => panic!("expected mtext, got {}", other.type_name()),
        }
    }

    #[test]
    fn blocks_and_inserts_are_linked_by_name() {
        let dxf = "0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n2\nDESK\n10\n0.0\n20\n0.0\n0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n11\n600.0\n21\n300.0\n0\nENDBLK\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nINSERT\n8\n0\n2\nDESK\n10\n1000.0\n20\n2000.0\n41\n2.0\n42\n2.0\n0\nENDSEC\n0\nEOF\n";
        let (db, _) = read_str(dxf).expect("reads");
        let block = db.tables.blocks.id_of("DESK").expect("block exists");
        assert_eq!(db.entities_in(block).count(), 1, "the block has contents");

        let (id, e) = db
            .entities_in(db.model_space())
            .next()
            .expect("one insert in model space");
        match &e.geom {
            Geometry::BlockRef(b) => assert_eq!(b.block, block),
            other => panic!("expected an insert, got {}", other.type_name()),
        }
        let bounds = db.entity_bounds(id);
        assert!(od_core::tol::eq_len(bounds.max.x, 1000.0 + 600.0 * 2.0));
        assert!(db.validate().is_empty());
    }

    #[test]
    fn text_alignment_and_style_survive() {
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nTEXT\n8\n0\n10\n10.0\n20\n20.0\n40\n3.5\n1\n通り芯\n72\n1\n73\n2\n50\n90.0\n0\nENDSEC\n0\nEOF\n";
        let (db, _) = read_str(dxf).expect("reads");
        let (_, e) = db.entities().next().expect("one entity");
        match &e.geom {
            Geometry::Text(t) => {
                assert_eq!(t.value, "通り芯");
                assert_eq!(t.h_align, od_core::HAlign::Center);
                assert_eq!(t.v_align, od_core::VAlign::Middle);
                assert!(od_core::tol::eq_len(t.height, 3.5));
                assert!(od_core::tol::eq_angle(
                    t.rotation,
                    std::f64::consts::FRAC_PI_2
                ));
            }
            other => panic!("expected text, got {}", other.type_name()),
        }
    }
}
