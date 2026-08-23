#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "an integration test asserts by panicking"
)]
//! The compatibility contract (`docs/03-data-model.md` §5.2).
//!
//! These tests simulate the situation the contract exists for: a file written
//! by a *newer* build, opened by this one. Getting it wrong is not a crash —
//! it is a file that opens, looks fine, and has quietly lost the parts this
//! build did not recognise. That is the failure the incumbents are criticised
//! for, so it is tested against a hand-built "future" document rather than
//! trusted to review.

use od_core::{
    ActorId, AppId, Database, Entity, FieldDef, FieldType, Geometry, GridAxis, Level as Storey,
    Point2, Point3, Value, Vec2, XDataSchema,
};
use od_io_odc::{Feature, Level, Manifest, OdcError, read_bytes, write_bytes};
use std::io::{Cursor, Read, Write};

/// A document with something from every part of the model, so a round trip
/// exercises more than geometry.
fn rich_document() -> Database {
    let mut db = Database::new(ActorId(0x51));

    let schema = XDataSchema::new(
        AppId::new("org.example.plugin"),
        "0.1.0",
        vec![
            FieldDef::new(
                "airflow",
                FieldType::Real,
                "Pset_DuctSegment.NominalAirflow",
            )
            .with_unit("m3/h")
            .described("設計風量"),
            FieldDef::new("system", FieldType::Text, "Pset_DistributionSystem.Name"),
        ],
    );
    db.register_schema(schema).expect("valid schema");

    let layer = db.ensure_layer("M-DUCT-SA");
    let space = db.model_space();

    let id = db
        .insert_entity(Entity::new(
            layer,
            space,
            Geometry::Line {
                a: Point3::new(0.0, 0.0, 3200.0),
                b: Point3::new(12_000.0, 0.0, 3200.0),
            },
        ))
        .expect("inserts");

    // Extension data with a registered schema — the thing DXF cannot carry.
    let app = AppId::new("org.example.plugin");
    let object = db.object(id).cloned().expect("exists");
    let mut object = object;
    let record = object.xdata.entry(app);
    record.set("airflow", Value::Real(2400.0));
    record.set("system", Value::Text("SA".into()));
    let snapshot = {
        let mut s = db.to_snapshot();
        for o in &mut s.objects {
            if o.id == id {
                *o = object.clone();
            }
        }
        s
    };
    let mut db = Database::from_snapshot(snapshot);

    // Storeys and grids: first-class in the model, absent from DXF.
    let level_id = db.reserve_id();
    db.tables.levels.insert(
        level_id,
        Storey {
            name: "3FL".into(),
            elevation: 8400.0,
            height: Some(4200.0),
            order: 3,
        },
    );
    let grid_id = db.reserve_id();
    db.tables.grids.insert(
        grid_id,
        GridAxis {
            name: "X1".into(),
            origin: Point2::new(0.0, 0.0),
            direction: Vec2::new(0.0, 1.0),
            family: "X".into(),
        },
    );

    db
}

/// Rewrites a document as a newer build would: a manifest with an extra
/// feature, and an entry belonging to it.
fn as_written_by_a_newer_build(
    odc: &[u8],
    feature: Feature,
    path: &str,
    payload: &[u8],
) -> Vec<u8> {
    let mut source = zip::ZipArchive::new(Cursor::new(odc)).expect("valid odc");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let names: Vec<String> = source.file_names().map(ToOwned::to_owned).collect();
    for name in names {
        let mut entry = source.by_name(&name).expect("entry");
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).expect("read");

        if name == "manifest.json" {
            let mut manifest: Manifest = serde_json::from_slice(&buf).expect("manifest");
            manifest.features.push(feature.clone());
            manifest.generator = "OpenDraft 99.0.0".into();
            buf = serde_json::to_vec_pretty(&manifest).expect("write manifest");
        }

        out.start_file(&name, stored).expect("start");
        out.write_all(&buf).expect("write");
    }

    out.start_file(path, stored).expect("start");
    out.write_all(payload).expect("write");
    out.finish().expect("finish").into_inner()
}

