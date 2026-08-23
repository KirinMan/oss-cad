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

  test('an unknown target format is refused', async () => {
    const res = await app.request('/api/drawings/convert?to=rvt', {
      method: 'POST',
      body: upload(),
    });
    expect(res.status).toBe(400);
  });
});
