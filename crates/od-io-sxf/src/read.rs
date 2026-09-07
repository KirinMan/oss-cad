//! SFC reader.
//!
//! Two rules, matching `od-io-dxf`'s own:
//!
//! 1. **Never drop what you cannot read.** An unrecognised feature becomes
//!    [`Geometry::Unsupported`] carrying its own record text verbatim, so it
//!    survives a load-and-save cycle. `drawing_sheet_feature` is the one
//!    exception: it is structural (one per file, not drawing content) and
//!    `od-core` has no field for a sheet size at all, so it is read and
//!    discarded rather than preserved as a spurious entity — a documented,
//!    permanent loss (crate docs).
//! 2. **Never fail the whole file for one bad record.** A record whose
//!    parameters don't parse is recorded as a warning and skipped.

use crate::record::{Record, parse_list, scan_records};
use crate::table;
use od_core::{
    ActorId, Arc2, Color, Database, Entity, Geometry, GraphicStyle, LineWeight, Point2, Point3,
    Polyline2, ProxyGraphic, TextEntity, Vec3, tol,
};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub context: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct ReadOutcome {
    pub warnings: Vec<Warning>,
    /// Feature record names kept verbatim because this build does not model
    /// them.
    pub unsupported_types: Vec<String>,
}

struct Tables {
    layers: HashMap<u32, od_core::ObjectId>,
    next_layer: u32,
    colors: HashMap<u32, Color>,
    next_user_color: u32,
    linetypes: HashMap<u32, od_core::ObjectId>,
    next_user_linetype: u32,
    widths: HashMap<u32, f64>,
    next_user_width: u32,
    fonts: HashMap<u32, od_core::ObjectId>,
    next_font: u32,
}

impl Tables {
    fn new() -> Self {
        Self {
            layers: HashMap::new(),
            next_layer: 1,
            colors: HashMap::new(),
            next_user_color: table::FIRST_USER_COLOR,
            linetypes: HashMap::new(),
            next_user_linetype: table::FIRST_USER_LINETYPE,
            widths: HashMap::new(),
            next_user_width: table::FIRST_USER_WIDTH,
            fonts: HashMap::new(),
            next_font: 1,
        }
    }
}

/// Reads an SFC document. Infallible — a malformed file just yields fewer
/// entities and more warnings, following rule 2 above; only [`crate::read_file`]'s
/// disk I/O can actually fail.
#[must_use]
pub fn read_str(text: &str) -> (Database, ReadOutcome) {
    let mut db = Database::new(ActorId::IMPORT);
    let mut outcome = ReadOutcome::default();
    let space = db.model_space();
    let mut t = Tables::new();

    for r in scan_records(text) {
        let handled = match r.name {
            "drawing_sheet_feature" => true,
            "layer_feature" => read_layer(&mut db, &mut t, &r, &mut outcome),
            "pre_defined_colour_feature" => read_predefined_color(&mut t, &r, &mut outcome),
            "user_defined_colour_feature" => read_user_color(&mut t, &r, &mut outcome),
            "pre_defined_font_feature" => {
                read_predefined_linetype(&mut db, &mut t, &r, &mut outcome)
            }
            "user_defined_font_feature" => read_user_linetype(&mut db, &mut t, &r, &mut outcome),
            "width_feature" => read_width(&mut t, &r, &mut outcome),
            "text_font_feature" => read_font(&mut db, &mut t, &r, &mut outcome),
            "point_marker_feature" => {
                read_shape(&mut db, &t, space, &r, &mut outcome, parse_point_marker)
            }
            "line_feature" => read_shape(&mut db, &t, space, &r, &mut outcome, parse_line),
            "polyline_feature" => read_shape(&mut db, &t, space, &r, &mut outcome, parse_polyline),
            "circle_feature" => read_shape(&mut db, &t, space, &r, &mut outcome, parse_circle),
            "arc_feature" => read_shape(&mut db, &t, space, &r, &mut outcome, parse_arc),
            "ellipse_feature" => read_shape(&mut db, &t, space, &r, &mut outcome, parse_ellipse),
            "ellipse_arc_feature" => {
                read_shape(&mut db, &t, space, &r, &mut outcome, parse_ellipse_arc)
            }
            "text_string_feature" => read_shape(&mut db, &t, space, &r, &mut outcome, parse_text),
            _ => false,
        };
        if !handled && r.name != "drawing_sheet_feature" {
            preserve_unsupported(&mut db, &t, space, &r, &mut outcome);
        }
    }

    (db, outcome)
}

