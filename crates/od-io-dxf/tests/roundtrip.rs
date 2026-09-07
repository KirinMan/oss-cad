#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "an integration test asserts by panicking"
)]
//! Round-trip tests (F-206).
//!
//! The project's stated advantage over the incumbents is that attributes and
//! geometry survive a conversion. That promise is only worth something if it is
//! checked automatically, so these tests are a gate rather than a nicety: a
//! drawing read, written and read again must be indistinguishable from the
//! first read.

use od_core::{ActorId, Database, Entity, Geometry, ObjectId, Point3, Polyline2, Vec3, Vertex};
use od_io_dxf::{read_str, write_string};

/// Reads, writes, reads again.
fn cycle(dxf: &str) -> (Database, Database) {
    let (first, outcome) = read_str(dxf).expect("first read");
    assert!(
        outcome.warnings.is_empty(),
        "unexpected warnings: {:?}",
        outcome.warnings
    );
    let written = write_string(&first);
    let (second, outcome) = read_str(&written).expect("second read");
    assert!(
        outcome.warnings.is_empty(),
        "warnings on re-read: {:?}",
        outcome.warnings
    );
    (first, second)
}

/// Compares geometry structurally, since ids differ between reads.
fn geometries(db: &Database) -> Vec<String> {
    let mut v: Vec<String> = db
        .entities()
        .map(|(id, e)| {
            let b = db.entity_bounds(id);
            let layer = db
                .tables
                .layers
                .get(e.layer)
                .map_or("?".to_owned(), |l| l.name.clone());
            format!(
                "{} on {} [{:.6},{:.6}..{:.6},{:.6}]",
                e.geom.type_name(),
                layer,
                b.min.x,
                b.min.y,
                b.max.x,
                b.max.y
            )
        })
        .collect();
    v.sort();
    v
}

fn dxf_with_entities(body: &str) -> String {
    format!("0\nSECTION\n2\nENTITIES\n{body}0\nENDSEC\n0\nEOF\n")
}

#[test]
fn primitives_survive_a_round_trip() {
    let dxf = dxf_with_entities(concat!(
        "0\nLINE\n8\nA-Wall\n10\n0.0\n20\n0.0\n30\n0.0\n11\n3600.0\n21\n0.0\n31\n0.0\n",
        "0\nCIRCLE\n8\nA-Duct\n10\n1000.0\n20\n500.0\n40\n250.0\n",
        "0\nARC\n8\nA-Duct\n10\n0.0\n20\n0.0\n40\n100.0\n50\n315.0\n51\n45.0\n",
        "0\nPOINT\n8\n0\n10\n7.5\n20\n2.5\n",
    ));
    let (a, b) = cycle(&dxf);
    assert_eq!(a.entities().count(), 4);
    assert_eq!(geometries(&a), geometries(&b));
}

#[test]
fn layers_and_their_state_survive() {
    let dxf = format!(
        "0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n{}0\nENDTAB\n0\nENDSEC\n{}",
        concat!(
            "0\nLAYER\n2\nA-Duct-Supply\n62\n3\n70\n0\n",
            "0\nLAYER\n2\nA-Hidden\n62\n-5\n70\n4\n",
        ),
        dxf_with_entities("0\nLINE\n8\nA-Duct-Supply\n10\n0.0\n20\n0.0\n11\n1.0\n21\n1.0\n")
    );
    let (a, b) = cycle(&dxf);
    for db in [&a, &b] {
        let supply = db
            .tables
            .layers
            .by_name("a-duct-supply")
            .expect("supply layer");
        assert_eq!(supply.name, "A-Duct-Supply");
        assert_eq!(supply.color, od_core::Color::Index(3));
        assert!(supply.visible);

        let hidden = db.tables.layers.by_name("a-hidden").expect("hidden layer");
        assert!(!hidden.visible, "off state must survive");
        assert!(hidden.locked, "lock state must survive");
    }
}

