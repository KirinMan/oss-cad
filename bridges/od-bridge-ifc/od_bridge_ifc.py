#!/usr/bin/env python3
"""od-bridge-ifc — writes IFC4 from a JSON description of elements.

Standalone by design (docs/05-interop-license.md §1.3): this script is a
complete, independently useful tool on its own -- `python3 od_bridge_ifc.py
out.ifc < request.json` works without OpenDraft's Rust side present at all.
That independence, plus the fact that IfcOpenShell (LGPL-3.0) is never
linked into the Apache-2.0 core, is what keeps this bridge's licence
boundary the same shape as `od-bridge-dwg`'s -- the one difference is the
communication mechanism: LibreDWG has no Python binding, so that bridge is
a small compiled program talking JSON/CBOR over stdio; IfcOpenShell's only
supported binding *is* Python, so this bridge is a Python script talking
JSON over stdio instead. Same boundary, same reasons, different plumbing.

Request (stdin, JSON):
    {
      "project_name": str,
      "elements": [
        {
          "ifc_class": str,          # e.g. "IfcDuctSegment"
          "name": str,
          "positions": [[x, y, z], ...],   # millimetres, world space
          "triangles": [[a, b, c], ...],   # 0-indexed into positions
          "properties": {
            "<Pset name>": {"<Property name>": "<value>", ...},
            ...
          }
        },
        ...
      ]
    }

All geometry is already in world-space millimetres (the same MeshData
od-io-gltf consumes) baked directly into the tessellation, so every
element's ObjectPlacement is identity -- there is no per-element transform
to get right or wrong, only coordinates already computed upstream.

Response (stdout, JSON, on success): {"elements_written": <int>}
On failure: a message on stderr, non-zero exit.
"""

from __future__ import annotations

import json
import sys

try:
    import ifcopenshell
    import ifcopenshell.api.aggregate
    import ifcopenshell.api.context
    import ifcopenshell.api.geometry
    import ifcopenshell.api.pset
    import ifcopenshell.api.root
    import ifcopenshell.api.spatial
    import ifcopenshell.api.unit
except ImportError as exc:  # pragma: no cover - environment problem, not a code path
    print(
        f"od-bridge-ifc requires the `ifcopenshell` package (pip install ifcopenshell): {exc}",
        file=sys.stderr,
    )
    sys.exit(2)


def build(request: dict) -> "ifcopenshell.file":
    f = ifcopenshell.file(schema="IFC4")

    project = ifcopenshell.api.root.create_entity(
        f, ifc_class="IfcProject", name=request.get("project_name") or "OpenDraft export"
    )
    ifcopenshell.api.unit.assign_unit(f, length={"is_metric": True, "raw": "MILLIMETRE"})
    body_context = ifcopenshell.api.context.add_context(f, context_type="Model")
    body_subcontext = ifcopenshell.api.context.add_context(
        f,
        context_type="Model",
        context_identifier="Body",
        target_view="MODEL_VIEW",
        parent=body_context,
    )

    # A single fixed Site/Building/Storey -- there is no Level table on the
    # OpenDraft side yet to place elements against (the same documented gap
    # od-domain-mep's own module docs note elsewhere), so every element
    # lands in one nominal storey rather than guessing at a real one.
    site = ifcopenshell.api.root.create_entity(f, ifc_class="IfcSite", name="Site")
    building = ifcopenshell.api.root.create_entity(f, ifc_class="IfcBuilding", name="Building")
    storey = ifcopenshell.api.root.create_entity(
        f, ifc_class="IfcBuildingStorey", name="Storey"
    )
    ifcopenshell.api.aggregate.assign_object(f, relating_object=project, products=[site])
    ifcopenshell.api.aggregate.assign_object(f, relating_object=site, products=[building])
    ifcopenshell.api.aggregate.assign_object(f, relating_object=building, products=[storey])

    for element in request.get("elements", []):
        product = ifcopenshell.api.root.create_entity(
            f, ifc_class=element["ifc_class"], name=element.get("name", "")
        )
        ifcopenshell.api.spatial.assign_container(f, relating_structure=storey, products=[product])
        # Identity placement -- see the module docstring for why every
        # element's coordinates are already world-space.
        ifcopenshell.api.geometry.edit_object_placement(f, product=product)

        positions = tuple(tuple(float(c) for c in p) for p in element.get("positions", []))
        triangles = tuple(tuple(i + 1 for i in tri) for tri in element.get("triangles", []))
        if positions and triangles:
            point_list = f.create_entity("IfcCartesianPointList3D", CoordList=positions)
            tessellation = f.create_entity(
                "IfcTriangulatedFaceSet", Coordinates=point_list, CoordIndex=triangles
            )
            shape_rep = f.create_entity(
                "IfcShapeRepresentation",
                ContextOfItems=body_subcontext,
                RepresentationIdentifier="Body",
                RepresentationType="Tessellation",
                Items=[tessellation],
            )
            product.Representation = f.create_entity(
                "IfcProductDefinitionShape", Representations=[shape_rep]
            )

        for pset_name, props in element.get("properties", {}).items():
            pset = ifcopenshell.api.pset.add_pset(f, product=product, name=pset_name)
            ifcopenshell.api.pset.edit_pset(f, pset=pset, properties=props)

    return f


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {argv[0]} <output.ifc>  (request JSON read from stdin)", file=sys.stderr)
        return 2
    out_path = argv[1]

    try:
        request = json.load(sys.stdin)
    except json.JSONDecodeError as exc:
        print(f"malformed request JSON on stdin: {exc}", file=sys.stderr)
        return 1

    try:
        f = build(request)
        f.write(out_path)
    except Exception as exc:  # noqa: BLE001 - reported to the Rust caller, not swallowed
        print(f"building {out_path}: {exc}", file=sys.stderr)
        return 1

    print(json.dumps({"elements_written": len(request.get("elements", []))}))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
