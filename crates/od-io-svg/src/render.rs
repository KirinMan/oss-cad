//! Rendering a drawing to SVG.

use crate::color::{aci_to_rgb, hex, luminance};
use crate::{Background, SvgOptions};
use od_core::{Color, Database, Entity, Geometry, GraphicStyle, LineWeight, Point3, ResolvedStyle};
use od_geom3d::Aabb3;
use od_index::DrawingIndex;
use std::fmt::Write as _;

/// How deep a block reference chain is followed before it is treated as a
/// cycle. Matches the document model's own limit.
const MAX_BLOCK_DEPTH: usize = 32;

/// Hairline width, in drawing millimetres. What a zero or default lineweight
/// plots at.
const HAIRLINE_MM: f64 = 0.13;

pub(crate) fn render(db: &Database, options: &SvgOptions) -> String {
    let space = options.space.unwrap_or_else(|| db.model_space());
    let index = DrawingIndex::build(db, space);

    let extents = options.window.unwrap_or_else(|| index.bounds());
    let visible = if extents.is_empty() {
        Vec::new()
    } else {
        index.query(db, extents)
    };

    let mut out = String::with_capacity(64 * 1024);
    write_open(&mut out, &extents, options);

    // A stroke width is in drawing units, so a hairline at building scale would
    // be invisible. Scale the floor with the drawing's size, the way a plotted
    // sheet does.
    let scale_hint = extents.size().x.max(extents.size().y).max(1.0);
    let ctx = Ctx {
        db,
        options,
        min_stroke: (scale_hint / 4000.0).max(HAIRLINE_MM),
    };

    let mut painted = 0usize;
    for id in visible {
        if painted >= options.max_entities {
            break;
        }
        let Some(entity) = db.entity(id) else {
            continue;
        };
        if !is_visible(db, entity, options) {
            continue;
        }
        let style = resolve(db, entity, None);
        paint(&mut out, &ctx, entity, &style, 0);
        painted += 1;
    }

    out.push_str("</g>\n</svg>\n");
    out
}

struct Ctx<'a> {
    db: &'a Database,
    options: &'a SvgOptions,
    min_stroke: f64,
}