#[test]
fn a_polyline_keeps_its_bulges_and_closure() {
    let dxf = dxf_with_entities(concat!(
        "0\nLWPOLYLINE\n8\n0\n90\n3\n70\n1\n",
        "10\n0.0\n20\n0.0\n42\n0.5\n",
        "10\n1000.0\n20\n0.0\n",
        "10\n1000.0\n20\n800.0\n42\n-0.25\n",
    ));
    let (a, b) = cycle(&dxf);
    let pl = |db: &Database| match &db.entities().next().expect("one entity").1.geom {
        Geometry::Polyline { polyline, .. } => polyline.clone(),
        other => panic!("expected a polyline, got {}", other.type_name()),
    };
    let (first, second) = (pl(&a), pl(&b));
    assert_eq!(first.vertices.len(), second.vertices.len());
    assert_eq!(first.closed, second.closed);
    for (x, y) in first.vertices.iter().zip(second.vertices.iter()) {
        assert!(x.point.coincides_with(y.point));
        assert!(
            od_core::tol::eq_len(x.bulge, y.bulge),
            "bulge {} became {}",
            x.bulge,
            y.bulge
        );
    }
    assert!(od_core::tol::eq_len(first.length(), second.length()));
}

#[test]
fn an_unsupported_entity_survives_verbatim() {
    let dxf = dxf_with_entities(concat!(
        "0\nACAD_TABLE\n8\n0\n10\n5.0\n20\n7.0\n30\n0.0\n1\nsome payload\n",
        "0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n11\n1.0\n21\n1.0\n",
    ));
    let (first, outcome) = read_str(&dxf).expect("reads");
    assert_eq!(outcome.unsupported_types, vec!["ACAD_TABLE"]);

    let written = write_string(&first);
    assert!(
        written.contains("some payload"),
        "the payload must be re-emitted"
    );

    let (second, _) = read_str(&written).expect("re-reads");
    let kept = second
        .entities()
        .filter(|(_, e)| matches!(&e.geom, Geometry::Unsupported { .. }))
        .count();
    assert_eq!(
        kept, 1,
        "the preserved entity is still there after two trips"
    );
    assert_eq!(second.entities().count(), first.entities().count());
}

#[test]
fn an_unknown_section_survives_verbatim() {
    let dxf = concat!(
        "0\nSECTION\n2\nCLASSES\n0\nCLASS\n1\nExAcXREFPanelObject\n2\nAcDbPlaceHolder\n0\nENDSEC\n",
        "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n11\n1.0\n21\n1.0\n0\nENDSEC\n",
        "0\nEOF\n",
    );
    let (a, b) = cycle(dxf);
    assert_eq!(a.preserved.len(), 1);
    assert_eq!(b.preserved.len(), 1, "still preserved after a second trip");
    assert_eq!(b.preserved[0].section, "CLASSES");
    assert!(
        String::from_utf8_lossy(&b.preserved[0].payload).contains("ExAcXREFPanelObject"),
        "the section's contents must be intact"
    );
}

#[test]
fn blocks_and_inserts_survive_with_their_placement() {
    let dxf = concat!(
        "0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n2\nVALVE\n10\n0.0\n20\n0.0\n",
        "0\nLINE\n8\n0\n10\n-50.0\n20\n-50.0\n11\n50.0\n21\n50.0\n",
        "0\nENDBLK\n0\nENDSEC\n",
        "0\nSECTION\n2\nENTITIES\n",
        "0\nINSERT\n8\nA-Pipe\n2\nVALVE\n10\n2000.0\n20\n3000.0\n41\n2.0\n42\n2.0\n50\n90.0\n",
        "0\nENDSEC\n0\nEOF\n",
    );
    let (a, b) = cycle(dxf);
    for db in [&a, &b] {
        let block = db.tables.blocks.id_of("VALVE").expect("block survives");
        assert_eq!(db.entities_in(block).count(), 1, "block contents survive");
        let (id, _) = db
            .entities_in(db.model_space())
            .next()
            .expect("the insert survives");
        let bounds = db.entity_bounds(id);
        // A 100×100 block scaled ×2 spans 200 either way about the insertion.
        assert!(od_core::tol::eq_len(bounds.min.x, 1900.0));
        assert!(od_core::tol::eq_len(bounds.max.x, 2100.0));
    }
    assert_eq!(geometries(&a), geometries(&b));
}

