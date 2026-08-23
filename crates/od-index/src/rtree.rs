//! An R-tree over drawing bounds.
//!
//! Four operations in a CAD session are the same question — "what is near
//! here?" — asked at different scales: which entities are on screen, which one
//! is under the cursor, which endpoint should the cursor snap to, and which
//! pairs of parts might be clashing. Answering any of them by scanning every
//! entity is fine at a thousand entities and hopeless at a million, which is
//! the size the performance target is written against (N-01).
//!
//! The tree is built by Sort-Tile-Recursive bulk loading rather than by
//! repeated insertion. A drawing is loaded once and queried constantly, so the
//! shape of the tree matters far more than the cost of building it — and STR
//! produces near-optimal node overlap in one pass, where incremental insertion
//! degrades as a drawing grows in a direction the early splits did not expect.

use od_geom3d::{Aabb3, Point3};

/// Entries per node.
///
/// 16 is chosen for the CPU, not for disk: a node's children are scanned
/// linearly, so the sweet spot is where the scan still fits comfortably in
/// cache while keeping the tree shallow. At 16, a million entries is five
/// levels deep.
const NODE_CAPACITY: usize = 16;

/// A thing in the index, and where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry<T> {
    pub bounds: Aabb3,
    pub value: T,
}

#[derive(Debug, Clone)]
enum Node {
    Leaf {
        bounds: Aabb3,
        /// Indices into the entry list.
        entries: Vec<usize>,
    },
    Branch {
        bounds: Aabb3,
        children: Vec<Node>,
    },
}

impl Node {
    fn bounds(&self) -> Aabb3 {
        match self {
            Node::Leaf { bounds, .. } | Node::Branch { bounds, .. } => *bounds,
        }
    }
}

/// A bulk-loaded R-tree.
#[derive(Debug, Clone)]
pub struct RTree<T> {
    entries: Vec<Entry<T>>,
    root: Option<Node>,
}

impl<T> Default for RTree<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            root: None,
        }
    }
}

impl<T> RTree<T> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds an index over `items`.
    ///
    /// Entries with empty bounds are dropped: something with no extent cannot
    /// be found by a spatial query, and keeping it would make every node's
    /// bounds meaningless.
    #[must_use]
    pub fn bulk_load(items: impl IntoIterator<Item = Entry<T>>) -> Self {
        let entries: Vec<Entry<T>> = items.into_iter().filter(|e| !e.bounds.is_empty()).collect();
        if entries.is_empty() {
            return Self {
                entries,
                root: None,
            };
        }

        let indices: Vec<usize> = (0..entries.len()).collect();
        let root = build(&entries, indices);
        Self {
            entries,
            root: Some(root),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bounds of everything in the index.
    #[must_use]
    pub fn bounds(&self) -> Aabb3 {
        self.root.as_ref().map_or(Aabb3::EMPTY, Node::bounds)
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry<T>> {
        self.entries.iter()
    }

    /// Everything whose bounds intersect `window`.
    ///
    /// This is the broad phase: bounds overlap does not mean the geometry
    /// overlaps. Callers do the precise test on what comes back.
    #[must_use]
    pub fn query(&self, window: Aabb3) -> Vec<&Entry<T>> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            self.visit(root, &window, &mut out);
        }
        out
    }

    /// Like [`RTree::query`] but ignoring Z, for plan-view work where the
    /// window is a screen rectangle and depth is irrelevant.
    #[must_use]
    pub fn query_planar(&self, min: od_geom2d::Point2, max: od_geom2d::Point2) -> Vec<&Entry<T>> {
        self.query(Aabb3::new(
            Point3::new(min.x, min.y, f64::NEG_INFINITY),
            Point3::new(max.x, max.y, f64::INFINITY),
        ))
    }

    fn visit<'a>(&'a self, node: &'a Node, window: &Aabb3, out: &mut Vec<&'a Entry<T>>) {
        if !node.bounds().intersects(window) {
            return;
        }
        match node {
            Node::Leaf { entries, .. } => {
                for &i in entries {
                    if let Some(entry) = self.entries.get(i) {
                        if entry.bounds.intersects(window) {
                            out.push(entry);
                        }
                    }
                }
            }
            Node::Branch { children, .. } => {
                for child in children {
                    self.visit(child, window, out);
                }
            }
        }
    }

    /// The `count` entries closest to `point`, nearest first.
    ///
    /// Distance is to the entry's *bounds*, not its geometry — this is what
    /// object snap uses to decide which few entities are worth testing
    /// precisely, so an approximation that never excludes a true nearest is
    /// what is wanted.
    #[must_use]
    pub fn nearest(&self, point: Point3, count: usize) -> Vec<&Entry<T>> {
        if count == 0 || self.root.is_none() {
            return Vec::new();
        }

        // Best-first search: keep a frontier ordered by how close a node could
        // possibly be, and stop as soon as the frontier's best is further than
        // the worst result already found.
        let Some(root) = self.root.as_ref() else {
            return Vec::new();
        };
        let mut frontier: Vec<(f64, &Node)> = vec![(0.0, root)];
        let mut found: Vec<(f64, &Entry<T>)> = Vec::new();

        while let Some(best) = pop_min(&mut frontier) {
            let (node_distance, node) = best;
            if found.len() >= count {
                if let Some(worst) = found.last() {
                    if node_distance > worst.0 {
                        break;
                    }
                }
            }
            match node {
                Node::Leaf { entries, .. } => {
                    for &i in entries {
                        if let Some(entry) = self.entries.get(i) {
                            let d = distance_to(&entry.bounds, point);
                            insert_sorted(&mut found, (d, entry), count);
                        }
                    }
                }
                Node::Branch { children, .. } => {
                    for child in children {
                        frontier.push((distance_to(&child.bounds(), point), child));
                    }
                }
            }
        }

        found.into_iter().map(|(_, e)| e).collect()
    }
}

