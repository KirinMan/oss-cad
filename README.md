# OpenDraft

An open-source CAD platform for building services — a general drawing core with
a mechanical, electrical and plumbing (MEP) domain on top.

**Status: Phase 0.** The engine, the DXF path and the part catalogue work. There
is no drawing canvas yet; that is Phase 2. What exists today is already useful on
its own: batch conversion, drawing inspection, standards checking, and a
parametric part library you can browse in a browser.

## Why

The incumbent products are good. What they are not is reachable: a contractor
cannot licence a seat for every subcontractor on a job, versions drift between
firms until files stop opening, and attributes fall out of drawings on every
format conversion.
Those are the problems this project exists to solve, and they are the ones an
open project can actually solve. See [docs/00-research.md](docs/00-research.md).

> **Attributes must survive the trip.** Read → write → read produces an identical
> drawing, and anything this build cannot model is kept verbatim rather than
> discarded. That is a test in CI, not an aspiration.

## Quick start

Requires [Rust](https://rustup.rs) 1.85+ and [Bun](https://bun.com) 1.3+.

```bash
cargo build --release -p od-cli
export PATH="$PWD/target/release:$PATH"

od parts list                                # 65 parametric parts
od parts show duct.elbow.rect.90 --set W=600 # build one at a size
od inspect drawing.dxf                       # what is in a drawing
od check drawing.dxf --rules jp              # layer names, text heights, storeys
od roundtrip drawing.dxf                     # does it survive a save?
od convert drawing.dxf out.dxf
```

For the web interface:

```bash
bun install
bun run dev      # API on :8787, front end on :5173
```

## What is here

| | |
|---|---|
| **Document model** | Database, entities, symbol tables, extension data with schemas, transactions with real undo. Storeys and structural grids are first-class. |
| **DXF** | Reader and writer written in-house, R12–R2018. Unknown entities and sections are preserved verbatim and written back. |
| **Part catalogue** | 65 parametric parts, 24 systems, 4 specifications with JIS size tables. Parametric rather than enumerated, so one definition covers every size — and none of it needs a manufacturer agreement. |
| **CLI** | `od` — convert, inspect, check, roundtrip, parts. Human output by default, `--json` for machines. |
| **API** | Hono on Bun. Transport over the CLI; holds no drawing logic. Uploaded drawings are deleted as soon as the report is produced. |
| **Front end** | Vite + React 19 + TanStack Router/Query + Tailwind v4. Part browser with live parametric preview, and a DXF inspector. |

Not here yet: a drawing canvas, 3D, constraints, IFC, SXF, DWG. The plan and the
order are in [docs/06-roadmap.md](docs/06-roadmap.md).

## Design documents

| # | Document | Contents |
|---|---|---|
| 00 | [Research](docs/00-research.md) | Existing OSS and commercial CAD, DWG/DXF/IFC/SXF, the constraints that follow |
| 01 | [Requirements](docs/01-requirements.md) | Scope, personas, functional and non-functional requirements, non-goals |
| 02 | [Architecture](docs/02-architecture.md) | Layering, crates, and the ADRs behind each technical choice |
| 03 | [Data model](docs/03-data-model.md) | Document model, persistence, history, collaboration |
| 04 | [MEP domain](docs/04-mep.md) | Routes, fittings, ports, automatic routing, clash detection, take-off |
| 05 | [Interop & licence](docs/05-interop-license.md) | Format strategy, the GPL boundary, security |
| 06 | [Roadmap](docs/06-roadmap.md) | Phases, team, sustainability, risks |

Contributors should also read [CLAUDE.md](CLAUDE.md), which states the rules that
CI enforces.

## Licence

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

Chosen over MIT for its explicit patent grant, and over GPL so that companies can
adopt the core and third parties can build support businesses on it — the
condition under which an open CAD project survives.

No trademark of Autodesk, Daitec or NYK Systems is used to identify this product.
Format names appear only to describe interoperability.
