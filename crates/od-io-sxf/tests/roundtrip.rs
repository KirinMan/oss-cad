#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "an integration test asserts by panicking"
)]
//! Round-trip self-consistency: write this crate's own output, read it back,
//! and check the geometry survives. This is the only fidelity check
//! available without a real sample `.sfc` file from commercial CAD software
//! (crate docs) — it proves the writer and reader agree with each other, not
//! that either agrees with the SCADEC/OCF specification.

use od_core::{ActorId, Database, Entity, Geometry, HAlign, Point3, TextEntity, VAlign, Vec3};

fn line(x1: f64, y1: f64, x2: f64, y2: f64) -> Geometry {
    Geometry::Line {
        a: Point3::new(x1, y1, 0.0),
        b: Point3::new(x2, y2, 0.0),
    }
}

fn geom_near(a: &Geometry, b: &Geometry) {
    match (a, b) {
        (Geometry::Line { a: a1, b: b1 }, Geometry::Line { a: a2, b: b2 }) => {
            assert!(a1.distance_to(*a2) < 1e-6);
            assert!(b1.distance_to(*b2) < 1e-6);
        }
        (
            Geometry::Circle {
                center: c1,
                radius: r1,
                ..
            },
            Geometry::Circle {
                center: c2,
                radius: r2,
                ..
            },
        ) => {
            assert!(c1.distance_to(*c2) < 1e-6);
            assert!((r1 - r2).abs() < 1e-6);
        }
        (
            Geometry::Arc {
                center: c1,
                radius: r1,
                start_angle: s1,
                sweep: w1,
                ..
            },
            Geometry::Arc {
                center: c2,
                radius: r2,
                start_angle: s2,
                sweep: w2,
                ..
            },
        ) => {
            assert!(c1.distance_to(*c2) < 1e-6);
            assert!((r1 - r2).abs() < 1e-6);
            assert!((s1 - s2).abs() < 1e-6, "start_angle {s1} vs {s2}");
            assert!((w1 - w2).abs() < 1e-6, "sweep {w1} vs {w2}");
        }
        (Geometry::Point(p1), Geometry::Point(p2)) => {
            assert!(p1.distance_to(*p2) < 1e-6);
        }
        (Geometry::Text(t1), Geometry::Text(t2)) => {
            assert_eq!(t1.value, t2.value);
            assert!(t1.position.distance_to(t2.position) < 1e-6);
            assert!((t1.height - t2.height).abs() < 1e-6);
        }
        (Geometry::Polyline { polyline: p1, .. }, Geometry::Polyline { polyline: p2, .. }) => {
            assert_eq!(p1.closed, p2.closed);
            assert_eq!(p1.vertices.len(), p2.vertices.len());
            for (v1, v2) in p1.vertices.iter().zip(&p2.vertices) {
                assert!((v1.point.x - v2.point.x).abs() < 1e-6);
                assert!((v1.point.y - v2.point.y).abs() < 1e-6);
            }
        }
        _ => panic!("kind mismatch: {a:?} vs {b:?}"),
    }
}

#[test]
fn a_line_round_trips() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("M-DUCT-SA");
    let space = db.model_space();
    db.insert_entity(Entity::new(layer, space, line(0.0, 0.0, 1000.0, 500.0)))
        .expect("inserts");

    let text = od_io_sxf::write_string(&db);
    let (again, outcome) = od_io_sxf::read_str(&text);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    let entities: Vec<_> = again.entities().collect();
    assert_eq!(entities.len(), 1);
    geom_near(&entities[0].1.geom, &line(0.0, 0.0, 1000.0, 500.0));
}

#[test]
fn a_circle_and_arc_round_trip_with_their_angles() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("0");
    let space = db.model_space();
    db.insert_entity(Entity::new(
        layer,
        space,
        Geometry::Circle {
            center: Point3::new(100.0, 200.0, 0.0),
            radius: 50.0,
            normal: Vec3::Z,
        },
    ))
    .expect("inserts");
    db.insert_entity(Entity::new(
        layer,
        space,
        Geometry::Arc {
            center: Point3::new(0.0, 0.0, 0.0),
            radius: 25.0,
            start_angle: std::f64::consts::FRAC_PI_4,
            sweep: std::f64::consts::FRAC_PI_2,
            normal: Vec3::Z,
        },
    ))
    .expect("inserts");

    let text = od_io_sxf::write_string(&db);
    let (again, outcome) = od_io_sxf::read_str(&text);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    let entities: Vec<_> = again.entities().collect();
    assert_eq!(entities.len(), 2);
    let circle = entities
        .iter()
        .find(|(_, e)| matches!(e.geom, Geometry::Circle { .. }))
        .expect("a circle");
    geom_near(
        &circle.1.geom,
        &Geometry::Circle {
            center: Point3::new(100.0, 200.0, 0.0),
            radius: 50.0,
            normal: Vec3::Z,
        },
    );
    let arc = entities
        .iter()
        .find(|(_, e)| matches!(e.geom, Geometry::Arc { .. }))
        .expect("an arc");
    geom_near(
        &arc.1.geom,
        &Geometry::Arc {
            center: Point3::new(0.0, 0.0, 0.0),
            radius: 25.0,
            start_angle: std::f64::consts::FRAC_PI_4,
            sweep: std::f64::consts::FRAC_PI_2,
            normal: Vec3::Z,
        },
    );
}

