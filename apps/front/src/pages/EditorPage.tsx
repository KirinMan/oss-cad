import { useEffect, useRef, useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import type { Command, QueryHit } from '@opendraft/shared';
import { editDrawing, queryNear, renderDrawingWithViewBox } from '../api.ts';

/**
 * The editing canvas (Phase 2, `docs/06-roadmap.md`).
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
 * Selecting an entity, and object snap, both go through
 * `POST /api/drawings/query` — the same spatial index `od query --near`
 * already answers "what is near here" through — rather than hit-testing the
 * rendered `<img>`, which is untrusted content and is never inlined into the
 * page (CLAUDE.md), so there is no clickable DOM to hit-test against in the
 * first place. Snap candidates are re-queried on a debounce while the pointer
 * moves, not on every `mousemove` — a network round trip per pixel would
 * make the canvas feel like it is wading through mud.
 */

interface Snapshot {
  file: File;
  url: string;
  viewBox: [number, number, number, number];
}

type Tool = 'line' | 'select';

const DEFAULT_LAYER = 'A-EDIT';
/** How close, in screen pixels, a candidate has to be before it "grabs" the cursor. */
const SNAP_RADIUS_PX = 12;
/** How long to wait after the pointer stops before asking the server what is nearby. */
const SNAP_DEBOUNCE_MS = 100;

export function EditorPage() {
  const [current, setCurrent] = useState<Snapshot | null>(null);
  const [history, setHistory] = useState<Snapshot[]>([]);
  const [tool, setTool] = useState<Tool>('line');
  const [pendingStart, setPendingStart] = useState<{ x: number; y: number } | null>(null);
  const [selection, setSelection] = useState<QueryHit | null>(null);
  const [moving, setMoving] = useState(false);
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

  function resetInteraction() {
    setPendingStart(null);
    setSelection(null);
    setMoving(false);
    setHoverPoint(null);
    setSnapCandidates([]);
  }

  const open = useMutation({
    mutationFn: async (file: File) => {
      const { url, viewBox } = await renderDrawingWithViewBox(file);
      return { file, url, viewBox };
    },
    onSuccess: (snap) => {
      setHistory([]);
      resetInteraction();
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
      // The cached selection's bounds are for the document before this edit;
      // stale bounds would draw the highlight in the wrong place.
      setSelection(null);
      setMoving(false);
    },
    onError: (e: Error) => setError(e.message),
  });

  const pick = useMutation({
    mutationFn: async (point: { x: number; y: number }) => {
      if (!current) throw new Error('先に図面を開いてください');
      const [hit] = await queryNear(current.file, point, 1);
      return hit ?? null;
    },
    onSuccess: (hit) => setSelection(hit),
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
    resetInteraction();
  }

  function switchTool(next: Tool) {
    setTool(next);
    resetInteraction();
  }

  /** True while a click places a point precisely — where object snap applies. */
  function wantsSnap(): boolean {
    return tool === 'line' || (tool === 'select' && moving);
  }

  /**
   * The `object-contain` fit the `<img>` renders with — read fresh in event
   * handlers and effects, never during render, since it comes from the
   * container's live DOM layout.
   */
  function getFit() {
    const box = containerRef.current?.getBoundingClientRect();
    if (!box || !current) return null;
    const [vbX, vbY, vbW, vbH] = current.viewBox;
    const scale = Math.min(box.width / vbW, box.height / vbH);
    const offsetX = (box.width - vbW * scale) / 2;
    const offsetY = (box.height - vbH * scale) / 2;
    return { box, vbX, vbY, vbW, vbH, scale, offsetX, offsetY };
  }

  /**
   * Screen pixel → drawing millimetre, inverting the same `object-contain`
   * fit the `<img>` renders with. `naturalWidth`/`naturalHeight` are not used
   * here — an SVG with a `viewBox` but no `width`/`height` attribute has no
   * reliable natural size across browsers, but its aspect ratio is exact, and
   * `object-contain` only ever needs the aspect ratio to decide the fit.
   */
  function toWorld(clientX: number, clientY: number): { x: number; y: number } | null {
    const fit = getFit();
    if (!fit) return null;
    const svgX = (clientX - fit.box.left - fit.offsetX) / fit.scale + fit.vbX;
    const svgY = (clientY - fit.box.top - fit.offsetY) / fit.scale + fit.vbY;
    // The renderer draws inside a `scale(1 -1)` group (CLAUDE.md: coordinates
    // are Y-up drawing space), so the SVG's own Y is inverted from the
    // drawing's.
    return { x: svgX, y: -svgY };
  }

  function toScreen(fit: NonNullable<ReturnType<typeof getFit>>, x: number, y: number) {
    return {
      left: (x - fit.vbX) * fit.scale + fit.offsetX,
      top: (-y - fit.vbY) * fit.scale + fit.offsetY,
    };
  }

  /** The drawing-space point a move's delta is measured from. */
  function selectionAnchor(hit: QueryHit): { x: number; y: number } {
    const [minX, minY, , maxX, maxY] = hit.bounds_mm;
    return { x: (minX + maxX) / 2, y: (minY + maxY) / 2 };
  }

  // ── Object snap ────────────────────────────────────────────────────────
  //
  // The cursor's raw world position, and the entities the server last found
  // near it. A snap point is derived from these two in the effect below,
  // rather than stored directly, so it always reflects the current
  // container layout rather than whatever it was when the query resolved.
  const [hoverPoint, setHoverPoint] = useState<{ x: number; y: number } | null>(null);
  const [snapCandidates, setSnapCandidates] = useState<QueryHit[]>([]);
  const snapTimer = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (snapTimer.current !== null) window.clearTimeout(snapTimer.current);
    };
  }, []);

  function onCanvasMouseMove(e: React.MouseEvent) {
    const point = toWorld(e.clientX, e.clientY);
    setHoverPoint(point);
    if (!point || !current || !wantsSnap()) return;

    if (snapTimer.current !== null) window.clearTimeout(snapTimer.current);
    const file = current.file;
    snapTimer.current = window.setTimeout(() => {
      queryNear(file, point, 8)
        .then(setSnapCandidates)
        .catch(() => {
          /* a failed snap lookup just means no candidates this frame */
        });
    }, SNAP_DEBOUNCE_MS);
  }

  function onCanvasMouseLeave() {
    setHoverPoint(null);
  }

  function onCanvasClick(e: React.MouseEvent) {
    if (!current || edit.isPending || pick.isPending) return;
    const point = snapPoint?.world ?? toWorld(e.clientX, e.clientY);
    if (!point) return;

    if (tool === 'line') {
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
      return;
    }

    // tool === 'select'
    if (moving && selection) {
      const anchor = selectionAnchor(selection);
      edit.mutate({
        kind: 'move_entities',
        ids: [selection.id],
        delta: { x: point.x - anchor.x, y: point.y - anchor.y, z: 0 },
      });
      return;
    }
    pick.mutate(point);
  }

  function deleteSelection() {
    if (!selection) return;
    edit.mutate({ kind: 'delete_entities', ids: [selection.id] });
  }

  // Delete/Backspace removes the current selection — but not while a text
  // input has focus, where Backspace means "erase a character."
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== 'Delete' && e.key !== 'Backspace') return;
      const target = e.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)
        return;
      if (!selection || edit.isPending) return;
      e.preventDefault();
      deleteSelection();
    }
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selection, edit.isPending]);

  // Overlay geometry — the pending line's start marker, the selected
  // entity's bounding box, and the live snap indicator. Computed in an
  // effect rather than during render: all three read the container's live
  // layout through the ref, which render must not do.
  const [marker, setMarker] = useState<{ left: number; top: number } | null>(null);
  const [selectionBox, setSelectionBox] = useState<{
    left: number;
    top: number;
    width: number;
    height: number;
  } | null>(null);
  const [snapPoint, setSnapPoint] = useState<{
    left: number;
    top: number;
    world: { x: number; y: number };
  } | null>(null);

  useEffect(() => {
    // The `.current` read here (unused beyond gating) is what tells the
    // set-state-in-effect lint rule this effect synchronises with the DOM
    // rather than just derives state from props — losing it behind
    // `getFit()`'s own internal read makes the rule flag every `setState`
    // below as unsynchronized.
    const fit = containerRef.current ? getFit() : null;
    if (!current || !fit) {
      setMarker(null);
      setSelectionBox(null);
      setSnapPoint(null);
      return;
    }

    setMarker(pendingStart ? toScreen(fit, pendingStart.x, pendingStart.y) : null);

    if (!selection) {
      setSelectionBox(null);
    } else {
      const [minX, minY, , maxX, maxY] = selection.bounds_mm;
      // The SVG's Y-flip means the drawing's top edge (max Y) is the
      // screen's top edge.
      const topLeft = toScreen(fit, minX, maxY);
      const bottomRight = toScreen(fit, maxX, minY);
      setSelectionBox({
        left: topLeft.left,
        top: topLeft.top,
        width: Math.max(bottomRight.left - topLeft.left, 2),
        height: Math.max(bottomRight.top - topLeft.top, 2),
      });
    }

    if (!hoverPoint || !wantsSnap()) {
      setSnapPoint(null);
      return;
    }
    const thresholdWorld = SNAP_RADIUS_PX / fit.scale;
    let best: { x: number; y: number; dist: number } | null = null;
    for (const hit of snapCandidates) {
      for (const [x, y] of hit.snap_points_mm) {
        const dist = Math.hypot(x - hoverPoint.x, y - hoverPoint.y);
        if (dist <= thresholdWorld && (!best || dist < best.dist)) best = { x, y, dist };
      }
    }
    setSnapPoint(
      best ? { ...toScreen(fit, best.x, best.y), world: { x: best.x, y: best.y } } : null,
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingStart, selection, hoverPoint, snapCandidates, tool, moving, current]);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">図面を編集する</h1>
        <p className="mt-1 max-w-2xl text-sm text-ink-muted">
          線分ツールはクリックで始点、もう一度クリックで終点を指定します。既存の端点・中点・中心に近づくと吸着します。選択ツールはクリックで最も近いエンティティを選び、移動または削除できます。ファイルはこのブラウザだけが保持し、編集のたびに送り直されます
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
            <div className="flex gap-1">
              <ToolButton active={tool === 'line'} onClick={() => switchTool('line')}>
                線分
              </ToolButton>
              <ToolButton active={tool === 'select'} onClick={() => switchTool('select')}>
                選択
              </ToolButton>
            </div>

            {tool === 'line' &&
              (pendingStart ? (
                <span>
                  始点 ({pendingStart.x.toFixed(0)}, {pendingStart.y.toFixed(0)}) —
                  終点をクリック
                </span>
              ) : (
                <span>始点をクリック</span>
              ))}

            {tool === 'select' && !selection && (
              <span>エンティティをクリックして選択</span>
            )}
            {tool === 'select' && selection && !moving && (
              <>
                <span>
                  選択中: {selection.kind}（{selection.layer}）
                </span>
                <button
                  type="button"
                  onClick={() => setMoving(true)}
                  className="rounded border border-rule px-2 py-1 text-ink"
                >
                  移動先をクリック
                </button>
                <button
                  type="button"
                  onClick={deleteSelection}
                  className="rounded border border-sys-fire/40 px-2 py-1 text-sys-fire"
                >
                  削除
                </button>
              </>
            )}
            {tool === 'select' && selection && moving && <span>移動先をクリック</span>}
            {snapPoint && (
              <span className="text-sys-hydronic">
                スナップ中 ({snapPoint.world.x.toFixed(0)}, {snapPoint.world.y.toFixed(0)}
                )
              </span>
            )}

            <button
              type="button"
              onClick={undo}
              disabled={history.length === 0 || edit.isPending}
              className="ml-auto rounded border border-rule px-2 py-1 text-ink disabled:opacity-40"
            >
              元に戻す（{history.length}）
            </button>
            {(edit.isPending || pick.isPending) && <span>処理中…</span>}
          </div>

          <div
            ref={containerRef}
            onClick={onCanvasClick}
            onMouseMove={onCanvasMouseMove}
            onMouseLeave={onCanvasMouseLeave}
            className="relative h-[32rem] overflow-hidden rounded border border-rule bg-paper-raised"
            style={{ cursor: edit.isPending || pick.isPending ? 'wait' : 'crosshair' }}
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
            {selectionBox && (
              <div
                className="pointer-events-none absolute border-2 border-dashed border-accent"
                style={selectionBox}
              />
            )}
            {snapPoint && (
              <div
                className="pointer-events-none absolute h-3.5 w-3.5 -translate-x-1/2 -translate-y-1/2 rotate-45 border-2 border-sys-hydronic bg-paper-raised"
                style={{ left: snapPoint.left, top: snapPoint.top }}
              />
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function ToolButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={`rounded border px-2 py-1 transition-colors ${
        active
          ? 'border-accent bg-accent/10 text-ink'
          : 'border-rule bg-paper-raised text-ink-muted hover:text-ink'
      }`}
    >
      {children}
    </button>
  );
}
