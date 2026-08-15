---
name: add-part
description: Add a part to the MEP catalogue in parts/parts/*.json — a fitting, terminal, valve, fixture, equipment item or support. Use when asked to add, extend or fix a part definition, or when a drawing needs a component the catalogue does not have.
---

# Adding a part to the catalogue

Parts are parametric. One definition covers every size, which is the whole
strategy for competing with a vendor catalogue of tens of thousands of fixed
products (`docs/04-mep.md`). Adding a part means writing a definition that is
right at *any* size, not at one.

## 1. Decide where it goes

| Trade | File | Category |
|---|---|---|
| Duct fittings | `parts/parts/duct-fittings.json` | `duct-fitting`, `duct-equipment` |
| Air terminals | `parts/parts/duct-terminals.json` | `duct-terminal` |
| Pipe fittings | `parts/parts/pipe-fittings.json` | `pipe-fitting` |
| Valves | `parts/parts/valves.json` | `valve` |
| Sanitary fixtures | `parts/parts/sanitary.json` | `sanitary` |
| HVAC plant | `parts/parts/hvac-equipment.json` | `hvac-equipment` |
| Plumbing plant | `parts/parts/plumbing-equipment.json` | `plumbing-equipment` |
| Electrical | `parts/parts/electrical.json` | `electrical-fixture`, `electrical-equipment` |
| Supports, sleeves | `parts/parts/support.json` | `support`, `penetration` |

Id is dotted and specific: `duct.elbow.rect.90`, `valve.butterfly`,
`sanitary.wc.floor`.

## 2. Follow the local-frame conventions

Getting these wrong makes a part that needs a fudge transform at every use:

- **A run fitting** has its inlet at the origin facing `-X`. Bends turn towards
  `+Y`, so a 90° elbow's outlet sits at `(R, R, 0)` facing `+Y`.
- **A ceiling-mounted terminal** has its visible face at `Z = 0` and its
  connection facing `+Z`.
- **A floor- or wall-mounted fixture** is set out from the point a plumber
  dimensions it from.

## 3. Write it

```jsonc
{
  "id": "duct.elbow.rect.90",
  "name": { "ja": "長方形ダクト エルボ 90°", "en": "Rectangular duct elbow 90°" },
  "category": "duct-fitting",
  "tags": ["duct", "elbow", "エルボ"],       // searched in both languages
  "ifc_class": "IfcDuctFitting",
  "source": "generic",                        // or the standard it comes from
  "parameters": [
    { "name": "W", "label": { "ja": "幅", "en": "Width" },
      "default": 400, "min": 50, "max": 3000, "unit": "mm" },
    // A default may be an expression over earlier parameters.
    { "name": "R", "label": { "ja": "曲げ半径", "en": "Bend radius" },
      "default": "max(W, 150)", "unit": "mm" }
  ],
  "ports": [
    { "name": "in", "origin": [0, 0, 0], "direction": [-1, 0, 0],
      "profile": { "kind": "rect", "w": "W", "h": "H" }, "system_kind": "air" }
  ],
  "symbol2d": [ /* line | circle | arc | polyline | text */ ],
  "body3d":   [ /* box | cylinder | bend */ ],
  "properties": [
    { "name": "width", "label": { "ja": "幅", "en": "Width" }, "ty": "real",
      "unit": "mm", "ifc_property": "Pset_DuctFittingTypeCommon.NominalWidth" }
  ]
}
```

Expressions support `+ - * / ( )`, parameters, and `min`, `max`, `abs`, `sqrt`,
`round`, `ceil_to(value, step)`. Use `ceil_to` wherever a calculated dimension
must land on a stocked size.

## 4. Verify

```bash
cargo test -p od-parts
cargo run -q -p od-cli -- parts show <id>
cargo run -q -p od-cli -- parts show <id> --set W=1000 --set H=800
```

Check the printed port positions and directions at *both* sizes. The loader
already enforces that every expression uses declared parameters, that the part
builds at its defaults, that non-supports have ports, and that every property
carries an IFC mapping — so a passing test means the definition is well-formed,
not that it is correct.

Then look at it: start the front end (`bun run dev`) and open the part. The plan
symbol is what a site engineer reads; if it does not look like the symbol they
expect, the part is wrong regardless of what the numbers say.

## 5. Have the domain checked

Ask the `mep-reviewer` agent to review the definition before considering it
done. Bend radii, stocked sizes, port completeness and IFC Pset names are where
this goes wrong, and none of them are caught by a test.
