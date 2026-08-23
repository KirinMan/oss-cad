import { z } from 'zod';

/** The part catalogue as `od parts --json` reports it. */

export const localizedSchema = z.object({
  ja: z.string(),
  en: z.string().default(''),
});

export const categorySchema = z.enum([
  'duct-fitting',
  'duct-terminal',
  'duct-equipment',
  'pipe-fitting',
  'valve',
  'sanitary',
  'hvac-equipment',
  'plumbing-equipment',
  'electrical-fixture',
  'electrical-equipment',
  'support',
  'penetration',
]);

export const systemKindSchema = z.enum([
  'air',
  'water',
  'drainage',
  'hydronic',
  'fire_protection',
  'gas',
  'power',
  'signal',
  'generic',
]);

/**
 * A cross-section with its numbers resolved. `terminal` carries no flow — an
 * electrical connection point rather than a duct or pipe.
 */
export const profileSchema = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('rect'), w: z.number(), h: z.number() }),
  z.object({ kind: z.literal('round'), d: z.number() }),
  z.object({ kind: z.literal('oval'), w: z.number(), h: z.number() }),
  z.object({ kind: z.literal('terminal') }),
]);

export const partSummarySchema = z.object({
  id: z.string(),
  name_ja: z.string(),
  name_en: z.string(),
  category: categorySchema,
  ifc_class: z.string(),
  parameters: z.array(z.string()),
  ports: z.number().int().nonnegative(),
});

export const parameterSchema = z.object({
  name: z.string(),
  label: localizedSchema,
  /** A number, or an expression over other parameters. */
  default: z.union([z.number(), z.string()]),
  min: z.number().optional(),
  max: z.number().optional(),
  unit: z.string(),
  choices: z.array(z.number()).default([]),
});

export const propertyDefSchema = z.object({
  name: z.string(),
  label: localizedSchema,
  ty: z.enum(['bool', 'int', 'real', 'text', 'ref', 'list']),
  unit: z.string().optional(),
  /** `PropertySet.Property`. Required, so nothing can be exported nameless. */
  ifc_property: z.string(),
  default: z.unknown().optional(),
});

export const partSchema = z.object({
  id: z.string(),
  name: localizedSchema,
  category: categorySchema,
  tags: z.array(z.string()).default([]),
  ifc_class: z.string(),
  parameters: z.array(parameterSchema).default([]),
  properties: z.array(propertyDefSchema).default([]),
  source: z.string().default(''),
});

export const resolvedPortSchema = z.object({
  name: z.string(),
  origin: z.tuple([z.number(), z.number(), z.number()]),
  direction: z.tuple([z.number(), z.number(), z.number()]),
  profile: profileSchema,
  system_kind: systemKindSchema,
});

/**
 * Resolved plan-symbol geometry, as the engine emits it. Only the subset a part
 * symbol can contain is modelled here — a client that meets anything else draws
 * nothing rather than guessing.
 */
const point3Schema = z.object({ x: z.number(), y: z.number(), z: z.number() });

export const symbolGeometrySchema = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('line'), a: point3Schema, b: point3Schema }),
  z.object({
    kind: z.literal('circle'),
    center: point3Schema,
    radius: z.number(),
  }),
  z.object({
    kind: z.literal('arc'),
    center: point3Schema,
    radius: z.number(),
    /** Radians, counter-clockwise. */
    start_angle: z.number(),
    sweep: z.number(),
  }),
  z.object({
    kind: z.literal('polyline'),
    polyline: z.object({
      vertices: z.array(
        z.object({
          point: z.object({ x: z.number(), y: z.number() }),
          bulge: z.number(),
        }),
      ),
      closed: z.boolean(),
    }),
    elevation: z.number(),
  }),
]);

