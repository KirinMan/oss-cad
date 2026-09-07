//! DXF writer.
//!
//! Targets R2000 (`AC1015`), which every DWG/DXF-capable application made this
//! century can read. Preserved sections and preserved entities are written back
//! out verbatim, so a file that made a round trip through OpenDraft carries the
//! parts we understood *and* the parts we did not.

use crate::pair::format_f64;
use od_core::{
    BlockKind, Color, Database, Entity, Geometry, LineWeight, ObjectId, Point3, TextFlow,
};
use std::collections::HashMap;
use std::fmt::Write as _;

/// Assigns the hexadecimal handles DXF requires.
///
/// Handles are allocated here rather than derived from [`ObjectId`], because a
/// document edited by several actors has ids that do not form the single
/// ascending sequence DXF expects.
#[derive(Debug, Default)]
struct Handles {
    map: HashMap<ObjectId, u64>,
    next: u64,
}

impl Handles {
    fn new() -> Self {
        // Handle 0 is reserved and low values are conventionally taken by the
        // fixed tables, so user data starts clear of them.
        Self {
            map: HashMap::new(),
            next: 0x100,
        }
    }

    fn get(&mut self, id: ObjectId) -> String {
        let h = *self.map.entry(id).or_insert_with(|| {
            let h = self.next;
            self.next += 1;
            h
        });
        format!("{h:X}")
    }

    fn fresh(&mut self) -> String {
        let h = self.next;
        self.next += 1;
        format!("{h:X}")
    }
}

struct Writer {
    out: String,
    handles: Handles,
}

impl Writer {
    fn pair(&mut self, code: i32, value: &str) {
        let _ = writeln!(self.out, "{code}\n{value}");
    }

    fn num(&mut self, code: i32, value: f64) {
        let v = format_f64(value);
        self.pair(code, &v);
    }

    fn int(&mut self, code: i32, value: i64) {
        let v = value.to_string();
        self.pair(code, &v);
    }

    fn point(&mut self, base: i32, p: Point3) {
        self.num(base, p.x);
        self.num(base + 10, p.y);
        self.num(base + 20, p.z);
    }
}

/// Writes the document as an ASCII DXF string.
#[must_use]
pub fn write_string(db: &Database) -> String {
    let mut w = Writer {
        out: String::with_capacity(64 * 1024),
        handles: Handles::new(),
    };

    write_header(&mut w, db);
    write_tables(&mut w, db);
    write_blocks(&mut w, db);
    write_entities(&mut w, db);
    write_preserved(&mut w, db);

    w.pair(0, "EOF");
    w.out
}

fn write_header(w: &mut Writer, db: &Database) {
    w.pair(0, "SECTION");
    w.pair(2, "HEADER");
    w.pair(9, "$ACADVER");
    w.pair(1, "AC1015");
    w.pair(9, "$INSUNITS");
    w.int(70, units_to_dxf(db.header.units));
    w.pair(9, "$LUPREC");
    w.int(70, i64::from(db.header.linear_precision));
    w.pair(9, "$AUPREC");
    w.int(70, i64::from(db.header.angular_precision));
    w.pair(9, "$ANGBASE");
    w.num(50, db.header.angle_base.to_degrees());
    w.pair(9, "$ANGDIR");
    w.int(70, i64::from(db.header.angle_clockwise));
    w.pair(9, "$INSBASE");
    w.point(10, db.header.insertion_base);
    w.pair(9, "$HANDSEED");
    w.pair(5, "FFFF");
    w.pair(0, "ENDSEC");
}

fn units_to_dxf(u: od_core::Units) -> i64 {
    match u {
        od_core::Units::Unitless => 0,
        od_core::Units::Inches => 1,
        od_core::Units::Feet => 2,
        od_core::Units::Millimeters => 4,
        od_core::Units::Centimeters => 5,
        od_core::Units::Meters => 6,
    }
}

