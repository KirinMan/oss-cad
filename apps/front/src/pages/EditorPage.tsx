import { useEffect, useRef, useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import type { Command } from '@opendraft/shared';
import { editDrawing, renderDrawingWithViewBox } from '../api.ts';

/**
 * The first real editing canvas (Phase 2, `docs/06-roadmap.md`).
 *
 * Every edit is a `Command` (ADR-006, `docs/02-architecture.md`): a click here
 * builds the same JSON `od edit --command` and a future script would. There is
 * no server-side session — this component holds the current document as a
 * `File` in memory and sends the whole thing back on every edit, so the
 * service keeps the same "nothing outlives the request" guarantee the rest of
 * the API makes. Undo is therefore just "go back to the previous snapshot",
 * kept as a client-side stack rather than calling into `od-core`'s own
 * undo history — there is only ever one open edit in flight, so nothing is
 * lost by keeping it this simple.
 *
 * Only one tool exists so far: draw a line. `od-core::Command` already
 * supports moving and deleting entities; wiring those in needs a way to pick
 * an entity, which needs hit-testing against real geometry, not the rendered
 * `<img>` — a drawing is untrusted content and is never inlined into the page
 * (CLAUDE.md), so there is no clickable DOM to hit-test against. That is the
 * next slice, not this one.
 */

interface Snapshot {
  file: File;
  url: string;
  viewBox: [number, number, number, number];
}

const DEFAULT_LAYER = 'A-EDIT';

export function EditorPage() {
  const [current, setCurrent] = useState<Snapshot | null>(null);
  const [history, setHistory] = useState<Snapshot[]>([]);
  const [pendingStart, setPendingStart] = useState<{ x: number; y: number } | null>(null);
  const [layer, setLayer] = useState(DEFAULT_LAYER);
  const [error, setError] = useState<string | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  // Every snapshot owns an object URL; release whichever ones this component
  // itself created once nothing points at them any more.
  useEffect(() => {
    return () => {
      if (current) URL.revokeObjectURL(current.url);
      for (const s of history) URL.revokeObjectURL(s.url);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const open = useMutation({
    mutationFn: async (file: File) => {
      const { url, viewBox } = await renderDrawingWithViewBox(file);
      return { file, url, viewBox };
    },
    onSuccess: (snap) => {
      setHistory([]);
      setPendingStart(null);
      setError(null);
      setCurrent(snap);
    },
    onError: (e: Error) => setError(e.message),
  });

  const edit = useMutation({
    mutationFn: async (command: Command) => {
      if (!current) throw new Error('先に図面を開いてください');
      return editDrawing(current.file, command);
    },
    onSuccess: (result) => {
      setError(null);
      setHistory((h) => (current ? [...h, current] : h));
      setCurrent({ file: result.document, url: result.url, viewBox: result.viewBox });
    },
    onError: (e: Error) => setError(e.message),
  });

  function undo() {
    setHistory((h) => {
      const previous = h[h.length - 1];
      if (!previous || !current) return h;
      URL.revokeObjectURL(current.url);
      setCurrent(previous);
      return h.slice(0, -1);
    });
    setPendingStart(null);
  }

  /**
   * Screen pixel → drawing millimetre, inverting the same `object-contain`
   * fit the `<img>` renders with. `naturalWidth`/`naturalHeight` are not used
   * here — an SVG with a `viewBox` but no `width`/`height` attribute has no
   * reliable natural size across browsers, but its aspect ratio is exact, and
   * `object-contain` only ever needs the aspect ratio to decide the fit.
   */
  function toWorld(clientX: number, clientY: number): { x: number; y: number } | null {
    const box = containerRef.current?.getBoundingClientRect();
    if (!box || !current) return null;
    const [vbX, vbY, vbW, vbH] = current.viewBox;

    const scale = Math.min(box.width / vbW, box.height / vbH);
    const offsetX = (box.width - vbW * scale) / 2;
    const offsetY = (box.height - vbH * scale) / 2;

    const svgX = (clientX - box.left - offsetX) / scale + vbX;
    const svgY = (clientY - box.top - offsetY) / scale + vbY;
    // The renderer draws inside a `scale(1 -1)` group (CLAUDE.md: coordinates
    // are Y-up drawing space), so the SVG's own Y is inverted from the
    // drawing's.
    return { x: svgX, y: -svgY };
  }

  function onCanvasClick(e: React.MouseEvent) {
    if (!current || edit.isPending) return;
    const point = toWorld(e.clientX, e.clientY);
    if (!point) return;

    if (!pendingStart) {
      setPendingStart(point);
      return;
    }
    edit.mutate({
      kind: 'add_line',
      layer,
      a: { x: pendingStart.x, y: pendingStart.y, z: 0 },
      b: { x: point.x, y: point.y, z: 0 },
    });
    setPendingStart(null);
  }

  // Where the pending start point sits on screen, for the marker — the
  // forward direction of `toWorld`, not its inverse; both need to agree, so
  // this is deliberately the mirror-image computation of the click handler.
  // Computed in an effect rather than during render: it reads the
  // container's live layout through the ref, which render must not do.
  const [marker, setMarker] = useState<{ left: number; top: number } | null>(null);
  useEffect(() => {
    const box = containerRef.current?.getBoundingClientRect();
    if (!pendingStart || !current || !box) {
      setMarker(null);
      return;
    }
    const [vbX, vbY, vbW, vbH] = current.viewBox;
    const scale = Math.min(box.width / vbW, box.height / vbH);
    const offsetX = (box.width - vbW * scale) / 2;
    const offsetY = (box.height - vbH * scale) / 2;
    const svgX = pendingStart.x;
    const svgY = -pendingStart.y;
    setMarker({
      left: (svgX - vbX) * scale + offsetX,
      top: (svgY - vbY) * scale + offsetY,
    });
  }, [pendingStart, current]);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">図面を編集する</h1>
        <p className="mt-1 max-w-2xl text-sm text-ink-muted">
          クリックで始点、もう一度クリックで終点を指定して線分を描きます。ファイルはこのブラウザだけが保持し、編集のたびに送り直されます
          — サーバには残りません。
        </p>
      </div>

      <div className="flex flex-wrap items-center gap-4">
        <label className="cursor-pointer rounded border border-rule bg-paper-raised px-4 py-2 text-sm transition-colors hover:border-accent">
          <input
            type="file"
            accept=".dxf,.odc"
            className="sr-only"
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) open.mutate(f);
            }}
          />
          図面を開く（DXF / ODC）
        </label>
        {current && (
          <span className="text-sm text-ink-muted">
            {current.file.name}
            <span className="ml-2 tabular">
              {(current.file.size / 1024).toFixed(0)} KB
            </span>
          </span>
        )}
        <label className="ml-auto flex items-center gap-2 text-sm">
          レイヤ
          <input
            value={layer}
            onChange={(e) => setLayer(e.target.value)}
            className="w-32 rounded border border-rule bg-paper-raised px-2 py-1 font-mono text-xs"
          />
        </label>
      </div>

      {open.isPending && <p className="text-sm text-ink-muted">読み込み中…</p>}
      {error && (
        <p className="rounded border border-sys-fire/40 bg-sys-fire/10 px-4 py-3 text-sm">
          {error}
        </p>
      )}

      {current && (
        <div className="space-y-2">
          <div className="flex flex-wrap items-center gap-3 text-xs text-ink-muted">
            <span className="rounded border border-rule bg-paper-raised px-2 py-1 text-ink">
              線分ツール
            </span>
            {pendingStart ? (
              <span>
                始点 ({pendingStart.x.toFixed(0)}, {pendingStart.y.toFixed(0)}) —
                終点をクリック
              </span>
            ) : (
              <span>始点をクリック</span>
            )}
            <button
              type="button"
              onClick={undo}
              disabled={history.length === 0 || edit.isPending}
              className="ml-auto rounded border border-rule px-2 py-1 text-ink disabled:opacity-40"
            >
              元に戻す（{history.length}）
            </button>
            {edit.isPending && <span>適用中…</span>}
          </div>

          <div
            ref={containerRef}
            onClick={onCanvasClick}
            className="relative h-[32rem] overflow-hidden rounded border border-rule bg-paper-raised"
            style={{ cursor: edit.isPending ? 'wait' : 'crosshair' }}
          >
            <img
              src={current.url}
              alt="編集中の図面"
              draggable={false}
              className="absolute top-0 left-0 h-full w-full object-contain select-none"
            />
            {marker && (
              <div
                className="pointer-events-none absolute h-2.5 w-2.5 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-accent bg-paper-raised"
                style={marker}
              />
            )}
          </div>
        </div>
      )}
    </div>
  );
}
