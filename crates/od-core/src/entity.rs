//! Entities and their geometry.
//!
//! Every graphical thing in a drawing is an [`Entity`]: a layer reference, a
//! [`Geometry`], a [`GraphicStyle`] and the block record that owns it. Model
//! space and each paper space layout are themselves block records, so there is
//! no separate notion of "which space am I in" to keep consistent — the owner
//! answers it.

use crate::id::ObjectId;
use crate::style::GraphicStyle;
use od_geom2d::{Point2, Polyline2};
use od_geom3d::{Aabb3, Point3, SolidHandle, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub layer: ObjectId,
    pub geom: Geometry,
    #[serde(default)]
    pub style: GraphicStyle,
    #[serde(default = "default_true")]
    pub visible: bool,
    /// The block record this entity belongs to (model space, a layout, or a
    /// block definition).
    pub owner_space: ObjectId,
}

fn default_true() -> bool {
    true
}

impl Entity {
    #[must_use]
    pub fn new(layer: ObjectId, owner_space: ObjectId, geom: Geometry) -> Self {
        Self {
            layer,
            geom,
            style: GraphicStyle::default(),
            visible: true,
            owner_space,
        }
    }

    #[must_use]
    pub fn with_style(mut self, style: GraphicStyle) -> Self {
        self.style = style;
        self
    }
}

/// Text that is horizontal, vertical, or laid out along a path.
///
/// Vertical writing is a first-class variant rather than a rotation, because
/// Japanese vertical text reorders and re-forms glyphs rather than turning them,
/// and a renderer that treats it as rotation produces text a reviewer will
/// reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextFlow {
    #[default]
    Horizontal,
    /// 縦書き — top to bottom, columns right to left.
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
    /// Stretched to fit between two points.
    Fit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VAlign {
    #[default]
    Baseline,
    Bottom,
    Middle,
    Top,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextEntity {
    pub position: Point3,
    pub value: String,
    pub height: f64,
    /// Radians, counter-clockwise.
    pub rotation: f64,
    pub style: ObjectId,
    #[serde(default)]
    pub flow: TextFlow,
    #[serde(default)]
    pub h_align: HAlign,
    #[serde(default)]
    pub v_align: VAlign,
    /// Width factor; 1.0 leaves glyph proportions alone.
    pub width_factor: f64,
    /// Oblique angle in radians.
    #[serde(default)]
    pub oblique: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MTextEntity {
    pub position: Point3,
    /// Raw contents, including inline formatting codes.
    pub value: String,
    pub height: f64,
    pub rotation: f64,
    pub style: ObjectId,
    /// Wrap width; 0 means no wrapping.
    pub width: f64,
    pub line_spacing: f64,
    #[serde(default)]
    pub flow: TextFlow,
}

/// A block insertion, optionally arrayed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockRef {
    pub block: ObjectId,
    pub position: Point3,
    pub scale: Vec3,
    pub rotation: f64,
    /// Values for the block's attribute definitions, by tag.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<AttributeValue>,
    /// Rectangular array counts; `(1, 1)` for a plain insert.
    #[serde(default = "unit_array")]
    pub array: (u32, u32),
    #[serde(default)]
    pub array_spacing: (f64, f64),
}

fn unit_array() -> (u32, u32) {
    (1, 1)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttributeValue {
    pub tag: String,
    pub value: String,
}

/// A hatch or gradient fill bounded by closed loops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hatch {
    /// Outer loop first; islands follow.
    pub loops: Vec<Polyline2>,
    pub elevation: f64,
    pub pattern: HatchPattern,
    /// Pattern rotation in radians.
    pub angle: f64,
    pub scale: f64,
    /// True when the boundary is kept associative to its source entities.
    #[serde(default)]
    pub associative: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HatchPattern {
    Solid,
    /// Named pattern from a `.pat` definition, e.g. `ANSI31`.
    Named {
        name: String,
    },
}

/// Geometry an entity can carry.
///
/// [`Geometry::Unsupported`] is not a placeholder to be removed later. It is the
/// mechanism behind the "never destroy what you cannot read" rule: an importer
/// that meets something it does not model keeps the source bytes and a proxy
/// outline, so the entity survives a load-and-save cycle intact and still draws.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Geometry {
    Point(Point3),
    Line {
        a: Point3,
        b: Point3,
    },
    Circle {
        center: Point3,
        radius: f64,
        normal: Vec3,
    },
    Arc {
        center: Point3,
        radius: f64,
        start_angle: f64,
        sweep: f64,
        normal: Vec3,
    },
    Ellipse {
        center: Point3,
        /// Endpoint of the major axis, relative to `center`.
        major_axis: Vec3,
        /// Minor/major length ratio.
        ratio: f64,
        start_param: f64,
        end_param: f64,
        normal: Vec3,
    },
    /// A planar polyline with bulges, at a fixed elevation.
    Polyline {
        polyline: Polyline2,
        elevation: f64,
        normal: Vec3,
        /// Constant width; per-vertex widths live in xdata when present.
        #[serde(default)]
        width: f64,
    },
    Polyline3d {
        points: Vec<Point3>,
        closed: bool,
    },
    Spline {
        degree: u8,
        control_points: Vec<Point3>,
        knots: Vec<f64>,
        weights: Vec<f64>,
        closed: bool,
    },
    Text(Box<TextEntity>),
    MText(Box<MTextEntity>),
    BlockRef(Box<BlockRef>),
    Hatch(Box<Hatch>),
    /// A handle into the solid kernel (ADR-002). The core never inspects it.
    Solid3d {
        handle: SolidHandle,
        /// Cached so bounds and culling work without waking the kernel.
        bounds: Aabb3,
    },
    /// Preserved verbatim from an unsupported source construct.
    Unsupported {
        /// Source-format type name, e.g. `"ACAD_TABLE"`.
        source_type: String,
        /// Opaque original payload, replayed on export to the same format.
        payload: Vec<u8>,
        /// Drawable stand-in so the entity is still visible and selectable.
        proxy: Vec<ProxyGraphic>,
    },
}

/// The fallback rendering for something we preserve but do not understand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProxyGraphic {
    Polyline {
        points: Vec<Point3>,
        closed: bool,
    },
    Text {
        position: Point3,
        value: String,
        height: f64,
    },
}