#[test]
fn a_clockwise_arc_round_trips_through_the_direction_flag() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("0");
    let space = db.model_space();
    // A CCW arc from 200° to 100° is a 260° sweep the long way round;
    // od_geom2d::Arc2::from_start_end_ccw always picks that direction, so
    // this specific pair exercises the direction-flag round trip.
    let original = Geometry::Arc {
        center: Point3::new(10.0, 10.0, 0.0),
        radius: 5.0,
        start_angle: 200.0_f64.to_radians(),
        sweep: 260.0_f64.to_radians(),
        normal: Vec3::Z,
    };
    db.insert_entity(Entity::new(layer, space, original.clone()))
        .expect("inserts");

    let text = od_io_sxf::write_string(&db);
    let (again, outcome) = od_io_sxf::read_str(&text);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    let entities: Vec<_> = again.entities().collect();
    geom_near(&entities[0].1.geom, &original);
}

#[test]
fn a_polyline_round_trips_open_and_closed() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("0");
    let space = db.model_space();
    let open = Geometry::Polyline {
        polyline: od_core::Polyline2::from_points(
            [
                od_core::Point2::new(0.0, 0.0),
                od_core::Point2::new(10.0, 0.0),
                od_core::Point2::new(10.0, 10.0),
            ],
            false,
        ),
        elevation: 0.0,
        normal: Vec3::Z,
        width: 0.0,
    };
    let closed = Geometry::Polyline {
        polyline: od_core::Polyline2::from_points(
            [
                od_core::Point2::new(0.0, 0.0),
                od_core::Point2::new(10.0, 0.0),
                od_core::Point2::new(5.0, 10.0),
            ],
            true,
        ),
        elevation: 0.0,
        normal: Vec3::Z,
        width: 0.0,
    };
    db.insert_entity(Entity::new(layer, space, open.clone()))
        .expect("inserts");
    db.insert_entity(Entity::new(layer, space, closed.clone()))
        .expect("inserts");

    let text = od_io_sxf::write_string(&db);
    let (again, outcome) = od_io_sxf::read_str(&text);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    let entities: Vec<_> = again.entities().collect();
    assert_eq!(entities.len(), 2);
    let got_open = entities
        .iter()
        .find(|(_, e)| matches!(&e.geom, Geometry::Polyline { polyline, .. } if !polyline.closed))
        .expect("the open polyline");
    geom_near(&got_open.1.geom, &open);
    let got_closed = entities
        .iter()
        .find(|(_, e)| matches!(&e.geom, Geometry::Polyline { polyline, .. } if polyline.closed))
        .expect("the closed polyline");
    geom_near(&got_closed.1.geom, &closed);
}

#[test]
fn text_round_trips_its_alignment_and_flow() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("0");
    let space = db.model_space();
    let style = db
        .tables
        .text_styles
        .id_of("standard")
        .expect("Database::new always seeds a standard text style");
    db.insert_entity(Entity::new(
        layer,
        space,
        Geometry::Text(Box::new(TextEntity {
            position: Point3::new(500.0, 500.0, 0.0),
            value: "給気ダクト".into(),
            height: 3.5,
            rotation: std::f64::consts::FRAC_PI_2,
            style,
            flow: od_core::TextFlow::Vertical,
            h_align: HAlign::Center,
            v_align: VAlign::Middle,
            width_factor: 1.0,
            oblique: 0.0,
        })),
    ))
    .expect("inserts");

    let text = od_io_sxf::write_string(&db);
    let (again, outcome) = od_io_sxf::read_str(&text);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    let entities: Vec<_> = again.entities().collect();
    assert_eq!(entities.len(), 1);
    let Geometry::Text(t) = &entities[0].1.geom else {
        panic!("expected Text");
    };
    assert_eq!(t.value, "給気ダクト");
    assert_eq!(t.flow, od_core::TextFlow::Vertical);
    assert_eq!(t.h_align, HAlign::Center);
    assert_eq!(t.v_align, VAlign::Middle);
    assert!((t.rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-6);
}

#[test]
fn layer_visibility_and_colour_round_trip() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("A-WALL");
    if let Some(l) = db.tables.layers.get_mut(layer) {
        l.visible = false;
    }
    let space = db.model_space();
    let mut e = Entity::new(layer, space, line(0.0, 0.0, 1.0, 1.0));
    e.style.color = od_core::Color::Rgb {
        r: 10,
        g: 20,
        b: 30,
    };
    db.insert_entity(e).expect("inserts");

    let text = od_io_sxf::write_string(&db);
    let (again, outcome) = od_io_sxf::read_str(&text);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    let got_layer = again
        .tables
        .layers
        .by_name("A-WALL")
        .expect("layer survives");
    assert!(!got_layer.visible);
    let (_, entity) = again.entities().next().expect("one entity");
    assert_eq!(
        entity.style.color,
        od_core::Color::Rgb {
            r: 10,
            g: 20,
            b: 30
        }
    );
}

#[test]
fn an_unrecognised_feature_is_preserved_not_dropped() {
    let text = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n\n/*SXF\n#10 = sfig_org_feature(\\'BLOCK1\\','3')\nSXF*/\nENDSEC;\nEND-ISO-10303-21;\n";
    let (db, outcome) = od_io_sxf::read_str(text);
    assert_eq!(
        outcome.unsupported_types,
        vec!["sfig_org_feature".to_string()]
    );
    let (_, entity) = db.entities().next().expect("preserved as an entity");
    let Geometry::Unsupported {
        source_type,
        payload,
        ..
    } = &entity.geom
    else {
        panic!("expected Unsupported");
    };
    assert_eq!(source_type, "sfig_org_feature");
    assert!(String::from_utf8_lossy(payload).contains("sfig_org_feature"));
}
