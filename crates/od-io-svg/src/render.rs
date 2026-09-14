//! Rendering a drawing to SVG.

use crate::color::{aci_to_rgb, hex, luminance};
use crate::{Background, SvgOptions};
use od_core::{
    Color, Database, Entity, Geometry, GraphicStyle, LineWeight, ObjectId, Point3, ResolvedStyle,
};
use od_geom3d::Aabb3;
use od_index::DrawingIndex;
use std::fmt::Write as _;

/// How deep a block reference chain is followed before it is treated as a
/// cycle. Matches the document model's own limit.
const MAX_BLOCK_DEPTH: usize = 32;

/// Hairline width, in drawing millimetres. What a zero or default lineweight
/// plots at.
const HAIRLINE_MM: f64 = 0.13;

/// The SVG's `viewBox`, in drawing millimetres and already Y-flipped to match
/// the coordinate space the SVG itself uses. A client that wants to turn a
/// click on the rendered image back into a drawing coordinate needs exactly
/// this — not the drawing's raw extents, which the padding here and the
/// degenerate-extent fallbacks below both shift away from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewBox {
    pub min_x: f64,
    pub min_y: f64,
    pub width: f64,
    pub height: f64,
}

pub(crate) fn render(db: &Database, options: &SvgOptions) -> (String, ViewBox) {
    let space = options.space.unwrap_or_else(|| db.model_space());
    let index = DrawingIndex::build(db, space);

    let extents = options.window.unwrap_or_else(|| index.bounds());
    let visible = if extents.is_empty() {
        Vec::new()
    } else {
        index.query(db, extents)
    };

    let mut out = String::with_capacity(64 * 1024);
    let view_box = write_open(&mut out, &extents, options);

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
        paint(&mut out, &ctx, id, entity, &style, 0);
        painted += 1;
    }

    out.push_str("</g>\n</svg>\n");
    (out, view_box)
}

struct Ctx<'a> {
    db: &'a Database,
    options: &'a SvgOptions,
    min_stroke: f64,
}

/// A blank drawing's fallback `viewBox` size, in millimetres — roughly one
/// floor of a small building (20m square), centred on the origin so a click
/// near the middle of a freshly created drawing lands near `(0, 0)` rather
/// than at a corner. Not just a rendering nicety: an editing canvas turns a
/// click on the image back into a drawing coordinate through this
/// `ViewBox` (see its doc comment), so a too-small fallback here means
/// every click on a brand-new drawing lands within a few centimetres of
/// where it started, regardless of how far the canvas visually appears to
/// span. 100mm — office-drawer-drawing-paper-sized, not building-sized —
/// was the previous value and made a freshly created drawing nearly
/// unusable to draw on before anything else had been added to give the
/// extents a real size.
const EMPTY_DRAWING_SIZE_MM: f64 = 20_000.0;

