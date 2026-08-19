//! Making a source object visible (`docs/04-mep.md` §3).
//!
//! **The truth is centreline + profile + system; a view is derived from it.**
//! This module is where that sentence becomes code: [`route_geometry`] is a
//! pure function from a [`RouteSegment`] to the [`Geometry`] a view shows, and
//! [`insert_route`]/[`insert_part`] are the only functions in this crate that
//! write drawable [`Entity`] objects into the document. Nothing downstream —
//! the connection graph, the take-off — ever reads them back; they exist
//! purely so `od-io-svg` and `od-io-dxf`, neither of which has heard of MEP,
//! can show and export this drawing unmodified.
//!
//! There is deliberately no regeneration pass here yet. Derived entities are
//! created once, alongside their source, by the same transaction — there is
//! no editor yet to make a source dirty after the fact, so a dirty-tracking
//! system would have nothing to react to. See the crate documentation.

use crate::model::{MepError, PlacedPart, Result, RouteSegment, WorldPort};
use od_core::{Color, Entity, Geometry, ObjectId, Polyline2, Transaction, Vertex};
use od_parts::Catalog;

/// Which projection of a route to draw. `docs/04-mep.md` §3 lists five;
/// these two are the ones that need nothing beyond the centreline and profile
/// already on [`RouteSegment`] — a 3D solid needs the kernel behind
/// [`od_geom3d::SolidKernel`], and a cut section needs a cutting plane neither
/// of which exist yet to derive against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// One line down the centreline, the system's colour carrying the
    /// identity — a design/系統 drawing.
    SingleLine,
    /// The outline at the profile's full width, offset either side of the
    /// centreline — a construction drawing.
    DoubleLine,
}

/// The pure derivation: centreline + profile → geometry. No document, no
/// side effects — this is what makes the "derive" in "derived geometry" a
/// checkable claim rather than a comment.
#[must_use]
pub fn route_geometry(route: &RouteSegment, view: ViewMode) -> Vec<Geometry> {
    match view {
        ViewMode::SingleLine => vec![Geometry::Line {
            a: route.start,
            b: route.end,
        }],
        ViewMode::DoubleLine => {
            let half = route.profile.extent() * 0.5;
            let along = route.end - route.start;
            // The offset is computed in plan (ignoring any slope) because the
            // double-line view is a plan-view outline, not a true 3D surface;
            // a sloped drain still reads as two parallel lines in plan, which
            // is what a real construction drawing shows too.
            let Some(perp) = od_geom3d::Vec3::new(-along.y, along.x, 0.0).normalized() else {
                // No plan-view direction (a vertical stub, or a degenerate
                // segment): there is nothing to offset either side of, so the
                // centreline is the most honest fallback.
                return vec![Geometry::Line {
                    a: route.start,
                    b: route.end,
                }];
            };
            let offset = perp * half;
            vec![
                Geometry::Line {
                    a: route.start + offset,
                    b: route.end + offset,
                },
                Geometry::Line {
                    a: route.start - offset,
                    b: route.end - offset,
                },
            ]
        }
    }
}

/// Draws a route into the document: resolves its system to a layer and
/// colour, derives its geometry, and inserts it as ordinary entities that
/// `od-io-svg` and `od-io-dxf` already know how to show.
///
/// # Errors
/// [`MepError::NoSuchSystem`] if `route.system` does not name a system in
/// `catalog` — there is no reasonable layer or colour to fall back to that
/// would not misrepresent the drawing.
pub fn insert_route(
    tx: &mut Transaction<'_>,
    catalog: &Catalog,
    route: &RouteSegment,
    view: ViewMode,
) -> Result<Vec<ObjectId>> {
    let system_def = catalog
        .system(&route.system)
        .ok_or_else(|| MepError::NoSuchSystem(route.system.clone()))?;
    let layer = tx.ensure_layer(&system_def.layer);
    let color = Color::Index(system_def.color);
    let space = tx.db().model_space();

    route_geometry(route, view)
        .into_iter()
        .map(|geom| {
            let mut entity = Entity::new(layer, space, geom);
            entity.style.color = color;
            Ok(tx.add_entity(entity)?)
        })
        .collect()
}

