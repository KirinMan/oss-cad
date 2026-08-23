# OpenDraft part library

Systems, specifications and parts for building services (MEP) drawing.

## Why this exists in this form

The incumbent products' strongest asset is a catalogue of tens of thousands of
modelled manufacturer products. An open project cannot match that by enumeration,
and pretending otherwise would produce a library that is always 90% incomplete.

So the parts here are **parametric, not enumerated**. One definition of a
rectangular elbow, written in terms of `W`, `H` and `R`, draws correctly at every
size anyone will ever need. The starter set below covers the common trades well
enough to produce a real drawing, offline, on first run — no account, no
manufacturer agreement, no download.

This does not replace manufacturer data. When a product is specified, its real
dimensions matter, and a vendor-supplied definition should replace the generic
one. What it does mean is that the tool is useful before that happens.

## Layout

```
parts/
├── systems/     what a run carries, its colour, its layer, its default height
├── specs/       how a run is fabricated: fittings, bend limits, stocked sizes
└── parts/       the parts themselves, grouped by trade
```

Every file is JSON, and every file is loaded by
[`od-parts`](../crates/od-parts). The bundled set is compiled into the binary;
office and project libraries are loaded on top of it from disk, and a later
definition with the same `id` wins.

## What is in the starter set

| File | Contents |
|---|---|
| `systems/standard-jp.json` | 24 systems across HVAC, plumbing, fire, gas and electrical, with the layer names and colour indices used in Japanese practice |
| `specs/duct-galvanised.json` | Spiral and rectangular galvanised ductwork (SHASE-S 010 practice) |
| `specs/pipe-sgp.json` | Carbon steel pipe, JIS G 3452, 15A–300A |
| `specs/pipe-vp.json` | Rigid PVC pipe, JIS K 6741, VP13–VP200 |
| `specs/conduit-steel.json` | Thin-wall steel conduit, JIS C 8305, C19–C75 |
| `parts/duct-fittings.json` | Elbows, reducers, tees, wyes, flexible connectors, VD/FD/MD dampers, silencer |
| `parts/duct-terminals.json` | Anemostat, register, grille, nozzle, weather louver, plenum box |
| `parts/pipe-fittings.json` | Elbows, tee, reducer, socket, flange, union, cap |
| `parts/valves.json` | Gate, globe, ball, butterfly, check, strainer, pressure reducing |
| `parts/sanitary.json` | WC, urinal, basin, sink, floor drain, cleanout, vent terminal |
| `parts/hvac-equipment.json` | Cassette and ducted indoor units, outdoor unit, FCU, ERV, fan, AHU |
| `parts/plumbing-equipment.json` | Pump, storage tank, water heater, water meter |
| `parts/electrical.json` | Luminaires, emergency and exit lighting, outlet, switch, distribution board, exhaust fan, smoke detector |
| `parts/support.json` | Rod hanger, sleeve, cable tray |

## Writing a part

```jsonc
{
  "id": "duct.elbow.rect.90",
  "name": { "ja": "長方形ダクト エルボ 90°", "en": "Rectangular duct elbow 90°" },
  "category": "duct-fitting",
  "ifc_class": "IfcDuctFitting",
  "parameters": [
    { "name": "W", "label": {...}, "default": 400, "min": 50, "max": 3000, "unit": "mm" },
    // A default may be an expression over earlier parameters.
    { "name": "R", "label": {...}, "default": "max(W, 150)", "unit": "mm" }
  ],
  "ports": [
    { "name": "in",  "origin": [0, 0, 0], "direction": [-1, 0, 0],
      "profile": { "kind": "rect", "w": "W", "h": "H" }, "system_kind": "air" }
  ],
  "symbol2d": [ /* plan symbol, in the part's local frame */ ],
  "body3d":   [ /* box | cylinder | bend */ ],
  "properties": [
    { "name": "width", "ty": "real", "unit": "mm",
      "ifc_property": "Pset_DuctFittingTypeCommon.NominalWidth" }
  ]
}
```

### Rules the loader enforces

These are checked when the catalogue loads, so a mistake in a part file is a
startup error rather than a failure halfway through someone's drawing:

- every expression may only use parameters the part declares;
- every part builds successfully at its default parameters, and produces some
  geometry;
- every part has at least one port, unless it is a `support`;
- every property declares an IFC `Pset.Property` mapping — an attribute with
  nowhere to go in IFC is an attribute that will be lost in the first exchange;
- every spec points at parts that exist, and has a non-empty size table.

### Expressions

Arithmetic, parentheses, and `min`, `max`, `abs`, `sqrt`, `round`, `ceil_to`.
`ceil_to(x, step)` rounds up to the next multiple, which is how a calculated
size is snapped to a stocked one:

```
"default": "ceil_to(W * 0.8, 50)"
```

Anything needing conditionals or state belongs in a plugin, where it can be
sandboxed — not in a part file.

### Local frames

- A run fitting has its inlet at the origin facing `-X`, so it drops onto the end
  of a run with no extra transform. Bends turn towards `+Y`.
- A ceiling-mounted terminal has its visible face at `Z = 0` and its connection
  facing `+Z`.
- A floor- or wall-mounted fixture is set out from the point a plumber would
  dimension it from.

## Contributing a part

Manufacturer data is welcome, under a licence compatible with Apache-2.0, and
must be marked as such in `source`. Generic parts should say `"generic"` and
carry dimensions typical for the class rather than copied from one product's
catalogue.
