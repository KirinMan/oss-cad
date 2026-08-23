import { Hono, type Context } from 'hono';
import { cors } from 'hono/cors';
import { logger } from 'hono/logger';
import { z } from 'zod';
import {
  checkReportSchema,
  conversionSchema,
  inspectionSchema,
  partDetailSchema,
  partSummarySchema,
  specSchema,
  systemDefSchema,
  type ApiError,
} from '@opendraft/shared';
import { od, odAvailable, odBinary, OdError, OdMissingError } from './od.ts';

/**
 * The OpenDraft service.
 *
 * Two kinds of endpoint. The catalogue ones are cheap and cacheable. The
 * drawing ones take an upload, hand it to the CLI, and hand back a report —
 * they never keep the file, because a service that quietly retains someone's
 * construction drawings is not one a contractor will use.
 */

/** Uploads above this are refused outright rather than buffered. */
const MAX_UPLOAD_BYTES = Number(process.env.OD_MAX_UPLOAD ?? 64 * 1024 * 1024);

export function createApp() {
  const app = new Hono();

  app.use('*', logger());
  app.use(
    '/api/*',
    cors({
      origin: (process.env.OD_CORS_ORIGIN ?? 'http://localhost:5173').split(','),
      allowMethods: ['GET', 'POST', 'OPTIONS'],
    }),
  );

  app.onError((err, c) => {
    if (err instanceof OdMissingError) {
      return c.json<ApiError>({ error: 'engine unavailable', detail: err.message }, 503);
    }
    if (err instanceof OdError) {
      return c.json<ApiError>(
        { error: 'drawing engine failed', detail: err.message },
        422,
      );
    }
    console.error(err);
    return c.json<ApiError>({ error: 'internal error' }, 500);
  });

  app.notFound((c) => c.json<ApiError>({ error: 'not found' }, 404));

  app.get('/health', async (c) => {
    const engine = await odAvailable();
    return c.json(
      {
        status: engine ? 'ok' : 'degraded',
        engine: { binary: odBinary, available: engine },
      },
      engine ? 200 : 503,
    );
  });

  // ── Catalogue ────────────────────────────────────────────────────────────

  app.get('/api/parts', async (c) => {
    const q = c.req.query('q') ?? '';
    const parts = await od(z.array(partSummarySchema), [
      'parts',
      'list',
      ...(q ? [q] : []),
    ]);
    const category = c.req.query('category');
    const filtered = category ? parts.filter((p) => p.category === category) : parts;
    return c.json({ parts: filtered, total: filtered.length });
  });

  app.get('/api/parts/:id', async (c) => {
    const id = c.req.param('id');

    // Query parameters other than the reserved ones are parameter overrides:
    // /api/parts/duct.elbow.rect.90?W=500&H=300
    const overrides: string[] = [];
    for (const [key, value] of Object.entries(c.req.query())) {
      if (key === 'lang') continue;
      const n = Number(value);
      if (!Number.isFinite(n)) {
        return c.json<ApiError>(
          { error: 'bad parameter', detail: `${key}=${value} is not a number` },
          400,
        );
      }
      overrides.push('--set', `${key}=${n}`);
    }

    const detail = await od(partDetailSchema, ['parts', 'show', id, ...overrides]);
    return c.json(detail);
  });

  app.get('/api/systems', async (c) => {
    const systems = await od(z.array(systemDefSchema), ['parts', 'systems']);
    return c.json({ systems });
  });

  app.get('/api/specs', async (c) => {
    const specs = await od(z.array(specSchema), ['parts', 'specs']);
    return c.json({ specs });
  });

  app.get('/api/specs/:id', async (c) => {
    const spec = await od(specSchema, ['parts', 'specs', c.req.param('id')]);
    return c.json(spec);
  });

  // ── Drawings ─────────────────────────────────────────────────────────────

  app.post('/api/drawings/inspect', (c) =>
    withUpload(c, async (path) => {
      const report = await od(inspectionSchema, ['inspect', path]);
      return c.json(report);
    }),
  );

  app.post('/api/drawings/check', (c) =>
    withUpload(c, async (path) => {
      const rules = c.req.query('rules') === 'jp' ? 'jp' : 'basic';
      const report = await od(checkReportSchema, ['check', path, '--rules', rules]);
      return c.json(report);
    }),
  );

  app.post('/api/drawings/convert', (c) =>
    withUpload(c, async (path, name) => {
      const target = c.req.query('to') === 'json' ? 'json' : 'dxf';
      const out = `${path}.${target}`;
      const report = await od(conversionSchema, ['convert', path, out]);
      const bytes = await Bun.file(out).arrayBuffer();
      await Bun.file(out).delete();

      const base = name.replace(/\.[^./\\]+$/, '');
      return new Response(bytes, {
        headers: {
          'content-type': target === 'json' ? 'application/json' : 'application/dxf',
          'content-disposition': `attachment; filename="${encodeURIComponent(base)}.${target}"`,
          // The summary rides along in a header so a client can report what
          // happened without a second request.
          'x-opendraft-summary': JSON.stringify({
            entities: report.entities,
            layers: report.layers,
            preserved: report.preserved_entities,
            warnings: report.warnings,
          }),
        },
      });
    }),
  );

  return app;
}

/**
 * Takes the uploaded drawing, writes it somewhere the CLI can reach, runs the
 * handler, and removes it — including when the handler throws. Uploaded
 * construction drawings are confidential; nothing outlives the request.
 */
async function withUpload(
  c: Context,
  handler: (path: string, filename: string) => Promise<Response>,
): Promise<Response> {
  const form = await c.req.formData().catch(() => null);
  const file = form?.get('file');
  if (!(file instanceof File)) {
    return c.json<ApiError>(
      { error: 'no file', detail: 'send the drawing as multipart form field `file`' },
      400,
    );
  }
  if (file.size > MAX_UPLOAD_BYTES) {
    return c.json<ApiError>(
      {
        error: 'file too large',
        detail: `${file.size} bytes exceeds the ${MAX_UPLOAD_BYTES} byte limit`,
      },
      413,
    );
  }

  const extension = file.name.toLowerCase().endsWith('.dxf') ? 'dxf' : null;
  if (!extension) {
    return c.json<ApiError>(
      { error: 'unsupported format', detail: 'only .dxf is accepted at present' },
      415,
    );
  }

  const path = `${tmpRoot()}/upload-${crypto.randomUUID()}.${extension}`;

  await Bun.write(path, file);
  try {
    return await handler(path, file.name);
  } finally {
    await Bun.file(path)
      .delete()
      .catch(() => {
        /* already gone */
      });
  }
}

function tmpRoot(): string {
  return process.env.OD_TMPDIR ?? '/tmp';
}