fn write_tables(w: &mut Writer, db: &Database) {
    w.pair(0, "SECTION");
    w.pair(2, "TABLES");

    // LTYPE — every drawing needs at least ByLayer, ByBlock and Continuous, and
    // readers reject the file if a referenced linetype is absent.
    w.pair(0, "TABLE");
    w.pair(2, "LTYPE");
    let handle = w.handles.fresh();
    w.pair(5, &handle);
    w.pair(100, "AcDbSymbolTable");
    w.int(70, db.tables.linetypes.len() as i64 + 2);
    for name in ["ByLayer", "ByBlock"] {
        write_ltype_record(w, name, "", &[]);
    }
    for (_, lt) in db.tables.linetypes.iter() {
        write_ltype_record(w, &lt.name, &lt.description, &lt.pattern);
    }
    w.pair(0, "ENDTAB");

    // LAYER
    w.pair(0, "TABLE");
    w.pair(2, "LAYER");
    let handle = w.handles.fresh();
    w.pair(5, &handle);
    w.pair(100, "AcDbSymbolTable");
    w.int(70, db.tables.layers.len() as i64);
    let layers: Vec<(ObjectId, od_core::Layer)> = db
        .tables
        .layers
        .iter()
        .map(|(id, l)| (id, l.clone()))
        .collect();
    for (id, layer) in layers {
        let handle = w.handles.get(id);
        w.pair(0, "LAYER");
        w.pair(5, &handle);
        w.pair(100, "AcDbSymbolTableRecord");
        w.pair(100, "AcDbLayerTableRecord");
        w.pair(2, &layer.name);
        let mut flags = 0i64;
        if layer.frozen {
            flags |= 1;
        }
        if layer.locked {
            flags |= 4;
        }
        w.int(70, flags);
        let index = match layer.color {
            Color::Index(i) => i64::from(i),
            Color::Rgb { .. } => 7,
            _ => 7,
        };
        // Off is signalled by negating the colour number, matching the reader.
        w.int(62, if layer.visible { index } else { -index });
        w.pair(6, "Continuous");
        w.int(370, lineweight_to_dxf(layer.lineweight));
        w.int(290, i64::from(layer.plottable));
    }
    w.pair(0, "ENDTAB");

    // STYLE
    w.pair(0, "TABLE");
    w.pair(2, "STYLE");
    let handle = w.handles.fresh();
    w.pair(5, &handle);
    w.pair(100, "AcDbSymbolTable");
    w.int(70, db.tables.text_styles.len() as i64);
    let styles: Vec<(ObjectId, od_core::TextStyle)> = db
        .tables
        .text_styles
        .iter()
        .map(|(id, s)| (id, s.clone()))
        .collect();
    for (id, style) in styles {
        let handle = w.handles.get(id);
        w.pair(0, "STYLE");
        w.pair(5, &handle);
        w.pair(100, "AcDbSymbolTableRecord");
        w.pair(100, "AcDbTextStyleTableRecord");
        w.pair(2, &style.name);
        w.int(70, 0);
        w.num(40, style.height);
        w.num(41, style.width_factor);
        w.num(50, style.oblique.to_degrees());
        w.int(71, 0);
        w.num(42, style.height.max(2.5));
        w.pair(3, &style.font);
        w.pair(4, style.big_font.as_deref().unwrap_or(""));
    }
    w.pair(0, "ENDTAB");

    w.pair(0, "ENDSEC");
}

fn write_ltype_record(w: &mut Writer, name: &str, description: &str, pattern: &[f64]) {
    let handle = w.handles.fresh();
    w.pair(0, "LTYPE");
    w.pair(5, &handle);
    w.pair(100, "AcDbSymbolTableRecord");
    w.pair(100, "AcDbLinetypeTableRecord");
    w.pair(2, name);
    w.int(70, 0);
    w.pair(3, description);
    w.int(72, 65);
    w.int(73, pattern.len() as i64);
    w.num(40, pattern.iter().map(|d| d.abs()).sum());
    for d in pattern {
        w.num(49, *d);
        w.int(74, 0);
    }
}

