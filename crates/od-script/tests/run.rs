#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "an integration test asserts by panicking"
)]

use od_core::{ActorId, Database, Document};

fn doc() -> Document {
    Document::new(Database::new(ActorId::SYSTEM))
}

#[test]
fn od_execute_draws_through_the_same_command_every_other_caller_uses() {
    let (doc, outcome) = od_script::run(
        doc(),
        r#"
        od.execute({
            kind: "add_line", layer: "A-WALL",
            a: { x: 0, y: 0, z: 0 }, b: { x: 3600, y: 0, z: 0 }
        });
        "#,
    )
    .expect("the script runs");

    assert_eq!(outcome.created.len(), 1);
    assert_eq!(doc.db.entities().count(), 1);
    let (_, entity) = doc.db.entities().next().expect("one entity");
    assert_eq!(
        entity.geom,
        od_core::Geometry::Line {
            a: od_core::Point3::ORIGIN,
            b: od_core::Point3::new(3600.0, 0.0, 0.0),
        }
    );
}

#[test]
fn multiple_execute_calls_accumulate_in_the_outcome() {
    let (doc, outcome) = od_script::run(
        doc(),
        r#"
        for (var i = 0; i < 3; i++) {
            od.execute({
                kind: "add_circle", layer: "0",
                center: { x: i, y: 0, z: 0 }, radius: 5
            });
        }
        "#,
    )
    .expect("the script runs");

    assert_eq!(outcome.created.len(), 3);
    assert_eq!(doc.db.entities().count(), 3);
}

#[test]
fn od_entities_reads_back_what_was_just_drawn() {
    let (_, outcome) = od_script::run(
        doc(),
        r#"
        od.execute({
            kind: "add_line", layer: "0",
            a: { x: 0, y: 0, z: 0 }, b: { x: 1, y: 0, z: 0 }
        });
        var list = od.entities();
        console.log(list.length + " " + list[0].kind);
        "#,
    )
    .expect("the script runs");

    assert_eq!(outcome.log, vec!["1 line"]);
}

#[test]
fn od_inspect_reports_entity_and_layer_counts() {
    let (_, outcome) = od_script::run(
        doc(),
        r#"
        od.execute({ kind: "add_line", layer: "A", a: { x: 0, y: 0, z: 0 }, b: { x: 1, y: 0, z: 0 } });
        od.execute({ kind: "add_line", layer: "B", a: { x: 0, y: 0, z: 0 }, b: { x: 1, y: 0, z: 0 } });
        var s = od.inspect();
        console.log(s.entities + " " + s.layers);
        "#,
    )
    .expect("the script runs");

    // Layer "0" always exists (Database::new), plus A and B.
    assert_eq!(outcome.log, vec!["2 3"]);
}

#[test]
fn a_command_the_engine_rejects_surfaces_as_a_thrown_js_error() {
    let (_, outcome) = od_script::run(
        doc(),
        r#"
        try {
            od.execute({ kind: "delete_entities", ids: ["0-9999"] });
            console.log("no error");
        } catch (e) {
            console.log("caught");
        }
        "#,
    )
    .expect("the script itself still runs to completion");

    assert_eq!(outcome.log, vec!["caught"]);
}

#[test]
fn a_malformed_script_is_reported_as_a_script_error() {
    let err = od_script::run(doc(), "this is not valid javascript {{{").unwrap_err();
    assert!(matches!(err, od_script::ScriptError::Js(_)));
}

#[test]
fn console_log_joins_multiple_arguments_with_spaces() {
    let (_, outcome) = od_script::run(doc(), r#"console.log("a", 1, true);"#).expect("runs");
    assert_eq!(outcome.log, vec!["a 1 true"]);
}
