//! The connection graph (`docs/04-mep.md` §2).
//!
//! Every [`WorldPort`] in the document is a node; two ports are joined when
//! they occupy the same point, face opposite ways, and agree on system and
//! profile. That single rule is what turns "a pile of lines" into "a system":
//! it is how a dangling pipe end gets caught before it reaches a contractor,
//! and it is the basis every later feature — pressure-drop walks,
//! system-colour propagation, take-off by connected run — builds on.
//!
//! Matching goes through [`od_index::RTree`] rather than a pairwise scan
//! (`O(n log n)` candidate narrowing instead of `O(n²)`), the same structure
//! used for view culling and picking — connection matching is "what is near
//! here" too, just asked once at build time instead of once per frame.

use crate::model::WorldPort;
use crate::ports::{place, route_ports};
use crate::store::{all_parts, all_routes};
use od_core::{Database, ObjectId, tol};
use od_index::{Entry, RTree};
use od_parts::{Catalog, SystemKind};

#[derive(Debug)]
pub struct ConnectionGraph {
    ports: Vec<WorldPort>,
    /// Parallel to `ports`: the index of the port it mates with, if any.
    links: Vec<Option<usize>>,
    /// Part or route ids whose catalogue lookup failed, so a caller can report
    /// why the graph is incomplete rather than silently missing objects.
    skipped: Vec<ObjectId>,
}

impl ConnectionGraph {
    /// Builds the graph over every MEP object in the document.
    #[must_use]
    pub fn build(db: &Database, catalog: &Catalog) -> Self {
        let mut ports = Vec::new();
        let mut skipped = Vec::new();

        for (id, route) in all_routes(db) {
            let kind = catalog
                .system(&route.system)
                .map_or(SystemKind::Generic, |s| s.kind);
            ports.extend(route_ports(id, &route, kind));
        }
        for (id, part) in all_parts(db) {
            match place(catalog, id, &part) {
                Ok((_, world)) => ports.extend(world),
                Err(_) => skipped.push(id),
            }
        }

        let links = match_ports(&ports);
        Self {
            ports,
            links,
            skipped,
        }
    }

    #[must_use]
    pub fn ports(&self) -> &[WorldPort] {
        &self.ports
    }

    /// Object ids the graph could not resolve — a part whose catalogue entry
    /// is missing or fails to build at its stored parameters.
    #[must_use]
    pub fn skipped(&self) -> &[ObjectId] {
        &self.skipped
    }

    /// Every mated pair, each appearing once.
    pub fn connections(&self) -> impl Iterator<Item = (&WorldPort, &WorldPort)> {
        self.links.iter().enumerate().filter_map(|(i, link)| {
            link.filter(|&j| j > i)
                .map(|j| (&self.ports[i], &self.ports[j]))
        })
    }

    /// Ports with nothing on the other end — a dangling run, or a fitting
    /// missing its neighbour. This is the check that matters most: an
    /// unconnected port in a submitted drawing is a mistake someone will find
    /// on site rather than on screen (F-108).
    #[must_use]
    pub fn unconnected(&self) -> Vec<&WorldPort> {
        self.links
            .iter()
            .enumerate()
            .filter(|(_, link)| link.is_none())
            .map(|(i, _)| &self.ports[i])
            .collect()
    }

    /// Every port, alongside whether it currently mates with another — what a
    /// caller offering ports up as snap targets needs, since an already-mated
    /// port is rarely one a user meant to route toward again.
    pub fn ports_with_status(&self) -> impl Iterator<Item = (&WorldPort, bool)> {
        self.ports
            .iter()
            .zip(self.links.iter().map(Option::is_some))
    }

    /// The ports directly connected to anything owned by `owner`.
    pub fn neighbors(&self, owner: ObjectId) -> Vec<&WorldPort> {
        self.ports
            .iter()
            .enumerate()
            .filter(|(_, p)| p.owner == owner)
            .filter_map(|(i, _)| self.links[i].map(|j| &self.ports[j]))
            .collect()
    }
}

/// For each port, the index of the one port that mates with it, or `None`.
fn match_ports(ports: &[WorldPort]) -> Vec<Option<usize>> {
    let tree: RTree<usize> = RTree::bulk_load(ports.iter().enumerate().map(|(i, p)| Entry {
        bounds: od_geom3d::Aabb3::new(p.position, p.position),
        value: i,
    }));

    ports
        .iter()
        .enumerate()
        .map(|(i, port)| {
            // Ask for a few neighbours, not one: several ports can legitimately
            // sit at the same point (a tee's three legs all start there), and
            // the nearest by raw distance is not necessarily the one that
            // actually mates — direction and system still have to agree.
            tree.nearest(port.position, 6)
                .into_iter()
                .map(|e| e.value)
                .find(|&j| j != i && mates(port, &ports[j]))
        })
        .collect()
}

