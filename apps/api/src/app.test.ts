import { describe, expect, test } from 'bun:test';
import { createApp } from './app.ts';

/**
 * These run against the real `od` binary when it is present, and check the
 * contract when it is not. Both matter: a developer without a Rust toolchain
 * should still get a sensible failure rather than a stack trace.
 */

const app = createApp();

async function engineIsAvailable(): Promise<boolean> {
  const res = await app.request('/health');
  return res.status === 200;
}

const hasEngine = await engineIsAvailable();

describe('health', () => {
  test('reports whether the engine is reachable', async () => {
    const res = await app.request('/health');
    const body = (await res.json()) as { status: string; engine: { available: boolean } };
    expect(['ok', 'degraded']).toContain(body.status);
    expect(body.engine.available).toBe(res.status === 200);
  });
});

describe('errors', () => {
  test('unknown routes return a structured 404', async () => {
    const res = await app.request('/api/nothing-here');
    expect(res.status).toBe(404);
    expect(await res.json()).toEqual({ error: 'not found' });
  });

  test('an upload with no file is refused with an explanation', async () => {
    const res = await app.request('/api/drawings/inspect', {
      method: 'POST',
      body: new FormData(),
    });
    expect(res.status).toBe(400);
    const body = (await res.json()) as { error: string; detail?: string };
    expect(body.error).toBe('no file');
    expect(body.detail).toContain('file');
  });

  test('a non-DXF upload is refused before it reaches the engine', async () => {
    const form = new FormData();
    form.set('file', new File(['nope'], 'drawing.rvt'));
    const res = await app.request('/api/drawings/inspect', {
      method: 'POST',
      body: form,
    });
    expect(res.status).toBe(415);
  });
});

describe.if(hasEngine)('catalogue', () => {
  test('lists parts', async () => {
    const res = await app.request('/api/parts');
    expect(res.status).toBe(200);
    const body = (await res.json()) as { parts: unknown[]; total: number };
    expect(body.total).toBeGreaterThan(40);
    expect(body.parts.length).toBe(body.total);
  });

  test('searches by Japanese name', async () => {
    const res = await app.request('/api/parts?q=' + encodeURIComponent('エルボ'));
    const body = (await res.json()) as { total: number };
    expect(body.total).toBeGreaterThan(0);
  });

  test('filters by category', async () => {
    const res = await app.request('/api/parts?category=valve');
    const body = (await res.json()) as { parts: { category: string }[] };
    expect(body.parts.length).toBeGreaterThan(0);
    expect(body.parts.every((p) => p.category === 'valve')).toBe(true);
  });

  test('builds a part at the requested size', async () => {
    const res = await app.request('/api/parts/duct.elbow.rect.90?W=500&H=300');
    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      parameters: Record<string, number>;
      ports: { profile: { kind: string; w?: number } }[];
    };
    expect(body.parameters.W).toBe(500);
    expect(body.ports).toHaveLength(2);
    expect(body.ports[0]?.profile.w).toBe(500);
  });

  test('rejects a non-numeric parameter', async () => {
    const res = await app.request('/api/parts/duct.elbow.rect.90?W=wide');
    expect(res.status).toBe(400);
  });

  test('reports an unknown part as an engine failure, not a crash', async () => {
    const res = await app.request('/api/parts/no.such.part');
    expect(res.status).toBe(422);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBe('drawing engine failed');
  });

  test('lists systems and specs', async () => {
    const systems = (await (await app.request('/api/systems')).json()) as {
      systems: unknown[];
    };
    expect(systems.systems.length).toBeGreaterThan(15);

    const specs = (await (await app.request('/api/specs')).json()) as {
      specs: unknown[];
    };
    expect(specs.specs.length).toBeGreaterThanOrEqual(4);

    const one = await app.request('/api/specs/spec.pipe.sgp');
    expect(one.status).toBe(200);
    const spec = (await one.json()) as { sizes: unknown[] };
    expect(spec.sizes.length).toBeGreaterThan(10);
  });
});

