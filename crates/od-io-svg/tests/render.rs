#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "an integration test asserts by panicking"
)]
//! Rendering tests.
//!
//! SVG has no schema we can check against, and "it looked right when I opened
//! it" does not survive a refactor — so these assert on the structure the
//! output must have: one element per entity, coordinates where the drawing puts
//! them, and the resolutions (ByLayer, ByBlock, colour 7 against the
//! background) that are invisible until they are wrong.

use od_core::{
    ActorId, BlockRef, Color, Database, Entity, Geometry, GraphicStyle, PAPER_SPACE, Point3, Vec3,
    ViewportEntity,
};
use od_geom3d::Aabb3;
use od_io_svg::{Background, SvgOptions, to_svg, to_svg_with_view_box};

fn drawing_with(geoms: Vec<(&str, Geometry)>) -> Database {
    let mut db = Database::new(ActorId::SYSTEM);
    let space = db.model_space();
    for (layer_name, geom) in geoms {
        let layer = db.ensure_layer(layer_name);
        db.insert_entity(Entity::new(layer, space, geom))
            .expect("inserts");
    }
    db
}

fn line(x1: f64, y1: f64, x2: f64, y2: f64) -> Geometry {
    Geometry::Line {
        a: Point3::new(x1, y1, 0.0),
        b: Point3::new(x2, y2, 0.0),
    }
}

/// Counts occurrences of an SVG element.
fn count(svg: &str, element: &str) -> usize {
    svg.matches(&format!("<{element}")).count()
}

#[test]
fn an_empty_drawing_still_produces_a_valid_document() {
    let svg = to_svg(&Database::default(), &SvgOptions::default());
    assert!(svg.starts_with("<svg"));
    assert!(svg.trim_end().ends_with("</svg>"));
    assert!(svg.contains("viewBox="));
}

#[test]
fn every_entity_becomes_an_element() {
    let db = drawing_with(vec![
        ("M-DUCT-SA", line(0.0, 0.0, 1000.0, 0.0)),
        ("M-DUCT-SA", line(1000.0, 0.0, 1000.0, 1000.0)),
        (
            "M-DUCT-SA",
            Geometry::Circle {
                center: Point3::new(500.0, 500.0, 0.0),
                radius: 250.0,
                normal: Vec3::Z,
            },
        ),
    ]);
    let svg = to_svg(&db, &SvgOptions::default());
    assert_eq!(count(&svg, "line"), 2);
    assert_eq!(count(&svg, "circle"), 1);
}

