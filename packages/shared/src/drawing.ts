import { z } from 'zod';
import { profileSchema, systemKindSchema } from './parts.ts';

/**
 * The contract between `od --json` and everything that consumes it.
 *
 * These schemas are not documentation of the CLI's output — they are how the
 * API validates it. The Rust side and the TypeScript side are separate
 * codebases, and the only thing stopping them drifting apart is that this
 * boundary is checked at runtime rather than assumed.
 */

/** `[minX, minY, minZ, maxX, maxY, maxZ]` in millimetres. */
export const extentsSchema = z.tuple([
  z.number(),
  z.number(),
  z.number(),
  z.number(),
  z.number(),
  z.number(),
]);

export const drawingSummarySchema = z.object({
  file: z.string(),
  /** The format the drawing was read from. */
  format: z.string().default('dxf'),
  entities: z.number().int().nonnegative(),
  layers: z.number().int().nonnegative(),
  blocks: z.number().int().nonnegative(),
  /** Entities kept verbatim because this build does not model them. */
  preserved_entities: z.number().int().nonnegative(),
  preserved_sections: z.number().int().nonnegative(),
  /** Entity types preserved verbatim because this build does not model them. */
  unsupported_types: z.array(z.string()),
  /** Container features written by a newer build, preserved but not understood. */
  unsupported_features: z.array(z.string()).default([]),
  warnings: z.number().int().nonnegative(),
  extents_mm: extentsSchema,
});

export const inspectionSchema = drawingSummarySchema.extend({
  entities_by_type: z.record(z.string(), z.number().int().nonnegative()),
  layer_names: z.array(z.string()),
  /** Ordinary, insertable block definitions — not model/paper space. */
  block_names: z.array(z.string()),
});

export const conversionSchema = drawingSummarySchema.extend({
  output: z.string(),
  /**
   * What the target format could not carry. Empty for `.odc`, which holds the
   * whole document; non-empty for DXF, which has no way to express schemas,
   * storeys, grids or domain objects.
   */
  losses: z.array(z.string()).default([]),
});

/**
 * `[min_x, min_y, width, height]` of an SVG's `viewBox`, in drawing
 * millimetres and already Y-flipped to match the SVG's own coordinate space.
 * What an editing canvas needs to turn a click on the rendered image back
 * into a drawing coordinate — the raw extents alone are not this, once
 * padding and the degenerate-extent fallbacks `od-io-svg` applies are
 * accounted for.
 */
export const viewBoxSchema = z.tuple([z.number(), z.number(), z.number(), z.number()]);
export type ViewBox = z.infer<typeof viewBoxSchema>;

export const renderSchema = z.object({
  input: z.string(),
  output: z.string(),
  entities: z.number().int().nonnegative(),
  svg_bytes: z.number().int().nonnegative(),
  view_box: viewBoxSchema,
});

/** `od_geom3d::Point3` / `Vec3`'s own JSON shape — millimetres. */
export const point3Schema = z.object({ x: z.number(), y: z.number(), z: z.number() });
export type Point3 = z.infer<typeof point3Schema>;

/**
 * An `od-core` `Command` (ADR-006, `docs/02-architecture.md`) — the same
 * shape `od edit --command` and `Command::apply` read. Kept in one place so
 * the front end cannot construct a command shape the engine does not
 * recognise without a type error first.
 */