fn warn(outcome: &mut ReadOutcome, record: &Record<'_>, message: impl Into<String>) {
    outcome.warnings.push(Warning {
        context: format!("#{} {}", record.id, record.name),
        message: message.into(),
    });
}

fn preserve_unsupported(
    db: &mut Database,
    t: &Tables,
    space: od_core::ObjectId,
    r: &Record<'_>,
    outcome: &mut ReadOutcome,
) {
    // Most feature records — including every one this function ever sees,
    // since every *modeled* kind is handled before falling through here —
    // put their layer code first; resolve it the same way `read_shape` does
    // rather than defaulting every unsupported entity onto layer "0"
    // regardless of where it actually was.
    let layer = r
        .params
        .first()
        .and_then(|v| v.parse::<u32>().ok())
        .and_then(|code| t.layers.get(&code).copied())
        .unwrap_or_else(|| db.ensure_layer("0"));
    let geom = Geometry::Unsupported {
        source_type: r.name.to_owned(),
        payload: r.raw.as_bytes().to_vec(),
        proxy: extract_proxy(r.name, &r.params),
    };
    if db.insert_entity(Entity::new(layer, space, geom)).is_ok()
        && !outcome.unsupported_types.iter().any(|t| t == r.name)
    {
        outcome.unsupported_types.push(r.name.to_owned());
    }
}

/// A best-effort "so it is still visible and selectable" outline
/// ([`Geometry::Unsupported`]'s own doc comment) for the feature kinds this
/// crate deliberately does not model. Not a claim of correctness — a
/// zero-length line at a shape's own base/origin point, or (for
/// `spline_feature`, which stores its control points the same
/// `VertexX`/`VertexY`-list way `polyline_feature` does) the real control
/// polygon. Positions come from the parameter layouts documented in
/// `crate::table`'s module docs (the same `SfcHelper`-derived source as
/// every other feature this crate reads); an entirely unrecognised future
/// feature name falls through to no proxy at all, same as before this
/// function existed.
fn extract_proxy(name: &str, p: &[String]) -> Vec<ProxyGraphic> {
    let point_at = |xi: usize, yi: usize| -> Vec<ProxyGraphic> {
        let found: Option<ProxyGraphic> = (|| {
            let x: f64 = p.get(xi)?.parse().ok()?;
            let y: f64 = p.get(yi)?.parse().ok()?;
            let pt = Point3::new(x, y, 0.0);
            Some(ProxyGraphic::Polyline {
                points: vec![pt, pt],
                closed: false,
            })
        })();
        found.into_iter().collect()
    };
    match name {
        // sfig_locate_feature(Layer, Name, X, Y, Angle, RatioX, RatioY)
        "sfig_locate_feature" => point_at(2, 3),
        // externally_defined_symbol_feature(Layer, ColorFlag, Color, Name, X, Y, Angle, Scale)
        "externally_defined_symbol_feature" => point_at(4, 5),
        // clothoid_feature(Layer, Color, LineType, LineWidth, BaseX, BaseY, ...)
        "clothoid_feature" => point_at(4, 5),
        // linear_dim_feature / curve_dim_feature / angular_dim_feature all
        // open with (Layer, Color, LineType, LineWidth, <origin X>, <origin Y>, ...).
        "linear_dim_feature" | "curve_dim_feature" | "angular_dim_feature" => point_at(4, 5),
        // spline_feature(Layer, Color, LineType, LineWidth, Flag, Count, VertexX, VertexY)
        "spline_feature" => {
            let xs = p.get(6).map(|s| parse_list(s)).unwrap_or_default();
            let ys = p.get(7).map(|s| parse_list(s)).unwrap_or_default();
            if xs.len() >= 2 && xs.len() == ys.len() {
                vec![ProxyGraphic::Polyline {
                    points: xs
                        .iter()
                        .zip(&ys)
                        .map(|(&x, &y)| Point3::new(x, y, 0.0))
                        .collect(),
                    closed: false,
                }]
            } else {
                Vec::new()
            }
        }
        // sfig_org_feature is a *definition* (like a DXF BLOCK), not
        // something placed at a point of its own — there is nothing here to
        // extract a location from, honestly.
        _ => Vec::new(),
    }
}

// ── Tables ───────────────────────────────────────────────────────────────

