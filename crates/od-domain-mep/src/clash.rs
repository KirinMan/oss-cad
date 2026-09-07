//! Interference checking (`docs/04-mep.md` §4.4, F-108).
//!
//! `docs/04-mep.md` describes a `ClashRule` filtering on an arbitrary
//! `EntityFilter` (e.g. "category = structural") and falling back to an OCCT
//! boolean for shapes a cylinder/box test cannot resolve. Neither exists yet:
//! there is no structural domain to filter on, and no OCCT bridge (ADR-002 —
//! `od-geom3d-lite` is the only kernel this workspace has). So this is the
//! part of F-108 buildable today, narrowed on purpose rather than guessed at:
//!
//! - **What's checked**: [`RouteSegment`] pairs only. A [`crate::PlacedPart`]
//!   (equipment, a fitting) has a footprint too, but giving it one here would
//!   mean inventing per-catalogue-part collision geometry this pass does not
//!   attempt — the same boundary [`crate::derive::route_solid`] already draws
//!   around what has a swept solid and what does not.
//! - **What a route looks like to this check**: every profile — `Rect`,
//!   `Round`, `Oval`, `Terminal` — is treated as its *circumscribing
//!   cylinder* (see [`effective_radius`]), and every run as a capsule (a
//!   cylinder with hemispherical caps) along its centreline. This is exact
//!   for `Round`, and a deliberate over-approximation for the others: a
//!   rectangular duct's corners never reach as far as its circumscribing
//!   circle, so this can report a clash a true box/box test would clear, but
//!   it can never miss one a box/box test would catch. Given F-108's own
//!   workflow — every issue is triaged by a person, never auto-resolved — a
//!   conservative false positive is the safe direction to err in; true
//!   oriented-box precision is future work, not attempted here.
//! - **What a rule can filter on**: [`SystemFilter`] — a route's `system` id,
//!   or "any". Not [`ClashRule`]'s `EntityFilter` from the design doc, which
//!   implies categories (structural, architectural) this workspace has no
//!   domain for yet.
//! - **What's not here**: BVH sharing with a renderer (`od-render` does not
//!   exist — Phase 1), Rayon parallelism (premature at the scale a
//!   `RouteSegment`-only check runs at), incremental re-checking, and
//!   persisting a `ClashIssue` in the document with a workflow state
//!   (unaddressed / in progress / approved / ignored) and BCF import/export —
//!   [`find_clashes`] is a pure computation over a snapshot, the same shape
//!   [`crate::takeoff::take_off`] already has.
//!
//! Broad phase goes through [`od_index::RTree`], the same structure
//! [`crate::graph`] uses for port matching — "which pairs might be
//! clashing" is literally one of the four questions that index exists to
//! answer (`od-index`'s own module docs).

use crate::model::RouteSegment;
use crate::store::all_routes;
use od_core::{Database, ObjectId};
use od_geom2d::tol;
use od_geom3d::{Aabb3, Point3, Vec3};
use od_index::{Entry, RTree};
use od_parts::Profile;
use serde::{Deserialize, Serialize};

/// A route's `system` id, or "any" — see the module docs for why this stands
/// in for the design doc's more general `EntityFilter`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemFilter(Option<String>);

impl SystemFilter {
    #[must_use]
    pub fn any() -> Self {
        Self(None)
    }

    #[must_use]
    pub fn system(id: impl Into<String>) -> Self {
        Self(Some(id.into()))
    }

