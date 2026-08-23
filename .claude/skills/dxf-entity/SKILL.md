---
name: dxf-entity
description: Add support for a DXF entity type, or fix how an existing one is read or written. Use when a drawing shows entities preserved as Unsupported that should be modelled, or when a round-trip loses something.
---

# Supporting a DXF entity

Adding an entity means touching four places, in this order. Skipping the last
one is how an entity gets read correctly and then silently dropped on save.

## 1. Decide whether to model it at all

Check what is actually in the drawings first:

```bash
cargo run -q -p od-cli -- inspect path/to/drawing.dxf
```

Entities listed under "preserved verbatim" already survive a round trip and
still draw. Modelling one is worth it when users need to *edit* it, not merely
keep it. `ACAD_TABLE` in a background drawing does not need modelling; `HATCH`
in a drawing being worked on does.

## 2. Add the geometry to `od-core`

`crates/od-core/src/entity.rs`:

- add a `Geometry` variant carrying what the entity means, not what DXF stores.
  Our `Arc2` holds a signed sweep rather than DXF's start/end pair because the
  pair is ambiguous about direction;
- add its arm to `Geometry::type_name`;
- add its arm to `Geometry::local_bounds`. Bounds must be *tight* — an arc's box
  is not its circle's box — and honest: return `Aabb3::EMPTY` rather than
  guessing, because a wrong box quietly corrupts the spatial index.

## 3. Read it

`crates/od-io-dxf/src/read.rs`, in `build_entity`:

- required codes use `DxfError::MissingCode`, which turns into a per-entity
  warning rather than a failed file;
- optional codes get the DXF default, not zero — `unwrap_or(1.0)` for a scale,
  not `unwrap_or_default()`;
- angles arrive in degrees and are stored in radians;
- codes that repeat (vertices, MTEXT chunks) are collected in file order.

## 4. Write it

`crates/od-io-dxf/src/write.rs`, in `write_entity`. **This is the step that gets
forgotten.** The match has no wildcard arm on purpose: a new variant makes the
compiler point at the missing writer.

- emit the subclass markers (`AcDbEntity`, then `AcDbLine`, `AcDbCircle`…) —
  some readers reject entities without them;
- write the common preamble through `write_common`;
- only emit optional codes when they differ from the default, so files stay
  readable and diffs stay small.

## 5. Test the round trip

`crates/od-io-dxf/tests/roundtrip.rs`. Add a case with the entity in a realistic
form, and assert on structure rather than bytes: entity count, geometry compared
through bounds or reconstructed values, and layer/attribute survival.

```bash
cargo test -p od-io-dxf
cargo run -q -p od-cli -- roundtrip path/to/drawing.dxf
```

`od roundtrip` exits non-zero when a drawing does not survive, which is what
makes it usable as a gate.

## 6. Ask `interop-guard` to review

Angle conventions, coordinate precision and multi-byte text splitting are the
three places this goes wrong, and all three pass a casual read.