#[test]
fn a_document_round_trips_with_everything_dxf_cannot_carry() {
    let db = rich_document();
    let bytes = write_bytes(&db).expect("writes");
    let (back, outcome) = read_bytes(&bytes).expect("reads");

    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert_eq!(back.entities().count(), db.entities().count());

    // Schema, and the attribute values it gives meaning to.
    let app = AppId::new("org.example.plugin");
    let schema = back.schemas.get(&app).expect("schema survived");
    assert_eq!(schema.fields.len(), 2);
    assert_eq!(
        schema.field("airflow").and_then(|f| f.unit.clone()),
        Some("m3/h".to_owned()),
        "a unit is what makes the number mean anything"
    );

    let (_, entity_object) = back
        .objects()
        .find(|(_, o)| !o.xdata.is_empty())
        .expect("extension data survived");
    assert_eq!(
        entity_object.xdata.field(&app, "airflow"),
        Some(&Value::Real(2400.0))
    );

    // Storeys and grids.
    let storey = back.tables.levels.by_name("3FL").expect("storey survived");
    assert!(od_core::tol::eq_len(storey.elevation, 8400.0));
    assert!(back.tables.grids.by_name("X1").is_some());

    assert!(back.validate().is_empty());
}

#[test]
fn saving_twice_produces_the_same_bytes() {
    // Deterministic output is what makes a drawing diffable and a save
    // reviewable. Insertion-ordered maps throughout the model are what buy it.
    let db = rich_document();
    let first = write_bytes(&db).expect("writes");
    let reloaded = read_bytes(&first).expect("reads").0;
    let second = write_bytes(&reloaded).expect("writes again");
    assert_eq!(first, second, "a save/load/save cycle must be stable");
}

#[test]
fn a_required_feature_we_do_not_understand_refuses_to_open() {
    let bytes = write_bytes(&rich_document()).expect("writes");
    let future = as_written_by_a_newer_build(
        &bytes,
        Feature::new("mep.routes", Level::Required, &["mep/"]),
        "mep/routes.cbor",
        b"future data",
    );

    match read_bytes(&future) {
        Err(OdcError::UnsupportedFeatures {
            features,
            generator,
        }) => {
            assert_eq!(features, vec!["mep.routes".to_owned()]);
            assert!(
                generator.contains("99.0.0"),
                "the error should name what wrote the file"
            );
        }
        Ok(_) => {
            panic!("opening this file would discard mep/ on the next save — it must be refused")
        }
        Err(other) => panic!("expected UnsupportedFeatures, got {other}"),
    }
}

#[test]
fn an_unknown_preserved_feature_survives_a_round_trip() {
    let bytes = write_bytes(&rich_document()).expect("writes");
    let payload = b"a future build's markup data";
    let future = as_written_by_a_newer_build(
        &bytes,
        Feature::new("annotation.markup", Level::OptionalPreserve, &["markup/"]),
        "markup/sheet1.bin",
        payload,
    );

    let (db, outcome) = read_bytes(&future).expect("opens despite the unknown feature");
    assert_eq!(outcome.preserved_features, vec!["annotation.markup"]);

    // And it comes back out when this build saves.
    let rewritten = write_bytes(&db).expect("writes");
    let mut zip = zip::ZipArchive::new(Cursor::new(rewritten.as_slice())).expect("valid");
    let mut entry = zip
        .by_name("markup/sheet1.bin")
        .expect("the unknown entry was written back");
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).expect("read");
    assert_eq!(buf, payload, "byte-for-byte, or it was not preserved");
}

#[test]
fn an_ignorable_feature_is_dropped_without_complaint() {
    let bytes = write_bytes(&rich_document()).expect("writes");
    let future = as_written_by_a_newer_build(
        &bytes,
        Feature::new("index.rtree3d", Level::OptionalIgnore, &["idx3/"]),
        "idx3/tree.bin",
        b"a rebuildable cache",
    );

    let (db, outcome) = read_bytes(&future).expect("opens");
    assert_eq!(outcome.ignored_features, vec!["index.rtree3d"]);

    let rewritten = write_bytes(&db).expect("writes");
    let zip = zip::ZipArchive::new(Cursor::new(rewritten.as_slice())).expect("valid");
    assert!(
        !zip.file_names().any(|n| n.starts_with("idx3/")),
        "a cache is allowed to be dropped; that is what the level means"
    );
}