impl Geometry {
    /// A stable name for diagnostics, inspection reports and CLI output.
    #[must_use]
    pub fn type_name(&self) -> &str {
        match self {
            Geometry::Point(_) => "point",
            Geometry::Line { .. } => "line",
            Geometry::Circle { .. } => "circle",
            Geometry::Arc { .. } => "arc",
            Geometry::Ellipse { .. } => "ellipse",
            Geometry::Polyline { .. } => "polyline",
            Geometry::Polyline3d { .. } => "polyline3d",
            Geometry::Spline { .. } => "spline",
            Geometry::Text(_) => "text",
            Geometry::MText(_) => "mtext",
            Geometry::BlockRef(_) => "blockref",
            Geometry::Hatch(_) => "hatch",
            Geometry::Solid3d { .. } => "solid3d",
            Geometry::Unsupported { source_type, .. } => source_type,
        }
    }

    /// World-space bounds.
    ///
    /// A block reference reports only its insertion point: resolving the block's
    /// contents needs the database, so [`crate::Database::entity_bounds`] does
    /// that. Returning a wrong-but-nonempty box here would quietly corrupt the
    /// spatial index, so this returns the honest minimum instead.
    #[must_use]
    pub fn local_bounds(&self) -> Aabb3 {
        match self {
            Geometry::Point(p) => Aabb3::from_points([*p]),
            Geometry::Line { a, b } => Aabb3::from_points([*a, *b]),
            Geometry::Circle { center, radius, .. } => Aabb3::new(
                Point3::new(center.x - radius, center.y - radius, center.z - radius),
                Point3::new(center.x + radius, center.y + radius, center.z + radius),
            ),
            Geometry::Arc {
                center,
                radius,
                start_angle,
                sweep,
                ..
            } => {
                let arc2 = od_geom2d::Arc2::new(
                    Point2::new(center.x, center.y),
                    *radius,
                    *start_angle,
                    *sweep,
                );
                let b = arc2.bounds();
                Aabb3::new(
                    Point3::new(b.min.x, b.min.y, center.z),
                    Point3::new(b.max.x, b.max.y, center.z),
                )
            }
            Geometry::Ellipse {
                center,
                major_axis,
                ratio,
                ..
            } => {
                // Bounding the full ellipse rather than the swept range: a tight
                // parametric bound is worth doing, but over-reporting here is
                // safe where under-reporting is not.
                let a = major_axis.length();
                let b = a * ratio.abs();
                let r = a.max(b);
                Aabb3::new(
                    Point3::new(center.x - r, center.y - r, center.z - r),
                    Point3::new(center.x + r, center.y + r, center.z + r),
                )
            }
            Geometry::Polyline {
                polyline,
                elevation,
                ..
            } => {
                let b = polyline.bounds();
                if b.is_empty() {
                    Aabb3::EMPTY
                } else {
                    Aabb3::new(
                        Point3::new(b.min.x, b.min.y, *elevation),
                        Point3::new(b.max.x, b.max.y, *elevation),
                    )
                }
            }
            Geometry::Polyline3d { points, .. } => Aabb3::from_points(points.iter().copied()),
            Geometry::Spline { control_points, .. } => {
                // The convex hull of the control points contains the curve.
                Aabb3::from_points(control_points.iter().copied())
            }
            Geometry::Text(t) => Aabb3::from_points([t.position]),
            Geometry::MText(t) => Aabb3::from_points([t.position]),
            Geometry::BlockRef(b) => Aabb3::from_points([b.position]),
            Geometry::Hatch(h) => h.loops.iter().fold(Aabb3::EMPTY, |acc, l| {
                let b = l.bounds();
                if b.is_empty() {
                    acc
                } else {
                    acc.union(Aabb3::new(
                        Point3::new(b.min.x, b.min.y, h.elevation),
                        Point3::new(b.max.x, b.max.y, h.elevation),
                    ))
                }
            }),
            Geometry::Solid3d { bounds, .. } => *bounds,
            Geometry::Unsupported { proxy, .. } => {
                proxy.iter().fold(Aabb3::EMPTY, |acc, g| match g {
                    ProxyGraphic::Polyline { points, .. } => {
                        acc.union(Aabb3::from_points(points.iter().copied()))
                    }
                    ProxyGraphic::Text { position, .. } => acc.union_point(*position),
                })
            }
        }
    }