#[test]
fn japanese_text_survives_unmangled() {
    let dxf = dxf_with_entities(concat!(
        "0\nTEXT\n8\nA-Text\n10\n100.0\n20\n200.0\n40\n3.5\n1\n給水管 VP-50 通り芯より\n",
        "0\nMTEXT\n8\nA-Text\n10\n0.0\n20\n0.0\n40\n2.5\n1\n屋上階 排気ダクト 400×300\n",
    ));
    let (a, b) = cycle(&dxf);
    let texts = |db: &Database| -> Vec<String> {
        let mut v: Vec<String> = db
            .entities()
            .filter_map(|(_, e)| match &e.geom {
                Geometry::Text(t) => Some(t.value.clone()),
                Geometry::MText(t) => Some(t.value.clone()),
                _ => None,
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(texts(&a), texts(&b));
    assert!(texts(&b).iter().any(|t| t.contains("給水管")));
    assert!(texts(&b).iter().any(|t| t.contains("400×300")));
}

#[test]
fn coordinates_do_not_drift_across_repeated_trips() {
    // A site-scale coordinate with sub-millimetre detail: the case where a
    // sloppy float format loses precision one trip at a time.
    let x = 123_456.789_012_345_f64;
    let dxf = dxf_with_entities(&format!(
        "0\nLINE\n8\n0\n10\n{x}\n20\n{x}\n30\n0.0\n11\n{x}\n21\n0.0\n31\n0.0\n"
    ));
    let (mut db, _) = read_str(&dxf).expect("reads");
    for _ in 0..5 {
        let written = write_string(&db);
        db = read_str(&written).expect("re-reads").0;
    }
    let (_, e) = db.entities().next().expect("one entity");
    match &e.geom {
        Geometry::Line { a, .. } => {
            assert!(
                (a.x - x).abs() <= f64::EPSILON * x.abs(),
                "after five trips {x} became {}",
                a.x
            );
        }
        other => panic!("expected a line, got {}", other.type_name()),
    }
}

#[test]
fn a_document_built_in_memory_exports_and_reads_back() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("M-Duct-Supply");
    let space = db.model_space();

    let add = |db: &mut Database, geom: Geometry| -> ObjectId {
        db.insert_entity(Entity::new(layer, space, geom))
            .expect("inserts")
    };
    add(
        &mut db,
        Geometry::Line {
            a: Point3::new(0.0, 0.0, 2800.0),
            b: Point3::new(5000.0, 0.0, 2800.0),
        },
    );
    add(
        &mut db,
        Geometry::Polyline {
            polyline: Polyline2::new(
                vec![
                    Vertex::with_bulge(od_core::Point2::new(0.0, 0.0), 1.0),
                    Vertex::straight(od_core::Point2::new(600.0, 0.0)),
                ],
                false,
            ),
            elevation: 2800.0,
            normal: Vec3::Z,
            width: 0.0,
        },
    );

    let written = write_string(&db);
    let (back, outcome) = read_str(&written).expect("reads what we wrote");
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert_eq!(back.entities().count(), 2);
    assert!(back.tables.layers.by_name("m-duct-supply").is_some());
    assert!(back.validate().is_empty());
}

#[test]
fn paper_space_entities_are_dropped_silently_on_dxf_write() {
    // write_entities (write.rs) only iterates db.entities_in(db.model_space()),
    // and read_entities (read.rs) always files a top-level ENTITIES record
    // into model space regardless of group code 67 (the paperspace flag,
    // never read anywhere in this crate) -- so this is self-consistent
    // internally, but it means a layout's own content (title block, a
    // viewport border drawn on paper space) is not "reported as a loss"
    // anywhere the way Dimension/Viewport explicitly are: it is simply
    // absent from the written file, with no warning at all.
    let mut db = od_core::Database::new(od_core::ActorId::SYSTEM);
    let layer = db.ensure_layer("0");
    let paper_space = db
        .tables
        .blocks
        .id_of(od_core::PAPER_SPACE)
        .expect("Database::new always seeds a default paper space");
    db.insert_entity(od_core::Entity::new(
        layer,
        paper_space,
        od_core::Geometry::Line {
            a: od_core::Point3::ORIGIN,
            b: od_core::Point3::new(100.0, 0.0, 0.0),
        },
    ))
    .expect("inserts");

    let text = od_io_dxf::write_string(&db);
    assert!(
        !text.contains("LINE"),
        "a paper-space LINE is missing from the DXF output entirely, with \
         no warning, unlike Dimension/Viewport which are at least reported \
         as a loss by od-cli's conversion_losses"
    );
}