#[test]
fn the_view_box_covers_the_drawing() {
    let db = drawing_with(vec![("M-TEST", line(1000.0, 2000.0, 5000.0, 4000.0))]);
    let svg = to_svg(&db, &SvgOptions::default());

    let view = svg
        .split("viewBox=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("a viewBox");
    let parts: Vec<f64> = view
        .split_whitespace()
        .map(|v| v.parse().expect("numeric"))
        .collect();
    assert_eq!(parts.len(), 4);

    // The drawing spans 4000 × 2000; the box must cover it, plus padding.
    assert!(parts[2] >= 4000.0, "width {} is too small", parts[2]);
    assert!(parts[3] >= 2000.0, "height {} is too small", parts[3]);
    assert!(parts[0] <= 1000.0, "left edge misses the drawing");
}

#[test]
fn to_svg_with_view_box_matches_the_string_it_wrote() {
    let db = drawing_with(vec![("M-TEST", line(1000.0, 2000.0, 5000.0, 4000.0))]);
    let (svg, view_box) = to_svg_with_view_box(&db, &SvgOptions::default());

    let view = svg
        .split("viewBox=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("a viewBox");
    let parts: Vec<f64> = view
        .split_whitespace()
        .map(|v| v.parse().expect("numeric"))
        .collect();

    assert_eq!(
        parts,
        [
            view_box.min_x,
            view_box.min_y,
            view_box.width,
            view_box.height
        ]
    );
}

#[test]
fn a_window_draws_only_what_it_contains() {
    let mut geoms = Vec::new();
    for i in 0..50 {
        geoms.push((
            "M-TEST",
            line(f64::from(i) * 1000.0, 0.0, f64::from(i) * 1000.0, 500.0),
        ));
    }
    let db = drawing_with(geoms);

    let all = to_svg(&db, &SvgOptions::default());
    assert_eq!(count(&all, "line"), 50);

    let windowed = to_svg(
        &db,
        &SvgOptions::default().with_window(Aabb3::new(
            Point3::new(-100.0, -100.0, -100.0),
            Point3::new(4100.0, 600.0, 100.0),
        )),
    );
    assert_eq!(count(&windowed, "line"), 5, "lines at x = 0..4000");
}

#[test]
fn a_frozen_layer_is_not_drawn() {
    let mut db = drawing_with(vec![
        ("M-VISIBLE", line(0.0, 0.0, 1000.0, 0.0)),
        ("M-HIDDEN", line(0.0, 100.0, 1000.0, 100.0)),
    ]);
    let hidden = db.tables.layers.id_of("M-HIDDEN").expect("layer");
    if let Some(layer) = db.tables.layers.get_mut(hidden) {
        layer.frozen = true;
    }

    let svg = to_svg(&db, &SvgOptions::default());
    assert_eq!(count(&svg, "line"), 1, "a frozen layer does not plot");
}

#[test]
fn layers_can_be_filtered_by_name() {
    let db = drawing_with(vec![
        ("M-DUCT-SA", line(0.0, 0.0, 1000.0, 0.0)),
        ("P-PIPE-CW", line(0.0, 100.0, 1000.0, 100.0)),
        ("E-POWR-LTG", line(0.0, 200.0, 1000.0, 200.0)),
    ]);

    let svg = to_svg(
        &db,
        &SvgOptions::default().with_layers(vec!["m-duct-sa".into(), "P-PIPE-CW".into()]),
    );
    assert_eq!(count(&svg, "line"), 2, "matched case-insensitively");
}

#[test]
fn layer_colour_reaches_the_stroke() {
    let mut db = drawing_with(vec![("M-DUCT-SA", line(0.0, 0.0, 1000.0, 0.0))]);
    let id = db.tables.layers.id_of("M-DUCT-SA").expect("layer");
    if let Some(layer) = db.tables.layers.get_mut(id) {
        layer.color = Color::Index(1); // red
    }

    let svg = to_svg(&db, &SvgOptions::default());
    assert!(
        svg.contains(r##"stroke="#ff0000""##),
        "ByLayer must resolve"
    );
}

#[test]
fn colour_seven_follows_the_background() {
    let db = drawing_with(vec![("M-TEST", line(0.0, 0.0, 1000.0, 0.0))]);

    let on_paper = to_svg(
        &db,
        &SvgOptions::default().with_background(Background::Paper),
    );
    assert!(on_paper.contains(r##"stroke="#000000""##), "black on white");

    let on_dark = to_svg(
        &db,
        &SvgOptions::default().with_background(Background::Dark),
    );
    assert!(
        !on_dark.contains(r##"stroke="#000000""##),
        "black on a dark ground is invisible, which is the bug this prevents"
    );
    assert!(
        on_dark.contains(r##"fill="#141d26""##),
        "the ground is painted"
    );
}

#[test]
fn a_transparent_background_paints_no_ground() {
    let db = drawing_with(vec![("M-TEST", line(0.0, 0.0, 1000.0, 0.0))]);
    let svg = to_svg(
        &db,
        &SvgOptions::default().with_background(Background::None),
    );
    assert_eq!(count(&svg, "rect"), 0);
}

#[test]
fn a_block_reference_is_expanded_where_it_is_placed() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("M-EQUIP");
    let block = db.ensure_block("VALVE");
    db.insert_entity(Entity::new(layer, block, line(-50.0, 0.0, 50.0, 0.0)))
        .expect("inserts");
    db.insert_entity(Entity::new(layer, block, line(0.0, -50.0, 0.0, 50.0)))
        .expect("inserts");

    let space = db.model_space();
    db.insert_entity(Entity::new(
        layer,
        space,
        Geometry::BlockRef(Box::new(BlockRef {
            block,
            position: Point3::new(10_000.0, 5_000.0, 0.0),
            scale: Vec3::new(2.0, 2.0, 1.0),
            rotation: std::f64::consts::FRAC_PI_2,
            attributes: Vec::new(),
            array: (1, 1),
            array_spacing: (0.0, 0.0),
        })),
    ))
    .expect("inserts");

    let svg = to_svg(&db, &SvgOptions::default());
    assert_eq!(count(&svg, "line"), 2, "the block's contents are drawn");
    assert!(
        svg.contains("translate(10000 5000)"),
        "placed, not at the origin"
    );
    assert!(svg.contains("rotate(90)"));
    assert!(svg.contains("scale(2 2)"));
}

#[test]
fn byblock_inside_a_block_takes_the_references_colour() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("0");
    let block = db.ensure_block("MARK");

    let mut member = Entity::new(layer, block, line(0.0, 0.0, 100.0, 0.0));
    member.style = GraphicStyle {
        color: Color::ByBlock,
        ..Default::default()
    };
    db.insert_entity(member).expect("inserts");

    let space = db.model_space();
    let mut reference = Entity::new(
        layer,
        space,
        Geometry::BlockRef(Box::new(BlockRef {
            block,
            position: Point3::ORIGIN,
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attributes: Vec::new(),
            array: (1, 1),
            array_spacing: (0.0, 0.0),
        })),
    );
    reference.style = GraphicStyle {
        color: Color::Index(3), // green
        ..Default::default()
    };
    db.insert_entity(reference).expect("inserts");

    let svg = to_svg(&db, &SvgOptions::default());
    assert!(
        svg.contains(r##"stroke="#00ff00""##),
        "ByBlock must pick up the reference's colour"
    );
}

#[test]
fn japanese_text_survives_and_is_not_upside_down() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("M-TEXT");
    let space = db.model_space();
    let style = db.tables.text_styles.id_of("Standard").expect("standard");

    db.insert_entity(Entity::new(
        layer,
        space,
        Geometry::Text(Box::new(od_core::TextEntity {
            position: Point3::new(100.0, 200.0, 0.0),
            value: "給気ダクト 400×300".into(),
            height: 250.0,
            rotation: 0.0,
            style,
            flow: od_core::TextFlow::Horizontal,
            h_align: od_core::HAlign::Left,
            v_align: od_core::VAlign::Baseline,
            width_factor: 1.0,
            oblique: 0.0,
        })),
    ))
    .expect("inserts");

    let svg = to_svg(&db, &SvgOptions::default());
    assert!(svg.contains("給気ダクト 400×300"));
    assert!(
        svg.contains("scale(1 -1)") && count(&svg, "text") == 1,
        "text must be flipped back inside the flipped group"
    );
}

#[test]
fn markup_characters_in_text_are_escaped() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("M-TEXT");
    let space = db.model_space();
    let style = db.tables.text_styles.id_of("Standard").expect("standard");

    db.insert_entity(Entity::new(
        layer,
        space,
        Geometry::Text(Box::new(od_core::TextEntity {
            position: Point3::ORIGIN,
            value: "<A & B>".into(),
            height: 100.0,
            rotation: 0.0,
            style,
            flow: od_core::TextFlow::Horizontal,
            h_align: od_core::HAlign::Left,
            v_align: od_core::VAlign::Baseline,
            width_factor: 1.0,
            oblique: 0.0,
        })),
    ))
    .expect("inserts");

    let svg = to_svg(&db, &SvgOptions::default());
    assert!(svg.contains("&lt;A &amp; B&gt;"));
    assert!(
        !svg.contains("<A & B>"),
        "raw markup would break the document"
    );
}

#[test]
fn a_preserved_entity_is_drawn_through_its_proxy() {
    // An entity this build cannot model still has to appear on screen —
    // otherwise "preserved" means "invisible", and a reviewer misses it.
    let dxf = concat!(
        "0\nSECTION\n2\nENTITIES\n",
        "0\nACAD_TABLE\n8\nA-ANNO\n10\n1000.0\n20\n2000.0\n11\n3000.0\n21\n2500.0\n",
        "0\nENDSEC\n0\nEOF\n",
    );
    let (db, _) = od_io_dxf::read_str(dxf).expect("reads");

    let svg = to_svg(&db, &SvgOptions::default());
    assert_eq!(count(&svg, "path"), 1, "the proxy outline is drawn");
}

#[test]
fn a_real_drawing_renders_end_to_end() {
    let dxf = concat!(
        "0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n",
        "0\nLAYER\n2\nM-DUCT-SA\n62\n5\n70\n0\n",
        "0\nENDTAB\n0\nENDSEC\n",
        "0\nSECTION\n2\nENTITIES\n",
        "0\nLINE\n8\nM-DUCT-SA\n10\n0.0\n20\n0.0\n11\n8000.0\n21\n0.0\n",
        "0\nLWPOLYLINE\n8\nM-DUCT-SA\n90\n3\n70\n0\n10\n0.0\n20\n0.0\n42\n1.0\n10\n1000.0\n20\n0.0\n10\n1000.0\n20\n800.0\n",
        "0\nARC\n8\nM-DUCT-SA\n10\n4000.0\n20\n1000.0\n40\n500.0\n50\n0.0\n51\n90.0\n",
        "0\nCIRCLE\n8\nM-DUCT-SA\n10\n6000.0\n20\n1000.0\n40\n300.0\n",
        "0\nENDSEC\n0\nEOF\n",
    );
    let (db, outcome) = od_io_dxf::read_str(dxf).expect("reads");
    assert!(outcome.warnings.is_empty());

    let svg = to_svg(&db, &SvgOptions::default());
    assert_eq!(count(&svg, "line"), 1);
    assert_eq!(count(&svg, "circle"), 1);
    assert_eq!(count(&svg, "path"), 2, "the polyline and the arc");
    assert!(
        svg.contains(r##"stroke="#0000ff""##),
        "layer colour 5 is blue"
    );

    // Nothing malformed: every attribute value is closed.
    assert_eq!(svg.matches('"').count() % 2, 0);
}

#[test]
fn the_entity_cap_stops_a_runaway_document() {
    let mut geoms = Vec::new();
    for i in 0..100 {
        geoms.push(("M-TEST", line(f64::from(i), 0.0, f64::from(i), 10.0)));
    }
    let db = drawing_with(geoms);

    let options = SvgOptions {
        max_entities: 10,
        ..Default::default()
    };
    let svg = to_svg(&db, &options);
    assert_eq!(count(&svg, "line"), 10);
}

#[test]
fn a_viewport_draws_its_paper_space_boundary_and_clips_its_content() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("0");
    let model_space = db.model_space();
    db.insert_entity(Entity::new(layer, model_space, line(0.0, 0.0, 1000.0, 0.0)))
        .expect("inserts");

    let paper_space = db
        .tables
        .blocks
        .id_of(PAPER_SPACE)
        .expect("Database::new always seeds a default paper space");
    db.insert_entity(Entity::new(
        layer,
        paper_space,
        Geometry::Viewport(Box::new(ViewportEntity {
            position: Point3::new(100.0, 100.0, 0.0),
            width: 200.0,
            height: 150.0,
            target: Point3::new(500.0, 0.0, 0.0),
            scale: 0.1,
        })),
    ))
    .expect("inserts");

    let svg = to_svg(
        &db,
        &SvgOptions {
            space: Some(paper_space),
            ..Default::default()
        },
    );

    assert!(svg.contains("<clipPath"), "the viewport clips its content");
    assert!(
        svg.contains(r#"<rect x="0" y="25" width="200" height="150""#),
        "the paper-space boundary rectangle, at the viewport's own size: {svg}"
    );
    assert_eq!(
        count(&svg, "line"),
        1,
        "the model-space line inside the window is drawn nested"
    );
}

#[test]
fn a_viewport_only_shows_model_space_entities_within_its_window() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("0");
    let model_space = db.model_space();
    // Inside the viewport's model-space window (target ± half-size/scale).
    db.insert_entity(Entity::new(layer, model_space, line(0.0, 0.0, 1000.0, 0.0)))
        .expect("inserts");
    // Far outside it.
    db.insert_entity(Entity::new(
        layer,
        model_space,
        line(1_000_000.0, 0.0, 1_000_001.0, 0.0),
    ))
    .expect("inserts");

    let paper_space = db
        .tables
        .blocks
        .id_of(PAPER_SPACE)
        .expect("Database::new always seeds a default paper space");
    db.insert_entity(Entity::new(
        layer,
        paper_space,
        Geometry::Viewport(Box::new(ViewportEntity {
            position: Point3::new(100.0, 100.0, 0.0),
            width: 200.0,
            height: 150.0,
            target: Point3::new(500.0, 0.0, 0.0),
            scale: 0.1,
        })),
    ))
    .expect("inserts");

    let svg = to_svg(
        &db,
        &SvgOptions {
            space: Some(paper_space),
            ..Default::default()
        },
    );

    assert_eq!(
        count(&svg, "line"),
        1,
        "only the in-window line is drawn: {svg}"
    );
}
