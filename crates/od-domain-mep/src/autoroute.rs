//! Automatic route search (F-107, `docs/04-mep.md` §4.3).
//!
//! The design doc calls for a non-uniform 3D grid (dense where a ceiling
//! void is tight, sparse in open space), A* with a cost of
//! `length + turns×α + level_change×β + proximity_to_other_systems×γ`,
//! and three output candidates (shortest / fewest turns / closest to
//! existing routes). This module narrows all three axes to what is
//! honestly buildable without data this workspace does not have yet:
//!
//! - **Planar only.** [`find_route`] rejects `start.z != end.z`
//!   ([`MepError::NotPlanar`], the exact error [`crate::route::draw_route`]
//!   already raises for the same reason: nothing downstream can build a
//!   riser yet). This turns "3D grid" into "2D grid at a fixed height",
//!   which also means the design doc's `level_change×β` term never applies
//!   — there is no level to change.
//! - **A *coordinate-compression* grid, not a density-aware one.** Grid
//!   lines sit exactly on every obstacle edge and on the start/end points
//!   (a well-known exact technique for rectilinear obstacle avoidance,
//!   sometimes called a trellis), rather than being denser near a ceiling
//!   void this workspace has no model of. This is still non-uniform, just
//!   for a different, simpler reason: it guarantees no obstacle corner or
//!   endpoint is ever missed to grid discretisation, and it scales with
//!   obstacle count rather than drawing size.
//! - **Orthogonal moves only.** Every step is axis-aligned, so every turn
//!   is exactly 90° — which [`crate::route::draw_route`]'s fitting rules
//!   already know how to build. The design doc's step 5 ("verify
//!   feasibility against the fitting rules, reject infeasible bends") is
//!   therefore satisfied by construction rather than a separate check: a
//!   candidate this module returns is always usable as-is by
//!   `RouteSpec::path`.
//! - **Two candidates, not three.** Shortest and fewest-turns come from
//!   running the same search with a different turn-cost weighting.
//!   "Closest to existing routes" needs the design doc's `γ` proximity
//!   term — biasing the search toward other routes' corridors — which
//!   needs a notion of "nearby route direction" this module does not
//!   implement; [`find_candidates`] returns what it can compute honestly
//!   rather than a placeholder third entry.
//!
//! A caller is always free to hand-edit a returned [`RouteCandidate`]'s
//! `path` before feeding it to [`crate::route::draw_route`] — the design
//! doc's own requirement that "a person always chooses and can redraw"
//! holds simply because the output is a plain `Vec<Point3>`, the same
//! shape a manually-drawn route already takes.

use crate::model::MepError;
use od_core::tol;
use od_geom3d::{Aabb3, Point3};
use std::collections::{HashMap, HashSet};

/// How much a direction change costs, in the same units as physical length
/// (mm) — a turn "costs" as much as this many millimetres of extra travel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteSearchWeights {
    pub turn_penalty: f64,
}