export const commandSchema = z.discriminatedUnion('kind', [
  z.object({
    kind: z.literal('add_line'),
    layer: z.string(),
    a: point3Schema,
    b: point3Schema,
  }),
  z.object({
    kind: z.literal('add_circle'),
    layer: z.string(),
    center: point3Schema,
    radius: z.number().positive(),
  }),
  z.object({
    kind: z.literal('add_arc'),
    layer: z.string(),
    center: point3Schema,
    radius: z.number().positive(),
    /** Radians, matching `Geometry::Arc`'s own convention. */
    start_angle: z.number(),
    sweep: z.number(),
  }),
  z.object({
    kind: z.literal('add_polyline'),
    layer: z.string(),
    /** Must all share one Z — `Geometry::Polyline` is planar. */
    points: z.array(point3Schema).min(2),
    closed: z.boolean(),
  }),
  z.object({
    kind: z.literal('move_entities'),
    ids: z.array(z.string()),
    delta: point3Schema,
  }),
  z.object({
    kind: z.literal('rotate_entities'),
    ids: z.array(z.string()),
    radians: z.number(),
  }),
  z.object({
    kind: z.literal('delete_entities'),
    ids: z.array(z.string()),
  }),
  z.object({
    kind: z.literal('copy_entities'),
    ids: z.array(z.string()),
    delta: point3Schema,
  }),
  z.object({
    kind: z.literal('mirror_entities'),
    ids: z.array(z.string()),
    a: point3Schema,
    b: point3Schema,
    keep_original: z.boolean(),
  }),
  z.object({
    kind: z.literal('offset_entity'),
    id: z.string(),
    distance: z.number(),
  }),
  z.object({
    kind: z.literal('set_vertex'),
    id: z.string(),
    index: z.number().int().nonnegative(),
    position: point3Schema,
  }),
  z.object({
    kind: z.literal('add_text'),
    layer: z.string(),
    position: point3Schema,
    text: z.string(),
    height: z.number().positive(),
    /** Radians, counter-clockwise. */
    rotation: z.number(),
  }),
  z.object({
    kind: z.literal('add_dimension'),
    layer: z.string(),
    point_a: point3Schema,
    point_b: point3Schema,
    /** Perpendicular distance from the measured segment to the dimension line. */
    offset: z.number(),
    /** Overrides the displayed measurement entirely when given. */
    text_override: z.string().optional(),
  }),
  z.object({
    kind: z.literal('create_block'),
    name: z.string(),
    layer: z.string(),
    base_point: point3Schema,
    ids: z.array(z.string()).min(1),
  }),
  z.object({
    kind: z.literal('insert_block'),
    layer: z.string(),
    block_name: z.string(),
    position: point3Schema,
    /** Radians, counter-clockwise. */
    rotation: z.number(),
    scale: point3Schema,
  }),
  z.object({
    kind: z.literal('create_layout'),
    name: z.string(),
  }),
  z.object({
    kind: z.literal('add_viewport'),
    /** An existing paper-space layout's name, e.g. `*Paper_Space`. */
    layout: z.string(),
    /** Paper-space centre of the viewport rectangle. */
    position: point3Schema,
    width: z.number().positive(),
    height: z.number().positive(),
    /** Model-space point this viewport is centred on. */
    target: point3Schema,
    /** Paper units per model unit. */
    scale: z.number().positive(),
  }),
]);
export type Command = z.infer<typeof commandSchema>;

/** `od --json edit`'s own report shape. */
export const editReportSchema = z.object({
  input: z.string(),
  output: z.string(),
  created: z.array(z.string()),
  modified: z.array(z.string()),
  deleted: z.array(z.string()),
  render: z.object({ output: z.string(), view_box: viewBoxSchema }).nullable(),
});
export type EditReport = z.infer<typeof editReportSchema>;

/** `od --json script`'s own report shape (od-script). */
export const scriptReportSchema = z.object({
  input: z.string(),
  output: z.string(),
  created: z.array(z.string()),
  modified: z.array(z.string()),
  deleted: z.array(z.string()),
  /** Everything the script's own `console.log` calls produced, in order. */
  log: z.array(z.string()),
});
export type ScriptReport = z.infer<typeof scriptReportSchema>;

/**
 * `POST /api/drawings/edit`'s response — a web-friendly envelope around the
 * same outcome {@link editReportSchema} describes, carrying the updated
 * document and a fresh render inline rather than by path, since there is no
 * shared filesystem between the service and a browser.
 */
export const editResponseSchema = z.object({
  created: z.array(z.string()),
  modified: z.array(z.string()),
  deleted: z.array(z.string()),
  /** The updated document, base64-encoded. */
  document: z.string(),
  svg: z.string(),
  view_box: viewBoxSchema.nullable(),
});
export type EditResponse = z.infer<typeof editResponseSchema>;

