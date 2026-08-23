//! Loading a catalogue and turning a definition into drawable geometry.

use crate::expr::Value;
use crate::model::{Body3d, Part, ProfileDef, Shape2d, Spec, SystemDef, SystemKind};
use od_core::{Aabb3, Frame3, Geometry, Point2, Point3, Polyline2, Vec3, Vertex};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("io reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("no part `{0}` in the catalogue")]
    NoSuchPart(String),
    #[error("no spec `{0}` in the catalogue")]
    NoSuchSpec(String),
    #[error("part `{part}`: {message}")]
    Invalid { part: String, message: String },
    #[error("expression in part `{part}`: {source}")]
    Expr {
        part: String,
        #[source]
        source: crate::expr::ExprError,
    },
}

pub type Result<T> = std::result::Result<T, CatalogError>;

/// One file of parts, systems or specs.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CatalogFile {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub systems: Vec<SystemDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub specs: Vec<Spec>,
}

/// Parts, systems and specs, indexed by id.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    parts: HashMap<String, Part>,
    systems: HashMap<String, SystemDef>,
    specs: HashMap<String, Spec>,
    order: Vec<String>,
}

/// The catalogue shipped inside the binary.
///
/// Bundling rather than only reading from disk is what makes the promise "works
/// without a manufacturer agreement" true at first run: a fresh install can
/// draw a duct, a pipe and a fixture with no downloads and no account.
macro_rules! bundled {
    ($($file:literal),* $(,)?) => {
        &[$((concat!("bundled:", $file), include_str!(concat!("../../../parts/", $file)))),*]
    };
}

const BUNDLED: &[(&str, &str)] = bundled![
    "systems/standard-jp.json",
    "specs/duct-galvanised.json",
    "specs/pipe-sgp.json",
    "specs/pipe-vp.json",
    "specs/conduit-steel.json",
    "parts/duct-fittings.json",
    "parts/duct-terminals.json",
    "parts/pipe-fittings.json",
    "parts/valves.json",
    "parts/sanitary.json",
    "parts/hvac-equipment.json",
    "parts/plumbing-equipment.json",
    "parts/electrical.json",
    "parts/support.json",
];

impl Catalog {
    /// The built-in catalogue. Parsed on demand rather than at startup, since a
    /// pure DXF conversion never touches it.
    pub fn bundled() -> Result<Self> {
        let mut c = Self::default();
        for (name, text) in BUNDLED {
            let file: CatalogFile =
                serde_json::from_str(text).map_err(|source| CatalogError::Json {
                    path: (*name).to_owned(),
                    source,
                })?;
            c.absorb(file);
        }
        c.validate()?;
        Ok(c)
    }