fn lineweight_to_dxf(lw: LineWeight) -> i64 {
    match lw {
        LineWeight::ByLayer => -1,
        LineWeight::ByBlock => -2,
        LineWeight::Default => -3,
        LineWeight::Hundredths(v) => i64::from(v),
    }
}

fn write_blocks(w: &mut Writer, db: &Database) {
    w.pair(0, "SECTION");
    w.pair(2, "BLOCKS");
    let blocks: Vec<(ObjectId, od_core::BlockRecord)> = db
        .tables
        .blocks
        .iter()
        .map(|(id, b)| (id, b.clone()))
        .collect();
    for (id, block) in blocks {
        let handle = w.handles.get(id);
        w.pair(0, "BLOCK");
        w.pair(5, &handle);
        w.pair(100, "AcDbEntity");
        w.pair(8, "0");
        w.pair(100, "AcDbBlockBegin");
        w.pair(2, &block.name);
        w.int(70, 0);
        w.point(10, block.base_point);
        w.pair(3, &block.name);
        w.pair(1, block.xref.as_ref().map_or("", |x| x.path.as_str()));
        // Model and paper space records are declared but their entities belong
        // to the ENTITIES section, which is where readers expect them.
        if matches!(block.kind, BlockKind::Definition) {
            for eid in &block.entities {
                if let Some(e) = db.entity(*eid) {
                    write_entity(w, db, *eid, e);
                }
            }
        }
        w.pair(0, "ENDBLK");
        let handle = w.handles.fresh();
        w.pair(5, &handle);
        w.pair(100, "AcDbEntity");
        w.pair(8, "0");
        w.pair(100, "AcDbBlockEnd");
    }
    w.pair(0, "ENDSEC");
}

fn write_entities(w: &mut Writer, db: &Database) {
    w.pair(0, "SECTION");
    w.pair(2, "ENTITIES");
    let ids: Vec<ObjectId> = db.entities_in(db.model_space()).map(|(id, _)| id).collect();
    for id in ids {
        if let Some(e) = db.entity(id) {
            write_entity(w, db, id, e);
        }
    }
    w.pair(0, "ENDSEC");
}

fn write_preserved(w: &mut Writer, db: &Database) {
    for blob in &db.preserved {
        if blob.source != "dxf" {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&blob.payload) else {
            continue;
        };
        w.pair(0, "SECTION");
        w.pair(2, &blob.section);
        w.out.push_str(text);
        w.pair(0, "ENDSEC");
    }
}

/// Common entity preamble: handle, layer, colour, lineweight.
fn write_common(w: &mut Writer, db: &Database, id: ObjectId, e: &Entity, subclass: &str) {
    let handle = w.handles.get(id);
    w.pair(5, &handle);
    w.pair(100, "AcDbEntity");
    let layer_name = db
        .tables
        .layers
        .get(e.layer)
        .map_or_else(|| "0".to_owned(), |l| l.name.clone());
    w.pair(8, &layer_name);
    match e.style.color {
        Color::ByLayer => {}
        Color::ByBlock => w.int(62, 0),
        Color::Index(i) => w.int(62, i64::from(i)),
        Color::Rgb { r, g, b } => {
            w.int(62, 256);
            w.int(
                420,
                (i64::from(r) << 16) | (i64::from(g) << 8) | i64::from(b),
            );
        }
    }
    if !matches!(e.style.lineweight, LineWeight::ByLayer) {
        w.int(370, lineweight_to_dxf(e.style.lineweight));
    }
    if !od_core::tol::eq_len(e.style.linetype_scale, 1.0) {
        w.num(48, e.style.linetype_scale);
    }
    if !e.visible {
        w.int(60, 1);
    }
    if !subclass.is_empty() {
        w.pair(100, subclass);
    }
}

