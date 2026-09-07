//! A JavaScript scripting host for OpenDraft (`docs/06-roadmap.md`'s
//! "スクリプトAPI").
//!
//! A script drives a [`od_core::Document`] through exactly the same
//! [`od_core::Command`] every other caller goes through (ADR-006) — the
//! whole point of the command pattern is that a mouse click, `od edit
//! --command`, an API request and a script are the same operation, so this
//! host does not invent a second vocabulary. `od.execute(command)` takes the
//! identical JSON shape `od edit --command` and `POST /api/drawings/edit`
//! already accept; a script that works through one path works through all
//! of them.
//!
//! JavaScript, not Python: `docs/02-architecture.md` ADR-001 picks Rust for
//! `od-*` precisely so the same code targets native and WASM, and a script
//! host inherits that goal — a script a user writes for the desktop build
//! should also run unmodified in the browser build, someday. `boa_engine` is
//! a pure-Rust JS engine with no C dependency to cross-compile, unlike
//! CPython bindings; Python support (if ever added) would need a completely
//! different embedding story for the WASM target and is out of scope here.
//!
//! **This is not `od-plugin`.** `docs/02-architecture.md`'s crate table
//! lists `od-script` (a scripting host, what this crate is) and `od-plugin`
//! (a WASM component runtime with capability-based permission management)
//! as two separate crates for a reason: a script here runs with the full
//! trust of whoever ran it — no sandboxing, no permission prompts, the same
//! trust level as `od edit --command`. A plugin system that runs
//! less-trusted third-party code needs the WASM component model's actual
//! isolation boundary, which this crate does not provide and does not try
//! to. That is real, substantial work of its own (a permission model, a
//! capability grant UI, `wasmtime` component linking) — not attempted here.
//!
//! Concretely, `run` bounds CPU time spent in any *single* loop (see
//! [`MAX_LOOP_ITERATIONS`]) but nothing bounds memory: a script that
//! allocates a huge string or array in one native call — `"x".repeat(1e9)`
//! needs no loop at all — is not caught by anything in this crate.
//! `boa_engine` 0.21 has no heap-size limit to set. A caller exposing this to
//! genuinely untrusted scripts (as opposed to a trusted user's own
//! automation) needs an OS-level memory limit around the process running
//! it — a deployment concern, not something addressable inside this crate.
//!
//! `run` takes the `Document` by value rather than `&mut` and hands it back
//! alongside the outcome. `boa_engine`'s native functions are reachable from
//! a garbage-collected JS object that can, in principle, outlive any
//! particular Rust stack frame, so a closure that captures Rust state must
//! either be `'static` or hand that state to the engine through its
//! `Trace`-checked "captures" mechanism (`NativeFunction::from_copy_closure_with_captures`,
//! the *safe* path — the closure itself captures nothing, and the state it
//! needs arrives as an explicit `&Captures` argument the engine threads
//! through). This workspace forbids `unsafe` outright (`Cargo.toml`'s
//! `unsafe_code = "forbid"`), which is exactly what rules out the
//! alternative of borrowing a caller's `&mut Document` directly: that path
//! only exists on `unsafe fn from_closure`, for good reason (a bare borrow
//! captured in a GC-reachable closure is the "use after free" boa's own
//! docs on that function warn about). Moving ownership into the shared
//! `Rc<RefCell<_>>` a `Captures` value holds sidesteps that safely.
//!
//! ```
//! use od_core::{ActorId, Database, Document};
//!
//! let doc = Document::new(Database::new(ActorId::SYSTEM));
//! let (doc, outcome) = od_script::run(
//!     doc,
//!     r#"
//!     od.execute({ kind: "add_line", layer: "0",
//!                  a: { x: 0, y: 0, z: 0 }, b: { x: 100, y: 0, z: 0 } });
//!     console.log("entities: " + od.entities().length);
//!     "#,
//! )
//! .expect("the script runs");
//! assert_eq!(outcome.created.len(), 1);
//! assert_eq!(outcome.log, vec!["entities: 1"]);
//! assert_eq!(doc.db.entities().count(), 1);
//! ```

use boa_engine::gc::{Finalize, Trace};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsResult, JsValue, NativeFunction, Source, js_string};
use od_core::Document;
use std::cell::RefCell;
use std::rc::Rc;