fn read_layer(
    db: &mut Database,
    t: &mut Tables,
    r: &Record<'_>,
    outcome: &mut ReadOutcome,
) -> bool {
    let (Some(name), Some(flag)) = (
        r.params.first(),
        r.params.get(1).and_then(|v| v.parse::<i32>().ok()),
    ) else {
        warn(outcome, r, "malformed layer_feature");
        return true;
    };
    let id = db.ensure_layer(name);
    if let Some(layer) = db.tables.layers.get_mut(id) {
        layer.visible = flag != 0;
    }
    t.layers.insert(t.next_layer, id);
    t.next_layer += 1;
    true
}

fn read_predefined_color(t: &mut Tables, r: &Record<'_>, outcome: &mut ReadOutcome) -> bool {
    let Some(name) = r.params.first() else {
        warn(outcome, r, "malformed pre_defined_colour_feature");
        return true;
    };
    let Some(code) = table::PREDEFINED_COLORS
        .iter()
        .position(|(n, ..)| n == name)
    else {
        warn(outcome, r, format!("unknown predefined colour `{name}`"));
        return true;
    };
    if let Some(color) = table::predefined_color_by_name(name) {
        t.colors.insert(u32::try_from(code).unwrap_or(0) + 1, color);
    }
    true
}

fn read_user_color(t: &mut Tables, r: &Record<'_>, outcome: &mut ReadOutcome) -> bool {
    let parsed: Option<[u8; 3]> = (|| {
        Some([
            r.params.first()?.parse().ok()?,
            r.params.get(1)?.parse().ok()?,
            r.params.get(2)?.parse().ok()?,
        ])
    })();
    let Some([red, green, blue]) = parsed else {
        warn(outcome, r, "malformed user_defined_colour_feature");
        return true;
    };
    t.colors.insert(
        t.next_user_color,
        Color::Rgb {
            r: red,
            g: green,
            b: blue,
        },
    );
    t.next_user_color += 1;
    true
}

fn read_predefined_linetype(
    db: &mut Database,
    t: &mut Tables,
    r: &Record<'_>,
    outcome: &mut ReadOutcome,
) -> bool {
    let Some(name) = r.params.first() else {
        warn(outcome, r, "malformed pre_defined_font_feature");
        return true;
    };
    let Some(code) = table::PREDEFINED_LINETYPES
        .iter()
        .position(|(n, _)| n == name)
    else {
        warn(outcome, r, format!("unknown predefined linetype `{name}`"));
        return true;
    };
    let Some(pitch) = table::predefined_linetype_by_name(name) else {
        return true;
    };
    let id = db.reserve_id();
    db.tables.linetypes.insert(
        id,
        od_core::LineType {
            name: name.clone(),
            description: String::new(),
            pattern: table::to_od_pattern(pitch),
        },
    );
    t.linetypes.insert(u32::try_from(code).unwrap_or(0) + 1, id);
    true
}

fn read_user_linetype(
    db: &mut Database,
    t: &mut Tables,
    r: &Record<'_>,
    outcome: &mut ReadOutcome,
) -> bool {
    let Some(name) = r.params.first() else {
        warn(outcome, r, "malformed user_defined_font_feature");
        return true;
    };
    let Some(pitch_text) = r.params.get(2) else {
        warn(outcome, r, "malformed user_defined_font_feature");
        return true;
    };
    let pitch = parse_list(pitch_text);
    let id = db.reserve_id();
    db.tables.linetypes.insert(
        id,
        od_core::LineType {
            name: name.clone(),
            description: String::new(),
            pattern: table::to_od_pattern(&pitch),
        },
    );
    t.linetypes.insert(t.next_user_linetype, id);
    t.next_user_linetype += 1;
    true
}

fn read_width(t: &mut Tables, r: &Record<'_>, outcome: &mut ReadOutcome) -> bool {
    let Some(w) = r.params.first().and_then(|v| v.parse::<f64>().ok()) else {
        warn(outcome, r, "malformed width_feature");
        return true;
    };
    let matched = table::PREDEFINED_WIDTHS
        .iter()
        .position(|&p| (p - w).abs() < 1e-5);
    let code = if let Some(i) = matched {
        u32::try_from(i).unwrap_or(0) + 1
    } else {
        let code = t.next_user_width;
        t.next_user_width += 1;
        code
    };
    t.widths.insert(code, w);
    true
}

fn read_font(db: &mut Database, t: &mut Tables, r: &Record<'_>, outcome: &mut ReadOutcome) -> bool {
    let Some(name) = r.params.first() else {
        warn(outcome, r, "malformed text_font_feature");
        return true;
    };
    let id = db.reserve_id();
    db.tables.text_styles.insert(
        id,
        od_core::TextStyle {
            name: name.clone(),
            font: name.clone(),
            big_font: None,
            height: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
        },
    );
    t.fonts.insert(t.next_font, id);
    t.next_font += 1;
    true
}