fn write_entity(w: &mut Writer, db: &Database, id: ObjectId, e: &Entity) {
    match &e.geom {
        Geometry::Line { a, b } => {
            w.pair(0, "LINE");
            write_common(w, db, id, e, "AcDbLine");
            w.point(10, *a);
            w.point(11, *b);
        }
        Geometry::Point(p) => {
            w.pair(0, "POINT");
            write_common(w, db, id, e, "AcDbPoint");
            w.point(10, *p);
        }
        Geometry::Circle {
            center,
            radius,
            normal,
        } => {
            w.pair(0, "CIRCLE");
            write_common(w, db, id, e, "AcDbCircle");
            w.point(10, *center);
            w.num(40, *radius);
            write_normal(w, *normal);
        }
        Geometry::Arc {
            center,
            radius,
            start_angle,
            sweep,
            normal,
        } => {
            w.pair(0, "ARC");
            write_common(w, db, id, e, "AcDbCircle");
            w.point(10, *center);
            w.num(40, *radius);
            w.pair(100, "AcDbArc");
            // DXF stores a counter-clockwise start/end pair, so a clockwise
            // sweep is written by swapping the ends rather than negating.
            let (start, end) = if *sweep >= 0.0 {
                (*start_angle, start_angle + sweep)
            } else {
                (start_angle + sweep, *start_angle)
            };
            w.num(50, start.to_degrees().rem_euclid(360.0));
            w.num(51, end.to_degrees().rem_euclid(360.0));
            write_normal(w, *normal);
        }
        Geometry::Ellipse {
            center,
            major_axis,
            ratio,
            start_param,
            end_param,
            normal,
        } => {
            w.pair(0, "ELLIPSE");
            write_common(w, db, id, e, "AcDbEllipse");
            w.point(10, *center);
            w.point(11, Point3::new(major_axis.x, major_axis.y, major_axis.z));
            w.num(40, *ratio);
            w.num(41, *start_param);
            w.num(42, *end_param);
            write_normal(w, *normal);
        }
        Geometry::Polyline {
            polyline,
            elevation,
            normal,
            width,
        } => {
            w.pair(0, "LWPOLYLINE");
            write_common(w, db, id, e, "AcDbPolyline");
            w.int(90, polyline.vertices.len() as i64);
            w.int(70, i64::from(polyline.closed));
            if !od_core::tol::is_zero_len(*width) {
                w.num(43, *width);
            }
            for v in &polyline.vertices {
                w.num(10, v.point.x);
                w.num(20, v.point.y);
                if !od_core::tol::is_zero_len(v.bulge) {
                    w.num(42, v.bulge);
                }
            }
            if !od_core::tol::is_zero_len(*elevation) {
                w.num(38, *elevation);
            }
            write_normal(w, *normal);
        }
        Geometry::Polyline3d { points, closed } => {
            w.pair(0, "POLYLINE");
            write_common(w, db, id, e, "AcDb3dPolyline");
            w.int(66, 1);
            w.point(10, Point3::ORIGIN);
            w.int(70, 8 | i64::from(*closed));
            for p in points {
                w.pair(0, "VERTEX");
                let handle = w.handles.fresh();
                w.pair(5, &handle);
                w.pair(100, "AcDbEntity");
                w.pair(8, "0");
                w.pair(100, "AcDbVertex");
                w.pair(100, "AcDb3dPolylineVertex");
                w.point(10, *p);
                w.int(70, 32);
            }
            w.pair(0, "SEQEND");
            let handle = w.handles.fresh();
            w.pair(5, &handle);
            w.pair(100, "AcDbEntity");
            w.pair(8, "0");
        }
        Geometry::Spline {
            degree,
            control_points,
            knots,
            weights,
            closed,
        } => {
            w.pair(0, "SPLINE");
            write_common(w, db, id, e, "AcDbSpline");
            w.int(70, if *closed { 1 } else { 8 });
            w.int(71, i64::from(*degree));
            w.int(72, knots.len() as i64);
            w.int(73, control_points.len() as i64);
            for k in knots {
                w.num(40, *k);
            }
            for (i, p) in control_points.iter().enumerate() {
                w.point(10, *p);
                if let Some(weight) = weights.get(i) {
                    w.num(41, *weight);
                }
            }
        }
        Geometry::Text(t) => {
            w.pair(0, "TEXT");
            write_common(w, db, id, e, "AcDbText");
            w.point(10, t.position);
            w.num(40, t.height);
            w.pair(1, &t.value);
            if !od_core::tol::is_zero_len(t.rotation) {
                w.num(50, t.rotation.to_degrees());
            }
            if !od_core::tol::eq_len(t.width_factor, 1.0) {
                w.num(41, t.width_factor);
            }
            if !od_core::tol::is_zero_len(t.oblique) {
                w.num(51, t.oblique.to_degrees());
            }
            if let Some(style) = db.tables.text_styles.get(t.style) {
                let name = style.name.clone();
                w.pair(7, &name);
            }
            w.int(
                72,
                match t.h_align {
                    od_core::HAlign::Left => 0,
                    od_core::HAlign::Center => 1,
                    od_core::HAlign::Right => 2,
                    od_core::HAlign::Fit => 5,
                },
            );
            w.pair(100, "AcDbText");
            w.int(
                73,
                match t.v_align {
                    od_core::VAlign::Baseline => 0,
                    od_core::VAlign::Bottom => 1,
                    od_core::VAlign::Middle => 2,
                    od_core::VAlign::Top => 3,
                },
            );
            // Vertical Japanese text has no DXF representation, so it is
            // exported as horizontal and flagged for the operator rather than
            // silently written as rotated text, which would look wrong on the
            // receiving end.
            if matches!(t.flow, TextFlow::Vertical) {
                w.pair(1001, "OPENDRAFT");
                w.pair(1000, "vertical");
            }
        }
        Geometry::MText(t) => {
            w.pair(0, "MTEXT");
            write_common(w, db, id, e, "AcDbMText");
            w.point(10, t.position);
            w.num(40, t.height);
            w.num(41, t.width);
            // MTEXT strings longer than 250 bytes must be split, with the tail
            // in code 1 and the rest in repeated code 3 chunks.
            write_mtext_value(w, &t.value);
            if !od_core::tol::is_zero_len(t.rotation) {
                w.num(50, t.rotation.to_degrees());
            }
            if let Some(style) = db.tables.text_styles.get(t.style) {
                let name = style.name.clone();
                w.pair(7, &name);
            }
        }
        Geometry::Dimension(_) => {
            // DXF's own DIMENSION entity is a defining-points record plus a
            // cached anonymous block of pre-rendered graphics most readers
            // rely on instead of recomputing from the definition — full
            // fidelity is a real feature, not a quick match arm, so this
            // skips rather than writing something that looks plausible but
            // is not what a real DXF DIMENSION needs. Reported as a loss by
            // the caller, same as Solid3d below.
        }
        Geometry::Viewport(_) => {
            // DXF's viewport system spans a LAYOUT object, an
            // ACAD_LAYOUT dictionary entry and a VPORT entity with its own
            // large field set — real, substantial work of its own, so this
            // skips writing one rather than emitting something incomplete.
            // Reported as a loss by the caller, same as Dimension above.
        }
        Geometry::BlockRef(b) => {
            w.pair(0, "INSERT");
            write_common(w, db, id, e, "AcDbBlockReference");
            let name = db
                .tables
                .blocks
                .get(b.block)
                .map_or_else(|| "*U0".to_owned(), |r| r.name.clone());
            w.pair(2, &name);
            w.point(10, b.position);
            w.num(41, b.scale.x);
            w.num(42, b.scale.y);
            w.num(43, b.scale.z);
            if !od_core::tol::is_zero_len(b.rotation) {
                w.num(50, b.rotation.to_degrees());
            }
            if b.array != (1, 1) {
                w.int(70, i64::from(b.array.0));
                w.int(71, i64::from(b.array.1));
                w.num(44, b.array_spacing.0);
                w.num(45, b.array_spacing.1);
            }
        }
        Geometry::Hatch(h) => {
            // Written as its boundary until hatch patterns are modelled: an
            // outline that plots is better than a solid that does not exist.
            for lp in &h.loops {
                w.pair(0, "LWPOLYLINE");
                write_common(w, db, id, e, "AcDbPolyline");
                w.int(90, lp.vertices.len() as i64);
                w.int(70, 1);
                for v in &lp.vertices {
                    w.num(10, v.point.x);
                    w.num(20, v.point.y);
                    if !od_core::tol::is_zero_len(v.bulge) {
                        w.num(42, v.bulge);
                    }
                }
                if !od_core::tol::is_zero_len(h.elevation) {
                    w.num(38, h.elevation);
                }
            }
        }
        Geometry::Solid3d { .. } => {
            // A solid has no faithful DXF form without meshing it; skipped
            // rather than approximated, and reported by the caller.
        }
        Geometry::Unsupported { payload, .. } => {
            if let Ok(text) = std::str::from_utf8(payload) {
                w.out.push_str(text);
            }
        }
    }
}