/// Sort-Tile-Recursive: sort by X, cut into vertical slices, sort each slice by
/// Y, and cut into tiles. The result is a set of leaves that are compact in both
/// directions, which is what keeps query cost down — a tall thin leaf gets
/// touched by every horizontal window that crosses it.
fn build<T>(entries: &[Entry<T>], mut indices: Vec<usize>) -> Node {
    if indices.len() <= NODE_CAPACITY {
        let bounds = indices.iter().fold(Aabb3::EMPTY, |acc, &i| {
            entries.get(i).map_or(acc, |e| acc.union(e.bounds))
        });
        return Node::Leaf {
            bounds,
            entries: indices,
        };
    }

    let leaf_count = indices.len().div_ceil(NODE_CAPACITY);
    // Slices are a square-ish grid of leaves, so tiles come out roughly square.
    #[expect(
        clippy::cast_precision_loss,
        reason = "leaf_count is an entry count; f64 is exact well past any real drawing"
    )]
    let slice_count = (leaf_count as f64).sqrt().ceil().max(1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "sqrt of a positive count, clamped below by max(1.0)"
    )]
    let slice_count = slice_count as usize;
    let per_slice = indices.len().div_ceil(slice_count);

    indices.sort_by(|&a, &b| center_x(entries, a).total_cmp(&center_x(entries, b)));

    let mut children = Vec::with_capacity(slice_count);
    for slice in indices.chunks(per_slice.max(1)) {
        let mut slice: Vec<usize> = slice.to_vec();
        slice.sort_by(|&a, &b| center_y(entries, a).total_cmp(&center_y(entries, b)));
        for tile in slice.chunks(NODE_CAPACITY) {
            children.push(build(entries, tile.to_vec()));
        }
    }

    // One level is enough when the tiles already fit in a node; otherwise the
    // children are themselves grouped, recursively.
    if children.len() > NODE_CAPACITY {
        let child_entries: Vec<Entry<usize>> = children
            .iter()
            .enumerate()
            .map(|(i, c)| Entry {
                bounds: c.bounds(),
                value: i,
            })
            .collect();
        let grouping = build(&child_entries, (0..children.len()).collect());
        return regroup(&mut children, &grouping);
    }

    let bounds = children
        .iter()
        .fold(Aabb3::EMPTY, |acc, c| acc.union(c.bounds()));
    Node::Branch { bounds, children }
}

