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
 * moves, not on every `pointermove` — a round trip per pixel would make the
 * canvas feel like it is wading through mud.
 *
 * The select tool's pointer handler does triple duty — plain click
 * (re)selects, shift-click toggles an entity into or out of the selection,
 * and a drag past a small threshold moves the whole selection — because all
 * three start the same way (a `pointerdown` somewhere on the canvas) and only
 * `pointerup` reveals which one actually happened.
 */

interface Snapshot {
  file: File;
  url: string;
  viewBox: [number, number, number, number];
}

type Tool = 'line' | 'select';
type WorldPoint = { x: number; y: number };

const DEFAULT_LAYER = 'A-EDIT';
/** How close, in screen pixels, a candidate has to be before it "grabs" the cursor. */
const SNAP_RADIUS_PX = 12;
/** How long to wait after the pointer stops before asking the server what is nearby. */
const SNAP_DEBOUNCE_MS = 100;
/** How far the pointer has to move before a pointerdown becomes a drag. */
const DRAG_THRESHOLD_PX = 4;
/** How close a click has to land to an entity's bounds before it counts as picking it. */
const PICK_RADIUS_PX = 20;

interface DragState {
  downClient: { x: number; y: number };
  downWorld: WorldPoint;
  shift: boolean;
  moved: boolean;
}