fn write_normal(w: &mut Writer, n: od_core::Vec3) {
    if od_core::tol::eq_len(n.x, 0.0)
        && od_core::tol::eq_len(n.y, 0.0)
        && od_core::tol::eq_len(n.z, 1.0)
    {
        return;
    }
    w.num(210, n.x);
    w.num(220, n.y);
    w.num(230, n.z);
}

/// DXF caps a single text value at 250 bytes, and the split must land on a
/// character boundary or the receiving application shows mojibake.
fn write_mtext_value(w: &mut Writer, value: &str) {
    const LIMIT: usize = 250;
    if value.len() <= LIMIT {
        w.pair(1, value);
        return;
    }
    let mut rest = value;
    while rest.len() > LIMIT {
        let mut cut = LIMIT;
        while cut > 0 && !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        if cut == 0 {
            break;
        }
        let (chunk, tail) = rest.split_at(cut);
        w.pair(3, chunk);
        rest = tail;
    }
    w.pair(1, rest);
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_core::{ActorId, Entity, MTextEntity, TextEntity};

    /// The group pairs of the first record of type `kind`, up to the next
    /// record. Tests must look inside one entity, not grep the whole file:
    /// codes like 3 and 50 are used by tables and headers too.
    fn record_of<'a>(text: &'a str, kind: &str) -> Vec<(i32, &'a str)> {
        let lines: Vec<&str> = text.lines().collect();
        let pairs: Vec<(i32, &str)> = lines
            .chunks(2)
            .filter_map(|c| Some((c.first()?.trim().parse::<i32>().ok()?, *c.get(1)?)))
            .collect();
        let start = pairs
            .iter()
            .position(|(c, v)| *c == 0 && *v == kind)
            .unwrap_or_else(|| panic!("no {kind} record in the output"));
        let len = pairs[start + 1..]
            .iter()
            .position(|(c, _)| *c == 0)
            .unwrap_or(pairs.len() - start - 1);
        pairs[start + 1..start + 1 + len].to_vec()
    }

    fn db_with(geom: Geometry) -> Database {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.ensure_layer("A-Test");
        let space = db.model_space();
        db.insert_entity(Entity::new(layer, space, geom))
            .expect("inserts");
        db
    }

    #[test]
    fn output_is_a_well_formed_pair_stream() {
        let db = db_with(Geometry::Line {
            a: Point3::ORIGIN,
            b: Point3::new(100.0, 0.0, 0.0),
        });
        let text = write_string(&db);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len() % 2, 0, "every code needs a value");
        for chunk in lines.chunks(2) {
            assert!(
                chunk[0].trim().parse::<i32>().is_ok(),
                "line `{}` should be a group code",
                chunk[0]
            );
        }
        assert!(text.ends_with("0\nEOF\n"));
    }

    #[test]
    fn a_clockwise_arc_is_written_as_ccw_start_end() {
        let db = db_with(Geometry::Arc {
            center: Point3::ORIGIN,
            radius: 10.0,
            start_angle: std::f64::consts::FRAC_PI_2,
            sweep: -std::f64::consts::FRAC_PI_2,
            normal: od_core::Vec3::Z,
        });
        let text = write_string(&db);
        let record = record_of(&text, "ARC");
        let angle = |code: i32| {
            record
                .iter()
                .find(|(c, _)| *c == code)
                .and_then(|(_, v)| v.trim().parse::<f64>().ok())
                .unwrap_or_else(|| panic!("no group code {code} on the arc"))
        };
        let start = angle(50);
        let end = angle(51);
        assert!((start - 0.0).abs() < 1e-6, "start became {start}");
        assert!((end - 90.0).abs() < 1e-6, "end became {end}");
    }

    #[test]
    fn long_mtext_splits_on_character_boundaries() {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.ensure_layer("A-Text");
        let space = db.model_space();
        let style = db.tables.text_styles.id_of("Standard").expect("standard");
        // 300 multi-byte characters — the split must not land mid-character.
        let long: String = "設備".repeat(150);
        db.insert_entity(Entity::new(
            layer,
            space,
            Geometry::MText(Box::new(MTextEntity {
                position: Point3::ORIGIN,
                value: long.clone(),
                height: 2.5,
                rotation: 0.0,
                style,
                width: 0.0,
                line_spacing: 1.0,
                flow: TextFlow::Horizontal,
            })),
        ))
        .expect("inserts");

        let text = write_string(&db);
        // Reassembling code 3 chunks plus the code 1 tail must give the original.
        let record = record_of(&text, "MTEXT");
        let mut assembled: String = record
            .iter()
            .filter(|(c, _)| *c == 3)
            .map(|(_, v)| *v)
            .collect();
        let tail = record
            .iter()
            .find(|(c, _)| *c == 1)
            .map(|(_, v)| *v)
            .expect("a code 1 tail");
        assembled.push_str(tail);
        assert_eq!(assembled, long);
        assert!(
            record.iter().filter(|(c, _)| *c == 3).count() > 1,
            "a 300-character string must actually be split"
        );
    }

    #[test]
    fn a_preserved_entity_is_written_back_verbatim() {
        let payload = b"0\nACAD_TABLE\n8\n0\n10\n5.0\n20\n7.0\n".to_vec();
        let db = db_with(Geometry::Unsupported {
            source_type: "ACAD_TABLE".into(),
            payload: payload.clone(),
            proxy: Vec::new(),
        });
        let text = write_string(&db);
        assert!(text.contains("ACAD_TABLE"));
        assert!(text.contains(std::str::from_utf8(&payload).expect("utf8")));
    }

    #[test]
    fn vertical_text_is_flagged_rather_than_silently_rotated() {
        let mut db = Database::new(ActorId::SYSTEM);
        let layer = db.ensure_layer("A-Text");
        let space = db.model_space();
        let style = db.tables.text_styles.id_of("Standard").expect("standard");
        db.insert_entity(Entity::new(
            layer,
            space,
            Geometry::Text(Box::new(TextEntity {
                position: Point3::ORIGIN,
                value: "通り芯".into(),
                height: 3.0,
                rotation: 0.0,
                style,
                flow: TextFlow::Vertical,
                h_align: od_core::HAlign::Left,
                v_align: od_core::VAlign::Baseline,
                width_factor: 1.0,
                oblique: 0.0,
            })),
        ))
        .expect("inserts");
        let text = write_string(&db);
        let record = record_of(&text, "TEXT");
        assert!(record.contains(&(1001, "OPENDRAFT")));
        assert!(record.contains(&(1000, "vertical")));
        assert!(
            !record.iter().any(|(c, _)| *c == 50),
            "it must not be written as rotated text"
        );
    }

    #[test]
    fn layers_are_written_with_their_state() {
        let mut db = Database::new(ActorId::SYSTEM);
        let id = db.ensure_layer("A-Hidden");
        if let Some(l) = db.tables.layers.get_mut(id) {
            l.visible = false;
            l.color = Color::Index(5);
            l.locked = true;
        }
        let text = write_string(&db);
        assert!(text.contains("A-Hidden"));
        assert!(
            text.contains("\n62\n-5\n"),
            "an off layer is written with a negative colour number"
        );
    }
}