    /// Loads every `.json` under `dir`, recursively. User and office libraries
    /// layer over the bundled one by loading after it.
    pub fn load_dir(&mut self, dir: impl AsRef<Path>) -> Result<usize> {
        let dir = dir.as_ref();
        let mut count = 0;
        let entries = std::fs::read_dir(dir).map_err(|source| CatalogError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                count += self.load_dir(&path)?;
            } else if path.extension().is_some_and(|e| e == "json") {
                let text = std::fs::read_to_string(&path).map_err(|source| CatalogError::Io {
                    path: path.display().to_string(),
                    source,
                })?;
                // A schema file sitting alongside the data is not a catalogue.
                if text.contains("\"$schema\"") && !text.contains("\"parts\"") {
                    continue;
                }
                let file: CatalogFile =
                    serde_json::from_str(&text).map_err(|source| CatalogError::Json {
                        path: path.display().to_string(),
                        source,
                    })?;
                count += file.parts.len();
                self.absorb(file);
            }
        }
        Ok(count)
    }

    fn absorb(&mut self, file: CatalogFile) {
        for p in file.parts {
            if !self.parts.contains_key(&p.id) {
                self.order.push(p.id.clone());
            }
            self.parts.insert(p.id.clone(), p);
        }
        for s in file.systems {
            self.systems.insert(s.id.clone(), s);
        }
        for s in file.specs {
            self.specs.insert(s.id.clone(), s);
        }
    }

    /// Checks that every part can actually be built, that its expressions only
    /// use declared parameters, and that specs point at parts that exist.
    /// Running this at load turns a typo in a catalogue file into a startup
    /// error rather than a failure halfway through someone's drawing.
    pub fn validate(&self) -> Result<()> {
        for id in &self.order {
            let Some(part) = self.parts.get(id) else {
                continue;
            };
            let declared: Vec<&str> = part.parameters.iter().map(|p| p.name.as_str()).collect();
            let mut used = Vec::new();
            for v in part_values(part) {
                v.0.params(&mut used);
            }
            for u in &used {
                if !declared.contains(&u.as_str()) {
                    return Err(CatalogError::Invalid {
                        part: part.id.clone(),
                        message: format!("uses undeclared parameter `{u}`"),
                    });
                }
            }
            if part.ports.is_empty() && !matches!(part.category, crate::model::Category::Support) {
                return Err(CatalogError::Invalid {
                    part: part.id.clone(),
                    message: "has no ports, so nothing can connect to it".into(),
                });
            }
            for p in &part.properties {
                if p.ifc_property.trim().is_empty() {
                    return Err(CatalogError::Invalid {
                        part: part.id.clone(),
                        message: format!("property `{}` has no IFC mapping", p.name),
                    });
                }
            }
            // Building with defaults proves the expressions evaluate.
            self.instantiate(&part.id, &HashMap::new())?;
        }
        for spec in self.specs.values() {
            for referenced in [&spec.elbow_part, &spec.branch_part, &spec.reducer_part] {
                if !referenced.is_empty() && !self.parts.contains_key(referenced) {
                    return Err(CatalogError::NoSuchPart(referenced.clone()));
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn part(&self, id: &str) -> Option<&Part> {
        self.parts.get(id)
    }

    #[must_use]
    pub fn spec(&self, id: &str) -> Option<&Spec> {
        self.specs.get(id)
    }

    #[must_use]
    pub fn system(&self, id: &str) -> Option<&SystemDef> {
        self.systems.get(id)
    }

    /// Parts in catalogue order, which is curated rather than alphabetical.
    pub fn parts(&self) -> impl Iterator<Item = &Part> {
        self.order.iter().filter_map(|id| self.parts.get(id))
    }

    pub fn systems(&self) -> impl Iterator<Item = &SystemDef> {
        self.systems.values()
    }

    pub fn specs(&self) -> impl Iterator<Item = &Spec> {
        self.specs.values()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.parts.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// Free-text search over id, names and tags.
    pub fn search<'a>(&'a self, query: &'a str) -> impl Iterator<Item = &'a Part> + 'a {
        let q = query.to_lowercase();
        self.parts().filter(move |p| {
            q.is_empty()
                || p.id.to_lowercase().contains(&q)
                || p.name.ja.contains(&q)
                || p.name.ja.to_lowercase().contains(&q)
                || p.name.en.to_lowercase().contains(&q)
                || p.tags.iter().any(|t| t.to_lowercase().contains(&q))
        })
    }

    /// Builds a part at the given parameter values.
    pub fn instantiate(&self, id: &str, overrides: &HashMap<String, f64>) -> Result<Instance> {
        let part = self
            .parts
            .get(id)
            .ok_or_else(|| CatalogError::NoSuchPart(id.to_owned()))?;
        let params = part.resolve_params(overrides);

        let ev = |v: &Value| -> Result<f64> {
            v.eval(&params).map_err(|source| CatalogError::Expr {
                part: part.id.clone(),
                source,
            })
        };
        let ev3 = |v: &[Value; 3]| -> Result<Point3> {
            Ok(Point3::new(ev(&v[0])?, ev(&v[1])?, ev(&v[2])?))
        };
        let ev2 = |v: &[Value; 2]| -> Result<Point2> { Ok(Point2::new(ev(&v[0])?, ev(&v[1])?)) };

        let mut ports = Vec::new();
        for p in &part.ports {
            let origin = ev3(&p.origin)?;
            let d = ev3(&p.direction)?;
            let direction = Vec3::new(d.x, d.y, d.z);
            let frame =
                Frame3::from_normal(origin, direction).ok_or_else(|| CatalogError::Invalid {
                    part: part.id.clone(),
                    message: format!("port `{}` has no direction", p.name),
                })?;
            ports.push(Port {
                name: p.name.clone(),
                frame,
                profile: resolve_profile(&p.profile, &ev)?,
                system_kind: p.system_kind,
            });
        }

        let mut symbol = Vec::new();
        for s in &part.symbol2d {
            symbol.push(match s {
                Shape2d::Line { a, b } => {
                    let (a, b) = (ev2(a)?, ev2(b)?);
                    Geometry::Line {
                        a: Point3::from_2d(a, 0.0),
                        b: Point3::from_2d(b, 0.0),
                    }
                }
                Shape2d::Circle { c, r } => Geometry::Circle {
                    center: Point3::from_2d(ev2(c)?, 0.0),
                    radius: ev(r)?,
                    normal: Vec3::Z,
                },
                Shape2d::Arc { c, r, start, sweep } => Geometry::Arc {
                    center: Point3::from_2d(ev2(c)?, 0.0),
                    radius: ev(r)?,
                    start_angle: ev(start)?.to_radians(),
                    sweep: ev(sweep)?.to_radians(),
                    normal: Vec3::Z,
                },
                Shape2d::Polyline { points, closed } => {
                    let mut verts = Vec::with_capacity(points.len());
                    for p in points {
                        verts.push(Vertex::straight(ev2(p)?));
                    }
                    Geometry::Polyline {
                        polyline: Polyline2::new(verts, *closed),
                        elevation: 0.0,
                        normal: Vec3::Z,
                        width: 0.0,
                    }
                }
                Shape2d::Text { at, value, height } => Geometry::Polyline {
                    // Symbol text needs a text style id, which only exists once
                    // the part is placed in a document. The placer substitutes a
                    // real TEXT entity; here it is a bounding outline so the
                    // preview and the bounds are still right.
                    polyline: {
                        let p = ev2(at)?;
                        let h = ev(height)?;
                        #[expect(
                            clippy::cast_precision_loss,
                            reason = "a label is never long enough for the count to lose precision"
                        )]
                        let w = h * 0.6 * value.chars().count() as f64;
                        Polyline2::from_points(
                            [
                                p,
                                Point2::new(p.x + w, p.y),
                                Point2::new(p.x + w, p.y + h),
                                Point2::new(p.x, p.y + h),
                            ],
                            true,
                        )
                    },
                    elevation: 0.0,
                    normal: Vec3::Z,
                    width: 0.0,
                },
            });
        }

        let mut bodies = Vec::new();
        for b in &part.body3d {
            bodies.push(match b {
                Body3d::Box { min, max } => Solid::Box {
                    min: ev3(min)?,
                    max: ev3(max)?,
                },
                Body3d::Cylinder { base, axis, radius } => {
                    let a = ev3(axis)?;
                    Solid::Cylinder {
                        base: ev3(base)?,
                        axis: Vec3::new(a.x, a.y, a.z),
                        radius: ev(radius)?,
                    }
                }
                Body3d::Bend {
                    center,
                    axis,
                    radius,
                    angle,
                    profile,
                } => {
                    let a = ev3(axis)?;
                    Solid::Bend {
                        center: ev3(center)?,
                        axis: Vec3::new(a.x, a.y, a.z),
                        radius: ev(radius)?,
                        angle: ev(angle)?.to_radians(),
                        profile: resolve_profile(profile, &ev)?,
                    }
                }
            });
        }

        Ok(Instance {
            part_id: part.id.clone(),
            params,
            ports,
            symbol,
            bodies,
        })
    }
}

fn part_values(part: &Part) -> Vec<&Value> {
    let mut out: Vec<&Value> = Vec::new();
    for p in &part.parameters {
        out.push(&p.default);
    }
    for p in &part.ports {
        out.extend(p.origin.iter());
        out.extend(p.direction.iter());
        out.extend(profile_values(&p.profile));
    }
    for s in &part.symbol2d {
        match s {
            Shape2d::Line { a, b } => {
                out.extend(a.iter());
                out.extend(b.iter());
            }
            Shape2d::Circle { c, r } => {
                out.extend(c.iter());
                out.push(r);
            }
            Shape2d::Arc { c, r, start, sweep } => {
                out.extend(c.iter());
                out.push(r);
                out.push(start);
                out.push(sweep);
            }
            Shape2d::Polyline { points, .. } => {
                for p in points {
                    out.extend(p.iter());
                }
            }
            Shape2d::Text { at, height, .. } => {
                out.extend(at.iter());
                out.push(height);
            }
        }
    }
    for b in &part.body3d {
        match b {
            Body3d::Box { min, max } => {
                out.extend(min.iter());
                out.extend(max.iter());
            }
            Body3d::Cylinder { base, axis, radius } => {
                out.extend(base.iter());
                out.extend(axis.iter());
                out.push(radius);
            }
            Body3d::Bend {
                center,
                axis,
                radius,
                angle,
                profile,
            } => {
                out.extend(center.iter());
                out.extend(axis.iter());
                out.push(radius);
                out.push(angle);
                out.extend(profile_values(profile));
            }
        }
    }
    out
}

fn profile_values(p: &ProfileDef) -> Vec<&Value> {
    match p {
        ProfileDef::Rect { w, h } | ProfileDef::Oval { w, h } => vec![w, h],
        ProfileDef::Round { d } => vec![d],
        ProfileDef::Terminal => Vec::new(),
    }
}

fn resolve_profile<F>(p: &ProfileDef, ev: &F) -> Result<Profile>
where
    F: Fn(&Value) -> Result<f64>,
{
    Ok(match p {
        ProfileDef::Rect { w, h } => Profile::Rect {
            w: ev(w)?,
            h: ev(h)?,
        },
        ProfileDef::Round { d } => Profile::Round { d: ev(d)? },
        ProfileDef::Oval { w, h } => Profile::Oval {
            w: ev(w)?,
            h: ev(h)?,
        },
        ProfileDef::Terminal => Profile::Terminal,
    })
}

/// A cross-section with its numbers resolved.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Profile {
    Rect { w: f64, h: f64 },
    Round { d: f64 },
    Oval { w: f64, h: f64 },
    Terminal,
}