describe.if(hasEngine)('drawings', () => {
  const dxf =
    '0\nSECTION\n2\nENTITIES\n' +
    '0\nLINE\n8\nM-DUCT-SA\n10\n0.0\n20\n0.0\n11\n5000.0\n21\n0.0\n' +
    '0\nCIRCLE\n8\nM-DUCT-SA\n10\n2500.0\n20\n1000.0\n40\n250.0\n' +
    '0\nENDSEC\n0\nEOF\n';

  function upload(name = 'plan.dxf'): FormData {
    const form = new FormData();
    form.set('file', new File([dxf], name, { type: 'application/dxf' }));
    return form;
  }

  test('inspects an uploaded drawing', async () => {
    const res = await app.request('/api/drawings/inspect', {
      method: 'POST',
      body: upload(),
    });
    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      entities: number;
      layer_names: string[];
      entities_by_type: Record<string, number>;
    };
    expect(body.entities).toBe(2);
    expect(body.layer_names).toContain('M-DUCT-SA');
    expect(body.entities_by_type.line).toBe(1);
  });

  test('checks a drawing against the Japanese rule set', async () => {
    const res = await app.request('/api/drawings/check?rules=jp', {
      method: 'POST',
      body: upload(),
    });
    expect(res.status).toBe(200);
    const body = (await res.json()) as { passed: boolean; findings: { rule: string }[] };
    expect(body.passed).toBe(true);
    expect(Array.isArray(body.findings)).toBe(true);
  });

  test('saves as .odc by default — the format that keeps everything', async () => {
    const res = await app.request('/api/drawings/convert', {
      method: 'POST',
      body: upload('第1階平面図.dxf'),
    });
    expect(res.status).toBe(200);
    expect(res.headers.get('content-type')).toBe(
      'application/vnd.opendraft.document+zip',
    );
    expect(res.headers.get('content-disposition')).toContain('.odc');

    const summary = JSON.parse(res.headers.get('x-opendraft-summary') ?? '{}') as {
      entities: number;
      losses: string[];
    };
    expect(summary.entities).toBe(2);
    expect(summary.losses).toEqual([]);

    // A ZIP, whatever else it is.
    const bytes = new Uint8Array(await res.arrayBuffer());
    expect(Array.from(bytes.slice(0, 2))).toEqual([0x50, 0x4b]);
  });

  test('converts to DXF when asked, and says what that costs', async () => {
    const res = await app.request('/api/drawings/convert?to=dxf', {
      method: 'POST',
      body: upload(),
    });
    expect(res.status).toBe(200);
    expect(res.headers.get('content-type')).toBe('application/dxf');

    const text = await res.text();
    expect(text).toContain('M-DUCT-SA');
    expect(text.endsWith('0\nEOF\n')).toBe(true);
  });

  test('a .odc upload round-trips back through the service', async () => {
    const saved = await app.request('/api/drawings/convert', {
      method: 'POST',
      body: upload(),
    });
    const odc = new File([await saved.arrayBuffer()], 'plan.odc');

    const form = new FormData();
    form.set('file', odc);
    const res = await app.request('/api/drawings/inspect', {
      method: 'POST',
      body: form,
    });

    expect(res.status).toBe(200);
    const body = (await res.json()) as { entities: number; format: string };
    expect(body.entities).toBe(2);
    expect(body.format).toBe('odc');
  });

  test('renders to SVG for display', async () => {
    const res = await app.request('/api/drawings/render', {
      method: 'POST',
      body: upload(),
    });
    expect(res.status).toBe(200);
    // The content type matters: the client shows this in an <img>, where
    // scripts inside someone else's drawing cannot run.
    expect(res.headers.get('content-type')).toContain('image/svg+xml');
    expect(res.headers.get('x-opendraft-entities')).toBe('2');

    const svg = await res.text();
    expect(svg.startsWith('<svg')).toBe(true);
    expect(svg).toContain('<line');
    expect(svg).toContain('<circle');
  });

  test('rendering honours the layer filter and the dark canvas', async () => {
    const filtered = await app.request(
      '/api/drawings/render?layers=' + encodeURIComponent('M-DUCT-SA'),
      { method: 'POST', body: upload() },
    );
    expect((await filtered.text()).match(/<line/g)?.length).toBe(1);

    const empty = await app.request('/api/drawings/render?layers=NOT-A-LAYER', {
      method: 'POST',
      body: upload(),
    });
    const svg = await empty.text();
    expect(svg).not.toContain('<line');

    const dark = await app.request('/api/drawings/render?dark=1', {
      method: 'POST',
      body: upload(),
    });
    expect(await dark.text()).toContain('#141d26');
  });

  test('an unknown target format is refused', async () => {
    const res = await app.request('/api/drawings/convert?to=rvt', {
      method: 'POST',
      body: upload(),
    });
    expect(res.status).toBe(400);
  });

  test('rendering reports the view box a click needs to map back to a drawing coordinate', async () => {
    const res = await app.request('/api/drawings/render', {
      method: 'POST',
      body: upload(),
    });
    const box = JSON.parse(res.headers.get('x-opendraft-viewbox') ?? 'null') as
      [number, number, number, number] | null;
    expect(box).not.toBeNull();
    expect(box).toHaveLength(4);
  });

  test('editing draws a line and hands back the updated document and render', async () => {
    const form = upload();
    form.set(
      'command',
      JSON.stringify({
        kind: 'add_line',
        layer: 'A-TEST',
        a: { x: 0, y: 0, z: 0 },
        b: { x: 3600, y: 0, z: 0 },
      }),
    );
    const res = await app.request('/api/drawings/edit', { method: 'POST', body: form });
    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      created: string[];
      modified: string[];
      deleted: string[];
      document: string;
      svg: string;
      view_box: [number, number, number, number];
    };
    expect(body.created).toHaveLength(1);
    expect(body.modified).toEqual([]);
    expect(body.svg).toContain('<line');
    expect(body.view_box).toHaveLength(4);

    // The returned document really was edited: uploading it back shows 3
    // entities now, not the 2 the fixture started with. It comes back in the
    // same format it was uploaded in — DXF here, since `upload()` sends DXF.
    const edited = new File([Buffer.from(body.document, 'base64')], 'edited.dxf');
    const again = new FormData();
    again.set('file', edited);
    const inspected = await app.request('/api/drawings/inspect', {
      method: 'POST',
      body: again,
    });
    const inspection = (await inspected.json()) as { entities: number };
    expect(inspection.entities).toBe(3);
  });

  test('moving and deleting the entity just created round-trips through undo-free edits', async () => {
    const draw = upload();
    draw.set(
      'command',
      JSON.stringify({
        kind: 'add_line',
        layer: '0',
        a: { x: 0, y: 0, z: 0 },
        b: { x: 1000, y: 0, z: 0 },
      }),
    );
    const drawn = (await (
      await app.request('/api/drawings/edit', { method: 'POST', body: draw })
    ).json()) as { created: string[]; document: string };
    const id = drawn.created[0];
    if (id === undefined) throw new Error('expected a created id');

    const moved = new FormData();
    moved.set('file', new File([Buffer.from(drawn.document, 'base64')], 'a.dxf'));
    moved.set(
      'command',
      JSON.stringify({ kind: 'move_entities', ids: [id], delta: { x: 0, y: 500, z: 0 } }),
    );
    const moveRes = await app.request('/api/drawings/edit', {
      method: 'POST',
      body: moved,
    });
    expect(moveRes.status).toBe(200);
    const movedBody = (await moveRes.json()) as { modified: string[]; document: string };
    expect(movedBody.modified).toEqual([id]);

    const deleted = new FormData();
    deleted.set('file', new File([Buffer.from(movedBody.document, 'base64')], 'b.dxf'));
    deleted.set('command', JSON.stringify({ kind: 'delete_entities', ids: [id] }));
    const deleteRes = await app.request('/api/drawings/edit', {
      method: 'POST',
      body: deleted,
    });
    expect(deleteRes.status).toBe(200);
    const deletedBody = (await deleteRes.json()) as { deleted: string[] };
    expect(deletedBody.deleted).toEqual([id]);
  });

  test('an edit naming a nonexistent entity fails as an engine error, not a crash', async () => {
    const form = upload();
    form.set('command', JSON.stringify({ kind: 'delete_entities', ids: ['0-9999'] }));
    const res = await app.request('/api/drawings/edit', { method: 'POST', body: form });
    expect(res.status).toBe(422);
  });

  test('a malformed command is rejected before it reaches the engine', async () => {
    const form = upload();
    form.set('command', JSON.stringify({ kind: 'frobnicate' }));
    const res = await app.request('/api/drawings/edit', { method: 'POST', body: form });
    expect(res.status).toBe(400);
    const body = (await res.json()) as { error: string };
    expect(body.error).toBe('bad command');
  });
});