// ── Geometry ─────────────────────────────────────────────────────────────

/// A parsed shape's layer/style codes, common to every geometry feature.
struct ShapeStyle {
    layer_code: u32,
    color_code: Option<u32>,
    linetype_code: Option<u32>,
    width_code: Option<u32>,
    /// Only text carries a font code; resolved into `TextEntity::style`
    /// once `read_shape` has the `Tables` in scope (`parse_text` itself
    /// only sees raw parameters).
    font_code: Option<u32>,
}

fn read_shape(
    db: &mut Database,
    t: &Tables,
    space: od_core::ObjectId,
    r: &Record<'_>,
    outcome: &mut ReadOutcome,
    parse: impl FnOnce(&[String]) -> Option<(ShapeStyle, Geometry)>,
) -> bool {
    let Some((style, mut geom)) = parse(&r.params) else {
        warn(outcome, r, format!("malformed {}", r.name));
        return true;
    };
    if let (Some(code), Geometry::Text(text)) = (style.font_code, &mut geom) {
        match t.fonts.get(&code) {
            Some(&id) => text.style = id,
            None => {
                warn(outcome, r, format!("references undefined font {code}"));
                return true;
            }
        }
    }
    let Some(&layer) = t.layers.get(&style.layer_code) else {
        warn(
            outcome,
            r,
            format!("references undefined layer {}", style.layer_code),
        );
        return true;
    };
    let mut gs = GraphicStyle::default();
    if let Some(code) = style.color_code {
        match t.colors.get(&code) {
            Some(&c) => gs.color = c,
            None => warn(outcome, r, format!("references undefined colour {code}")),
        }
    }
    if let Some(code) = style.linetype_code {
        match t.linetypes.get(&code) {
            Some(&id) => gs.linetype = Some(id),
            None => warn(outcome, r, format!("references undefined linetype {code}")),
        }
    }
    if let Some(code) = style.width_code {
        match t.widths.get(&code) {
            Some(&mm) => gs.lineweight = LineWeight::Hundredths(mm_to_hundredths(mm)),
            None => warn(outcome, r, format!("references undefined width {code}")),
        }
    }
    let mut entity = Entity::new(layer, space, geom);
    entity.style = gs;
    let _ = db.insert_entity(entity);
    true
}

fn mm_to_hundredths(mm: f64) -> u16 {
    let hundredths = (mm * 100.0).round();
    if hundredths < 0.0 {
        0
    } else if hundredths > f64::from(u16::MAX) {
        u16::MAX
    } else {
        // Rounded and range-checked above, so this narrowing is exact.
        #[allow(clippy::cast_possible_truncation)]
        {
            hundredths as u16
        }
    }
}

fn codes4(p: &[String]) -> Option<(u32, u32, u32, u32)> {
    Some((
        p.first()?.parse().ok()?,
        p.get(1)?.parse().ok()?,
        p.get(2)?.parse().ok()?,
        p.get(3)?.parse().ok()?,
    ))
}

fn parse_point_marker(p: &[String]) -> Option<(ShapeStyle, Geometry)> {
    let layer_code = p.first()?.parse().ok()?;
    let color_code = p.get(1)?.parse().ok()?;
    let x: f64 = p.get(2)?.parse().ok()?;
    let y: f64 = p.get(3)?.parse().ok()?;
    // marker_code/rotate_angle/scale (params 4-6) have no equivalent on
    // `Geometry::Point` — narrow scope, documented in the crate docs.
    Some((
        ShapeStyle {
            layer_code,
            color_code: Some(color_code),
            linetype_code: None,
            width_code: None,
            font_code: None,
        },
        Geometry::Point(Point3::new(x, y, 0.0)),
    ))
}

fn parse_line(p: &[String]) -> Option<(ShapeStyle, Geometry)> {
    let (layer_code, color_code, linetype_code, width_code) = codes4(p)?;
    let x1: f64 = p.get(4)?.parse().ok()?;
    let y1: f64 = p.get(5)?.parse().ok()?;
    let x2: f64 = p.get(6)?.parse().ok()?;
    let y2: f64 = p.get(7)?.parse().ok()?;
    Some((
        ShapeStyle {
            layer_code,
            color_code: Some(color_code),
            linetype_code: Some(linetype_code),
            width_code: Some(width_code),
            font_code: None,
        },
        Geometry::Line {
            a: Point3::new(x1, y1, 0.0),
            b: Point3::new(x2, y2, 0.0),
        },
    ))
}