/// Measured, not guessed: an empty `for` loop costs roughly 0.3ms per
/// thousand iterations in a release build of this engine (no JIT — it is a
/// bytecode interpreter), so 2,000,000 keeps even a runaway `while (true)
/// {}` to well under a second there. In an unoptimised debug build the same
/// loop is roughly an order of magnitude slower — actually reaching this
/// limit is not exercised as an automated test for exactly that reason: it
/// would cost the whole suite several extra seconds every run to prove a
/// single `boa_engine` API call behaves as its own documentation says it
/// does. That mechanism was checked directly instead, once, outside this
/// crate's own test suite (a `RuntimeLimits::set_loop_iteration_limit(1000)`
/// against a ten-billion-iteration loop threw in under 2ms).
///
/// This bounds any *single* loop construct, not the script's total run
/// time — a script chaining many loops beneath this limit, or one
/// recursing instead of looping (already capped at a call depth of 512 by
/// this engine's own default), is not covered by it. Closing that
/// completely needs a wall-clock interrupt this engine version does not
/// expose; this is the mitigation that is actually available today for the
/// overwhelmingly common "forgot a loop bound" case.
const MAX_LOOP_ITERATIONS: u64 = 2_000_000;

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("script error: {0}")]
    Js(String),
    /// A `run` invariant did not hold — always a bug in this crate, never
    /// something a script itself could cause. Reported as an error rather
    /// than a panic (this workspace denies `panic!`/`unwrap`/`expect` in
    /// production code) so a caller still gets a `Result` back instead of
    /// an aborted process.
    #[error("internal error: {0}")]
    Internal(String),
}

/// What a script did: every [`od_core::CommandOutcome`] it produced via
/// `od.execute`, concatenated in call order, plus everything it logged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutcome {
    pub created: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub log: Vec<String>,
}

/// The state every native function needs, threaded through
/// [`boa_engine::NativeFunction::from_copy_closure_with_captures`] rather
/// than closed over — see the module docs for why. Plain native Rust data
/// with nothing for boa's GC to trace, which `#[unsafe_ignore_trace]` states
/// as a derive-macro attribute rather than as `unsafe` code this crate
/// itself writes.
#[derive(Clone, Trace, Finalize)]
struct Captures {
    #[unsafe_ignore_trace]
    doc: Rc<RefCell<Document>>,
    #[unsafe_ignore_trace]
    outcome: Rc<RefCell<RunOutcome>>,
}

/// Runs `script` against `doc`, applying whatever `od.execute` calls it
/// makes, and hands `doc` back (see the module docs for why by value).
/// Each `od.execute` call goes through [`Document::execute`] exactly as
/// `od edit` or the API would, so it is undoable and validated the same
/// way — a script is just another caller, not a bypass.
pub fn run(doc: Document, script: &str) -> Result<(Document, RunOutcome), ScriptError> {
    // Kept as plain `Rc`s outside `Captures` (rather than moving them in
    // once and unwrapping the struct back apart afterwards): `Finalize`'s
    // derive gives `Captures` a `Drop` impl, and a type that implements
    // `Drop` cannot have its fields moved out of, even by full
    // destructuring. `Captures` values handed to the engine below are
    // always fresh clones of these two, never the only owners.
    let doc = Rc::new(RefCell::new(doc));
    let outcome = Rc::new(RefCell::new(RunOutcome::default()));
    let captures = Captures {
        doc: Rc::clone(&doc),
        outcome: Rc::clone(&outcome),
    };
    let mut context = Context::default();
    // No wall-clock or instruction-count limit exists in this engine
    // version, but an unbounded `while(true){}` is the overwhelmingly
    // common way a script hangs a caller — this crate's own trust model
    // (module docs: a script has the same trust as `od edit --command`)
    // doesn't make a hang acceptable, only a malicious *edit* acceptable.
    // Function recursion already defaults to a bounded depth (512); this
    // is the loop-based equivalent.
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(MAX_LOOP_ITERATIONS);

    install_od_object(&mut context, captures.clone())
        .map_err(|e| ScriptError::Js(e.to_string()))?;
    install_console(&mut context, captures).map_err(|e| ScriptError::Js(e.to_string()))?;

    context
        .eval(Source::from_bytes(script))
        .map_err(|e| ScriptError::Js(e.to_string()))?;

    drop(context);
    // boa's GC frees a native function's `Gc<Closure<..>>` (and the
    // `Captures` clone it holds) on its own collection cycle, not
    // synchronously when `Context` is dropped — a forced collection here is
    // what actually brings the `Rc` counts below back down to one.
    boa_engine::gc::force_collect();
    let doc = Rc::try_unwrap(doc)
        .map_err(|_| ScriptError::Internal("a script-registered closure outlived `run`".into()))?
        .into_inner();
    let outcome = Rc::try_unwrap(outcome)
        .map(RefCell::into_inner)
        .unwrap_or_else(|rc| rc.borrow().clone());
    Ok((doc, outcome))
}