export const partDetailSchema = z.object({
  part: partSchema,
  /** Parameter values this instance was built at. */
  parameters: z.record(z.string(), z.number()),
  ports: z.array(resolvedPortSchema),
  symbol: z.array(symbolGeometrySchema).catch([]),
  bounds_mm: z.tuple([
    z.number(),
    z.number(),
    z.number(),
    z.number(),
    z.number(),
    z.number(),
  ]),
});

export const systemDefSchema = z.object({
  id: z.string(),
  name: localizedSchema,
  kind: systemKindSchema,
  color: z.number().int().min(0).max(255),
  layer: z.string(),
  default_elevation: z.number().default(0),
  abbreviation: z.string().default(''),
});

export const sizeEntrySchema = z.object({
  designation: z.string(),
  nominal: z.number(),
  outside: z.number(),
  thickness: z.number().default(0),
  mass_per_m: z.number().default(0),
});

export const specSchema = z.object({
  id: z.string(),
  name: localizedSchema,
  system_kind: systemKindSchema,
  material: localizedSchema,
  elbow_part: z.string(),
  branch_part: z.string(),
  reducer_part: z.string(),
  min_bend_radius_ratio: z.number(),
  stock_length: z.number().default(0),
  joint: localizedSchema,
  sizes: z.array(sizeEntrySchema).default([]),
  standard: z.string().default(''),
});

export type Localized = z.infer<typeof localizedSchema>;
export type Category = z.infer<typeof categorySchema>;
export type SystemKind = z.infer<typeof systemKindSchema>;
export type Profile = z.infer<typeof profileSchema>;
export type PartSummary = z.infer<typeof partSummarySchema>;
export type Parameter = z.infer<typeof parameterSchema>;
export type PropertyDef = z.infer<typeof propertyDefSchema>;
export type Part = z.infer<typeof partSchema>;
export type ResolvedPort = z.infer<typeof resolvedPortSchema>;
export type SymbolGeometry = z.infer<typeof symbolGeometrySchema>;
export type PartDetail = z.infer<typeof partDetailSchema>;
export type SystemDef = z.infer<typeof systemDefSchema>;
export type SizeEntry = z.infer<typeof sizeEntrySchema>;
export type Spec = z.infer<typeof specSchema>;

/** How a profile reads on screen and in a schedule: `400×300`, `φ200`. */
export function profileLabel(p: Profile): string {
  switch (p.kind) {
    case 'rect':
      return `${p.w}×${p.h}`;
    case 'oval':
      return `${p.w}×${p.h} (oval)`;
    case 'round':
      return `φ${p.d}`;
    case 'terminal':
      return '—';
  }
}

/** Free area in mm², for velocity and flow checks. */
export function profileArea(p: Profile): number {
  switch (p.kind) {
    case 'rect':
      return p.w * p.h;
    case 'round':
      return (Math.PI * p.d * p.d) / 4;
    case 'oval': {
      const straight = Math.max(p.w - p.h, 0);
      return straight * p.h + (Math.PI * p.h * p.h) / 4;
    }
    case 'terminal':
      return 0;
  }
}

/** Japanese label for a category, for grouping in the UI. */
export const categoryLabels: Record<Category, string> = {
  'duct-fitting': 'ダクト継手',
  'duct-terminal': '吹出口・吸込口',
  'duct-equipment': 'ダクト機器',
  'pipe-fitting': '配管継手',
  valve: '弁類',
  sanitary: '衛生器具',
  'hvac-equipment': '空調機器',
  'plumbing-equipment': '給排水機器',
  'electrical-fixture': '電気器具',
  'electrical-equipment': '電気機器',
  support: '支持金物',
  penetration: '貫通処理',
};

export const systemKindLabels: Record<SystemKind, string> = {
  air: '空調',
  water: '給水・給湯',
  drainage: '排水・通気',
  hydronic: '冷温水・冷媒',
  fire_protection: '消火',
  gas: 'ガス',
  power: '電力',
  signal: '弱電',
  generic: '共通',
};