impl Profile {
    /// Free area, in mm². What airflow and flow-velocity checks need.
    #[must_use]
    pub fn area(&self) -> f64 {
        match self {
            Profile::Rect { w, h } => w * h,
            Profile::Round { d } => std::f64::consts::PI * d * d / 4.0,
            // A flat oval: a rectangle with semicircular ends.
            Profile::Oval { w, h } => {
                let straight = (w - h).max(0.0);
                straight * h + std::f64::consts::PI * h * h / 4.0
            }
            Profile::Terminal => 0.0,
        }
    }

    /// Largest dimension, used for clearances and bend radii.
    #[must_use]
    pub fn extent(&self) -> f64 {
        match self {
            Profile::Rect { w, h } | Profile::Oval { w, h } => w.max(*h),
            Profile::Round { d } => *d,
            Profile::Terminal => 0.0,
        }
    }

    /// Whether two ports can be joined without a transition piece.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Profile::Rect { w: w1, h: h1 }, Profile::Rect { w: w2, h: h2 })
            | (Profile::Oval { w: w1, h: h1 }, Profile::Oval { w: w2, h: h2 }) => {
                od_core::tol::eq_len(*w1, *w2) && od_core::tol::eq_len(*h1, *h2)
            }
            (Profile::Round { d: d1 }, Profile::Round { d: d2 }) => od_core::tol::eq_len(*d1, *d2),
            (Profile::Terminal, Profile::Terminal) => true,
            _ => false,
        }
    }
}