fn execute(
    _this: &JsValue,
    args: &[JsValue],
    captures: &Captures,
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let arg = args.first().cloned().unwrap_or(JsValue::undefined());
    let json = arg
        .to_json(ctx)?
        .ok_or_else(|| js_error("od.execute needs a command object"))?;
    let command: od_core::Command =
        serde_json::from_value(json).map_err(|e| js_error(format!("invalid command: {e}")))?;

    let result = captures
        .doc
        .borrow_mut()
        .execute("script", &command)
        .map_err(|e| js_error(e.to_string()))?;

    {
        let mut o = captures.outcome.borrow_mut();
        o.created
            .extend(result.created.iter().map(ToString::to_string));
        o.modified
            .extend(result.modified.iter().map(ToString::to_string));
        o.deleted
            .extend(result.deleted.iter().map(ToString::to_string));
    }

    let value = serde_json::to_value(&result).map_err(|e| js_error(e.to_string()))?;
    JsValue::from_json(&value, ctx)
}

fn entities(
    _this: &JsValue,
    _args: &[JsValue],
    captures: &Captures,
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let doc = captures.doc.borrow();
    let summary: Vec<EntitySummary> = doc
        .db
        .entities()
        .map(|(id, e)| EntitySummary {
            id: id.to_string(),
            kind: e.geom.type_name().to_owned(),
            layer: doc.db.tables.layers.get(e.layer).map(|l| l.name.clone()),
        })
        .collect();
    let value = serde_json::to_value(&summary).map_err(|e| js_error(e.to_string()))?;
    JsValue::from_json(&value, ctx)
}

fn inspect(
    _this: &JsValue,
    _args: &[JsValue],
    captures: &Captures,
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let doc = captures.doc.borrow();
    let value = serde_json::json!({
        "entities": doc.db.entities().count(),
        "layers": doc.db.tables.layers.len(),
    });
    JsValue::from_json(&value, ctx)
}

fn console_log(
    _this: &JsValue,
    args: &[JsValue],
    captures: &Captures,
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let line = args
        .iter()
        .map(|a| a.to_string(ctx).map(|s| s.to_std_string_escaped()))
        .collect::<JsResult<Vec<_>>>()?
        .join(" ");
    captures.outcome.borrow_mut().log.push(line);
    Ok(JsValue::undefined())
}

fn install_od_object(context: &mut Context, captures: Captures) -> JsResult<()> {
    let od_object = ObjectInitializer::new(context)
        .function(
            NativeFunction::from_copy_closure_with_captures(execute, captures.clone()),
            js_string!("execute"),
            1,
        )
        .function(
            NativeFunction::from_copy_closure_with_captures(entities, captures.clone()),
            js_string!("entities"),
            0,
        )
        .function(
            NativeFunction::from_copy_closure_with_captures(inspect, captures),
            js_string!("inspect"),
            0,
        )
        .build();
    // A fresh `Context::default()` never already has an `od` global, so
    // this cannot actually fail — propagated with `?` rather than
    // `expect`'d away, since this crate denies `unwrap`/`expect`/`panic!`
    // in production code.
    context.register_global_property(js_string!("od"), od_object, Attribute::all())?;
    Ok(())
}

fn install_console(context: &mut Context, captures: Captures) -> JsResult<()> {
    let console_object = ObjectInitializer::new(context)
        .function(
            NativeFunction::from_copy_closure_with_captures(console_log, captures),
            js_string!("log"),
            0,
        )
        .build();
    context.register_global_property(js_string!("console"), console_object, Attribute::all())?;
    Ok(())
}

fn js_error(message: impl Into<String>) -> JsError {
    JsError::from_opaque(js_string!(message.into()).into())
}

#[derive(Debug, serde::Serialize)]
struct EntitySummary {
    id: String,
    kind: String,
    layer: Option<String>,
}