/// Coincident position, opposing direction, matching system and profile. All
/// four have to hold — a duct end resting against a pipe end is not a
/// connection, and this is exactly the mistake the graph exists to catch
/// rather than paper over.
fn mates(a: &WorldPort, b: &WorldPort) -> bool {
    a.position.coincides_with(b.position)
        && a.system_kind == b.system_kind
        && a.profile.matches(&b.profile)
        && tol::eq_len(a.direction.dot(b.direction), -1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PlacedPart, PlacementKind, RouteSegment};
    use crate::store::{add_part, add_route};
    use od_core::{ActorId, Document, Point3};
    use od_parts::Profile;
    use std::collections::HashMap;

    fn catalog() -> Catalog {
        Catalog::bundled().expect("bundled catalogue is valid")
    }

    fn duct(w: f64, h: f64) -> Profile {
        Profile::Rect { w, h }
    }

    #[test]
    fn two_routes_that_share_an_endpoint_connect() {
        let mut d = Document::new(Database::new(ActorId::SYSTEM));
        let a = RouteSegment {
            system: "sys.air.supply".into(),
            spec: "spec.duct.galvanised.rect".into(),
            profile: duct(400.0, 300.0),
            start: Point3::ORIGIN,
            end: Point3::new(5000.0, 0.0, 0.0),
        };
        let b = RouteSegment {
            system: "sys.air.supply".into(),
            spec: "spec.duct.galvanised.rect".into(),
            profile: duct(400.0, 300.0),
            start: Point3::new(5000.0, 0.0, 0.0),
            end: Point3::new(10_000.0, 0.0, 0.0),
        };
        d.edit("Route", |tx| {
            add_route(tx, &a)?;
            add_route(tx, &b)
        })
        .expect("commits");

        let graph = ConnectionGraph::build(&d.db, &catalog());
        assert!(graph.skipped().is_empty());
        assert_eq!(graph.connections().count(), 1);
        assert!(graph.unconnected().len() == 2, "the two free ends");
    }

    #[test]
    fn a_lone_route_has_two_unconnected_ports() {
        let mut d = Document::new(Database::new(ActorId::SYSTEM));
        let r = RouteSegment {
            system: "sys.water.cold".into(),
            spec: "spec.pipe.sgp".into(),
            profile: Profile::Round { d: 50.0 },
            start: Point3::ORIGIN,
            end: Point3::new(3000.0, 0.0, 0.0),
        };
        d.edit("Route", |tx| add_route(tx, &r)).expect("commits");

        let graph = ConnectionGraph::build(&d.db, &catalog());
        assert_eq!(graph.unconnected().len(), 2);
        assert_eq!(graph.connections().count(), 0);
    }

    #[test]
    fn different_systems_do_not_mate_even_at_the_same_point() {
        let mut d = Document::new(Database::new(ActorId::SYSTEM));
        let water = RouteSegment {
            system: "sys.water.cold".into(),
            spec: "spec.pipe.sgp".into(),
            profile: Profile::Round { d: 50.0 },
            start: Point3::ORIGIN,
            end: Point3::new(3000.0, 0.0, 0.0),
        };
        let drain = RouteSegment {
            system: "sys.drain.waste".into(),
            spec: "spec.pipe.vp".into(),
            profile: Profile::Round { d: 50.0 },
            start: Point3::new(3000.0, 0.0, 0.0),
            end: Point3::new(6000.0, 0.0, 0.0),
        };
        d.edit("Route", |tx| {
            add_route(tx, &water)?;
            add_route(tx, &drain)
        })
        .expect("commits");

        let graph = ConnectionGraph::build(&d.db, &catalog());
        assert_eq!(
            graph.connections().count(),
            0,
            "a supply duct joined to a drain is exactly the mistake this must catch"
        );
        assert_eq!(graph.unconnected().len(), 4);
    }

    #[test]
    fn a_route_connects_to_an_equipment_port_at_the_same_point() {
        let catalog = catalog();
        let mut d = Document::new(Database::new(ActorId::SYSTEM));

        let ahu = PlacedPart {
            part_id: "hvac.fan.sirocco".into(),
            position: Point3::new(10_000.0, 10_000.0, 2800.0),
            rotation: 0.0,
            mirror_y: false,
            params: HashMap::new(),
            kind: PlacementKind::Equipment,
            system: None,
        };
        let ahu_id = d
            .edit("Place fan", |tx| add_part(tx, &ahu))
            .expect("commits");

        let (_, fan_ports) = crate::ports::place(&catalog, ahu_id, &ahu).expect("places");
        let outlet = fan_ports
            .iter()
            .find(|p| p.name == "out")
            .expect("the fan has an outlet")
            .clone();

        let run = RouteSegment {
            system: "sys.air.supply".into(),
            spec: "spec.duct.galvanised.rect".into(),
            profile: outlet.profile,
            start: outlet.position,
            end: outlet.position + outlet.direction * 3000.0,
        };
        d.edit("Route from fan", |tx| add_route(tx, &run))
            .expect("commits");

        let graph = ConnectionGraph::build(&d.db, &catalog);
        assert_eq!(
            graph.connections().count(),
            1,
            "the duct must auto-connect to the fan's outlet"
        );
        // The fan's `out` port is now spoken for, but its `in` duct port and
        // `power` terminal are not, and neither is the free end of the run.
        assert_eq!(graph.unconnected().len(), 3);
    }

    #[test]
    fn an_unresolvable_part_is_reported_rather_than_silently_dropped() {
        let mut d = Document::new(Database::new(ActorId::SYSTEM));
        let bogus = PlacedPart {
            part_id: "no.such.part".into(),
            position: Point3::ORIGIN,
            rotation: 0.0,
            mirror_y: false,
            params: HashMap::new(),
            kind: PlacementKind::Equipment,
            system: None,
        };
        let id = d
            .edit("Place bogus", |tx| add_part(tx, &bogus))
            .expect("commits");

        let graph = ConnectionGraph::build(&d.db, &catalog());
        assert_eq!(graph.skipped(), &[id]);
    }
}