fn parse_polyline(p: &[String]) -> Option<(ShapeStyle, Geometry)> {
    let (layer_code, color_code, linetype_code, width_code) = codes4(p)?;
    let xs = parse_list(p.get(5)?);
    let ys = parse_list(p.get(6)?);
    if xs.len() != ys.len() || xs.len() < 2 {
        return None;
    }
    let mut points: Vec<Point2> = xs
        .iter()
        .zip(&ys)
        .map(|(&x, &y)| Point2::new(x, y))
        .collect();
    let closed = points.len() > 2
        && points
            .first()
            .zip(points.last())
            .is_some_and(|(a, b)| a.coincides_with(*b));
    if closed {
        points.pop();
    }
    Some((
        ShapeStyle {
            layer_code,
            color_code: Some(color_code),
            linetype_code: Some(linetype_code),
            width_code: Some(width_code),
            font_code: None,
        },
        Geometry::Polyline {
            polyline: Polyline2::from_points(points, closed),
            elevation: 0.0,
            normal: Vec3::Z,
            width: 0.0,
        },
    ))
}

fn parse_circle(p: &[String]) -> Option<(ShapeStyle, Geometry)> {
    let (layer_code, color_code, linetype_code, width_code) = codes4(p)?;
    let cx: f64 = p.get(4)?.parse().ok()?;
    let cy: f64 = p.get(5)?.parse().ok()?;
    let radius: f64 = p.get(6)?.parse().ok()?;
    Some((
        ShapeStyle {
            layer_code,
            color_code: Some(color_code),
            linetype_code: Some(linetype_code),
            width_code: Some(width_code),
            font_code: None,
        },
        Geometry::Circle {
            center: Point3::new(cx, cy, 0.0),
            radius,
            normal: Vec3::Z,
        },
    ))
}

fn parse_arc(p: &[String]) -> Option<(ShapeStyle, Geometry)> {
    let (layer_code, color_code, linetype_code, width_code) = codes4(p)?;
    let cx: f64 = p.get(4)?.parse().ok()?;
    let cy: f64 = p.get(5)?.parse().ok()?;
    let radius: f64 = p.get(6)?.parse().ok()?;
    let direction: i32 = p.get(7)?.parse().ok()?;
    let start: f64 = p.get(8)?.parse().ok()?;
    let end: f64 = p.get(9)?.parse().ok()?;
    // SXF's direction flag (0 = CCW, 1 = CW): a clockwise arc from `start`
    // to `end` traces the same span as a counter-clockwise arc from `end`
    // to `start`, so swapping the pair normalises to the CCW-only
    // convention `od_geom2d::Arc2` (and DXF's own reader) already use.
    let (from, to) = if direction == 1 {
        (end, start)
    } else {
        (start, end)
    };
    let center = Point2::new(cx, cy);
    let arc = Arc2::from_start_end_ccw(center, radius, from.to_radians(), to.to_radians());
    Some((
        ShapeStyle {
            layer_code,
            color_code: Some(color_code),
            linetype_code: Some(linetype_code),
            width_code: Some(width_code),
            font_code: None,
        },
        Geometry::Arc {
            center: Point3::new(cx, cy, 0.0),
            radius,
            start_angle: arc.start_angle,
            sweep: arc.sweep,
            normal: Vec3::Z,
        },
    ))
}

fn parse_ellipse(p: &[String]) -> Option<(ShapeStyle, Geometry)> {
    let (layer_code, color_code, linetype_code, width_code) = codes4(p)?;
    let cx: f64 = p.get(4)?.parse().ok()?;
    let cy: f64 = p.get(5)?.parse().ok()?;
    let rx: f64 = p.get(6)?.parse().ok()?;
    let ry: f64 = p.get(7)?.parse().ok()?;
    let rot: f64 = p.get(8)?.parse().ok()?;
    if rx <= tol::POINT_EPS {
        return None;
    }
    let rot = rot.to_radians();
    Some((
        ShapeStyle {
            layer_code,
            color_code: Some(color_code),
            linetype_code: Some(linetype_code),
            width_code: Some(width_code),
            font_code: None,
        },
        Geometry::Ellipse {
            center: Point3::new(cx, cy, 0.0),
            major_axis: Vec3::new(rx * rot.cos(), rx * rot.sin(), 0.0),
            ratio: ry / rx,
            start_param: 0.0,
            end_param: std::f64::consts::TAU,
            normal: Vec3::Z,
        },
    ))
}