/** `od --json mep route`'s own report shape. */
export const mepRouteReportSchema = z.object({
  input: z.string(),
  output: z.string(),
  segments: z.number().int().nonnegative(),
  fittings: z.number().int().nonnegative(),
  render: z.object({ output: z.string(), view_box: viewBoxSchema }).nullable(),
});
export type MepRouteReport = z.infer<typeof mepRouteReportSchema>;

/**
 * `POST /api/mep/route`'s response — the same web-friendly envelope
 * {@link editResponseSchema} uses, for a route instead of a single `Command`:
 * a route is not one, and cannot be, without `od-core` learning what a
 * "system" or a "spec" is (rule 1) — see `od mep route`, this endpoint's own
 * CLI counterpart.
 */
export const mepRouteResponseSchema = z.object({
  segments: z.number().int().nonnegative(),
  fittings: z.number().int().nonnegative(),
  document: z.string(),
  svg: z.string(),
  view_box: viewBoxSchema.nullable(),
});
export type MepRouteResponse = z.infer<typeof mepRouteResponseSchema>;

/** `od --json mep place`'s own report shape. */
export const mepPlaceReportSchema = z.object({
  input: z.string(),
  output: z.string(),
  created: z.string(),
  render: z.object({ output: z.string(), view_box: viewBoxSchema }).nullable(),
});
export type MepPlaceReport = z.infer<typeof mepPlaceReportSchema>;

/**
 * `POST /api/mep/place`'s response — the same web-friendly envelope
 * {@link editResponseSchema} uses, for placing equipment instead of a single
 * `Command`: a placement needs a part id and a system, vocabulary `od-core`
 * must never learn (rule 1).
 */
export const mepPlaceResponseSchema = z.object({
  created: z.string(),
  document: z.string(),
  svg: z.string(),
  view_box: viewBoxSchema.nullable(),
});
export type MepPlaceResponse = z.infer<typeof mepPlaceResponseSchema>;

/** `[x, y, z]` in drawing millimetres. */
export const point3TupleSchema = z.tuple([z.number(), z.number(), z.number()]);

/** `od --json mep takeoff`'s own report shape — read-only, so this doubles as the API response. */
export const mepTakeoffReportSchema = z.object({
  file: z.string(),
  total_length_mm: z.number(),
  routes: z.array(
    z.object({
      system: z.string(),
      spec: z.string(),
      length_mm: z.number(),
      count: z.number().int().nonnegative(),
    }),
  ),
  /** Fitting part id → count. */
  fittings: z.record(z.string(), z.number().int().nonnegative()),
  /** Equipment part id → count. */
  equipment: z.record(z.string(), z.number().int().nonnegative()),
});
export type MepTakeoffReport = z.infer<typeof mepTakeoffReportSchema>;

/** `od --json mep check`'s own report shape — read-only, so this doubles as the API response. */
export const mepCheckReportSchema = z.object({
  file: z.string(),
  passed: z.boolean(),
  ports: z.number().int().nonnegative(),
  connections: z.number().int().nonnegative(),
  unconnected: z.array(
    z.object({
      owner: z.string(),
      name: z.string(),
      position_mm: point3TupleSchema,
    }),
  ),
  /** Custom objects the bundled catalogue could not resolve a part for. */
  skipped: z.array(z.string()),
});
export type MepCheckReport = z.infer<typeof mepCheckReportSchema>;

/**
 * `od --json mep ports`'s own report shape — every port in the document,
 * connected or not. What an editing canvas offers up as snap targets (F-104):
 * a route that lands exactly on a port's position, facing the opposite way,
 * on a matching system, is what the connection graph links without any extra
 * step — see `mepCheckReportSchema.unconnected`, the read-back of the same
 * data after that link either did or didn't happen.
 */
export const mepPortSchema = z.object({
  owner: z.string(),
  name: z.string(),
  position_mm: point3TupleSchema,
  /** Outward — the direction a connecting run leaves along. */
  direction: point3TupleSchema,
  profile: profileSchema,
  system_kind: systemKindSchema,
  connected: z.boolean(),
});
export type MepPort = z.infer<typeof mepPortSchema>;