    fn matches(&self, route: &RouteSegment) -> bool {
        self.0.as_deref().is_none_or(|s| s == route.system)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClashKind {
    /// The two runs' swept solids physically overlap.
    Hard,
    /// The two runs stay apart, but by less than the rule's `tolerance`.
    Clearance,
    /// The same run, drawn twice — same system and spec, and endpoints
    /// (in either order) within `tolerance` of each other. A distinct check
    /// from `Hard`/`Clearance`: two coincident ducts are "clashing" in a
    /// sense no clearance rule captures, they are one run pasted twice.
    /// Only exact (within tolerance) duplicates are caught — a segment that
    /// merely *overlaps part of* another is not, which would need the same
    /// projection-and-overlap-fraction math `Hard`/`Clearance` already do,
    /// just re-purposed; not attempted in this pass.
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClashRule {
    pub a: SystemFilter,
    pub b: SystemFilter,
    pub kind: ClashKind,
    /// Millimetres. Ignored by [`ClashKind::Hard`] — a hard clash is
    /// measured against zero clearance by definition, not a configurable
    /// one.
    pub tolerance: f64,
    pub severity: Severity,
}

/// One finding from [`find_clashes`] — a candidate for the `ClashIssue`
/// object docs/04-mep.md describes, once a document has somewhere to persist
/// one with a workflow state (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ClashIssue {
    pub kind: ClashKind,
    pub severity: Severity,
    pub a: ObjectId,
    pub b: ObjectId,
    /// Shortest distance found between the two runs' capsule approximations,
    /// in millimetres. Negative is penetration depth (how far the two
    /// solids overlap), used by `Hard` and `Clearance`. `Duplicate` always
    /// reports `0.0` — the two runs are considered coincident, not measured
    /// apart.
    pub gap_mm: f64,
    /// The midpoint between the two closest points found — where to point a
    /// viewer at the issue.
    pub location: Point3,
}

/// A run's centreline plus the radius of the cylinder that circumscribes its
/// profile (module docs).
struct Capsule {
    a: Point3,
    b: Point3,
    radius: f64,
}

impl Capsule {
    fn aabb(&self) -> Aabb3 {
        Aabb3::new(self.a, self.b).inflated(self.radius)
    }

    /// `(gap, midpoint)` between this capsule and `other`: the distance
    /// between their centrelines' closest points, minus both radii, and the
    /// midpoint of that closest pair. Negative gap is penetration depth.
    fn gap_to(&self, other: &Self) -> (f64, Point3) {
        let (p, q) = closest_points_on_segments(self.a, self.b, other.a, other.b);
        let gap = p.distance_to(q) - self.radius - other.radius;
        (gap, p.midpoint(q))
    }
}

/// The radius of the circle that circumscribes a profile, centred on the
/// route's centreline (module docs: exact for `Round`, a conservative
/// over-approximation otherwise).
#[must_use]
pub fn effective_radius(profile: Profile) -> f64 {
    match profile {
        Profile::Rect { w, h } => (w * w + h * h).sqrt() / 2.0,
        Profile::Round { d } => d / 2.0,
        Profile::Oval { w, h } => w.max(h) / 2.0,
        Profile::Terminal => 0.0,
    }
}

/// Runs `rules` against every [`RouteSegment`] in `db` and returns every
/// issue found, across all rules, in no particular order.
#[must_use]
pub fn find_clashes(db: &Database, rules: &[ClashRule]) -> Vec<ClashIssue> {
    let routes = all_routes(db);
    let capsules: Vec<Capsule> = routes
        .iter()
        .map(|(_, r)| Capsule {
            a: r.start,
            b: r.end,
            radius: effective_radius(r.profile),
        })
        .collect();

    let tree: RTree<usize> = RTree::bulk_load(capsules.iter().enumerate().map(|(i, c)| Entry {
        bounds: c.aabb(),
        value: i,
    }));

    let mut issues = Vec::new();
    for rule in rules {
        // Duplicate has no meaningful "clearance", but still needs a margin
        // to widen the broad-phase box far enough to catch a near-duplicate
        // before the precise check runs.
        let margin = match rule.kind {
            ClashKind::Hard => 0.0,
            ClashKind::Clearance | ClashKind::Duplicate => rule.tolerance.max(0.0),
        };

        for (i, (id_a, route_a)) in routes.iter().enumerate() {
            let query = capsules[i].aabb().inflated(margin);
            for entry in tree.query(query) {
                let j = entry.value;
                if j <= i {
                    continue; // each unordered pair once, and never against itself
                }
                let (id_b, route_b) = &routes[j];
                let applies = (rule.a.matches(route_a) && rule.b.matches(route_b))
                    || (rule.a.matches(route_b) && rule.b.matches(route_a));
                if !applies {
                    continue;
                }

                match rule.kind {
                    ClashKind::Hard | ClashKind::Clearance => {
                        let (gap_mm, location) = capsules[i].gap_to(&capsules[j]);
                        let threshold = match rule.kind {
                            ClashKind::Hard => 0.0,
                            ClashKind::Clearance | ClashKind::Duplicate => rule.tolerance,
                        };
                        if gap_mm < threshold {
                            issues.push(ClashIssue {
                                kind: rule.kind,
                                severity: rule.severity,
                                a: *id_a,
                                b: *id_b,
                                gap_mm,
                                location,
                            });
                        }
                    }
                    ClashKind::Duplicate => {
                        if is_duplicate(route_a, route_b, rule.tolerance) {
                            issues.push(ClashIssue {
                                kind: rule.kind,
                                severity: rule.severity,
                                a: *id_a,
                                b: *id_b,
                                gap_mm: 0.0,
                                location: route_a.start.midpoint(route_a.end),
                            });
                        }
                    }
                }
            }
        }
    }
    issues
}

/// Same system, same spec, and endpoints matching within `tolerance` — in
/// either order, since a duplicate can be drawn walking the opposite way.
fn is_duplicate(a: &RouteSegment, b: &RouteSegment, tolerance: f64) -> bool {
    if a.system != b.system || a.spec != b.spec {
        return false;
    }
    let same_direction =
        a.start.distance_to(b.start) <= tolerance && a.end.distance_to(b.end) <= tolerance;
    let reversed =
        a.start.distance_to(b.end) <= tolerance && a.end.distance_to(b.start) <= tolerance;
    same_direction || reversed
}

/// The closest pair of points between segments `a0..a1` and `b0..b1`
/// (Ericson, *Real-Time Collision Detection* §5.1.9), with the zero-division
/// guards this workspace's tolerances rather than a bare epsilon: a
/// zero-length segment is `tol::is_zero_len` on its own direction, and
/// near-parallel segments (the case that would otherwise divide by
/// approximately zero) are `tol::ANGLE_EPS` on the angle between them.
fn closest_points_on_segments(a0: Point3, a1: Point3, b0: Point3, b1: Point3) -> (Point3, Point3) {
    let d1 = a1 - a0;
    let d2 = b1 - b0;
    let r = a0 - b0;
    let len1 = d1.length();
    let len2 = d2.length();

    if tol::is_zero_len(len1) && tol::is_zero_len(len2) {
        return (a0, b0);
    }
    if tol::is_zero_len(len1) {
        let t = closest_param_on_segment(b0, d2, len2, a0);
        return (a0, b0 + d2 * t);
    }
    if tol::is_zero_len(len2) {
        let s = closest_param_on_segment(a0, d1, len1, b0);
        return (a0 + d1 * s, b0);
    }

    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    let c = d1.dot(r);
    let b = d1.dot(d2);

    let parallel = (d1 / len1).cross(d2 / len2).length() <= tol::ANGLE_EPS;
    let denom = a * e - b * b;
    let mut s = if parallel {
        0.0
    } else {
        ((b * f - c * e) / denom).clamp(0.0, 1.0)
    };
    let mut t = (b * s + f) / e;
    if t < 0.0 {
        t = 0.0;
        s = (-c / a).clamp(0.0, 1.0);
    } else if t > 1.0 {
        t = 1.0;
        s = ((b - c) / a).clamp(0.0, 1.0);
    }
    (a0 + d1 * s, b0 + d2 * t)
}

/// The parameter (clamped to `[0, 1]`) of the point on a segment — starting
/// at `origin`, direction `dir`, length `len` — closest to `point`.
fn closest_param_on_segment(origin: Point3, dir: Vec3, len: f64, point: Point3) -> f64 {
    (dir.dot(point - origin) / (len * len)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::add_route;
    use od_core::{ActorId, Document};
    use od_parts::Profile;

    fn doc() -> Document {
        Document::new(Database::new(ActorId::SYSTEM))
    }

    fn duct(system: &str, start: Point3, end: Point3) -> RouteSegment {
        RouteSegment {
            system: system.into(),
            spec: "spec.duct.galvanised.rect".into(),
            profile: Profile::Rect { w: 400.0, h: 300.0 },
            start,
            end,
        }
    }

    fn hard_rule() -> ClashRule {
        ClashRule {
            a: SystemFilter::any(),
            b: SystemFilter::any(),
            kind: ClashKind::Hard,
            tolerance: 0.0,
            severity: Severity::Error,
        }
    }

    #[test]
    fn two_crossing_ducts_hard_clash() {
        let mut d = doc();
        let a = duct(
            "sys.air.supply",
            Point3::new(-1000.0, 0.0, 0.0),
            Point3::new(1000.0, 0.0, 0.0),
        );
        let b = duct(
            "sys.air.return",
            Point3::new(0.0, -1000.0, 0.0),
            Point3::new(0.0, 1000.0, 0.0),
        );
        d.edit("Route", |tx| {
            add_route(tx, &a)?;
            add_route(tx, &b)
        })
        .expect("commits");

        let issues = find_clashes(&d.db, &[hard_rule()]);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, ClashKind::Hard);
        assert!(issues[0].gap_mm < 0.0, "the ducts overlap");
    }

    #[test]
    fn parallel_ducts_with_room_between_them_do_not_hard_clash() {
        let mut d = doc();
        let a = duct(
            "sys.air.supply",
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(5000.0, 0.0, 0.0),
        );
        let b = duct(
            "sys.air.supply",
            Point3::new(0.0, 2000.0, 0.0),
            Point3::new(5000.0, 2000.0, 0.0),
        );
        d.edit("Route", |tx| {
            add_route(tx, &a)?;
            add_route(tx, &b)
        })
        .expect("commits");

        assert!(find_clashes(&d.db, &[hard_rule()]).is_empty());
    }

    #[test]
    fn a_narrow_gap_is_a_clearance_violation_not_a_hard_clash() {
        let mut d = doc();
        // Two round pipes, radius 50 each, centres 150 apart: a 50mm gap
        // between their surfaces — no overlap, but tighter than 100mm.
        let a = RouteSegment {
            system: "sys.water.cold".into(),
            spec: "spec.pipe.sgp".into(),
            profile: Profile::Round { d: 100.0 },
            start: Point3::new(0.0, 0.0, 0.0),
            end: Point3::new(5000.0, 0.0, 0.0),
        };
        let b = RouteSegment {
            system: "sys.water.cold".into(),
            spec: "spec.pipe.sgp".into(),
            profile: Profile::Round { d: 100.0 },
            start: Point3::new(0.0, 150.0, 0.0),
            end: Point3::new(5000.0, 150.0, 0.0),
        };
        d.edit("Route", |tx| {
            add_route(tx, &a)?;
            add_route(tx, &b)
        })
        .expect("commits");

        assert!(
            find_clashes(&d.db, &[hard_rule()]).is_empty(),
            "no overlap yet"
        );

        let clearance_rule = ClashRule {
            a: SystemFilter::any(),
            b: SystemFilter::any(),
            kind: ClashKind::Clearance,
            tolerance: 100.0,
            severity: Severity::Warning,
        };
        let issues = find_clashes(&d.db, &[clearance_rule]);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, ClashKind::Clearance);
        assert!((issues[0].gap_mm - 50.0).abs() < 1e-6);
    }

    #[test]
    fn a_system_filter_only_matches_the_named_system() {
        let mut d = doc();
        let water = duct(
            "sys.water.cold",
            Point3::new(-1000.0, 0.0, 0.0),
            Point3::new(1000.0, 0.0, 0.0),
        );
        let drain = duct(
            "sys.drain.waste",
            Point3::new(0.0, -1000.0, 0.0),
            Point3::new(0.0, 1000.0, 0.0),
        );
        d.edit("Route", |tx| {
            add_route(tx, &water)?;
            add_route(tx, &drain)
        })
        .expect("commits");

        let rule = ClashRule {
            a: SystemFilter::system("sys.water.cold"),
            b: SystemFilter::system("sys.drain.waste"),
            kind: ClashKind::Hard,
            tolerance: 0.0,
            severity: Severity::Error,
        };
        assert_eq!(find_clashes(&d.db, &[rule]).len(), 1);

        let no_match = ClashRule {
            a: SystemFilter::system("sys.water.cold"),
            b: SystemFilter::system("sys.gas"),
            kind: ClashKind::Hard,
            tolerance: 0.0,
            severity: Severity::Error,
        };
        assert!(find_clashes(&d.db, &[no_match]).is_empty());
    }

    #[test]
    fn a_route_pasted_twice_is_a_duplicate() {
        let mut d = doc();
        let a = duct(
            "sys.air.supply",
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(5000.0, 0.0, 0.0),
        );
        let b = a.clone(); // drawn the same way twice — the common mistake
        let reversed = duct(
            "sys.air.supply",
            Point3::new(5000.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
        );
        d.edit("Route", |tx| {
            add_route(tx, &a)?;
            add_route(tx, &b)?;
            add_route(tx, &reversed)
        })
        .expect("commits");

        let rule = ClashRule {
            a: SystemFilter::any(),
            b: SystemFilter::any(),
            kind: ClashKind::Duplicate,
            tolerance: 1.0,
            severity: Severity::Warning,
        };
        // a-b, a-reversed, b-reversed: three duplicate pairs among the three
        // mutually-coincident routes.
        assert_eq!(find_clashes(&d.db, &[rule]).len(), 3);
    }

    #[test]
    fn a_partial_overlap_is_not_a_duplicate() {
        let mut d = doc();
        let a = duct(
            "sys.air.supply",
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(5000.0, 0.0, 0.0),
        );
        let overlapping_half = duct(
            "sys.air.supply",
            Point3::new(2500.0, 0.0, 0.0),
            Point3::new(7500.0, 0.0, 0.0),
        );
        d.edit("Route", |tx| {
            add_route(tx, &a)?;
            add_route(tx, &overlapping_half)
        })
        .expect("commits");

        let rule = ClashRule {
            a: SystemFilter::any(),
            b: SystemFilter::any(),
            kind: ClashKind::Duplicate,
            tolerance: 1.0,
            severity: Severity::Warning,
        };
        assert!(
            find_clashes(&d.db, &[rule]).is_empty(),
            "module docs: only exact duplicates are caught"
        );
    }

    #[test]
    fn a_rectangular_profiles_circumscribing_radius_is_conservative() {
        // 400x300: half-diagonal is 250mm, wider than either half-side.
        let r = effective_radius(Profile::Rect { w: 400.0, h: 300.0 });
        assert!((r - 250.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_zero_length_segments_do_not_panic() {
        let mut d = doc();
        let a = duct("sys.air.supply", Point3::ORIGIN, Point3::ORIGIN);
        let b = duct(
            "sys.air.supply",
            Point3::new(100.0, 0.0, 0.0),
            Point3::new(100.0, 0.0, 0.0),
        );
        d.edit("Route", |tx| {
            add_route(tx, &a)?;
            add_route(tx, &b)
        })
        .expect("commits");

        // Two 250mm-radius points, 100mm apart: well inside a hard clash.
        let issues = find_clashes(&d.db, &[hard_rule()]);
        assert_eq!(issues.len(), 1);
    }
}