/// Rebuilds the node hierarchy described by `grouping` using the real children.
///
/// `children` is drained as it goes — each index appears in exactly one leaf of
/// the grouping, so every child is taken exactly once. The whole list has to be
/// threaded through the recursion rather than moved into it, or the first group
/// takes everything and the rest come back empty.
fn regroup(children: &mut [Node], grouping: &Node) -> Node {
    fn take(children: &mut [Node], index: usize) -> Option<Node> {
        children.get_mut(index).map(|slot| {
            std::mem::replace(
                slot,
                Node::Leaf {
                    bounds: Aabb3::EMPTY,
                    entries: Vec::new(),
                },
            )
        })
    }

    match grouping {
        Node::Leaf { entries, .. } => {
            let taken: Vec<Node> = entries.iter().filter_map(|&i| take(children, i)).collect();
            let bounds = taken
                .iter()
                .fold(Aabb3::EMPTY, |acc, c| acc.union(c.bounds()));
            Node::Branch {
                bounds,
                children: taken,
            }
        }
        Node::Branch {
            children: groups, ..
        } => {
            let mut out = Vec::with_capacity(groups.len());
            for group in groups {
                out.push(regroup(children, group));
            }
            let bounds = out
                .iter()
                .fold(Aabb3::EMPTY, |acc, c| acc.union(c.bounds()));
            Node::Branch {
                bounds,
                children: out,
            }
        }
    }
}

fn center_x<T>(entries: &[Entry<T>], i: usize) -> f64 {
    entries.get(i).map_or(0.0, |e| e.bounds.center().x)
}

fn center_y<T>(entries: &[Entry<T>], i: usize) -> f64 {
    entries.get(i).map_or(0.0, |e| e.bounds.center().y)
}

/// Distance from a point to the nearest part of a box; zero inside it.
fn distance_to(b: &Aabb3, p: Point3) -> f64 {
    if b.is_empty() {
        return f64::INFINITY;
    }
    let dx = (b.min.x - p.x).max(0.0).max(p.x - b.max.x);
    let dy = (b.min.y - p.y).max(0.0).max(p.y - b.max.y);
    let dz = (b.min.z - p.z).max(0.0).max(p.z - b.max.z);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn pop_min<'a>(frontier: &mut Vec<(f64, &'a Node)>) -> Option<(f64, &'a Node)> {
    if frontier.is_empty() {
        return None;
    }
    let mut best = 0;
    for (i, item) in frontier.iter().enumerate() {
        if item.0 < frontier[best].0 {
            best = i;
        }
    }
    Some(frontier.swap_remove(best))
}