fn write_open(out: &mut String, extents: &Aabb3, options: &SvgOptions) {
    let (w, h, min_x, max_y) = if extents.is_empty() {
        (100.0, 100.0, 0.0, 100.0)
    } else {
        let size = extents.size();
        // A drawing that is a single straight line has no height; give it one
        // so the viewBox is valid and the line is actually on screen.
        let w = if size.x > 0.0 {
            size.x
        } else {
            size.y.max(1.0)
        };
        let h = if size.y > 0.0 {
            size.y
        } else {
            size.x.max(1.0)
        };
        (w, h, extents.min.x, extents.min.y + h)
    };

    let pad = (w.max(h)) * 0.02;
    let view = format!(
        "{} {} {} {}",
        min_x - pad,
        -(max_y + pad),
        w + pad * 2.0,
        h + pad * 2.0
    );

    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{view}""#
    );
    if let Some(px) = options.width_px {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a pixel height derived from a positive aspect ratio"
        )]
        let height = (f64::from(px) * (h / w.max(1e-9))).round().max(1.0) as u32;
        let _ = write!(out, r#" width="{px}" height="{height}""#);
    }
    let _ = writeln!(out, r#" fill="none">"#);

    if let Some(colour) = options.background.fill() {
        let _ = writeln!(
            out,
            r#"<rect x="-1e9" y="-1e9" width="2e9" height="2e9" fill="{colour}"/>"#
        );
    }

    // Everything is drawn in drawing coordinates inside a Y-flipped group, so
    // no geometry needs converting on the way out. A conversion applied here
    // would be one more place for a coordinate to go wrong.
    out.push_str(r#"<g transform="scale(1 -1)" stroke-linecap="round" stroke-linejoin="round">"#);
    out.push('\n');
}

fn is_visible(db: &Database, entity: &Entity, options: &SvgOptions) -> bool {
    if !entity.visible {
        return false;
    }
    let Some(layer) = db.tables.layers.get(entity.layer) else {
        return true;
    };
    if !layer.is_drawable() {
        return false;
    }
    match &options.layers {
        Some(wanted) => wanted.iter().any(|n| n.eq_ignore_ascii_case(&layer.name)),
        None => true,
    }
}

/// Resolves ByLayer/ByBlock against the layer and the containing block.
fn resolve(db: &Database, entity: &Entity, block: Option<&ResolvedStyle>) -> ResolvedStyle {
    let layer_style = db.tables.layers.get(entity.layer).map_or(
        ResolvedStyle {
            color: Color::FOREGROUND,
            lineweight: LineWeight::Default,
            transparency: 0,
        },
        |l| ResolvedStyle {
            color: l.color,
            lineweight: l.lineweight,
            transparency: l.transparency,
        },
    );
    entity.style.resolve(&layer_style, block)
}

fn paint(out: &mut String, ctx: &Ctx<'_>, entity: &Entity, style: &ResolvedStyle, depth: usize) {
    let attrs = attributes(ctx, style, &entity.style);

    match &entity.geom {
        Geometry::Line { a, b } => {
            let _ = writeln!(
                out,
                r#"<line x1="{}" y1="{}" x2="{}" y2="{}" {attrs}/>"#,
                n(a.x),
                n(a.y),
                n(b.x),
                n(b.y)
            );
        }

        Geometry::Point(p) => {
            // A point has no extent; drawn as a small cross so it can be seen
            // and so it does not vanish at any zoom.
            let r = ctx.min_stroke * 3.0;
            let _ = writeln!(
                out,
                r#"<path d="M {} {} L {} {} M {} {} L {} {}" {attrs}/>"#,
                n(p.x - r),
                n(p.y),
                n(p.x + r),
                n(p.y),
                n(p.x),
                n(p.y - r),
                n(p.x),
                n(p.y + r)
            );
        }

        Geometry::Circle { center, radius, .. } => {
            let _ = writeln!(
                out,
                r#"<circle cx="{}" cy="{}" r="{}" {attrs}/>"#,
                n(center.x),
                n(center.y),
                n(*radius)
            );
        }

        Geometry::Arc {
            center,
            radius,
            start_angle,
            sweep,
            ..
        } => {
            let _ = writeln!(
                out,
                r#"<path d="{}" {attrs}/>"#,
                arc_path(*center, *radius, *start_angle, *sweep)
            );
        }

        Geometry::Ellipse {
            center,
            major_axis,
            ratio,
            ..
        } => {
            let rx = major_axis.length();
            let ry = rx * ratio.abs();
            let rotation = major_axis.y.atan2(major_axis.x).to_degrees();
            let _ = writeln!(
                out,
                r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" transform="rotate({} {} {})" {attrs}/>"#,
                n(center.x),
                n(center.y),
                n(rx),
                n(ry),
                n(rotation),
                n(center.x),
                n(center.y)
            );
        }

        Geometry::Polyline { polyline, .. } => {
            if let Some(d) = polyline_path(polyline) {
                let _ = writeln!(out, r#"<path d="{d}" {attrs}/>"#);
            }
        }

        Geometry::Polyline3d { points, closed } => {
            if points.len() >= 2 {
                let mut d = format!("M {} {}", n(points[0].x), n(points[0].y));
                for p in &points[1..] {
                    let _ = write!(d, " L {} {}", n(p.x), n(p.y));
                }
                if *closed {
                    d.push_str(" Z");
                }
                let _ = writeln!(out, r#"<path d="{d}" {attrs}/>"#);
            }
        }

        Geometry::Spline { control_points, .. } => {
            // The control polygon, not the curve. An approximation that is
            // visibly an approximation beats one that looks authoritative and
            // is subtly wrong; NURBS evaluation belongs in od-geom2d.
            if control_points.len() >= 2 {
                let mut d = format!("M {} {}", n(control_points[0].x), n(control_points[0].y));
                for p in &control_points[1..] {
                    let _ = write!(d, " L {} {}", n(p.x), n(p.y));
                }
                let _ = writeln!(out, r#"<path d="{d}" {attrs} stroke-dasharray="4 4"/>"#);
            }
        }

        Geometry::Text(t) => {
            write_text(out, ctx, &t.value, t.position, t.height, t.rotation, style)
        }
        Geometry::MText(t) => {
            write_text(out, ctx, &t.value, t.position, t.height, t.rotation, style);
        }

        Geometry::Hatch(h) => {
            // Boundaries only, matching what the DXF writer does until patterns
            // are modelled.
            for lp in &h.loops {
                if let Some(d) = polyline_path(lp) {
                    let _ = writeln!(out, r#"<path d="{d}" {attrs}/>"#);
                }
            }
        }

        Geometry::BlockRef(bref) => {
            if depth >= MAX_BLOCK_DEPTH {
                return;
            }
            let Some(def) = ctx.db.tables.blocks.get(bref.block) else {
                return;
            };
            let base = def.base_point;
            let _ = writeln!(
                out,
                r#"<g transform="translate({} {}) rotate({}) scale({} {}) translate({} {})">"#,
                n(bref.position.x),
                n(bref.position.y),
                n(bref.rotation.to_degrees()),
                n(bref.scale.x),
                n(bref.scale.y),
                n(-base.x),
                n(-base.y)
            );
            let members = def.entities.clone();
            for member_id in members {
                if let Some(member) = ctx.db.entity(member_id) {
                    if !is_visible(ctx.db, member, ctx.options) {
                        continue;
                    }
                    // Inside a block, ByBlock resolves against the reference.
                    let inner = resolve(ctx.db, member, Some(style));
                    paint(out, ctx, member, &inner, depth + 1);
                }
            }
            out.push_str("</g>\n");
        }

        Geometry::Solid3d { .. } => {}

        Geometry::Unsupported { proxy, .. } => {
            for graphic in proxy {
                match graphic {
                    od_core::ProxyGraphic::Polyline { points, closed } => {
                        if points.len() >= 2 {
                            let mut d = format!("M {} {}", n(points[0].x), n(points[0].y));
                            for p in &points[1..] {
                                let _ = write!(d, " L {} {}", n(p.x), n(p.y));
                            }
                            if *closed {
                                d.push_str(" Z");
                            }
                            let _ = writeln!(out, r#"<path d="{d}" {attrs}/>"#);
                        }
                    }
                    od_core::ProxyGraphic::Text {
                        position,
                        value,
                        height,
                    } => write_text(out, ctx, value, *position, *height, 0.0, style),
                }
            }
        }
    }
}

fn write_text(
    out: &mut String,
    ctx: &Ctx<'_>,
    value: &str,
    position: Point3,
    height: f64,
    rotation: f64,
    style: &ResolvedStyle,
) {
    if value.is_empty() {
        return;
    }
    let fill = hex(paint_colour(ctx, style));
    // Text is drawn inside the Y-flipped group, so it needs flipping back or
    // every label reads upside down.
    let _ = writeln!(
        out,
        r#"<text x="0" y="0" transform="translate({} {}) scale(1 -1) rotate({})" font-size="{}" fill="{fill}" stroke="none" font-family="{}">{}</text>"#,
        n(position.x),
        n(position.y),
        n(-rotation.to_degrees()),
        n(height.max(ctx.min_stroke * 4.0)),
        ctx.options.font_family,
        escape(value)
    );
}

fn attributes(ctx: &Ctx<'_>, style: &ResolvedStyle, raw: &GraphicStyle) -> String {
    let colour = hex(paint_colour(ctx, style));
    let width = style
        .lineweight
        .millimetres()
        .unwrap_or(HAIRLINE_MM)
        .max(ctx.min_stroke);

    let mut attrs = format!(r#"stroke="{colour}" stroke-width="{}""#, n(width));
    if style.transparency > 0 {
        let alpha = 1.0 - f64::from(style.transparency) / 90.0;
        let _ = write!(attrs, r#" stroke-opacity="{}""#, n(alpha.clamp(0.0, 1.0)));
    }
    // Linetype patterns are per-entity in drawing units; scaling them with the
    // entity's own factor is what makes a dashed line look right at any size.
    if let Some(pattern) = &ctx.options.dash_pattern {
        let scaled: Vec<String> = pattern
            .iter()
            .map(|d| n(d * raw.linetype_scale * ctx.min_stroke * 8.0))
            .collect();
        let _ = write!(attrs, r#" stroke-dasharray="{}""#, scaled.join(" "));
    }
    attrs
}

/// Resolves colour 7 against the background: it is the "draw in the opposite of
/// the paper" colour, and rendering it black on a dark ground makes a drawing
/// that is technically correct and completely unreadable.
fn paint_colour(ctx: &Ctx<'_>, style: &ResolvedStyle) -> (u8, u8, u8) {
    match style.color {
        Color::Rgb { r, g, b } => (r, g, b),
        Color::Index(7) | Color::ByLayer | Color::ByBlock => ctx.options.background.foreground(),
        Color::Index(i) => {
            let rgb = aci_to_rgb(i);
            // A dark colour on a dark ground gets lifted rather than replaced,
            // so the drawing keeps its colour coding.
            if ctx.options.background.is_dark() && luminance(rgb) < 0.15 {
                ctx.options.background.foreground()
            } else {
                rgb
            }
        }
    }
}

fn arc_path(center: Point3, radius: f64, start: f64, sweep: f64) -> String {
    if sweep.abs() >= std::f64::consts::TAU - 1e-9 {
        // A full turn has no distinct endpoints; two half arcs draw it.
        return format!(
            "M {} {} A {} {} 0 1 1 {} {} A {} {} 0 1 1 {} {}",
            n(center.x + radius),
            n(center.y),
            n(radius),
            n(radius),
            n(center.x - radius),
            n(center.y),
            n(radius),
            n(radius),
            n(center.x + radius),
            n(center.y)
        );
    }
    let x1 = center.x + radius * start.cos();
    let y1 = center.y + radius * start.sin();
    let end = start + sweep;
    let x2 = center.x + radius * end.cos();
    let y2 = center.y + radius * end.sin();
    let large = i32::from(sweep.abs() > std::f64::consts::PI);
    // Inside the Y-flipped group, a mathematically counter-clockwise sweep is
    // SVG's positive direction.
    let positive = i32::from(sweep > 0.0);
    format!(
        "M {} {} A {} {} 0 {large} {positive} {} {}",
        n(x1),
        n(y1),
        n(radius),
        n(radius),
        n(x2),
        n(y2)
    )
}

fn polyline_path(pl: &od_core::Polyline2) -> Option<String> {
    let first = pl.vertices.first()?;
    if pl.vertices.len() < 2 {
        return None;
    }
    let mut d = format!("M {} {}", n(first.point.x), n(first.point.y));
    for span in pl.spans() {
        match span {
            od_core::Span::Line(s) => {
                let _ = write!(d, " L {} {}", n(s.b.x), n(s.b.y));
            }
            od_core::Span::Arc(a) => {
                let end = a.end_point();
                let large = i32::from(a.sweep.abs() > std::f64::consts::PI);
                let positive = i32::from(a.sweep > 0.0);
                let _ = write!(
                    d,
                    " A {} {} 0 {large} {positive} {} {}",
                    n(a.radius),
                    n(a.radius),
                    n(end.x),
                    n(end.y)
                );
            }
        }
    }
    if pl.closed {
        d.push_str(" Z");
    }
    Some(d)
}

/// Formats a coordinate compactly. Six decimals is a nanometre at drawing
/// scale — well past what any output device resolves, and far shorter than the
/// default rendering of an f64.
fn n(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_owned();
    }
    let rounded = (v * 1e6).round() / 1e6;
    if (rounded - rounded.trunc()).abs() < f64::EPSILON {
        format!("{}", rounded.trunc())
    } else {
        format!("{rounded}")
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

pub(crate) fn background_fill(bg: Background) -> Option<&'static str> {
    match bg {
        Background::Paper => Some("#ffffff"),
        Background::Dark => Some("#141d26"),
        Background::None => None,
    }
}

pub(crate) fn background_foreground(bg: Background) -> (u8, u8, u8) {
    match bg {
        Background::Dark => (223, 230, 235),
        // With no background painted, the page could be anything; black is the
        // safe assumption because documents default to light.
        Background::Paper | Background::None => (0, 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_are_written_compactly() {
        assert_eq!(n(0.0), "0");
        assert_eq!(n(1000.0), "1000");
        assert_eq!(n(-3.5), "-3.5");
        assert_eq!(n(1.0 / 3.0), "0.333333");
        assert_eq!(n(f64::NAN), "0", "a NaN must not reach the output");
    }

    #[test]
    fn text_is_escaped() {
        assert_eq!(escape("A & B"), "A &amp; B");
        assert_eq!(escape("<script>"), "&lt;script&gt;");
        assert_eq!(escape("給気ダクト"), "給気ダクト", "CJK passes through");
    }

    #[test]
    fn a_full_circle_arc_is_drawn_as_two_halves() {
        let d = arc_path(Point3::ORIGIN, 10.0, 0.0, std::f64::consts::TAU);
        assert_eq!(
            d.matches('A').count(),
            2,
            "one arc command cannot close a circle"
        );
    }

    #[test]
    fn arc_direction_follows_the_sweep_sign() {
        let ccw = arc_path(Point3::ORIGIN, 10.0, 0.0, std::f64::consts::FRAC_PI_2);
        let cw = arc_path(Point3::ORIGIN, 10.0, 0.0, -std::f64::consts::FRAC_PI_2);
        assert!(
            ccw.contains(" 0 1 "),
            "counter-clockwise is the positive flag"
        );
        assert!(cw.contains(" 0 0 "), "clockwise is the negative flag");
    }
}