/// A connection point on a built part.
#[derive(Debug, Clone, PartialEq)]
pub struct Port {
    pub name: String,
    pub frame: Frame3,
    pub profile: Profile,
    pub system_kind: SystemKind,
}

/// A 3D body with its numbers resolved. Handed to the solid kernel, or bounded
/// directly for clash detection.
#[derive(Debug, Clone, PartialEq)]
pub enum Solid {
    Box {
        min: Point3,
        max: Point3,
    },
    Cylinder {
        base: Point3,
        axis: Vec3,
        radius: f64,
    },
    Bend {
        center: Point3,
        axis: Vec3,
        radius: f64,
        angle: f64,
        profile: Profile,
    },
}

impl Solid {
    #[must_use]
    pub fn bounds(&self) -> Aabb3 {
        match self {
            Solid::Box { min, max } => Aabb3::new(*min, *max),
            Solid::Cylinder { base, axis, radius } => {
                let tip = *base + *axis;
                Aabb3::new(*base, tip).inflated(*radius)
            }
            Solid::Bend {
                center,
                radius,
                profile,
                ..
            } => {
                let r = radius + profile.extent() * 0.5;
                Aabb3::new(
                    Point3::new(center.x - r, center.y - r, center.z - r),
                    Point3::new(center.x + r, center.y + r, center.z + r),
                )
            }
        }
    }
}

/// A part built at specific parameter values.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub part_id: String,
    pub params: HashMap<String, f64>,
    pub ports: Vec<Port>,
    /// Plan symbol, in the part's local frame.
    pub symbol: Vec<Geometry>,
    pub bodies: Vec<Solid>,
}

impl Instance {
    #[must_use]
    pub fn bounds(&self) -> Aabb3 {
        let from_bodies = self
            .bodies
            .iter()
            .fold(Aabb3::EMPTY, |acc, b| acc.union(b.bounds()));
        self.symbol
            .iter()
            .fold(from_bodies, |acc, g| acc.union(g.local_bounds()))
    }