export function EditorPage() {
  const [current, setCurrent] = useState<Snapshot | null>(null);
  const [history, setHistory] = useState<Snapshot[]>([]);
  const [tool, setTool] = useState<Tool>('line');
  const [pendingStart, setPendingStart] = useState<WorldPoint | null>(null);
  const [selection, setSelection] = useState<QueryHit[]>([]);
  const [drag, setDrag] = useState<DragState | null>(null);
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
    setSelection([]);
    setDrag(null);
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
      setSelection([]);
    },
    onError: (e: Error) => setError(e.message),
  });

  const pick = useMutation({
    mutationFn: async ({ point }: { point: WorldPoint; shift: boolean }) => {
      if (!current) throw new Error('先に図面を開いてください');
      const fit = getFit();
      const [hit] = await queryNear(current.file, point, 1);
      if (!hit || !fit) return null;
      const maxWorld = PICK_RADIUS_PX / fit.scale;
      return distanceToBounds(point, hit.bounds_mm) <= maxWorld ? hit : null;
    },
    onSuccess: (hit, { shift }) => {
      setSelection((prev) => {
        if (!hit) return shift ? prev : [];
        if (!shift) return [hit];
        return prev.some((p) => p.id === hit.id)
          ? prev.filter((p) => p.id !== hit.id)
          : [...prev, hit];
      });
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
    resetInteraction();
  }

  function switchTool(next: Tool) {
    setTool(next);
    resetInteraction();
  }

  /** True while a pointer action is about to place a point precisely — where object snap applies. */
  function wantsSnap(): boolean {
    return tool === 'line' || (tool === 'select' && drag?.moved === true);
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
  function toWorld(clientX: number, clientY: number): WorldPoint | null {
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

  // ── Object snap ────────────────────────────────────────────────────────
  //
  // The cursor's raw world position, and the entities the server last found
  // near it. A snap point is derived from these two in the effect below,
  // rather than stored directly, so it always reflects the current
  // container layout rather than whatever it was when the query resolved.
  const [hoverPoint, setHoverPoint] = useState<WorldPoint | null>(null);
  const [snapCandidates, setSnapCandidates] = useState<QueryHit[]>([]);
  const snapTimer = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (snapTimer.current !== null) window.clearTimeout(snapTimer.current);
    };
  }, []);

  function onCanvasPointerMove(e: React.PointerEvent) {
    const point = toWorld(e.clientX, e.clientY);
    setHoverPoint(point);

    if (drag && !drag.moved) {
      const moved =
        Math.hypot(e.clientX - drag.downClient.x, e.clientY - drag.downClient.y) >
        DRAG_THRESHOLD_PX;
      if (moved) setDrag({ ...drag, moved: true });
    }

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

  function onCanvasPointerLeave() {
    if (!drag) setHoverPoint(null);
  }

  function onCanvasClick(e: React.MouseEvent) {
    if (tool !== 'line' || !current || edit.isPending) return;
    const point = snapPoint?.world ?? toWorld(e.clientX, e.clientY);
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

  function onCanvasPointerDown(e: React.PointerEvent) {
    if (tool !== 'select' || !current || edit.isPending) return;
    const world = toWorld(e.clientX, e.clientY);
    if (!world) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({
      downClient: { x: e.clientX, y: e.clientY },
      downWorld: world,
      shift: e.shiftKey,
      moved: false,
    });
  }

  function onCanvasPointerUp(e: React.PointerEvent) {
    if (tool !== 'select' || !drag || !current) {
      setDrag(null);
      return;
    }
    const upPoint = snapPoint?.world ?? toWorld(e.clientX, e.clientY);
    const { moved, shift, downWorld } = drag;
    setDrag(null);
    if (!upPoint) return;

    if (moved) {
      if (selection.length === 0) return;
      edit.mutate({
        kind: 'move_entities',
        ids: selection.map((s) => s.id),
        delta: { x: upPoint.x - downWorld.x, y: upPoint.y - downWorld.y, z: 0 },
      });
      return;
    }
    pick.mutate({ point: upPoint, shift });
  }

  function deleteSelection() {
    if (selection.length === 0) return;
    edit.mutate({ kind: 'delete_entities', ids: selection.map((s) => s.id) });
  }

  function rotateSelection(degrees: number) {
    if (selection.length === 0) return;
    edit.mutate({
      kind: 'rotate_entities',
      ids: selection.map((s) => s.id),
      radians: (degrees * Math.PI) / 180,
    });
  }

  // Delete/Backspace removes the current selection — but not while a text
  // input has focus, where Backspace means "erase a character."
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== 'Delete' && e.key !== 'Backspace') return;
      const target = e.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)
        return;
      if (selection.length === 0 || edit.isPending) return;
      e.preventDefault();
      deleteSelection();
    }
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selection, edit.isPending]);

  // Overlay geometry — the pending line's start marker, every selected
  // entity's bounding box (and, while dragging, a translated ghost of each),
  // and the live snap indicator. Computed in an effect rather than during
  // render: all of it reads the container's live layout through the ref,
  // which render must not do.
  type Box = { left: number; top: number; width: number; height: number };
  const [marker, setMarker] = useState<{ left: number; top: number } | null>(null);
  const [selectionBoxes, setSelectionBoxes] = useState<Box[]>([]);
  const [ghostBoxes, setGhostBoxes] = useState<Box[]>([]);
  const [snapPoint, setSnapPoint] = useState<{
    left: number;
    top: number;
    world: WorldPoint;
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
      setSelectionBoxes([]);
      setGhostBoxes([]);
      setSnapPoint(null);
      return;
    }

    setMarker(pendingStart ? toScreen(fit, pendingStart.x, pendingStart.y) : null);

    const boxes = selection.map((hit): Box => {
      const [minX, minY, , maxX, maxY] = hit.bounds_mm;
      // The SVG's Y-flip means the drawing's top edge (max Y) is the
      // screen's top edge.
      const topLeft = toScreen(fit, minX, maxY);
      const bottomRight = toScreen(fit, maxX, minY);
      return {
        left: topLeft.left,
        top: topLeft.top,
        width: Math.max(bottomRight.left - topLeft.left, 2),
        height: Math.max(bottomRight.top - topLeft.top, 2),
      };
    });
    setSelectionBoxes(boxes);

    let snap: { left: number; top: number; world: WorldPoint } | null = null;
    if (hoverPoint && wantsSnap()) {
      const thresholdWorld = SNAP_RADIUS_PX / fit.scale;
      let best: { x: number; y: number; dist: number } | null = null;
      for (const hit of snapCandidates) {
        for (const [x, y] of hit.snap_points_mm) {
          const dist = Math.hypot(x - hoverPoint.x, y - hoverPoint.y);
          if (dist <= thresholdWorld && (!best || dist < best.dist))
            best = { x, y, dist };
        }
      }
      snap = best
        ? { ...toScreen(fit, best.x, best.y), world: { x: best.x, y: best.y } }
        : null;
    }
    setSnapPoint(snap);

    if (drag?.moved && selection.length > 0) {
      const live = snap?.world ?? hoverPoint;
      if (live) {
        const dx = (live.x - drag.downWorld.x) * fit.scale;
        const dy = -(live.y - drag.downWorld.y) * fit.scale;
        setGhostBoxes(boxes.map((b) => ({ ...b, left: b.left + dx, top: b.top + dy })));
      } else {
        setGhostBoxes([]);
      }
    } else {
      setGhostBoxes([]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingStart, selection, hoverPoint, snapCandidates, drag, tool, current]);

  const canRotate = selection.length > 0 && selection.every((s) => s.kind === 'blockref');

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">図面を編集する</h1>
        <p className="mt-1 max-w-2xl text-sm text-ink-muted">
          線分ツールはクリックで始点、もう一度クリックで終点を指定します。選択ツールはクリックで最も近いエンティティを選び（Shiftクリックで複数選択）、ドラッグで移動、Deleteキーまたはボタンで削除します。既存の端点・中点・中心に近づくと吸着します。ファイルはこのブラウザだけが保持し、編集のたびに送り直されます
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

            {tool === 'select' && selection.length === 0 && (
              <span>クリックで選択（Shiftで複数選択）</span>
            )}
            {tool === 'select' && selection.length > 0 && (
              <>
                <span>選択中: {selection.length}件（ドラッグで移動）</span>
                {canRotate && (
                  <>
                    <button
                      type="button"
                      onClick={() => rotateSelection(-90)}
                      className="rounded border border-rule px-2 py-1 text-ink"
                    >
                      ↺90°
                    </button>
                    <button
                      type="button"
                      onClick={() => rotateSelection(90)}
                      className="rounded border border-rule px-2 py-1 text-ink"
                    >
                      ↻90°
                    </button>
                  </>
                )}
                <button
                  type="button"
                  onClick={deleteSelection}
                  className="rounded border border-sys-fire/40 px-2 py-1 text-sys-fire"
                >
                  削除
                </button>
              </>
            )}
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
            onPointerDown={onCanvasPointerDown}
            onPointerMove={onCanvasPointerMove}
            onPointerUp={onCanvasPointerUp}
            onPointerLeave={onCanvasPointerLeave}
            className="relative h-[32rem] overflow-hidden rounded border border-rule bg-paper-raised"
            style={{
              cursor: edit.isPending || pick.isPending ? 'wait' : 'crosshair',
              touchAction: 'none',
            }}
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
            {selectionBoxes.map((box, i) => (
              <div
                key={selection[i]?.id ?? i}
                className="pointer-events-none absolute border-2 border-dashed border-accent"
                style={box}
              />
            ))}
            {ghostBoxes.map((box, i) => (
              <div
                key={selection[i]?.id ?? i}
                className="pointer-events-none absolute border-2 border-accent bg-accent/20"
                style={box}
              />
            ))}
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

/** Distance from `point` to the nearest point on or in `bounds` — zero when `point` is inside. */
function distanceToBounds(
  point: WorldPoint,
  bounds: readonly [number, number, number, number, number, number],
): number {
  const [minX, minY, , maxX, maxY] = bounds;
  const dx = Math.max(minX - point.x, 0, point.x - maxX);
  const dy = Math.max(minY - point.y, 0, point.y - maxY);
  return Math.hypot(dx, dy);
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