impl RouteSearchWeights {
    pub const SHORTEST: Self = Self { turn_penalty: 0.0 };
    /// A duct or pipe run spanning even a few hundred metres in one
    /// drawing would be unusual; a penalty far past that (100km) makes the
    /// search prefer any route with fewer turns over any route that is
    /// merely shorter, for every drawing this system is meant to handle.
    pub const FEWEST_TURNS: Self = Self {
        turn_penalty: 100_000_000.0,
    };
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteCandidate {
    /// Smoothed: a point only where the path starts, ends, or turns —
    /// ready to hand to `RouteSpec::path` unmodified.
    pub path: Vec<Point3>,
    pub length_mm: f64,
    pub turns: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Direction {
    PlusX,
    MinusX,
    PlusY,
    MinusY,
}

type Key = (usize, usize, Option<Direction>);

/// Searches for one route from `start` to `end`, avoiding `obstacles`
/// (already inflated for clearance/insulation by the caller — the same
/// convention [`od_geom3d::Aabb3::inflated`] and `crate::clash` already
/// use). Only obstacles whose Z-range spans `start.z` are considered; the
/// rest cannot matter to a route confined to that height.
///
/// # Errors
/// [`MepError::NotPlanar`] if `start`/`end` are not at the same height,
/// [`MepError::StartBlocked`]/[`MepError::EndBlocked`] if either point
/// already lies inside an obstacle, or [`MepError::NoRoute`] if no
/// obstacle-free path exists at all.
pub fn find_route(
    start: Point3,
    end: Point3,
    obstacles: &[Aabb3],
    weights: RouteSearchWeights,
) -> crate::Result<RouteCandidate> {
    if !tol::eq_len(start.z, end.z) {
        return Err(MepError::NotPlanar);
    }
    let z = start.z;

    let rects: Vec<(f64, f64, f64, f64)> = obstacles
        .iter()
        .filter(|o| !o.is_empty() && o.min.z <= z && z <= o.max.z)
        .map(|o| (o.min.x, o.min.y, o.max.x, o.max.y))
        .collect();

    let point_blocked = |p: (f64, f64)| {
        rects.iter().any(|&(x0, y0, x1, y1)| {
            p.0 > x0 + tol::POINT_EPS
                && p.0 < x1 - tol::POINT_EPS
                && p.1 > y0 + tol::POINT_EPS
                && p.1 < y1 - tol::POINT_EPS
        })
    };
    if point_blocked((start.x, start.y)) {
        return Err(MepError::StartBlocked);
    }
    if point_blocked((end.x, end.y)) {
        return Err(MepError::EndBlocked);
    }

    let mut xs: Vec<f64> = Vec::with_capacity(rects.len() * 2 + 2);
    let mut ys: Vec<f64> = Vec::with_capacity(rects.len() * 2 + 2);
    for &(x0, y0, x1, y1) in &rects {
        xs.push(x0);
        xs.push(x1);
        ys.push(y0);
        ys.push(y1);
    }
    xs.sort_by(f64::total_cmp);
    xs.dedup_by(|a, b| tol::eq_len(*a, *b));
    ys.sort_by(f64::total_cmp);
    ys.dedup_by(|a, b| tol::eq_len(*a, *b));

    let start_node = (
        insert_coord(&mut xs, start.x),
        insert_coord(&mut ys, start.y),
    );
    let end_node = (insert_coord(&mut xs, end.x), insert_coord(&mut ys, end.y));

    let path = search(&xs, &ys, &rects, start_node, end_node, weights).ok_or(MepError::NoRoute)?;
    Ok(smooth(&path, &xs, &ys, z))
}

/// Runs [`find_route`] under [`RouteSearchWeights::SHORTEST`] and
/// [`RouteSearchWeights::FEWEST_TURNS`], returning one candidate per
/// distinct result (module docs on why this is two, not the design doc's
/// three).
///
/// # Errors
/// Whatever [`find_route`] returns for the first (shortest) search — if no
/// route exists at all, there is nothing for a second search to find
/// either.
pub fn find_candidates(
    start: Point3,
    end: Point3,
    obstacles: &[Aabb3],
) -> crate::Result<Vec<RouteCandidate>> {
    let shortest = find_route(start, end, obstacles, RouteSearchWeights::SHORTEST)?;
    let fewest_turns = find_route(start, end, obstacles, RouteSearchWeights::FEWEST_TURNS)?;
    if fewest_turns.path == shortest.path {
        Ok(vec![shortest])
    } else {
        Ok(vec![shortest, fewest_turns])
    }
}

/// The index of `v` within `coords` (kept sorted), inserting it first if
/// not already present.
fn insert_coord(coords: &mut Vec<f64>, v: f64) -> usize {
    match coords.binary_search_by(|c| c.total_cmp(&v)) {
        Ok(i) => i,
        Err(i) => {
            coords.insert(i, v);
            i
        }
    }
}

/// A* over the trellis. State is `(x index, y index, direction used to
/// arrive)` rather than just the node, since the turn penalty depends on
/// which way the path was already travelling — the classic technique for
/// turn-aware grid search. Returns the sequence of states from start to
/// end (inclusive), or `None` if `end` is unreachable.
fn search(
    xs: &[f64],
    ys: &[f64],
    rects: &[(f64, f64, f64, f64)],
    start: (usize, usize),
    end: (usize, usize),
    weights: RouteSearchWeights,
) -> Option<Vec<Key>> {
    let edge_blocked = |a: (usize, usize), b: (usize, usize)| {
        let mid = ((xs[a.0] + xs[b.0]) / 2.0, (ys[a.1] + ys[b.1]) / 2.0);
        rects.iter().any(|&(x0, y0, x1, y1)| {
            mid.0 > x0 + tol::POINT_EPS
                && mid.0 < x1 - tol::POINT_EPS
                && mid.1 > y0 + tol::POINT_EPS
                && mid.1 < y1 - tol::POINT_EPS
        })
    };
    // Manhattan distance: admissible for axis-aligned moves with
    // non-negative costs (the turn penalty only ever adds cost the
    // heuristic already ignores, so it never overestimates).
    let heuristic =
        |node: (usize, usize)| (xs[node.0] - xs[end.0]).abs() + (ys[node.1] - ys[end.1]).abs();

    let start_key: Key = (start.0, start.1, None);
    let mut g_score: HashMap<Key, f64> = HashMap::from([(start_key, 0.0)]);
    let mut came_from: HashMap<Key, Key> = HashMap::new();
    let mut frontier: Vec<(f64, Key)> = vec![(heuristic(start), start_key)];
    let mut visited: HashSet<Key> = HashSet::new();

    while !frontier.is_empty() {
        // Linear scan for the minimum f-score: this workspace's
        // established pattern for an f64 priority (`od_index::RTree`'s
        // `pop_min`), avoiding a NaN-unsafe `Ord` wrapper for a binary
        // heap over a search space this small.
        let mut best = 0;
        for i in 1..frontier.len() {
            if frontier[i].0 < frontier[best].0 {
                best = i;
            }
        }
        let (_, current) = frontier.swap_remove(best);
        if !visited.insert(current) {
            continue;
        }

        let (ci, cj, cdir) = current;
        if (ci, cj) == end {
            let mut path = vec![current];
            let mut cur = current;
            while let Some(&prev) = came_from.get(&cur) {
                path.push(prev);
                cur = prev;
            }
            path.reverse();
            return Some(path);
        }

        let mut neighbors: Vec<((usize, usize), Direction)> = Vec::with_capacity(4);
        if ci + 1 < xs.len() {
            neighbors.push(((ci + 1, cj), Direction::PlusX));
        }
        if ci > 0 {
            neighbors.push(((ci - 1, cj), Direction::MinusX));
        }
        if cj + 1 < ys.len() {
            neighbors.push(((ci, cj + 1), Direction::PlusY));
        }
        if cj > 0 {
            neighbors.push(((ci, cj - 1), Direction::MinusY));
        }

        let current_g = g_score.get(&current).copied().unwrap_or(f64::INFINITY);
        for (n, dir) in neighbors {
            if edge_blocked((ci, cj), n) {
                continue;
            }
            let step_len = ((xs[n.0] - xs[ci]).powi(2) + (ys[n.1] - ys[cj]).powi(2)).sqrt();
            // No penalty on the very first step (cdir is None: there is no
            // prior direction to turn away from) or when continuing
            // straight; only an actual change between two real directions
            // costs anything.
            let turn_cost = match cdir {
                Some(prev) if prev != dir => weights.turn_penalty,
                _ => 0.0,
            };
            let tentative_g = current_g + step_len + turn_cost;

            let neighbor_key: Key = (n.0, n.1, Some(dir));
            let best_known = g_score.get(&neighbor_key).copied().unwrap_or(f64::INFINITY);
            if tentative_g < best_known {
                g_score.insert(neighbor_key, tentative_g);
                came_from.insert(neighbor_key, current);
                frontier.push((tentative_g + heuristic(n), neighbor_key));
            }
        }
    }
    None
}

/// Collapses a raw grid-step sequence to the points where the path starts,
/// ends, or changes direction — exactly the vertices `RouteSpec::path`
/// needs, no redundant collinear waypoints in between.
fn smooth(path: &[Key], xs: &[f64], ys: &[f64], z: f64) -> RouteCandidate {
    let world = |(i, j, _): Key| Point3::new(xs[i], ys[j], z);

    let mut points: Vec<Point3> = Vec::new();
    let mut turns = 0;
    if let Some(&first) = path.first() {
        points.push(world(first));
    }
    // path[i]'s direction is the one used to *arrive* at i; path[i+1]'s is
    // the one used to *leave* it. i is a corner exactly when those two
    // differ -- both are always `Some` here since i, i+1 >= 1. Pushing
    // path[i] itself (not path[i+1], which is one step past the actual
    // corner) is the fix for the off-by-one that used to connect two
    // non-adjacent grid nodes with a diagonal that no orthogonal move ever
    // produced.
    for i in 1..path.len().saturating_sub(1) {
        if path[i].2 != path[i + 1].2 {
            points.push(world(path[i]));
            turns += 1;
        }
    }
    if path.len() > 1 {
        points.push(world(path[path.len() - 1]));
    }

    let length_mm = points.windows(2).map(|w| w[0].distance_to(w[1])).sum();

    RouteCandidate {
        path: points,
        length_mm,
        turns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clear_line_of_sight_is_a_single_straight_segment() {
        let candidate = find_route(
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(5000.0, 0.0, 2800.0),
            &[],
            RouteSearchWeights::SHORTEST,
        )
        .expect("finds a route");
        assert_eq!(
            candidate.path,
            vec![
                Point3::new(0.0, 0.0, 2800.0),
                Point3::new(5000.0, 0.0, 2800.0),
            ]
        );
        assert!(tol::eq_len(candidate.length_mm, 5000.0));
        assert_eq!(candidate.turns, 0);
    }

    #[test]
    fn an_obstacle_directly_in_the_way_forces_a_detour() {
        // A wall spanning the direct path's full height range, centred
        // between start and end.
        let obstacle = Aabb3::new(
            Point3::new(2000.0, -1000.0, 0.0),
            Point3::new(3000.0, 1000.0, 5000.0),
        );
        let candidate = find_route(
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(5000.0, 0.0, 2800.0),
            &[obstacle],
            RouteSearchWeights::SHORTEST,
        )
        .expect("finds a route around the wall");
        assert!(
            candidate.length_mm > 5000.0,
            "a detour must be longer than the blocked straight line"
        );
        assert!(
            candidate.turns >= 2,
            "a detour needs at least an out-and-back pair of turns"
        );

        // Every leg of the smoothed path must actually clear the obstacle.
        for leg in candidate.path.windows(2) {
            assert!(
                tol::eq_len(leg[0].x, leg[1].x) || tol::eq_len(leg[0].y, leg[1].y),
                "leg {leg:?} is not axis-aligned"
            );
            let midpoint = leg[0].midpoint(leg[1]);
            let clear = midpoint.x <= obstacle.min.x
                || midpoint.x >= obstacle.max.x
                || midpoint.y <= obstacle.min.y
                || midpoint.y >= obstacle.max.y;
            assert!(clear, "leg {leg:?} passes through the obstacle");
        }
    }

    #[test]
    fn a_start_point_inside_an_obstacle_is_rejected() {
        let obstacle = Aabb3::new(
            Point3::new(-100.0, -100.0, 0.0),
            Point3::new(100.0, 100.0, 5000.0),
        );
        let err = find_route(
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(5000.0, 0.0, 2800.0),
            &[obstacle],
            RouteSearchWeights::SHORTEST,
        )
        .expect_err("start is inside the obstacle");
        assert!(matches!(err, MepError::StartBlocked));
    }

    #[test]
    fn an_end_point_inside_an_obstacle_is_rejected() {
        let obstacle = Aabb3::new(
            Point3::new(4900.0, -100.0, 0.0),
            Point3::new(5100.0, 100.0, 5000.0),
        );
        let err = find_route(
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(5000.0, 0.0, 2800.0),
            &[obstacle],
            RouteSearchWeights::SHORTEST,
        )
        .expect_err("end is inside the obstacle");
        assert!(matches!(err, MepError::EndBlocked));
    }

    #[test]
    fn a_fully_enclosed_end_point_has_no_route() {
        // A closed box around the end point, but not the start point --
        // the end is reachable geometrically yet walled off from outside.
        let walls = [
            Aabb3::new(
                Point3::new(4500.0, -600.0, 0.0),
                Point3::new(5500.0, -500.0, 5000.0),
            ),
            Aabb3::new(
                Point3::new(4500.0, 500.0, 0.0),
                Point3::new(5500.0, 600.0, 5000.0),
            ),
            Aabb3::new(
                Point3::new(4500.0, -600.0, 0.0),
                Point3::new(4600.0, 600.0, 5000.0),
            ),
            Aabb3::new(
                Point3::new(5400.0, -600.0, 0.0),
                Point3::new(5500.0, 600.0, 5000.0),
            ),
        ];
        let err = find_route(
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(5000.0, 0.0, 2800.0),
            &walls,
            RouteSearchWeights::SHORTEST,
        )
        .expect_err("the end point is fully enclosed");
        assert!(matches!(err, MepError::NoRoute));
    }

    #[test]
    fn different_heights_are_rejected_as_not_planar() {
        let err = find_route(
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(5000.0, 0.0, 3200.0),
            &[],
            RouteSearchWeights::SHORTEST,
        )
        .expect_err("a riser is not supported");
        assert!(matches!(err, MepError::NotPlanar));
    }

    #[test]
    fn fewest_turns_can_differ_from_shortest() {
        // Two parallel walls with offset gaps: the shortest path zigzags
        // through both gaps, but a route that goes around the near wall's
        // open end entirely is longer yet needs fewer turns.
        let wall_a = Aabb3::new(
            Point3::new(1000.0, -2000.0, 0.0),
            Point3::new(1200.0, 200.0, 5000.0),
        );
        let wall_b = Aabb3::new(
            Point3::new(2000.0, -200.0, 0.0),
            Point3::new(2200.0, 2000.0, 5000.0),
        );
        let obstacles = [wall_a, wall_b];

        let candidates = find_candidates(
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(3200.0, 0.0, 2800.0),
            &obstacles,
        )
        .expect("finds routes");

        assert_eq!(
            candidates.len(),
            2,
            "this obstacle layout has a real length/turns trade-off"
        );
        let (shortest, fewest_turns) = (&candidates[0], &candidates[1]);
        assert!(shortest.length_mm < fewest_turns.length_mm);
        assert!(fewest_turns.turns < shortest.turns);
        for candidate in &candidates {
            for leg in candidate.path.windows(2) {
                assert!(
                    tol::eq_len(leg[0].x, leg[1].x) || tol::eq_len(leg[0].y, leg[1].y),
                    "leg {leg:?} is not axis-aligned"
                );
            }
        }
    }

    #[test]
    fn a_start_equal_to_end_is_a_degenerate_single_point_route() {
        let candidate = find_route(
            Point3::new(1000.0, 2000.0, 2800.0),
            Point3::new(1000.0, 2000.0, 2800.0),
            &[],
            RouteSearchWeights::SHORTEST,
        )
        .expect("start equals end is not an error");
        assert!(tol::eq_len(candidate.length_mm, 0.0));
        assert_eq!(candidate.turns, 0);
    }

    #[test]
    fn obstacles_outside_the_routes_height_are_ignored() {
        // A "wall" that exists only far above the route's Z -- must not
        // force a detour.
        let obstacle = Aabb3::new(
            Point3::new(2000.0, -1000.0, 6000.0),
            Point3::new(3000.0, 1000.0, 7000.0),
        );
        let candidate = find_route(
            Point3::new(0.0, 0.0, 2800.0),
            Point3::new(5000.0, 0.0, 2800.0),
            &[obstacle],
            RouteSearchWeights::SHORTEST,
        )
        .expect("finds a route");
        assert_eq!(
            candidate.turns, 0,
            "the obstacle does not span this route's height"
        );
    }
}