/// Draws a placed part: builds its instance, ensures a block definition
/// carrying its symbol exists (created once per distinct part id and
/// parameter set — two elbows at different sizes are two different blocks,
/// same as two different DXF blocks would be), and inserts a reference to it
/// at the part's placement.
///
/// The block's own contents are drawn on layer `0`, so they take on the
/// colour of whichever layer the reference itself is placed on — the same
/// convention AutoCAD block definitions use, and the reason `od-core` keeps
/// layer `0` mandatory (`docs/03-data-model.md`).
///
/// # Errors
/// Whatever [`Catalog::instantiate`] returns for an unknown part id or a
/// parameter expression that fails to evaluate.
pub fn insert_part(
    tx: &mut Transaction<'_>,
    catalog: &Catalog,
    part: &PlacedPart,
) -> Result<ObjectId> {
    let instance = catalog.instantiate(&part.part_id, &part.params)?;

    let block_name = block_name_for(part);
    let block = tx.ensure_block(&block_name);
    let already_populated = tx
        .db()
        .tables
        .blocks
        .get(block)
        .is_some_and(|b| !b.entities.is_empty());
    if !already_populated {
        let layer_zero = tx.ensure_layer("0");
        for geom in &instance.symbol {
            tx.add_entity(Entity::new(layer_zero, block, geom.clone()))?;
        }
    }

    let (layer, color) = match &part.system {
        Some(system_id) => match catalog.system(system_id) {
            Some(def) => (tx.ensure_layer(&def.layer), Color::Index(def.color)),
            None => return Err(MepError::NoSuchSystem(system_id.clone())),
        },
        // No system assigned: draw on layer 0, ByLayer colour — visible, not
        // miscoloured, and easy to spot as needing a system.
        None => (tx.ensure_layer("0"), Color::ByLayer),
    };

    let mut entity = Entity::new(
        layer,
        tx.db().model_space(),
        Geometry::BlockRef(Box::new(od_core::BlockRef {
            block,
            position: part.position,
            scale: od_core::Vec3::new(1.0, 1.0, 1.0),
            rotation: part.rotation,
            attributes: Vec::new(),
            array: (1, 1),
            array_spacing: (0.0, 0.0),
        })),
    );
    entity.style.color = color;
    Ok(tx.add_entity(entity)?)
}

/// A deterministic block name for one (part id, size) combination, so two
/// placements at the same size share one definition and two at different
/// sizes do not silently share geometry that does not match either of them.
fn block_name_for(part: &PlacedPart) -> String {
    let mut keys: Vec<&String> = part.params.keys().collect();
    keys.sort();
    let mut name = format!("MEP~{}", part.part_id);
    for k in keys {
        // Six significant figures is well past drawing precision and keeps
        // near-equal float results from spuriously minting a second block.
        name.push_str(&format!("~{k}={:.6}", part.params[k]));
    }
    if part.mirror_y {
        name.push_str("~mirrored");
    }
    name
}