fn parse_ellipse_arc(p: &[String]) -> Option<(ShapeStyle, Geometry)> {
    let (layer_code, color_code, linetype_code, width_code) = codes4(p)?;
    let cx: f64 = p.get(4)?.parse().ok()?;
    let cy: f64 = p.get(5)?.parse().ok()?;
    let rx: f64 = p.get(6)?.parse().ok()?;
    let ry: f64 = p.get(7)?.parse().ok()?;
    let direction: i32 = p.get(8)?.parse().ok()?;
    let rot: f64 = p.get(9)?.parse().ok()?;
    let start: f64 = p.get(10)?.parse().ok()?;
    let end: f64 = p.get(11)?.parse().ok()?;
    if rx <= tol::POINT_EPS {
        return None;
    }
    // Same CW/CCW normalisation as `parse_arc`.
    let (from, to) = if direction == 1 {
        (end, start)
    } else {
        (start, end)
    };
    let rot = rot.to_radians();
    Some((
        ShapeStyle {
            layer_code,
            color_code: Some(color_code),
            linetype_code: Some(linetype_code),
            width_code: Some(width_code),
            font_code: None,
        },
        Geometry::Ellipse {
            center: Point3::new(cx, cy, 0.0),
            major_axis: Vec3::new(rx * rot.cos(), rx * rot.sin(), 0.0),
            ratio: ry / rx,
            start_param: from.to_radians(),
            end_param: to.to_radians(),
            normal: Vec3::Z,
        },
    ))
}

fn parse_text(p: &[String]) -> Option<(ShapeStyle, Geometry)> {
    let layer_code = p.first()?.parse().ok()?;
    let color_code: u32 = p.get(1)?.parse().ok()?;
    let font_code: u32 = p.get(2)?.parse().ok()?;
    let text = p.get(3)?.clone();
    let x: f64 = p.get(4)?.parse().ok()?;
    let y: f64 = p.get(5)?.parse().ok()?;
    let height: f64 = p.get(6)?.parse().ok()?;
    // params 7 (width box) and 8 (character spacing) have no equivalent on
    // `TextEntity` — narrow scope, documented in the crate docs.
    let angle: f64 = p.get(9)?.parse().ok()?;
    let slant: f64 = p.get(10)?.parse().ok()?;
    let bpnt: i32 = p.get(11)?.parse().ok()?;
    let direct: i32 = p.get(12)?.parse().ok()?;

    let (h_align, v_align) = match bpnt {
        1 => (od_core::HAlign::Left, od_core::VAlign::Bottom),
        2 => (od_core::HAlign::Center, od_core::VAlign::Bottom),
        3 => (od_core::HAlign::Right, od_core::VAlign::Bottom),
        4 => (od_core::HAlign::Left, od_core::VAlign::Middle),
        5 => (od_core::HAlign::Center, od_core::VAlign::Middle),
        6 => (od_core::HAlign::Right, od_core::VAlign::Middle),
        7 => (od_core::HAlign::Left, od_core::VAlign::Top),
        8 => (od_core::HAlign::Center, od_core::VAlign::Top),
        9 => (od_core::HAlign::Right, od_core::VAlign::Top),
        _ => return None,
    };
    let flow = if direct == 2 {
        od_core::TextFlow::Vertical
    } else {
        od_core::TextFlow::Horizontal
    };

    Some((
        ShapeStyle {
            layer_code,
            color_code: Some(color_code),
            linetype_code: None,
            width_code: None,
            font_code: Some(font_code),
        },
        Geometry::Text(Box::new(TextEntity {
            position: Point3::new(x, y, 0.0),
            value: text,
            height,
            rotation: angle.to_radians(),
            // A placeholder: `parse_text` only sees raw parameters, not
            // the `Tables` needed to resolve a font code into a real
            // `TextStyle` id. `read_shape` overwrites this from
            // `ShapeStyle::font_code` before the entity is inserted, or
            // skips insertion (with a warning) if the code has no
            // definition — the same "reference, not malformed" handling
            // colour/linetype/width already get.
            style: od_core::ObjectId::new(od_core::ActorId::SYSTEM, 0),
            flow,
            h_align,
            v_align,
            width_factor: 1.0,
            oblique: slant.to_radians(),
        })),
    ))
}
