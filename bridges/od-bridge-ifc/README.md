# od-bridge-ifc

Writes IFC4 files using [IfcOpenShell](https://ifcopenshell.org/) (LGPL-3.0).
This is the one place in OpenDraft where that dependency is allowed to exist
— see `docs/05-interop-license.md` §1.2/§1.3 for the licence boundary this
bridge is designed to keep. The Rust side (`crates/od-io-ifc`) never links
or imports IfcOpenShell directly; it only spawns this script and talks JSON
over stdio.

## Standalone use

This script is independently useful on its own, the same way
`od-bridge-dwg convert a.dwg a.json` is meant to be — you do not need the
rest of OpenDraft to run it:

```bash
pip install -r requirements.txt
python3 od_bridge_ifc.py out.ifc < request.json
```

`request.json`'s shape is documented in `od_bridge_ifc.py`'s module
docstring. On success the script prints `{"elements_written": N}` to stdout
and exits 0; on failure it prints a message to stderr and exits non-zero.

## Why a Python script instead of a compiled bridge

`od-bridge-dwg` (LibreDWG, GPL-3.0) is a small compiled program because
LibreDWG is a C library with no Python binding to speak of. IfcOpenShell is
the opposite: its only actively maintained, officially supported binding
*is* Python (`pip install ifcopenshell`) — there is no equivalent C API
this project could link against directly without reimplementing IfcOpenShell
itself. A Python subprocess is therefore the honest choice here, not a
shortcut: same process-boundary licence isolation as the DWG bridge, same
JSON-over-stdio contract, different plumbing because the upstream library
only offers one.

## Scope

Export only, and only routed centrelines (`IfcDuctSegment` /
`IfcPipeSegment` / `IfcCableCarrierSegment`, chosen by the caller from a
route's system kind) with already-tessellated, world-space geometry — no
placed equipment or fittings, no reading an IFC file back in, no IDS
self-verification, no `IfcBuildingStorey`/`IfcGrid` beyond one fixed nominal
pair (there is no Level/GridAxis data upstream yet to place against). See
`crates/od-io-ifc`'s crate docs for the full list of what this narrows away
and why.