/// Draws a small marker at a world port — a filled dot with a short tick along
/// its outward direction, coloured by system. Not part of the derivation
/// pipeline proper (a port is not a source object); this exists for the
/// `check` report, where seeing *where* an unconnected port is matters as much
/// as the coordinates in a table row.
#[must_use]
pub fn port_marker(port: &WorldPort, size: f64) -> Geometry {
    let tip = port.position + port.direction * size;
    Geometry::Polyline {
        polyline: Polyline2::new(
            vec![
                Vertex::straight(port.position.to_2d()),
                Vertex::straight(tip.to_2d()),
            ],
            false,
        ),
        elevation: port.position.z,
        normal: od_core::Vec3::Z,
        width: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PlacementKind;
    use od_core::{ActorId, Database, Document, Point3};
    use od_parts::Profile;

    fn catalog() -> Catalog {
        Catalog::bundled().expect("bundled catalogue is valid")
    }

    fn doc() -> Document {
        Document::new(Database::new(ActorId::SYSTEM))
    }

    fn route() -> RouteSegment {
        RouteSegment {
            system: "sys.air.supply".into(),
            spec: "spec.duct.galvanised.rect".into(),
            profile: Profile::Rect { w: 400.0, h: 300.0 },
            start: Point3::ORIGIN,
            end: Point3::new(5000.0, 0.0, 0.0),
        }
    }

    #[test]
    fn single_line_is_exactly_the_centreline() {
        let r = route();
        let g = route_geometry(&r, ViewMode::SingleLine);
        assert_eq!(
            g,
            vec![Geometry::Line {
                a: r.start,
                b: r.end
            }]
        );
    }

    #[test]
    fn double_line_offsets_are_symmetric_about_the_centreline() {
        let r = route();
        let g = route_geometry(&r, ViewMode::DoubleLine);
        let Geometry::Line { a: a1, b: b1 } = &g[0] else {
            panic!("expected a line");
        };
        let Geometry::Line { a: a2, b: b2 } = &g[1] else {
            panic!("expected a line");
        };
        let mid1 = a1.midpoint(*b1);
        let mid2 = a2.midpoint(*b2);
        let center = r.start.midpoint(r.end);
        assert!(mid1.midpoint(mid2).coincides_with(center));

        // The outline spans exactly the profile's width (400 mm).
        let width = a1.distance_to(*a2);
        assert!(od_core::tol::eq_len(width, 400.0));
    }

    #[test]
    fn a_route_is_inserted_on_its_systems_layer_and_colour() {
        let mut d = doc();
        let r = route();
        let ids = d
            .edit("Draw", |tx| {
                insert_route(tx, &catalog(), &r, ViewMode::SingleLine)
            })
            .expect("commits");
        assert_eq!(ids.len(), 1);

        let (_, e) = d.db.entities().next().expect("one entity");
        let layer = d.db.tables.layers.get(e.layer).expect("layer exists");
        assert_eq!(layer.name, "M-DUCT-SA");
        assert_eq!(e.style.color, Color::Index(5), "sys.air.supply is blue");
        assert!(d.db.validate().is_empty());
    }

    #[test]
    fn a_route_on_an_unknown_system_is_refused() {
        let mut d = doc();
        let mut r = route();
        r.system = "sys.not.real".into();
        let result = d.edit("Draw", |tx| {
            insert_route(tx, &catalog(), &r, ViewMode::SingleLine)
        });
        assert!(matches!(result, Err(MepError::NoSuchSystem(_))));
    }

    #[test]
    fn a_part_is_inserted_as_a_block_reference_with_populated_geometry() {
        let mut d = doc();
        let part = PlacedPart {
            part_id: "duct.elbow.rect.90".into(),
            position: Point3::new(10_000.0, 0.0, 2800.0),
            rotation: 0.0,
            mirror_y: false,
            params: [("W".to_owned(), 500.0), ("H".to_owned(), 300.0)]
                .into_iter()
                .collect(),
            kind: PlacementKind::Fitting {
                auto_generated: false,
            },
            system: Some("sys.air.supply".into()),
        };
        let id = d
            .edit("Place", |tx| insert_part(tx, &catalog(), &part))
            .expect("commits");

        let e = d.db.entity(id).expect("is an entity");
        let Geometry::BlockRef(bref) = &e.geom else {
            panic!("expected a block reference");
        };
        assert!(bref.position.coincides_with(part.position));

        let block = d.db.tables.blocks.get(bref.block).expect("block exists");
        assert!(
            !block.entities.is_empty(),
            "the block must carry the symbol"
        );
        assert!(d.db.validate().is_empty());
    }

    #[test]
    fn placing_the_same_size_twice_reuses_one_block() {
        let mut d = doc();
        let part = PlacedPart {
            part_id: "duct.elbow.rect.90".into(),
            position: Point3::ORIGIN,
            rotation: 0.0,
            mirror_y: false,
            params: [("W".to_owned(), 400.0), ("H".to_owned(), 300.0)]
                .into_iter()
                .collect(),
            kind: PlacementKind::Fitting {
                auto_generated: true,
            },
            system: None,
        };
        let mut other = part.clone();
        other.position = Point3::new(5000.0, 0.0, 0.0);

        let (id1, id2): (ObjectId, ObjectId) = d
            .edit("Place two", |tx| {
                let a = insert_part(tx, &catalog(), &part)?;
                let b = insert_part(tx, &catalog(), &other)?;
                Ok::<_, MepError>((a, b))
            })
            .expect("commits");

        let block_of = |id: ObjectId| match &d.db.entity(id).expect("entity").geom {
            Geometry::BlockRef(b) => b.block,
            _ => panic!("expected a block reference"),
        };
        assert_eq!(block_of(id1), block_of(id2));
        assert_eq!(
            d.db.tables.blocks.len(),
            3,
            "model space, paper space, and one shared block"
        );
    }

    #[test]
    fn placing_a_different_size_creates_a_second_block() {
        let mut d = doc();
        let small = PlacedPart {
            part_id: "duct.elbow.rect.90".into(),
            position: Point3::ORIGIN,
            rotation: 0.0,
            mirror_y: false,
            params: [("W".to_owned(), 300.0), ("H".to_owned(), 200.0)]
                .into_iter()
                .collect(),
            kind: PlacementKind::Fitting {
                auto_generated: true,
            },
            system: None,
        };
        let mut large = small.clone();
        large.params.insert("W".into(), 800.0);
        large.position = Point3::new(5000.0, 0.0, 0.0);

        let (id1, id2): (ObjectId, ObjectId) = d
            .edit("Place two sizes", |tx| {
                let a = insert_part(tx, &catalog(), &small)?;
                let b = insert_part(tx, &catalog(), &large)?;
                Ok::<_, MepError>((a, b))
            })
            .expect("commits");

        let block_of = |id: ObjectId| match &d.db.entity(id).expect("entity").geom {
            Geometry::BlockRef(b) => b.block,
            _ => panic!("expected a block reference"),
        };
        assert_ne!(
            block_of(id1),
            block_of(id2),
            "different sizes must not share a block"
        );
    }
}
