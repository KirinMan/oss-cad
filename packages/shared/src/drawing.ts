import { z } from 'zod';

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
  entities: z.number().int().nonnegative(),
  layers: z.number().int().nonnegative(),
  blocks: z.number().int().nonnegative(),
  /** Entities kept verbatim because this build does not model them. */
  preserved_entities: z.number().int().nonnegative(),
  preserved_sections: z.number().int().nonnegative(),
  unsupported_types: z.array(z.string()),
  warnings: z.number().int().nonnegative(),
  extents_mm: extentsSchema,
});

export const inspectionSchema = drawingSummarySchema.extend({
  entities_by_type: z.record(z.string(), z.number().int().nonnegative()),
  layer_names: z.array(z.string()),
});

export const conversionSchema = drawingSummarySchema.extend({
  output: z.string(),
});

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