fn write_open(out: &mut String, extents: &Aabb3, options: &SvgOptions) -> ViewBox {
    let (w, h, min_x, max_y) = if extents.is_empty() {
        let half = EMPTY_DRAWING_SIZE_MM / 2.0;
        (EMPTY_DRAWING_SIZE_MM, EMPTY_DRAWING_SIZE_MM, -half, half)
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
    let view_box = ViewBox {
        min_x: min_x - pad,
        min_y: -(max_y + pad),
        width: w + pad * 2.0,
        height: h + pad * 2.0,
    };
    let view = format!(
        "{} {} {} {}",
        view_box.min_x, view_box.min_y, view_box.width, view_box.height
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
        // Sized to the viewBox itself, not a "cover everything" rect: at the
        // extreme aspect ratios a single line or a short text label produces
        // (a viewBox a few units tall and thousands wide), a rect running
        // from -1e9 to 1e9 sits far enough outside the visible area that
        // Chromium's SVG rasteriser has been observed to paint only a sliver
        // of it instead of the whole viewport — found by placing a single
        // dimension in the editor and watching the "paper" disappear behind
        // the drawing. The viewBox bounds are provably enough: nothing
        // outside them is ever visible.
        let _ = writeln!(
            out,
            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{colour}"/>"#,
            n(view_box.min_x),
            n(view_box.min_y),
            n(view_box.width),
            n(view_box.height)
        );
    }

    // Everything is drawn in drawing coordinates inside a Y-flipped group, so
    // no geometry needs converting on the way out. A conversion applied here
    // would be one more place for a coordinate to go wrong.
    out.push_str(r#"<g transform="scale(1 -1)" stroke-linecap="round" stroke-linejoin="round">"#);
    out.push('\n');
    view_box
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

fn paint(
    out: &mut String,
    ctx: &Ctx<'_>,
    id: ObjectId,
    entity: &Entity,
    style: &ResolvedStyle,
    depth: usize,
) {
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

        Geometry::Dimension(d) => render_dimension(out, ctx, d, style, &attrs),

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
                    paint(out, ctx, member_id, member, &inner, depth + 1);
                }
            }
            out.push_str("</g>\n");
        }

        Geometry::Viewport(vp) => {
            // The paper-space boundary, in the viewport entity's own style —
            // drawn even if the nested content below is skipped, so a
            // recursion-guarded or empty viewport still shows where it is.
            let corners = vp.corners();
            let mut d = format!("M {} {}", n(corners[0].x), n(corners[0].y));
            for c in &corners[1..] {
                let _ = write!(d, " L {} {}", n(c.x), n(c.y));
            }
            d.push_str(" Z");
            let _ = writeln!(out, r#"<path d="{d}" {attrs}/>"#);

            if depth >= MAX_BLOCK_DEPTH {
                return;
            }
            let window = vp.model_window();
            if window.is_empty() {
                return;
            }
            // Z-unbounded, like `od query --window`'s own parsing: a
            // viewport frames an XY rectangle of model space, not a slab, so
            // an entity at any elevation within it should still show.
            let query_window = Aabb3::new(
                Point3::new(window.min.x, window.min.y, f64::NEG_INFINITY),
                Point3::new(window.max.x, window.max.y, f64::INFINITY),
            );

            // A real render of the model-space window this viewport frames,
            // not just its outline: query the same spatial index the
            // top-level render() uses, clipped to the paper-space rectangle
            // and mapped into it by translate/scale/translate, the same
            // three-step shape BlockRef above uses for its own nesting.
            let model_space = ctx.db.model_space();
            let index = DrawingIndex::build(ctx.db, model_space);
            let visible = index.query(ctx.db, query_window);

            let (hw, hh) = (vp.width / 2.0, vp.height / 2.0);
            let clip_id = format!("vp-clip-{}-{}", id.actor.0, id.seq);
            let _ = writeln!(
                out,
                r#"<clipPath id="{clip_id}"><rect x="{}" y="{}" width="{}" height="{}"/></clipPath>"#,
                n(vp.position.x - hw),
                n(vp.position.y - hh),
                n(vp.width),
                n(vp.height)
            );
            let _ = writeln!(
                out,
                r#"<g clip-path="url(#{clip_id})" transform="translate({} {}) scale({}) translate({} {})">"#,
                n(vp.position.x),
                n(vp.position.y),
                n(vp.scale),
                n(-vp.target.x),
                n(-vp.target.y)
            );
            for member_id in visible {
                let Some(member) = ctx.db.entity(member_id) else {
                    continue;
                };
                if !is_visible(ctx.db, member, ctx.options) {
                    continue;
                }
                let inner = resolve(ctx.db, member, None);
                paint(out, ctx, member_id, member, &inner, depth + 1);
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

/// The `Standard` `DimStyle`'s own defaults (`Database::new`) — used only if
/// `d.style` somehow does not resolve, which `AddText`/`AddDimension` never
/// produce; a rendering fallback should never be worse than "looks like the
/// default style", not a missing dimension.
const FALLBACK_TEXT_HEIGHT_MM: f64 = 2.5;
const FALLBACK_ARROW_SIZE_MM: f64 = 2.5;
const FALLBACK_EXTENSION_OFFSET_MM: f64 = 0.625;
const FALLBACK_EXTENSION_BEYOND_MM: f64 = 1.25;

fn render_dimension(
    out: &mut String,
    ctx: &Ctx<'_>,
    d: &od_core::DimensionEntity,
    style: &ResolvedStyle,
    attrs: &str,
) {
    let dim_style = ctx.db.tables.dim_styles.get(d.style);
    let scale = dim_style.map_or(1.0, |s| s.scale);
    let text_height = dim_style.map_or(FALLBACK_TEXT_HEIGHT_MM, |s| s.text_height) * scale;
    let arrow_size = dim_style.map_or(FALLBACK_ARROW_SIZE_MM, |s| s.arrow_size) * scale;
    let extension_offset =
        dim_style.map_or(FALLBACK_EXTENSION_OFFSET_MM, |s| s.extension_offset) * scale;
    let extension_beyond =
        dim_style.map_or(FALLBACK_EXTENSION_BEYOND_MM, |s| s.extension_beyond) * scale;

    let (line_a, line_b) = d.dimension_line();

    // The direction each extension line runs — perpendicular to the measured
    // segment, i.e. exactly the direction from a measured point to its own
    // end of the dimension line. Degenerate (zero-length or zero-offset)
    // dimensions draw no extension lines rather than dividing by zero.
    let ext_dir = |from: Point3, to: Point3| -> Option<(f64, f64)> {
        let (dx, dy) = (to.x - from.x, to.y - from.y);
        let len = dx.hypot(dy);
        (len > od_core::tol::POINT_EPS).then_some((dx / len, dy / len))
    };

    if let Some((ux, uy)) = ext_dir(d.point_a, line_a) {
        let start = (
            d.point_a.x + ux * extension_offset,
            d.point_a.y + uy * extension_offset,
        );
        let end = (
            line_a.x + ux * extension_beyond,
            line_a.y + uy * extension_beyond,
        );
        let _ = writeln!(
            out,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" {attrs}/>"#,
            n(start.0),
            n(start.1),
            n(end.0),
            n(end.1)
        );
    }
    if let Some((ux, uy)) = ext_dir(d.point_b, line_b) {
        let start = (
            d.point_b.x + ux * extension_offset,
            d.point_b.y + uy * extension_offset,
        );
        let end = (
            line_b.x + ux * extension_beyond,
            line_b.y + uy * extension_beyond,
        );
        let _ = writeln!(
            out,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" {attrs}/>"#,
            n(start.0),
            n(start.1),
            n(end.0),
            n(end.1)
        );
    }

    let _ = writeln!(
        out,
        r#"<line x1="{}" y1="{}" x2="{}" y2="{}" {attrs}/>"#,
        n(line_a.x),
        n(line_a.y),
        n(line_b.x),
        n(line_b.y)
    );

    // Arrowheads point inward, toward each other along the dimension line —
    // the standard drafting convention, and the opposite of the outward
    // extension lines above.
    let fill = hex(paint_colour(ctx, style));
    if let Some((ux, uy)) = ext_dir(line_b, line_a) {
        write_arrowhead(out, line_a, (ux, uy), arrow_size, &fill);
        write_arrowhead(out, line_b, (-ux, -uy), arrow_size, &fill);
    }

    let mid = Point3::new(
        (line_a.x + line_b.x) / 2.0,
        (line_a.y + line_b.y) / 2.0,
        line_a.z,
    );
    // Nudged off the dimension line by the text height itself, in whichever
    // direction the extension lines already run, so the label sits above
    // the line rather than straddling it.
    let (label_x, label_y) = match ext_dir(d.point_a, line_a) {
        Some((ux, uy)) => (mid.x + ux * text_height, mid.y + uy * text_height),
        None => (mid.x, mid.y + text_height),
    };
    let mut angle = (line_b.y - line_a.y).atan2(line_b.x - line_a.x);
    // Kept within ±90° of horizontal — the same "never upside down" rule a
    // human drafter applies, rather than a literal reading of whatever
    // direction point_a happened to be clicked before point_b.
    if !(-std::f64::consts::FRAC_PI_2..=std::f64::consts::FRAC_PI_2).contains(&angle) {
        angle += std::f64::consts::PI;
    }
    let text = d
        .text_override
        .clone()
        .unwrap_or_else(|| format_dimension_length(d.measured_length(), dim_style));
    write_text(
        out,
        ctx,
        &text,
        Point3::new(label_x, label_y, mid.z),
        text_height,
        angle,
        style,
    );
}

fn write_arrowhead(out: &mut String, tip: Point3, direction: (f64, f64), size: f64, fill: &str) {
    // A narrow triangle, base perpendicular to `direction`, pointing along it.
    let (dx, dy) = direction;
    let (px, py) = (-dy, dx);
    let half_width = size * 0.15;
    let base_x = tip.x - dx * size;
    let base_y = tip.y - dy * size;
    let _ = writeln!(
        out,
        r#"<polygon points="{},{} {},{} {},{}" fill="{fill}" stroke="none"/>"#,
        n(tip.x),
        n(tip.y),
        n(base_x + px * half_width),
        n(base_y + py * half_width),
        n(base_x - px * half_width),
        n(base_y - py * half_width)
    );
}

/// Formats a measured length per `style`'s decimal places, trimming trailing
/// zeros (and a bare trailing `.`) when the style asks for that — the JIS
/// convention `DimStyle`'s own doc comment calls out, and the reason
/// `Database::new`'s default style sets it.
fn format_dimension_length(value: f64, style: Option<&od_core::DimStyle>) -> String {
    let decimals = style.map_or(0, |s| usize::from(s.decimal_places));
    let suppress = style.is_none_or(|s| s.suppress_trailing_zeros);
    let formatted = format!("{value:.decimals$}");
    if !suppress || !formatted.contains('.') {
        return formatted;
    }
    let trimmed = formatted.trim_end_matches('0');
    trimmed.strip_suffix('.').unwrap_or(trimmed).to_owned()
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
