import {
  checkReportSchema,
  inspectionSchema,
  partDetailSchema,
  partSummarySchema,
  specSchema,
  systemDefSchema,
  type ApiError,
  type CheckReport,
  type Inspection,
  type PartDetail,
  type PartSummary,
  type Spec,
  type SystemDef,
} from '@opendraft/shared';
import { z } from 'zod';

/**
 * The API client.
 *
 * Every response is validated against the shared schema before it reaches a
 * component. A drawing tool that renders half-parsed data is worse than one
 * that says it could not load: the operator would not know which parts of what
 * they are looking at are real.
 */

export class ApiRequestError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly detail?: string,
  ) {
    super(message);
    this.name = 'ApiRequestError';
  }
}

async function request<T>(
  schema: z.ZodType<T>,
  path: string,
  init?: RequestInit,
): Promise<T> {
  const res = await fetch(path, init);
  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as ApiError | null;
    throw new ApiRequestError(
      body?.error ?? `request failed with ${res.status}`,
      res.status,
      body?.detail,
    );
  }
  const parsed = schema.safeParse(await res.json());
  if (!parsed.success) {
    throw new ApiRequestError(
      `the server sent something this build does not understand: ${parsed.error.issues[0]?.message ?? 'schema mismatch'}`,
      res.status,
    );
  }
  return parsed.data;
}

const partListSchema = z.object({
  parts: z.array(partSummarySchema),
  total: z.number(),
});

export async function fetchParts(
  query: string,
  category?: string,
): Promise<PartSummary[]> {
  const params = new URLSearchParams();
  if (query) params.set('q', query);
  if (category) params.set('category', category);
  const suffix = params.size > 0 ? `?${params.toString()}` : '';
  const { parts } = await request(partListSchema, `/api/parts${suffix}`);
  return parts;
}

export async function fetchPart(
  id: string,
  overrides: Record<string, number> = {},
): Promise<PartDetail> {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(overrides)) {
    params.set(key, String(value));
  }
  const suffix = params.size > 0 ? `?${params.toString()}` : '';
  return request(partDetailSchema, `/api/parts/${encodeURIComponent(id)}${suffix}`);
}

export async function fetchSystems(): Promise<SystemDef[]> {
  const { systems } = await request(
    z.object({ systems: z.array(systemDefSchema) }),
    '/api/systems',
  );
  return systems;
}

export async function fetchSpecs(): Promise<Spec[]> {
  const { specs } = await request(z.object({ specs: z.array(specSchema) }), '/api/specs');
  return specs;
}

export async function inspectDrawing(file: File): Promise<Inspection> {
  const form = new FormData();
  form.set('file', file);
  return request(inspectionSchema, '/api/drawings/inspect', {
    method: 'POST',
    body: form,
  });
}

export async function checkDrawing(
  file: File,
  rules: 'basic' | 'jp',
): Promise<CheckReport> {
  const form = new FormData();
  form.set('file', file);
  return request(checkReportSchema, `/api/drawings/check?rules=${rules}`, {
    method: 'POST',
    body: form,
  });
}

export interface RenderOptions {
  dark?: boolean;
  layers?: string[];
  /** `x1,y1,x2,y2` in drawing millimetres. */
  window?: string;
}

/**
 * Renders a drawing and returns an object URL for it.
 *
 * The SVG is shown in an `<img>` rather than inlined, so scripts inside a
 * drawing someone sent us cannot run. Callers must revoke the URL when done.
 */
export async function renderDrawing(
  file: File,
  options: RenderOptions = {},
): Promise<string> {
  const params = new URLSearchParams();
  if (options.dark) params.set('dark', '1');
  if (options.layers?.length) params.set('layers', options.layers.join(','));
  if (options.window) params.set('window', options.window);

  const form = new FormData();
  form.set('file', file);

  const suffix = params.size > 0 ? `?${params.toString()}` : '';
  const res = await fetch(`/api/drawings/render${suffix}`, {
    method: 'POST',
    body: form,
  });
  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as ApiError | null;
    throw new ApiRequestError(
      body?.error ?? `render failed with ${res.status}`,
      res.status,
      body?.detail,
    );
  }
  return URL.createObjectURL(await res.blob());
}

/** Formats a drawing can be saved as. */
export type SaveFormat = 'odc' | 'dxf' | 'json';

export interface SavedDrawing {
  blob: Blob;
  filename: string;
  /** What the chosen format could not carry. Empty for `.odc`. */
  losses: string[];
}

const summaryHeaderSchema = z.object({
  entities: z.number(),
  layers: z.number(),
  preserved: z.number(),
  warnings: z.number(),
  losses: z.array(z.string()).default([]),
});

/**
 * Converts a drawing and hands back the bytes.
 *
 * The losses come back with the file rather than after it, because "the
 * storeys did not fit in what you just downloaded" is not useful once the
 * download is the user's copy of record.
 */
export async function saveDrawing(file: File, to: SaveFormat): Promise<SavedDrawing> {
  const res = await fetch(`/api/drawings/convert?to=${to}`, {
    method: 'POST',
    body: (() => {
      const form = new FormData();
      form.set('file', file);
      return form;
    })(),
  });

  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as ApiError | null;
    throw new ApiRequestError(
      body?.error ?? `conversion failed with ${res.status}`,
      res.status,
      body?.detail,
    );
  }

  const parsed = summaryHeaderSchema.safeParse(
    JSON.parse(res.headers.get('x-opendraft-summary') ?? '{}'),
  );
  const base = file.name.replace(/\.[^.]+$/, '');

  return {
    blob: await res.blob(),
    filename: `${base}.${to}`,
    losses: parsed.success ? parsed.data.losses : [],
  };
}

const healthSchema = z.object({
  status: z.enum(['ok', 'degraded']),
  engine: z.object({ binary: z.string(), available: z.boolean() }),
});

export async function fetchHealth(): Promise<z.infer<typeof healthSchema>> {
  const res = await fetch('/health');
  const parsed = healthSchema.safeParse(await res.json().catch(() => null));
  if (!parsed.success) {
    return { status: 'degraded', engine: { binary: 'od', available: false } };
  }
  return parsed.data;
}
