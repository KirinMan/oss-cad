import { Hono, type Context } from 'hono';
import { cors } from 'hono/cors';
import { logger } from 'hono/logger';
import { z } from 'zod';
import {
  checkReportSchema,
  commandSchema,
  conversionSchema,
  editReportSchema,
  mepCheckReportSchema,
  mepPlaceReportSchema,
  mepPortsReportSchema,
  mepRouteReportSchema,
  mepTakeoffReportSchema,
  point3Schema,
  profileSchema,
  queryReportSchema,
  renderSchema,
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

/** Formats the engine can read and write. Kept in step with crates/od-cli/src/load.rs. */
const READABLE = ['dxf', 'odc'] as const;
const WRITABLE: string[] = ['odc', 'dxf', 'json'];

const CONTENT_TYPES: Record<string, string> = {
  // The container is a ZIP, and saying so lets a browser and a proxy handle it
  // sensibly even where the vendor type means nothing to them.
  odc: 'application/vnd.opendraft.document+zip',
  dxf: 'application/dxf',
  json: 'application/json',
};

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

  app.post('/api/drawings/render', (c) =>
    withUpload(c, async (path) => {
      const out = `${path}.svg`;
      const args = ['render', path, out];
      if (c.req.query('dark') === '1') args.push('--dark');
      const layers = c.req.query('layers');
      if (layers) args.push('--layers', layers);
      const window = c.req.query('window');
      if (window) args.push('--window', window);
      const layout = c.req.query('layout');
      if (layout) args.push('--layout', layout);

      const report = await od(renderSchema, args);
      const svg = await Bun.file(out).text();
      await Bun.file(out).delete();

      return new Response(svg, {
        headers: {
          // `image/svg+xml` is deliberate: the client displays this in an
          // <img>, where scripts do not run. A drawing is someone else's file,
          // and inlining it into the page would make its text a script vector.
          'content-type': 'image/svg+xml; charset=utf-8',
          'x-opendraft-entities': String(report.entities),
          // What an editing canvas needs to map a click on the image back to
          // a drawing coordinate — see od_io_svg::ViewBox.
          'x-opendraft-viewbox': JSON.stringify(report.view_box),
        },
      });
    }),
  );

  // Finds entities near a point, through the spatial index — how an editing
  // canvas turns a click into "which entity did that mean" without ever
  // hit-testing against the rendered SVG itself (untrusted content, never
  // inlined into the page).
  app.post('/api/drawings/query', (c) =>
    withUpload(c, async (path) => {
      const near = c.req.query('near');
      if (!near) {
        return c.json<ApiError>(
          { error: 'no point', detail: 'give a point to search near: ?near=x,y' },
          400,
        );
      }
      const count = c.req.query('count') ?? '5';
      const report = await od(queryReportSchema, [
        'query',
        path,
        '--near',
        near,
        '--count',
        count,
      ]);
      return c.json(report);
    }),
  );

  // Applies one edit command (ADR-006) and hands back the updated document
  // alongside a fresh render, so an editing canvas gets everything it needs
  // for its next frame in one round trip rather than an edit request followed
  // by a separate render request. Like every other drawing endpoint, nothing
  // outlives the request — the client holds the document between edits, not
  // this service.
  app.post('/api/drawings/edit', async (c) => {
    const form = await c.req.formData().catch(() => null);
    const file = form?.get('file');
    const commandRaw = form?.get('command');
    if (!(file instanceof File)) {
      return c.json<ApiError>(
        { error: 'no file', detail: 'send the drawing as multipart form field `file`' },
        400,
      );
    }
    if (typeof commandRaw !== 'string') {
      return c.json<ApiError>(
        {
          error: 'no command',
          detail: 'send the edit command as multipart form field `command`, as JSON',
        },
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
    const extension = READABLE.find((ext) => file.name.toLowerCase().endsWith(`.${ext}`));
    if (!extension) {
      return c.json<ApiError>(
        {
          error: 'unsupported format',
          detail: `accepted: ${READABLE.map((e) => `.${e}`).join(', ')}`,
        },
        415,
      );
    }

    let commandJson: unknown;
    try {
      commandJson = JSON.parse(commandRaw);
    } catch {
      return c.json<ApiError>(
        { error: 'bad command', detail: '`command` is not valid JSON' },
        400,
      );
    }
    const command = commandSchema.safeParse(commandJson);
    if (!command.success) {
      return c.json<ApiError>(
        {
          error: 'bad command',
          detail: command.error.issues
            .map((i) => `${i.path.join('.')}: ${i.message}`)
            .join('; '),
        },
        400,
      );
    }

    const inPath = `${tmpRoot()}/edit-in-${crypto.randomUUID()}.${extension}`;
    const outPath = `${tmpRoot()}/edit-out-${crypto.randomUUID()}.${extension}`;
    const svgPath = `${outPath}.svg`;
    await Bun.write(inPath, file);
    try {
      const report = await od(editReportSchema, [
        'edit',
        inPath,
        outPath,
        '--command',
        JSON.stringify(command.data),
        '--render',
        svgPath,
      ]);
      const [document, svg] = await Promise.all([
        Bun.file(outPath).arrayBuffer(),
        Bun.file(svgPath).text(),
      ]);
      return c.json({
        created: report.created,
        modified: report.modified,
        deleted: report.deleted,
        document: Buffer.from(document).toString('base64'),
        svg,
        view_box: report.render?.view_box ?? null,
      });
    } finally {
      await Promise.all(
        [inPath, outPath, svgPath].map((p) =>
          Bun.file(p)
            .delete()
            .catch(() => {
              /* already gone */
            }),
        ),
      );
    }
  });

  // Draws a route through od-domain-mep, auto-inserting the fittings any 90°
  // bends need, and hands back the updated document and a fresh render — the
  // same envelope /api/drawings/edit uses, for a route instead of a single
  // Command. A route cannot be a Command: od-core must never learn what a
  // "system" or a "spec" is (rule 1), so this shells out to `od mep route`,
  // the domain's own entry point, rather than /api/drawings/edit's generic
  // one.
  app.post('/api/mep/route', async (c) => {
    const form = await c.req.formData().catch(() => null);
    const file = form?.get('file');
    const system = form?.get('system');
    const spec = form?.get('spec');
    const profileRaw = form?.get('profile');
    const pathRaw = form?.get('path');
    if (!(file instanceof File)) {
      return c.json<ApiError>(
        { error: 'no file', detail: 'send the drawing as multipart form field `file`' },
        400,
      );
    }
    if (
      typeof system !== 'string' ||
      typeof spec !== 'string' ||
      typeof profileRaw !== 'string' ||
      typeof pathRaw !== 'string'
    ) {
      return c.json<ApiError>(
        {
          error: 'missing field',
          detail:
            'send `system`, `spec`, `profile` (JSON) and `path` (JSON) alongside `file`',
        },
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
    const extension = READABLE.find((ext) => file.name.toLowerCase().endsWith(`.${ext}`));
    if (!extension) {
      return c.json<ApiError>(
        {
          error: 'unsupported format',
          detail: `accepted: ${READABLE.map((e) => `.${e}`).join(', ')}`,
        },
        415,
      );
    }

    let profileJson: unknown;
    let pathJson: unknown;
    try {
      profileJson = JSON.parse(profileRaw);
      pathJson = JSON.parse(pathRaw);
    } catch {
      return c.json<ApiError>(
        { error: 'bad request', detail: '`profile` and `path` must be JSON' },
        400,
      );
    }
    const profile = profileSchema.safeParse(profileJson);
    if (!profile.success) {
      return c.json<ApiError>(
        {
          error: 'bad profile',
          detail: profile.error.issues
            .map((i) => `${i.path.join('.')}: ${i.message}`)
            .join('; '),
        },
        400,
      );
    }
    if (profile.data.kind === 'oval' || profile.data.kind === 'terminal') {
      return c.json<ApiError>(
        {
          error: 'bad profile',
          detail: `routing does not support a ${profile.data.kind} profile — use rect or round`,
        },
        400,
      );
    }
    const path = z.array(point3Schema).min(2).safeParse(pathJson);
    if (!path.success) {
      return c.json<ApiError>(
        { error: 'bad path', detail: 'a route needs at least two {x, y, z} points' },
        400,
      );
    }

    const profileArg =
      profile.data.kind === 'rect'
        ? `rect:${profile.data.w},${profile.data.h}`
        : `round:${profile.data.d}`;
    const pathArg = path.data.map((p) => `${p.x},${p.y},${p.z}`).join(';');

    const inPath = `${tmpRoot()}/route-in-${crypto.randomUUID()}.${extension}`;
    const outPath = `${tmpRoot()}/route-out-${crypto.randomUUID()}.${extension}`;
    const svgPath = `${outPath}.svg`;
    await Bun.write(inPath, file);
    try {
      const report = await od(mepRouteReportSchema, [
        'mep',
        'route',
        inPath,
        outPath,
        '--system',
        system,
        '--spec',
        spec,
        '--profile',
        profileArg,
        '--path',
        pathArg,
        '--render',
        svgPath,
      ]);
      const [document, svg] = await Promise.all([
        Bun.file(outPath).arrayBuffer(),
        Bun.file(svgPath).text(),
      ]);
      return c.json({
        segments: report.segments,
        fittings: report.fittings,
        document: Buffer.from(document).toString('base64'),
        svg,
        view_box: report.render?.view_box ?? null,
      });
    } finally {
      await Promise.all(
        [inPath, outPath, svgPath].map((p) =>
          Bun.file(p)
            .delete()
            .catch(() => {
              /* already gone */
            }),
        ),
      );
    }
  });

  // Places one piece of equipment through od-domain-mep and hands back the
  // updated document and a fresh render — the same envelope
  // /api/drawings/edit uses, for a placement instead of a single Command. A
  // placement cannot be a Command: od-core must never learn what a "system"
  // or a catalogue part id is (rule 1), so this shells out to `od mep place`
  // rather than /api/drawings/edit's generic one.
  app.post('/api/mep/place', async (c) => {
    const form = await c.req.formData().catch(() => null);
    const file = form?.get('file');
    const part = form?.get('part');
    const positionRaw = form?.get('position');
    const rotationRaw = form?.get('rotation');
    const mirror = form?.get('mirror');
    const system = form?.get('system');
    const setRaw = form?.get('set');
    if (!(file instanceof File)) {
      return c.json<ApiError>(
        { error: 'no file', detail: 'send the drawing as multipart form field `file`' },
        400,
      );
    }
    if (typeof part !== 'string' || typeof positionRaw !== 'string') {
      return c.json<ApiError>(
        {
          error: 'missing field',
          detail: 'send `part` and `position` (JSON) alongside `file`',
        },
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
    const extension = READABLE.find((ext) => file.name.toLowerCase().endsWith(`.${ext}`));
    if (!extension) {
      return c.json<ApiError>(
        {
          error: 'unsupported format',
          detail: `accepted: ${READABLE.map((e) => `.${e}`).join(', ')}`,
        },
        415,
      );
    }

    let positionJson: unknown;
    try {
      positionJson = JSON.parse(positionRaw);
    } catch {
      return c.json<ApiError>(
        { error: 'bad request', detail: '`position` must be JSON' },
        400,
      );
    }
    const position = point3Schema.safeParse(positionJson);
    if (!position.success) {
      return c.json<ApiError>(
        { error: 'bad position', detail: 'position must be an {x, y, z} object' },
        400,
      );
    }
    let set: string[] = [];
    if (typeof setRaw === 'string') {
      let setJson: unknown;
      try {
        setJson = JSON.parse(setRaw);
      } catch {
        return c.json<ApiError>(
          { error: 'bad request', detail: '`set` must be JSON' },
          400,
        );
      }
      const parsedSet = z.array(z.string()).safeParse(setJson);
      if (!parsedSet.success) {
        return c.json<ApiError>(
          { error: 'bad set', detail: '`set` must be an array of "NAME=VALUE" strings' },
          400,
        );
      }
      set = parsedSet.data;
    }

    const inPath = `${tmpRoot()}/place-in-${crypto.randomUUID()}.${extension}`;
    const outPath = `${tmpRoot()}/place-out-${crypto.randomUUID()}.${extension}`;
    const svgPath = `${outPath}.svg`;

    const args = [
      'mep',
      'place',
      inPath,
      outPath,
      '--part',
      part,
      '--x',
      String(position.data.x),
      '--y',
      String(position.data.y),
      '--z',
      String(position.data.z),
    ];
    if (typeof rotationRaw === 'string' && rotationRaw !== '') {
      args.push('--rotation', rotationRaw);
    }
    if (mirror === '1' || mirror === 'true') {
      args.push('--mirror');
    }
    if (typeof system === 'string' && system !== '') {
      args.push('--system', system);
    }
    for (const pair of set) {
      args.push('--set', pair);
    }
    args.push('--render', svgPath);

    await Bun.write(inPath, file);
    try {
      const report = await od(mepPlaceReportSchema, args);
      const [document, svg] = await Promise.all([
        Bun.file(outPath).arrayBuffer(),
        Bun.file(svgPath).text(),
      ]);
      return c.json({
        created: report.created,
        document: Buffer.from(document).toString('base64'),
        svg,
        view_box: report.render?.view_box ?? null,
      });
    } finally {
      await Promise.all(
        [inPath, outPath, svgPath].map((p) =>
          Bun.file(p)
            .delete()
            .catch(() => {
              /* already gone */
            }),
        ),
      );
    }
  });

  // Read-only reports over the MEP connection graph — no document comes
  // back, so these need none of `/api/mep/route`'s or `/api/mep/place`'s
  // web-friendly envelope, and follow `/api/drawings/check` instead: the
  // CLI's own report shape doubles as the API response.
  app.post('/api/mep/takeoff', (c) =>
    withUpload(c, async (path) => {
      const report = await od(mepTakeoffReportSchema, ['mep', 'takeoff', path]);
      return c.json(report);
    }),
  );

  app.post('/api/mep/check', (c) =>
    withUpload(c, async (path) => {
      const report = await od(mepCheckReportSchema, ['mep', 'check', path]);
      return c.json(report);
    }),
  );

  // Every port in the document, not just the unconnected ones `/api/mep/check`
  // reports — what the editing canvas offers up as snap targets (F-104).
  app.post('/api/mep/ports', (c) =>
    withUpload(c, async (path) => {
      const report = await od(mepPortsReportSchema, ['mep', 'ports', path]);
      return c.json(report);
    }),
  );

  app.post('/api/drawings/convert', (c) =>
    withUpload(c, async (path, name) => {
      // `.odc` is the default target because it is the only format that keeps
      // the whole document; asking for DXF is asking for an exchange copy.
      const target = c.req.query('to') ?? 'odc';
      if (!WRITABLE.includes(target)) {
        return c.json<ApiError>(
          {
            error: 'unsupported target',
            detail: `to must be one of ${WRITABLE.join(', ')}`,
          },
          400,
        );
      }
      const out = `${path}.${target}`;
      const report = await od(conversionSchema, ['convert', path, out]);
      const bytes = await Bun.file(out).arrayBuffer();
      await Bun.file(out).delete();

      const base = name.replace(/\.[^./\\]+$/, '');
      return new Response(bytes, {
        headers: {
          'content-type': CONTENT_TYPES[target] ?? 'application/octet-stream',
          'content-disposition': `attachment; filename="${encodeURIComponent(base)}.${target}"`,
          // The summary rides along in a header so a client can report what
          // happened without a second request — including what the target
          // format could not carry, which the user needs to see *before* they
          // treat the download as their copy of record.
          'x-opendraft-summary': JSON.stringify({
            entities: report.entities,
            layers: report.layers,
            preserved: report.preserved_entities,
            warnings: report.warnings,
            losses: report.losses,
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

  const extension = READABLE.find((ext) => file.name.toLowerCase().endsWith(`.${ext}`));
  if (!extension) {
    return c.json<ApiError>(
      {
        error: 'unsupported format',
        detail: `accepted: ${READABLE.map((e) => `.${e}`).join(', ')}`,
      },
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