#[test]
fn an_entry_no_feature_claims_is_kept_anyway() {
    let bytes = write_bytes(&rich_document()).expect("writes");
    // Same trick, but the manifest says nothing about the new entry — a file
    // written by a build whose manifest we cannot fully interpret.
    let future = as_written_by_a_newer_build(
        &bytes,
        Feature::new("something.else", Level::OptionalIgnore, &["nowhere/"]),
        "mystery/thing.dat",
        b"unclaimed bytes",
    );

    let (db, outcome) = read_bytes(&future).expect("opens");
    assert_eq!(outcome.undeclared_entries, vec!["mystery/thing.dat"]);

    let rewritten = write_bytes(&db).expect("writes");
    let mut zip = zip::ZipArchive::new(Cursor::new(rewritten.as_slice())).expect("valid");
    let mut entry = zip
        .by_name("mystery/thing.dat")
        .expect("unclaimed data is kept rather than dropped");
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).expect("read");
    assert_eq!(buf, b"unclaimed bytes");
}

#[test]
fn data_preserved_from_dxf_survives_the_detour_through_odc() {
    // The two preservation layers have to compose: read a DXF with an entity
    // and a section we do not model, save as .odc, reload, write DXF, and the
    // originals must still be there.
    let dxf = concat!(
        "0\nSECTION\n2\nCLASSES\n0\nCLASS\n1\nExAcXREFPanelObject\n0\nENDSEC\n",
        "0\nSECTION\n2\nENTITIES\n",
        "0\nLINE\n8\nM-DUCT-SA\n10\n0.0\n20\n0.0\n11\n5000.0\n21\n0.0\n",
        "0\nACAD_TABLE\n8\n0\n10\n99.0\n20\n88.0\n1\nirreplaceable\n",
        "0\nENDSEC\n0\nEOF\n",
    );

    let (from_dxf, _) = od_io_dxf::read_str(dxf).expect("reads dxf");
    assert_eq!(from_dxf.preserved.len(), 1, "the CLASSES section was kept");

    let odc = write_bytes(&from_dxf).expect("writes odc");
    let (from_odc, outcome) = read_bytes(&odc).expect("reads odc");
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);

    assert_eq!(
        from_odc.preserved.len(),
        1,
        "the DXF section survived the container"
    );
    assert_eq!(from_odc.preserved[0].source, "dxf");
    assert_eq!(from_odc.preserved[0].section, "CLASSES");

    let back_to_dxf = od_io_dxf::write_string(&from_odc);
    assert!(
        back_to_dxf.contains("ExAcXREFPanelObject"),
        "the unmodelled section must reach the DXF writer intact"
    );
    assert!(
        back_to_dxf.contains("irreplaceable"),
        "the unmodelled entity must too"
    );
}

#[test]
fn a_large_document_is_chunked_and_reassembles_in_order() {
    let mut db = Database::new(ActorId::SYSTEM);
    let layer = db.ensure_layer("M-TEST");
    let space = db.model_space();
    let ids: Vec<_> = (0..2500)
        .map(|i| {
            db.insert_entity(Entity::new(
                layer,
                space,
                Geometry::Point(Point3::new(f64::from(i), 0.0, 0.0)),
            ))
            .expect("inserts")
        })
        .collect();

    let bytes = write_bytes(&db).expect("writes");
    let zip = zip::ZipArchive::new(Cursor::new(bytes.as_slice())).expect("valid");
    let chunks = zip
        .file_names()
        .filter(|n| n.starts_with("objects/"))
        .count();
    assert!(
        chunks >= 3,
        "2500 objects should span several chunks, got {chunks}"
    );

    let (back, _) = read_bytes(&bytes).expect("reads");
    let order = back
        .tables
        .blocks
        .get(back.model_space())
        .expect("model space")
        .entities
        .clone();
    assert_eq!(order, ids, "draw order must survive chunking");
}