fn insert_sorted<'a, T>(
    found: &mut Vec<(f64, &'a Entry<T>)>,
    item: (f64, &'a Entry<T>),
    cap: usize,
) {
    let at = found.partition_point(|(d, _)| *d <= item.0);
    found.insert(at, item);
    found.truncate(cap);
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_geom2d::tol;

    fn cell(x: f64, y: f64, id: u32) -> Entry<u32> {
        Entry {
            bounds: Aabb3::new(Point3::new(x, y, 0.0), Point3::new(x + 10.0, y + 10.0, 0.0)),
            value: id,
        }
    }

    /// A grid of boxes, the shape a drawing's entities roughly have.
    fn grid(n: u32) -> RTree<u32> {
        let items = (0..n * n).map(|i| {
            let (x, y) = (f64::from(i % n) * 100.0, f64::from(i / n) * 100.0);
            cell(x, y, i)
        });
        RTree::bulk_load(items)
    }

    fn window(min: (f64, f64), max: (f64, f64)) -> Aabb3 {
        Aabb3::new(
            Point3::new(min.0, min.1, -1.0),
            Point3::new(max.0, max.1, 1.0),
        )
    }

    #[test]
    fn an_empty_index_answers_without_panicking() {
        let tree: RTree<u32> = RTree::new();
        assert!(tree.is_empty());
        assert!(tree.bounds().is_empty());
        assert!(tree.query(window((0.0, 0.0), (100.0, 100.0))).is_empty());
        assert!(tree.nearest(Point3::ORIGIN, 5).is_empty());
    }

    #[test]
    fn entries_with_no_extent_are_not_indexed() {
        let tree = RTree::bulk_load([
            cell(0.0, 0.0, 1),
            Entry {
                bounds: Aabb3::EMPTY,
                value: 2,
            },
        ]);
        assert_eq!(tree.len(), 1, "an empty box cannot be found by any query");
    }

    #[test]
    fn a_query_returns_exactly_what_a_linear_scan_would() {
        let tree = grid(20);
        let w = window((250.0, 250.0), (650.0, 450.0));

        let mut from_index: Vec<u32> = tree.query(w).into_iter().map(|e| e.value).collect();
        let mut from_scan: Vec<u32> = tree
            .entries()
            .filter(|e| e.bounds.intersects(&w))
            .map(|e| e.value)
            .collect();
        from_index.sort_unstable();
        from_scan.sort_unstable();

        assert_eq!(from_index, from_scan);
        assert!(!from_index.is_empty(), "the window should hit something");
    }

    #[test]
    fn every_entry_is_reachable_through_the_tree() {
        // A tree that loses entries during the build is the failure that a
        // window query would only reveal by accident.
        let tree = grid(30);
        let everything = tree.query(tree.bounds());
        assert_eq!(everything.len(), tree.len(), "900 entries, all findable");
    }

    #[test]
    fn a_window_outside_the_drawing_finds_nothing() {
        let tree = grid(10);
        assert!(
            tree.query(window((-5000.0, -5000.0), (-4000.0, -4000.0)))
                .is_empty()
        );
    }

    #[test]
    fn touching_bounds_count_as_a_hit() {
        let tree = RTree::bulk_load([cell(0.0, 0.0, 1)]);
        // The window's edge lands exactly on the entry's edge.
        let hits = tree.query(window((10.0, 0.0), (20.0, 10.0)));
        assert_eq!(hits.len(), 1, "a shared edge is a real adjacency");
    }

    #[test]
    fn nearest_orders_by_distance_and_respects_the_count() {
        let tree = grid(10);
        let found = tree.nearest(Point3::new(-50.0, -50.0, 0.0), 3);
        assert_eq!(found.len(), 3);

        let distances: Vec<f64> = found
            .iter()
            .map(|e| distance_to(&e.bounds, Point3::new(-50.0, -50.0, 0.0)))
            .collect();
        for pair in distances.windows(2) {
            assert!(pair[0] <= pair[1], "results must be nearest-first");
        }
        // The corner cell is closest to a point beyond the corner.
        assert_eq!(found[0].value, 0);
    }

    #[test]
    fn nearest_agrees_with_a_linear_scan() {
        let tree = grid(15);
        let probe = Point3::new(733.0, 291.0, 0.0);

        let from_index: Vec<u32> = tree
            .nearest(probe, 5)
            .into_iter()
            .map(|e| e.value)
            .collect();

        let mut scan: Vec<(f64, u32)> = tree
            .entries()
            .map(|e| (distance_to(&e.bounds, probe), e.value))
            .collect();
        scan.sort_by(|a, b| a.0.total_cmp(&b.0));
        let from_scan: Vec<u32> = scan.into_iter().take(5).map(|(_, v)| v).collect();

        assert_eq!(from_index, from_scan);
    }

    #[test]
    fn a_point_inside_an_entry_is_at_zero_distance() {
        let tree = RTree::bulk_load([cell(0.0, 0.0, 7)]);
        let found = tree.nearest(Point3::new(5.0, 5.0, 0.0), 1);
        assert_eq!(found.len(), 1);
        assert!(tol::is_zero_len(distance_to(
            &found[0].bounds,
            Point3::new(5.0, 5.0, 0.0)
        )));
    }

    #[test]
    fn planar_queries_ignore_height() {
        // Two entries at the same plan position, different heights — the case
        // that matters for MEP, where services stack in the ceiling void.
        let tree = RTree::bulk_load([
            Entry {
                bounds: Aabb3::new(
                    Point3::new(0.0, 0.0, 2800.0),
                    Point3::new(100.0, 100.0, 3000.0),
                ),
                value: 1,
            },
            Entry {
                bounds: Aabb3::new(
                    Point3::new(0.0, 0.0, 3400.0),
                    Point3::new(100.0, 100.0, 3600.0),
                ),
                value: 2,
            },
        ]);

        let hits = tree.query_planar(
            od_geom2d::Point2::new(10.0, 10.0),
            od_geom2d::Point2::new(90.0, 90.0),
        );
        assert_eq!(hits.len(), 2, "plan view sees both");
    }

    #[test]
    fn a_large_index_stays_correct() {
        // 10,000 entries exercises the recursive regrouping path, which the
        // small cases never reach.
        let tree = grid(100);
        assert_eq!(tree.len(), 10_000);
        assert_eq!(tree.query(tree.bounds()).len(), 10_000);

        let corner = tree.query(window((-10.0, -10.0), (150.0, 150.0)));
        assert_eq!(corner.len(), 4, "a 2×2 corner of the grid");
    }
}
