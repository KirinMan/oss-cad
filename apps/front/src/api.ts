import {
  checkReportSchema,
  editResponseSchema,
  inspectionSchema,
  mepCheckReportSchema,
  mepPlaceResponseSchema,
  mepRouteResponseSchema,
  mepTakeoffReportSchema,
  partDetailSchema,
  partSummarySchema,
  queryReportSchema,
  specSchema,
  systemDefSchema,
  viewBoxSchema,
  type ApiError,
  type CheckReport,
  type Command,
  type Inspection,
  type MepCheckReport,
  type MepTakeoffReport,
  type PartDetail,
  type PartSummary,
  type Profile,
  type QueryHit,
  type Spec,
  type SystemDef,
  type ViewBox,
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

/** Routed lengths by system/spec, and part counts — F-111, read-only. */
export async function takeoffMep(file: File): Promise<MepTakeoffReport> {
  const form = new FormData();
  form.set('file', file);
  return request(mepTakeoffReportSchema, '/api/mep/takeoff', {
    method: 'POST',
    body: form,
  });
}

/** Unconnected ports and parts the catalogue could not resolve — read-only. */
export async function checkMep(file: File): Promise<MepCheckReport> {
  const form = new FormData();
  form.set('file', file);
  return request(mepCheckReportSchema, '/api/mep/check', {
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

/**
 * Renders a drawing and reports the `viewBox` it was drawn with, alongside
 * the object URL — what the editing canvas needs to map a click on the image
 * back to a drawing coordinate. Kept separate from {@link renderDrawing}
 * rather than changing that function's return shape: the read-only viewer
 * has no use for the view box, and every existing caller of `renderDrawing`
 * would otherwise need to change for a value it does not need.
 */
export async function renderDrawingWithViewBox(
  file: File,
  options: RenderOptions = {},
): Promise<{ url: string; viewBox: ViewBox }> {
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
  const box = viewBoxSchema.safeParse(
    JSON.parse(res.headers.get('x-opendraft-viewbox') ?? 'null'),
  );
  if (!box.success) {
    throw new ApiRequestError('the server did not report a view box', res.status);
  }
  return { url: URL.createObjectURL(await res.blob()), viewBox: box.data };
}

export interface EditResult {
  outcome: { created: string[]; modified: string[]; deleted: string[] };
  /** The updated document, in the same format `file` was uploaded in. */
  document: File;
  /** An object URL for the fresh render. Caller must revoke it when done. */
  url: string;
  viewBox: ViewBox;
}

/**
 * Applies one edit command to `file` and hands back everything the canvas
 * needs for its next frame: the updated document (to hold in place of `file`
 * for the next edit) and a fresh render, in one round trip.
 *
 * The service never keeps a drawing between requests — the client is the one
 * holding the document, sending the whole thing back on every edit. That
 * keeps the same "nothing outlives the request" guarantee the rest of this
 * API makes, without needing a server-side session to expire or leak.
 */
export async function editDrawing(file: File, command: Command): Promise<EditResult> {
  const form = new FormData();
  form.set('file', file);
  form.set('command', JSON.stringify(command));

  const res = await fetch('/api/drawings/edit', { method: 'POST', body: form });
  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as ApiError | null;
    throw new ApiRequestError(
      body?.error ?? `edit failed with ${res.status}`,
      res.status,
      body?.detail,
    );
  }
  const parsed = editResponseSchema.safeParse(await res.json());
  if (!parsed.success) {
    throw new ApiRequestError(
      `the server sent something this build does not understand: ${parsed.error.issues[0]?.message ?? 'schema mismatch'}`,
      res.status,
    );
  }
  if (!parsed.data.view_box) {
    throw new ApiRequestError('the server did not report a view box', res.status);
  }
  const bytes = Uint8Array.from(atob(parsed.data.document), (c) => c.charCodeAt(0));
  const document = new File([bytes], file.name, { type: file.type });
  const url = URL.createObjectURL(new Blob([parsed.data.svg], { type: 'image/svg+xml' }));
  const viewBox = parsed.data.view_box;

  return {
    outcome: {
      created: parsed.data.created,
      modified: parsed.data.modified,
      deleted: parsed.data.deleted,
    },
    document,
    url,
    viewBox,
  };
}

export interface RouteResult {
  segments: number;
  fittings: number;
  document: File;
  url: string;
  viewBox: ViewBox;
}

/**
 * Draws a route on `file` — auto-inserting the fittings any 90° bends need —
 * and hands back the updated document and a fresh render, the same shape
 * {@link editDrawing} returns. A route is not a `Command`: `od-core` must
 * never learn what a "system" or a "spec" is (rule 1), so this is a
 * separate endpoint rather than another `Command` variant.
 */
export async function routeMep(
  file: File,
  params: {
    system: string;
    spec: string;
    profile: Profile;
    path: { x: number; y: number; z: number }[];
  },
): Promise<RouteResult> {
  const form = new FormData();
  form.set('file', file);
  form.set('system', params.system);
  form.set('spec', params.spec);
  form.set('profile', JSON.stringify(params.profile));
  form.set('path', JSON.stringify(params.path));

  const res = await fetch('/api/mep/route', { method: 'POST', body: form });
  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as ApiError | null;
    throw new ApiRequestError(
      body?.error ?? `route failed with ${res.status}`,
      res.status,
      body?.detail,
    );
  }
  const parsed = mepRouteResponseSchema.safeParse(await res.json());
  if (!parsed.success) {
    throw new ApiRequestError(
      `the server sent something this build does not understand: ${parsed.error.issues[0]?.message ?? 'schema mismatch'}`,
      res.status,
    );
  }
  if (!parsed.data.view_box) {
    throw new ApiRequestError('the server did not report a view box', res.status);
  }
  const bytes = Uint8Array.from(atob(parsed.data.document), (c) => c.charCodeAt(0));
  const document = new File([bytes], file.name, { type: file.type });
  const url = URL.createObjectURL(new Blob([parsed.data.svg], { type: 'image/svg+xml' }));

  return {
    segments: parsed.data.segments,
    fittings: parsed.data.fittings,
    document,
    url,
    viewBox: parsed.data.view_box,
  };
}

export interface PlaceResult {
  created: string;
  document: File;
  url: string;
  viewBox: ViewBox;
}

/**
 * Places one piece of equipment on `file` and hands back the updated
 * document and a fresh render, the same shape {@link editDrawing} returns.
 * A placement is not a `Command`: `od-core` must never learn what a
 * catalogue part id or a "system" is (rule 1), so this is a separate
 * endpoint rather than another `Command` variant.
 */
export async function placeMep(
  file: File,
  params: {
    part: string;
    position: { x: number; y: number; z: number };
    rotationDegrees?: number;
    mirror?: boolean;
    system?: string;
    set?: string[];
  },
): Promise<PlaceResult> {
  const form = new FormData();
  form.set('file', file);
  form.set('part', params.part);
  form.set('position', JSON.stringify(params.position));
  if (params.rotationDegrees !== undefined) {
    form.set('rotation', String(params.rotationDegrees));
  }
  if (params.mirror) form.set('mirror', '1');
  if (params.system) form.set('system', params.system);
  if (params.set?.length) form.set('set', JSON.stringify(params.set));

  const res = await fetch('/api/mep/place', { method: 'POST', body: form });
  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as ApiError | null;
    throw new ApiRequestError(
      body?.error ?? `place failed with ${res.status}`,
      res.status,
      body?.detail,
    );
  }
  const parsed = mepPlaceResponseSchema.safeParse(await res.json());
  if (!parsed.success) {
    throw new ApiRequestError(
      `the server sent something this build does not understand: ${parsed.error.issues[0]?.message ?? 'schema mismatch'}`,
      res.status,
    );
  }
  if (!parsed.data.view_box) {
    throw new ApiRequestError('the server did not report a view box', res.status);
  }
  const bytes = Uint8Array.from(atob(parsed.data.document), (c) => c.charCodeAt(0));
  const document = new File([bytes], file.name, { type: file.type });
  const url = URL.createObjectURL(new Blob([parsed.data.svg], { type: 'image/svg+xml' }));

  return { created: parsed.data.created, document, url, viewBox: parsed.data.view_box };
}

/**
 * Finds entities near a drawing-space point, through the spatial index —
 * how a click becomes "which entity did that mean" without ever hit-testing
 * against the rendered SVG itself, which is untrusted content and is never
 * inlined into the page (CLAUDE.md).
 */
export async function queryNear(
  file: File,
  point: { x: number; y: number },
  count = 1,
): Promise<QueryHit[]> {
  const form = new FormData();
  form.set('file', file);
  const res = await fetch(
    `/api/drawings/query?near=${point.x},${point.y}&count=${count}`,
    { method: 'POST', body: form },
  );
  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as ApiError | null;
    throw new ApiRequestError(
      body?.error ?? `query failed with ${res.status}`,
      res.status,
      body?.detail,
    );
  }
  const parsed = queryReportSchema.safeParse(await res.json());
  if (!parsed.success) {
    throw new ApiRequestError(
      `the server sent something this build does not understand: ${parsed.error.issues[0]?.message ?? 'schema mismatch'}`,
      res.status,
    );
  }
  return parsed.data.hits;
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