    /// Points worth snapping to when drawing or moving something else near
    /// this entity — endpoints and centres, not a sampled approximation of
    /// the curve. Deliberately not every mathematically snappable point (a
    /// circle's quadrants, for one): this is the set a click can reach
    /// exactly without first establishing an in-plane basis, which most of
    /// this crate's geometry does not carry.
    #[must_use]
    pub fn snap_points(&self) -> Vec<Point3> {
        match self {
            Geometry::Point(p) => vec![*p],
            Geometry::Line { a, b } => {
                vec![
                    *a,
                    *b,
                    Point3::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0, (a.z + b.z) / 2.0),
                ]
            }
            Geometry::Circle {
                center,
                radius,
                normal,
            } => {
                let mut points = vec![*center];
                // Quadrant points need an in-plane basis, which only exists
                // once `normal` is known to be non-degenerate — a circle with
                // a zero normal is already malformed, and reporting only the
                // centre for it is the honest answer, not a special case.
                if let Some(z) = normal.normalized() {
                    if let Some(x) = z.any_perpendicular() {
                        let y = z.cross(x);
                        points.push(*center + x * *radius);
                        points.push(*center - x * *radius);
                        points.push(*center + y * *radius);
                        points.push(*center - y * *radius);
                    }
                }
                points
            }
            Geometry::Arc {
                center,
                radius,
                start_angle,
                sweep,
                ..
            } => {
                let arc2 = od_geom2d::Arc2::new(
                    Point2::new(center.x, center.y),
                    *radius,
                    *start_angle,
                    *sweep,
                );
                [arc2.start_point(), arc2.end_point(), arc2.midpoint()]
                    .into_iter()
                    .map(|p| Point3::new(p.x, p.y, center.z))
                    .chain(std::iter::once(*center))
                    .collect()
            }
            Geometry::Ellipse { center, .. } => vec![*center],
            Geometry::Polyline {
                polyline,
                elevation,
                ..
            } => polyline
                .vertices
                .iter()
                .map(|v| Point3::new(v.point.x, v.point.y, *elevation))
                .collect(),
            Geometry::Polyline3d { points, .. } => points.clone(),
            Geometry::Spline { control_points, .. } => {
                match (control_points.first(), control_points.last()) {
                    (Some(&first), Some(&last)) => vec![first, last],
                    _ => Vec::new(),
                }
            }
            Geometry::Text(t) => vec![t.position],
            Geometry::MText(t) => vec![t.position],
            Geometry::BlockRef(b) => vec![b.position],
            Geometry::Hatch(_) | Geometry::Solid3d { .. } => Vec::new(),
            Geometry::Unsupported { proxy, .. } => proxy
                .iter()
                .flat_map(|g| match g {
                    ProxyGraphic::Polyline { points, .. } => points.clone(),
                    ProxyGraphic::Text { position, .. } => vec![*position],
                })
                .collect(),
        }
    }

    /// The ordered, individually-draggable points of this entity — a line's
    /// two endpoints, or a polyline's vertices — for an editing canvas to
    /// offer up as grip handles. Index `i` here is exactly the `index` a
    /// [`crate::edit::Command::SetVertex`] targeting this entity means.
    ///
    /// Deliberately narrower than [`Geometry::snap_points`]: a line's
    /// midpoint is worth snapping *to*, but it is not itself a vertex to
    /// drag, and a circle or arc's centre/quadrant points are geometry
    /// *derived from* the stored radius and centre, not independent state a
    /// grip could move without also deciding what should happen to the
    /// radius. Every kind this returns nothing for needs its own kind of
    /// grip — not this one, dressed up.
    #[must_use]
    pub fn editable_vertices(&self) -> Vec<Point3> {
        match self {
            Geometry::Line { a, b } => vec![*a, *b],
            Geometry::Polyline {
                polyline,
                elevation,
                ..
            } => polyline
                .vertices
                .iter()
                .map(|v| Point3::new(v.point.x, v.point.y, *elevation))
                .collect(),
            Geometry::Polyline3d { points, .. } => points.clone(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{ActorId, ObjectId};

    fn id(n: u64) -> ObjectId {
        ObjectId::new(ActorId::SYSTEM, n)
    }

    #[test]
    fn a_line_snaps_to_its_endpoints_and_midpoint() {
        let g = Geometry::Line {
            a: Point3::ORIGIN,
            b: Point3::new(2000.0, 0.0, 0.0),
        };
        let points = g.snap_points();
        assert_eq!(points.len(), 3);
        assert!(points.contains(&Point3::ORIGIN));
        assert!(points.contains(&Point3::new(2000.0, 0.0, 0.0)));
        assert!(points.contains(&Point3::new(1000.0, 0.0, 0.0)));
    }

    #[test]
    fn a_lines_editable_vertices_are_exactly_its_two_endpoints_in_order() {
        let g = Geometry::Line {
            a: Point3::ORIGIN,
            b: Point3::new(2000.0, 0.0, 0.0),
        };
        assert_eq!(
            g.editable_vertices(),
            vec![Point3::ORIGIN, Point3::new(2000.0, 0.0, 0.0)]
        );
    }

    #[test]
    fn a_circle_has_no_editable_vertices() {
        let g = Geometry::Circle {
            center: Point3::ORIGIN,
            radius: 500.0,
            normal: Vec3::Z,
        };
        assert!(g.editable_vertices().is_empty());
    }

    #[test]
    fn an_arc_snaps_to_its_endpoints_midpoint_and_centre() {
        let g = Geometry::Arc {
            center: Point3::new(0.0, 0.0, 500.0),
            radius: 10.0,
            start_angle: 0.0,
            sweep: std::f64::consts::FRAC_PI_2,
            normal: Vec3::Z,
        };
        let points = g.snap_points();
        assert_eq!(points.len(), 4);
        assert!(points.contains(&Point3::new(0.0, 0.0, 500.0)), "centre");
        assert!(
            points
                .iter()
                .any(|p| p.coincides_with(Point3::new(10.0, 0.0, 500.0)))
        );
        assert!(
            points
                .iter()
                .any(|p| p.coincides_with(Point3::new(0.0, 10.0, 500.0)))
        );
    }

    #[test]
    fn a_circle_snaps_to_its_centre_and_quadrants() {
        let g = Geometry::Circle {
            center: Point3::new(100.0, 200.0, 0.0),
            radius: 50.0,
            normal: Vec3::Z,
        };
        let points = g.snap_points();
        assert_eq!(points.len(), 5);
        assert!(points.contains(&Point3::new(100.0, 200.0, 0.0)), "centre");
        for expected in [
            Point3::new(150.0, 200.0, 0.0),
            Point3::new(50.0, 200.0, 0.0),
            Point3::new(100.0, 250.0, 0.0),
            Point3::new(100.0, 150.0, 0.0),
        ] {
            assert!(
                points.iter().any(|p| p.coincides_with(expected)),
                "missing quadrant at {expected:?}"
            );
        }
    }

    #[test]
    fn a_block_reference_snaps_to_its_insertion_point() {
        let g = Geometry::BlockRef(Box::new(BlockRef {
            block: id(9),
            position: Point3::new(1234.0, 5678.0, 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attributes: vec![],
            array: (1, 1),
            array_spacing: (0.0, 0.0),
        }));
        assert_eq!(g.snap_points(), vec![Point3::new(1234.0, 5678.0, 0.0)]);
    }

    #[test]
    fn arc_bounds_are_tight_not_the_whole_circle() {
        let g = Geometry::Arc {
            center: Point3::ORIGIN,
            radius: 10.0,
            start_angle: 0.0,
            sweep: std::f64::consts::FRAC_PI_2,
            normal: Vec3::Z,
        };
        let b = g.local_bounds();
        assert!(od_geom2d::tol::eq_len(b.min.x, 0.0));
        assert!(od_geom2d::tol::eq_len(b.max.x, 10.0));
    }

    #[test]
    fn unsupported_geometry_is_still_drawable_and_bounded() {
        let g = Geometry::Unsupported {
            source_type: "ACAD_TABLE".into(),
            payload: vec![1, 2, 3],
            proxy: vec![ProxyGraphic::Polyline {
                points: vec![Point3::ORIGIN, Point3::new(100.0, 50.0, 0.0)],
                closed: true,
            }],
        };
        assert_eq!(g.type_name(), "ACAD_TABLE");
        let b = g.local_bounds();
        assert!(!b.is_empty());
        assert!(b.contains_point(Point3::new(50.0, 25.0, 0.0)));
    }

    #[test]
    fn entities_serialise_to_self_describing_json() {
        let e = Entity::new(
            id(1),
            id(2),
            Geometry::Line {
                a: Point3::ORIGIN,
                b: Point3::new(1000.0, 0.0, 0.0),
            },
        );
        let json = serde_json::to_string(&e).expect("serialises");
        assert!(json.contains("\"kind\":\"line\""));
        let back: Entity = serde_json::from_str(&json).expect("round trips");
        assert_eq!(back, e);
    }

    #[test]
    fn empty_geometry_reports_empty_bounds() {
        let g = Geometry::Polyline3d {
            points: vec![],
            closed: false,
        };
        assert!(g.local_bounds().is_empty());
    }
}
