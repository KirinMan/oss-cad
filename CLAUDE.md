# OpenDraft

An open-source CAD platform for building services (MEP). A general drawing core
in Rust, a parametric part library, and a TypeScript front end and API on top.

Read `docs/` before making architectural decisions — every non-obvious choice
here has a written reason, and the reasons are what keep the layering intact.

## Repository layout

```
crates/          Rust: the engine
  od-geom2d      2D geometry. Pure functions, no I/O, no document model.
  od-geom3d      3D primitives and the SolidKernel trait (no B-rep bundled).
  od-core        The document model: Database, entities, tables, transactions.
  od-io-dxf      DXF reader and writer, written in-house.
  od-parts       Parametric part library + the bundled catalogue loader.
  od-cli         `od` — convert, inspect, check, roundtrip, parts.
parts/           The catalogue itself: systems, specs, parts (JSON).
apps/api         Hono on Bun. Transport over `od`; holds no drawing logic.
apps/front       Vite + React 19 + TanStack Router/Query + Tailwind v4.
packages/shared  Zod schemas shared by api and front.
docs/            System design, 00–06.
```

## Commands

```bash
# Rust
cargo test --workspace
cargo clippy --workspace --all-targets    # must be warning-free
cargo fmt --all
cargo run -q -p od-cli -- parts list

# TypeScript
bun install
bun run check          # format + lint + typecheck + test
bun run dev            # api and front together
OD_BIN=$PWD/target/debug/od bun test apps/api
```

The API needs the `od` binary. Either `cargo build --release -p od-cli` and put
it on PATH, or set `OD_BIN` to `target/debug/od`.

## Rules that are not negotiable

These are enforced by tests and CI, and each one exists because breaking it
causes damage that is hard to see and hard to undo.

1. **`od-core` never names a domain.** No "MEP", no "duct", no "pipe" in the
   core crate. Domains live on top, using extension data and custom objects. The
   moment the core knows about one domain, no second domain can be added.

2. **Never destroy what you cannot read.** An unrecognised DXF entity becomes
   `Geometry::Unsupported` with its original bytes and a proxy outline. An
   unrecognised section becomes a `PreservedBlob`. Load-and-save must be lossless
   even for constructs this build does not model.

3. **One set of tolerances.** `od_geom2d::tol` — `POINT_EPS`, `ANGLE_EPS`,
   `AREA_EPS`. Never write a bare epsilon in a comparison. Disagreeing epsilons
   are the single largest source of geometry bugs.

4. **Every document change goes through a `Transaction`.** Direct mutation
   cannot be undone, replayed, or synced. `Document::edit` is the normal entry
   point; it rolls back the whole group if any step fails.

5. **Every part property declares an IFC mapping.** The catalogue loader rejects
   a part without one. An attribute with nowhere to go in IFC is an attribute
   that will be lost in the first exchange, which is the incumbent products'
   loudest complaint.

6. **The round-trip tests are a gate, not a nicety.**
   `crates/od-io-dxf/tests/roundtrip.rs`. If a change makes a drawing differ
   after read → write → read, the change is wrong.

7. **Coordinates are `f64` millimetres.** No unit conversion in storage; convert
   at the input and output edges only.

## Conventions

- **Rust**: `cargo clippy` must be clean. `unwrap`/`expect`/`panic` are denied in
  production code (allowed in tests via `clippy.toml`). Prefer `try_from` over
  `as` for narrowing conversions.
- **TypeScript**: strict mode with `noUncheckedIndexedAccess`. Validate every
  external response with the shared Zod schema before it reaches a component —
  half-parsed drawing data on screen is worse than an error message.
- **Comments** explain *why*, not *what*. If a constant has a reason (a JIS
  standard, a DXF quirk, a numerical limit), the reason belongs next to it.
- **UI copy is Japanese**; identifiers, comments and commit messages are English.

## Adding to the catalogue

Part definitions live in `parts/parts/*.json`. See `parts/README.md` for the
schema and the local-frame conventions. The loader validates every part at
startup — expressions may only use declared parameters, every part must build at
its defaults, and every property needs its IFC mapping. Run:

```bash
cargo test -p od-parts
cargo run -q -p od-cli -- parts show <id>
```

## What this project is not doing

From `docs/01-requirements.md`, and worth re-reading before saying yes to a
feature request:

- reproducing AutoCAD's full command set or UI
- CAM, FEM, CFD, or energy simulation
- writing our own B-rep kernel
- full-fidelity DWG export (DXF is the supported path; DWG write is r2000)
