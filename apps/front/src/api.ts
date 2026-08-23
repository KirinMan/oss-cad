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