export const mepPortsReportSchema = z.object({
  file: z.string(),
  ports: z.array(mepPortSchema),
});
export type MepPortsReport = z.infer<typeof mepPortsReportSchema>;

/**
 * `od --json mep clash`'s own report shape — read-only, so this doubles as
 * the API response (F-108). Every profile is checked as its circumscribing
 * cylinder, not its true cross-section — see `od_domain_mep::clash`'s crate
 * docs for why that is a deliberate, conservative approximation rather than
 * a gap.
 */
export const mepClashIssueSchema = z.object({
  kind: z.enum(['hard', 'clearance', 'duplicate']),
  severity: z.enum(['info', 'warning', 'error']),
  a: z.string(),
  b: z.string(),
  /** Negative is penetration depth, for `hard` and `clearance` issues. */
  gap_mm: z.number(),
  location_mm: point3TupleSchema,
});
export type MepClashIssue = z.infer<typeof mepClashIssueSchema>;

export const mepClashReportSchema = z.object({
  file: z.string(),
  passed: z.boolean(),
  issues: z.array(mepClashIssueSchema),
});
export type MepClashReport = z.infer<typeof mepClashReportSchema>;

/** One `od query` hit — `od --json query`'s own shape. */
export const queryHitSchema = z.object({
  id: z.string(),
  kind: z.string(),
  layer: z.string(),
  bounds_mm: extentsSchema,
  /** Endpoints, centres and midpoints — what an editing canvas snaps to. */
  snap_points_mm: z.array(point3TupleSchema),
  /**
   * Ordered, individually-draggable points — grip handles. Index `i` is
   * exactly the `index` a `set_vertex` command targeting this entity means;
   * empty for a kind with no editable vertices (a circle, a block
   * reference, ...).
   */
  vertices_mm: z.array(point3TupleSchema),
});
export type QueryHit = z.infer<typeof queryHitSchema>;

export const queryReportSchema = z.object({
  matched: z.number().int().nonnegative(),
  indexed: z.number().int().nonnegative(),
  hits: z.array(queryHitSchema),
});
export type QueryReport = z.infer<typeof queryReportSchema>;

export const severitySchema = z.enum(['error', 'warning', 'info']);

export const findingSchema = z.object({
  rule: z.string(),
  severity: severitySchema,
  message: z.string(),
  count: z.number().int().nonnegative(),
});

export const checkReportSchema = z.object({
  file: z.string(),
  passed: z.boolean(),
  findings: z.array(findingSchema),
});

export const roundtripReportSchema = z.object({
  file: z.string(),
  identical: z.boolean(),
  entities_before: z.number().int().nonnegative(),
  entities_after: z.number().int().nonnegative(),
  layers_before: z.number().int().nonnegative(),
  layers_after: z.number().int().nonnegative(),
  max_extent_shift_mm: z.number(),
  warnings_on_reread: z.number().int().nonnegative(),
});

export type Extents = z.infer<typeof extentsSchema>;
export type DrawingSummary = z.infer<typeof drawingSummarySchema>;
export type Inspection = z.infer<typeof inspectionSchema>;
export type Conversion = z.infer<typeof conversionSchema>;
export type RenderReport = z.infer<typeof renderSchema>;
export type Severity = z.infer<typeof severitySchema>;
export type Finding = z.infer<typeof findingSchema>;
export type CheckReport = z.infer<typeof checkReportSchema>;
export type RoundtripReport = z.infer<typeof roundtripReportSchema>;

/** Width and height of a drawing's extents, in millimetres. */
export function extentsSize(e: Extents): { width: number; height: number } {
  return { width: e[3] - e[0], height: e[4] - e[1] };
}

/** True when the extents carry no area — an empty or single-point drawing. */
export function extentsAreEmpty(e: Extents): boolean {
  const { width, height } = extentsSize(e);
  return width === 0 && height === 0;
}