    #[must_use]
    pub fn port(&self, name: &str) -> Option<&Port> {
        self.ports.iter().find(|p| p.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_catalogue_loads_and_validates() {
        let c = Catalog::bundled().expect("the shipped catalogue must be valid");
        assert!(
            c.len() >= 40,
            "the starter set should cover the common trades, found {}",
            c.len()
        );
        assert!(
            c.systems().count() >= 15,
            "systems for all three disciplines"
        );
        assert!(c.specs().count() >= 4);
    }

    #[test]
    fn every_bundled_part_builds_at_its_defaults() {
        let c = Catalog::bundled().expect("loads");
        for part in c.parts() {
            let inst = c
                .instantiate(&part.id, &HashMap::new())
                .unwrap_or_else(|e| panic!("{} failed to build: {e}", part.id));
            assert!(
                !inst.bounds().is_empty(),
                "{} produced no geometry at all",
                part.id
            );
        }
    }

    #[test]
    fn every_bundled_part_declares_its_ifc_mapping() {
        let c = Catalog::bundled().expect("loads");
        for part in c.parts() {
            assert!(
                part.ifc_class.starts_with("Ifc"),
                "{} has no IFC class",
                part.id
            );
            for p in &part.properties {
                assert!(
                    p.ifc_property.contains('.'),
                    "{}.{} needs a Pset.Property mapping",
                    part.id,
                    p.name
                );
            }
        }
    }

    #[test]
    fn parts_resize_with_their_parameters() {
        let c = Catalog::bundled().expect("loads");
        let small = c
            .instantiate("duct.elbow.rect.90", &HashMap::new())
            .expect("builds at defaults");
        let mut over = HashMap::new();
        over.insert("W".to_owned(), 1000.0);
        over.insert("H".to_owned(), 800.0);
        let large = c
            .instantiate("duct.elbow.rect.90", &over)
            .expect("builds at a larger size");

        let port = |i: &Instance| match i.ports.first().expect("an inlet").profile {
            Profile::Rect { w, h } => (w, h),
            other => panic!("expected a rectangular port, got {other:?}"),
        };
        assert!(port(&large).0 > port(&small).0);
        assert_eq!(port(&large), (1000.0, 800.0));
        assert!(large.bounds().size().x > small.bounds().size().x);
    }

    #[test]
    fn ports_face_outwards_and_carry_a_system_kind() {
        let c = Catalog::bundled().expect("loads");
        let elbow = c
            .instantiate("duct.elbow.rect.90", &HashMap::new())
            .expect("builds");
        assert_eq!(elbow.ports.len(), 2, "an elbow connects two runs");
        for p in &elbow.ports {
            assert!(p.frame.is_orthonormal());
            assert_eq!(p.system_kind, SystemKind::Air);
        }
        // The two ends of a 90° elbow are perpendicular.
        let dot = elbow.ports[0]
            .frame
            .normal()
            .dot(elbow.ports[1].frame.normal());
        assert!(
            dot.abs() < 1e-9,
            "ends should be at right angles, dot={dot}"
        );
    }

    #[test]
    fn profiles_only_join_when_they_match() {
        let a = Profile::Round { d: 100.0 };
        assert!(a.matches(&Profile::Round { d: 100.0 }));
        assert!(!a.matches(&Profile::Round { d: 125.0 }));
        assert!(
            !a.matches(&Profile::Rect { w: 100.0, h: 100.0 }),
            "a round duct does not butt onto a rectangular one"
        );
    }

    #[test]
    fn profile_areas_are_right() {
        assert!(od_core::tol::eq_len(
            Profile::Rect { w: 400.0, h: 300.0 }.area(),
            120_000.0
        ));
        let round = Profile::Round { d: 200.0 }.area();
        assert!((round - 31_415.926_5).abs() < 0.01);
    }

    #[test]
    fn search_finds_parts_by_japanese_name_and_by_tag() {
        let c = Catalog::bundled().expect("loads");
        assert!(
            c.search("エルボ").count() > 0,
            "Japanese names are searchable"
        );
        assert!(
            c.search("elbow").count() > 0,
            "English names are searchable"
        );
        assert!(c.search("valve").count() > 0);
        assert_eq!(c.search("").count(), c.len(), "an empty query lists all");
    }

    #[test]
    fn an_unknown_part_is_an_error_with_its_name() {
        let c = Catalog::bundled().expect("loads");
        match c.instantiate("nope.not.here", &HashMap::new()) {
            Err(CatalogError::NoSuchPart(id)) => assert_eq!(id, "nope.not.here"),
            other => panic!("expected a NoSuchPart error, got {other:?}"),
        }
    }

    #[test]
    fn specs_point_at_parts_that_exist() {
        let c = Catalog::bundled().expect("loads");
        for spec in c.specs() {
            for id in [&spec.elbow_part, &spec.branch_part, &spec.reducer_part] {
                assert!(
                    c.part(id).is_some(),
                    "{} references missing part {id}",
                    spec.id
                );
            }
            assert!(!spec.sizes.is_empty(), "{} has no size table", spec.id);
        }
    }
}
