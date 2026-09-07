import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import type { Command, Profile, QueryHit } from '@opendraft/shared';
import {
  editDrawing,
  fetchMepPorts,
  fetchParts,
  fetchSpecs,
  fetchSystems,
  placeMep,
  queryNear,
  renderDrawingWithViewBox,
  routeMep,
} from '../api.ts';

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
 *
 * The MEP tool draws a route (`docs/04-mep.md` §1: centreline + profile +
 * system, auto-inserting whatever 90° fittings the path needs) rather than a
 * generic entity — which is why it goes through its own endpoint,
 * `POST /api/mep/route`, and not `od-core::Command`: a route cannot be one
 * without `od-core` learning what a "system" or a "spec" is (rule 1).
 */

interface Snapshot {
  file: File;
  url: string;
  viewBox: [number, number, number, number];
}

type Tool = 'line' | 'circle' | 'arc' | 'polyline' | 'select' | 'mep' | 'place';
type WorldPoint = { x: number; y: number };
type ProfileKind = 'rect' | 'round';

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

  // ── Circle / arc / polyline ────────────────────────────────────────────
  // 円 reuses `pendingStart` exactly like 線分 does — both are two clicks,
  // the second just means "radius point" instead of "line end". 円弧 needs
  // three (center, start, end), so it gets its own accumulator; ポリライン
  // is open-ended, so it mirrors 配管's click-to-add-vertex/確定 pattern.
  const [arcPoints, setArcPoints] = useState<WorldPoint[]>([]);
  const [polylinePoints, setPolylinePoints] = useState<WorldPoint[]>([]);
  const [polylineClosed, setPolylineClosed] = useState(false);

  // ── Mirror / offset (select tool actions) ──────────────────────────────
  // Mirroring needs two extra clicks to define the mirror line, but the
  // selection they act on only exists in the 選択 tool — switching to a
  // dedicated tool would lose it (switchTool resets selection). So mirroring
  // is armed from inside 選択 instead: pointerdown while armed accumulates
  // mirror-line points rather than doing its usual pick-or-drag.
  const [mirrorArmed, setMirrorArmed] = useState(false);
  const [mirrorPoints, setMirrorPoints] = useState<WorldPoint[]>([]);
  const [mirrorKeepOriginal, setMirrorKeepOriginal] = useState(true);
  const [offsetDistance, setOffsetDistance] = useState('100');

  // ── Grip editing ────────────────────────────────────────────────────────
  // A single selected entity with editable vertices (a line's endpoints, a
  // polyline's vertices — see od_core::Geometry::editable_vertices) shows a
  // grip at each one. A grip's own pointerdown stops propagation so the
  // container's onCanvasPointerDown never sees it — without that, grabbing
  // a grip would also start the whole-selection drag-to-move gesture.
  // pointermove is deliberately left to bubble, though: it is how the
  // container keeps feeding this drag the same live snapPoint every other
  // draw tool already gets, for free.
  const [gripDrag, setGripDrag] = useState<{ index: number } | null>(null);

  // ── MEP routing ────────────────────────────────────────────────────────
  const [mepPath, setMepPath] = useState<WorldPoint[]>([]);
  const [mepSystem, setMepSystem] = useState('');
  const [mepSpec, setMepSpec] = useState('');
  const [mepProfileKind, setMepProfileKind] = useState<ProfileKind>('rect');
  const [mepW, setMepW] = useState('400');
  const [mepH, setMepH] = useState('300');
  const [mepD, setMepD] = useState('150');
  const [mepElevation, setMepElevation] = useState('0');
  const systemsQuery = useQuery({ queryKey: ['systems'], queryFn: fetchSystems });
  const specsQuery = useQuery({ queryKey: ['specs'], queryFn: fetchSpecs });
  // Defaults to the catalogue's first entry until the user picks one —
  // computed each render rather than synced into state via an effect, since
  // it is fully derived from props/query data with nothing external to
  // subscribe to.
  const effectiveSystem = mepSystem || (systemsQuery.data?.[0]?.id ?? '');
  const effectiveSpec = mepSpec || (specsQuery.data?.[0]?.id ?? '');

  // ── MEP equipment placement ───────────────────────────────────────────
  // Shares `effectiveSystem` with routing above — a fan placed on 給気
  // usually gets routed from on 給気 too, so one "current system" concept
  // serves both tools rather than tracking it twice.
  const [placeQuery, setPlaceQuery] = useState('');
  const [placePartId, setPlacePartId] = useState('');
  const partsQuery = useQuery({
    queryKey: ['parts', placeQuery],
    queryFn: () => fetchParts(placeQuery),
  });
  const effectivePlacePart = placePartId || (partsQuery.data?.[0]?.id ?? '');

  // ── MEP port snapping ──────────────────────────────────────────────────
  // Every port in the currently open document, fetched once per snapshot
  // (keyed by its object URL) rather than per pointer-move like the
  // geometry-based snap candidates below — a real drawing's full port list
  // is small, so there is no need for the server-side spatial index
  // `queryNear` exists for. A route that snaps exactly onto a port's
  // position is what lets the connection graph link the two (F-104).
  const portsQuery = useQuery({
    queryKey: ['mepPorts', current?.url ?? null],
    queryFn: () => fetchMepPorts(current!.file),
    enabled: current !== null,
  });

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
    setMepPath([]);
    setArcPoints([]);
    setPolylinePoints([]);
    setMirrorArmed(false);
    setMirrorPoints([]);
    setGripDrag(null);
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

  const route = useMutation({
    mutationFn: async (params: {
      system: string;
      spec: string;
      profile: Profile;
      path: { x: number; y: number; z: number }[];
    }) => {
      if (!current) throw new Error('先に図面を開いてください');
      return routeMep(current.file, params);
    },
    onSuccess: (result) => {
      setError(null);
      setHistory((h) => (current ? [...h, current] : h));
      setCurrent({ file: result.document, url: result.url, viewBox: result.viewBox });
      setMepPath([]);
    },
    onError: (e: Error) => setError(e.message),
  });

  const place = useMutation({
    mutationFn: async (point: WorldPoint) => {
      if (!current) throw new Error('先に図面を開いてください');
      if (!effectivePlacePart) throw new Error('部品を選択してください');
      return placeMep(current.file, {
        part: effectivePlacePart,
        position: { x: point.x, y: point.y, z: 0 },
        ...(effectiveSystem ? { system: effectiveSystem } : {}),
      });
    },
    onSuccess: (result) => {
      setError(null);
      setHistory((h) => (current ? [...h, current] : h));
      setCurrent({ file: result.document, url: result.url, viewBox: result.viewBox });
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
    return (
      tool === 'line' ||
      tool === 'circle' ||
      tool === 'arc' ||
      tool === 'polyline' ||
      tool === 'mep' ||
      tool === 'place' ||
      (tool === 'select' && (drag?.moved === true || mirrorArmed || gripDrag !== null))
    );
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
    if (!current || edit.isPending) return;
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

    if (tool === 'circle') {
      if (!pendingStart) {
        setPendingStart(point);
        return;
      }
      const radius = Math.hypot(point.x - pendingStart.x, point.y - pendingStart.y);
      if (radius > 0) {
        edit.mutate({
          kind: 'add_circle',
          layer,
          center: { x: pendingStart.x, y: pendingStart.y, z: 0 },
          radius,
        });
      }
      setPendingStart(null);
      return;
    }

    if (tool === 'arc') {
      if (arcPoints.length < 2) {
        setArcPoints((p) => [...p, point]);
        return;
      }
      const [center, start] = arcPoints;
      if (center && start) {
        const { radius, startAngle, sweep } = arcFromThreePoints(center, start, point);
        if (radius > 0) {
          edit.mutate({
            kind: 'add_arc',
            layer,
            center: { x: center.x, y: center.y, z: 0 },
            radius,
            start_angle: startAngle,
            sweep,
          });
        }
      }
      setArcPoints([]);
      return;
    }

    if (tool === 'polyline') {
      setPolylinePoints((p) => [...p, point]);
      return;
    }

    if (tool === 'mep') {
      // Starting a run on a port: carry its height into the elevation field
      // rather than making the user look it up and type it — the whole run
      // still shares one Z (draw_route requires a planar path), so this only
      // makes sense for the first vertex.
      if (mepPath.length === 0 && snapPoint?.portElevation !== undefined) {
        setMepElevation(String(snapPoint.portElevation));
      }
      setMepPath((p) => [...p, point]);
      return;
    }

    if (tool === 'place' && !place.isPending) {
      place.mutate(point);
    }
  }

  function finishMepRoute() {
    if (mepPath.length < 2 || !effectiveSystem || !effectiveSpec || route.isPending)
      return;
    const elevation = Number(mepElevation) || 0;
    const profile: Profile =
      mepProfileKind === 'rect'
        ? { kind: 'rect', w: Number(mepW) || 0, h: Number(mepH) || 0 }
        : { kind: 'round', d: Number(mepD) || 0 };
    route.mutate({
      system: effectiveSystem,
      spec: effectiveSpec,
      profile,
      path: mepPath.map((p) => ({ x: p.x, y: p.y, z: elevation })),
    });
  }

  function finishPolyline() {
    if (polylinePoints.length < 2 || edit.isPending) return;
    edit.mutate({
      kind: 'add_polyline',
      layer,
      points: polylinePoints.map((p) => ({ x: p.x, y: p.y, z: 0 })),
      closed: polylineClosed,
    });
    setPolylinePoints([]);
  }

  function onCanvasPointerDown(e: React.PointerEvent) {
    if (tool !== 'select' || !current || edit.isPending) return;
    const world = toWorld(e.clientX, e.clientY);
    if (!world) return;

    if (mirrorArmed) {
      const point = snapPoint?.world ?? world;
      const next = [...mirrorPoints, point];
      if (next.length === 2) {
        const [a, b] = next;
        if (a && b) mirrorSelection(a, b);
        setMirrorPoints([]);
      } else {
        setMirrorPoints(next);
      }
      return;
    }

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
      const delta = { x: upPoint.x - downWorld.x, y: upPoint.y - downWorld.y, z: 0 };
      // Alt+drag copies instead of moving — the same modifier convention a
      // real CAD's drag-copy uses, and it reuses the whole move gesture
      // (ghost preview included) rather than needing a separate mode.
      if (e.altKey) {
        edit.mutate({ kind: 'copy_entities', ids: selection.map((s) => s.id), delta });
      } else {
        edit.mutate({ kind: 'move_entities', ids: selection.map((s) => s.id), delta });
      }
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

  function mirrorSelection(a: WorldPoint, b: WorldPoint) {
    if (selection.length === 0) return;
    edit.mutate({
      kind: 'mirror_entities',
      ids: selection.map((s) => s.id),
      a: { x: a.x, y: a.y, z: 0 },
      b: { x: b.x, y: b.y, z: 0 },
      keep_original: mirrorKeepOriginal,
    });
    setMirrorArmed(false);
  }

  function offsetSelection() {
    const id = selection[0]?.id;
    if (!id) return;
    const distance = Number(offsetDistance);
    if (!Number.isFinite(distance) || distance === 0) return;
    edit.mutate({ kind: 'offset_entity', id, distance });
  }

  function onGripPointerDown(e: React.PointerEvent, index: number) {
    if (!current || edit.isPending) return;
    // Must not reach the container: its own onCanvasPointerDown would
    // otherwise start a whole-selection drag from the same click.
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    setGripDrag({ index });
  }

  function onGripPointerUp(e: React.PointerEvent, index: number) {
    if (!gripDrag || gripDrag.index !== index) {
      setGripDrag(null);
      return;
    }
    const hit = selection[0];
    const original = hit?.vertices_mm[index];
    const point = snapPoint?.world ?? toWorld(e.clientX, e.clientY);
    setGripDrag(null);
    if (!hit || !original || !point) return;
    // The click only ever gives X/Y — the vertex's own original Z rides
    // along unchanged, which for a polyline is also the only Z the engine
    // will accept (Geometry::Polyline is planar at one shared elevation).
    edit.mutate({
      kind: 'set_vertex',
      id: hit.id,
      index,
      position: { x: point.x, y: point.y, z: original[2] },
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
    /** Set when this snap matched a component's port, not just geometry. */
    portElevation?: number;
  } | null>(null);
  const [mepPathScreen, setMepPathScreen] = useState<{ left: number; top: number }[]>([]);
  const [polylinePathScreen, setPolylinePathScreen] = useState<
    { left: number; top: number }[]
  >([]);
  const [circlePreview, setCirclePreview] = useState<{
    left: number;
    top: number;
    r: number;
  } | null>(null);
  const [arcPreview, setArcPreview] = useState<{
    center: { left: number; top: number };
    spokes: { left: number; top: number }[];
  } | null>(null);
  const [mirrorLineScreen, setMirrorLineScreen] = useState<
    { left: number; top: number }[]
  >([]);
  const [gripScreens, setGripScreens] = useState<{ left: number; top: number }[]>([]);
  // Dashed lines from the dragged vertex's immediate neighbours (in the
  // entity's own vertex order — a line's other endpoint, or a polyline
  // vertex's one or two neighbours) to wherever it would land right now.
  // Not a full redraw of the entity: neighbours are the only geometry that
  // actually changes shape while a single vertex moves.
  const [gripRubberBand, setGripRubberBand] = useState<
    { from: { left: number; top: number }; to: { left: number; top: number } }[]
  >([]);

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
      setMepPathScreen([]);
      setPolylinePathScreen([]);
      setCirclePreview(null);
      setArcPreview(null);
      setMirrorLineScreen([]);
      setGripScreens([]);
      setGripRubberBand([]);
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

    let snap: {
      left: number;
      top: number;
      world: WorldPoint;
      portElevation?: number;
    } | null = null;
    if (hoverPoint && wantsSnap()) {
      const thresholdWorld = SNAP_RADIUS_PX / fit.scale;
      let best: { x: number; y: number; dist: number; z?: number } | null = null;
      for (const hit of snapCandidates) {
        for (const [x, y] of hit.snap_points_mm) {
          const dist = Math.hypot(x - hoverPoint.x, y - hoverPoint.y);
          if (dist <= thresholdWorld && (!best || dist < best.dist))
            best = { x, y, dist };
        }
      }
      // A route can only start or end on a port that is not already spoken
      // for — offering an already-connected one up as a snap target would
      // invite drawing a second route onto it, which the connection graph
      // (one mate per port) would just as quietly refuse to link (F-104).
      if (tool === 'mep') {
        for (const port of portsQuery.data ?? []) {
          if (port.connected) continue;
          const [x, y, z] = port.position_mm;
          const dist = Math.hypot(x - hoverPoint.x, y - hoverPoint.y);
          if (dist <= thresholdWorld && (!best || dist < best.dist))
            best = { x, y, dist, z };
        }
      }
      snap = best
        ? {
            ...toScreen(fit, best.x, best.y),
            world: { x: best.x, y: best.y },
            ...(best.z !== undefined ? { portElevation: best.z } : {}),
          }
        : null;
    }
    setSnapPoint(snap);

    // The path preview includes a rubber-band segment to wherever the next
    // click would currently land, so the pending vertex is visible before
    // it is placed, not only after.
    if (tool === 'mep' && mepPath.length > 0) {
      const rubberBand = snap?.world ?? hoverPoint;
      const points = mepPath.map((p) => toScreen(fit, p.x, p.y));
      if (rubberBand) points.push(toScreen(fit, rubberBand.x, rubberBand.y));
      setMepPathScreen(points);
    } else {
      setMepPathScreen(mepPath.map((p) => toScreen(fit, p.x, p.y)));
    }

    if (tool === 'polyline' && polylinePoints.length > 0) {
      const rubberBand = snap?.world ?? hoverPoint;
      const points = polylinePoints.map((p) => toScreen(fit, p.x, p.y));
      if (rubberBand) points.push(toScreen(fit, rubberBand.x, rubberBand.y));
      setPolylinePathScreen(points);
    } else {
      setPolylinePathScreen(polylinePoints.map((p) => toScreen(fit, p.x, p.y)));
    }

    if (tool === 'circle' && pendingStart) {
      const radiusPoint = snap?.world ?? hoverPoint;
      const r = radiusPoint
        ? Math.hypot(radiusPoint.x - pendingStart.x, radiusPoint.y - pendingStart.y) *
          fit.scale
        : 0;
      setCirclePreview({ ...toScreen(fit, pendingStart.x, pendingStart.y), r });
    } else {
      setCirclePreview(null);
    }

    if (tool === 'arc' && arcPoints.length > 0) {
      const [center] = arcPoints;
      if (center) {
        const rubberBand = snap?.world ?? hoverPoint;
        const spokePoints = arcPoints.slice(1);
        if (rubberBand) spokePoints.push(rubberBand);
        setArcPreview({
          center: toScreen(fit, center.x, center.y),
          spokes: spokePoints.map((p) => toScreen(fit, p.x, p.y)),
        });
      }
    } else {
      setArcPreview(null);
    }

    if (mirrorArmed && mirrorPoints.length > 0) {
      const rubberBand = snap?.world ?? hoverPoint;
      const points = mirrorPoints.map((p) => toScreen(fit, p.x, p.y));
      if (rubberBand) points.push(toScreen(fit, rubberBand.x, rubberBand.y));
      setMirrorLineScreen(points);
    } else {
      setMirrorLineScreen([]);
    }

    const gripVertices = selection.length === 1 ? (selection[0]?.vertices_mm ?? []) : [];
    if (tool === 'select' && gripVertices.length > 0) {
      setGripScreens(gripVertices.map(([x, y]) => toScreen(fit, x, y)));
    } else {
      setGripScreens([]);
    }

    if (gripDrag && gripVertices.length > 0) {
      const live = snap?.world ?? hoverPoint;
      if (live) {
        const liveScreen = toScreen(fit, live.x, live.y);
        // A line has only its other endpoint as a neighbour; a polyline
        // vertex has whichever of its predecessor/successor exist (an open
        // polyline's first or last vertex has only one).
        const neighbourIndices =
          gripVertices.length === 2
            ? [1 - gripDrag.index]
            : [gripDrag.index - 1, gripDrag.index + 1].filter(
                (i) => i >= 0 && i < gripVertices.length,
              );
        setGripRubberBand(
          neighbourIndices
            .map((i) => gripVertices[i])
            .filter((v): v is [number, number, number] => v !== undefined)
            .map((v) => ({ from: toScreen(fit, v[0], v[1]), to: liveScreen })),
        );
      } else {
        setGripRubberBand([]);
      }
    } else {
      setGripRubberBand([]);
    }

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
  }, [
    pendingStart,
    selection,
    hoverPoint,
    snapCandidates,
    portsQuery.data,
    drag,
    tool,
    current,
    mepPath,
    polylinePoints,
    arcPoints,
    mirrorArmed,
    mirrorPoints,
    gripDrag,
  ]);

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
              <ToolButton active={tool === 'circle'} onClick={() => switchTool('circle')}>
                円
              </ToolButton>
              <ToolButton active={tool === 'arc'} onClick={() => switchTool('arc')}>
                円弧
              </ToolButton>
              <ToolButton
                active={tool === 'polyline'}
                onClick={() => switchTool('polyline')}
              >
                ポリライン
              </ToolButton>
              <ToolButton active={tool === 'select'} onClick={() => switchTool('select')}>
                選択
              </ToolButton>
              <ToolButton active={tool === 'mep'} onClick={() => switchTool('mep')}>
                配管
              </ToolButton>
              <ToolButton active={tool === 'place'} onClick={() => switchTool('place')}>
                配置
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

            {tool === 'circle' &&
              (pendingStart ? (
                <span>中心を指定 — 半径をクリック</span>
              ) : (
                <span>中心をクリック</span>
              ))}

            {tool === 'arc' && (
              <span>
                {arcPoints.length === 0 && '中心をクリック'}
                {arcPoints.length === 1 && '始点をクリック'}
                {arcPoints.length === 2 && '終点をクリック'}
              </span>
            )}

            {tool === 'polyline' && (
              <span>
                {polylinePoints.length === 0
                  ? '頂点をクリック'
                  : `点 ${polylinePoints.length} — クリックで追加、確定で作図`}
              </span>
            )}
            {tool === 'polyline' && polylinePoints.length > 0 && (
              <>
                <label className="flex items-center gap-1">
                  <input
                    type="checkbox"
                    checked={polylineClosed}
                    onChange={(e) => setPolylineClosed(e.target.checked)}
                  />
                  閉じる
                </label>
                <button
                  type="button"
                  onClick={() => setPolylinePoints((p) => p.slice(0, -1))}
                  className="rounded border border-rule px-2 py-1 text-ink"
                >
                  一点戻す
                </button>
                <button
                  type="button"
                  onClick={() => setPolylinePoints([])}
                  className="rounded border border-rule px-2 py-1 text-ink"
                >
                  クリア
                </button>
                <button
                  type="button"
                  onClick={finishPolyline}
                  disabled={polylinePoints.length < 2 || edit.isPending}
                  className="rounded border border-accent bg-accent/10 px-2 py-1 text-ink disabled:opacity-40"
                >
                  確定
                </button>
              </>
            )}

            {tool === 'select' && mirrorArmed && (
              <>
                <span>
                  {mirrorPoints.length === 0
                    ? 'ミラー線の始点をクリック'
                    : 'ミラー線の終点をクリック'}
                </span>
                <label className="flex items-center gap-1">
                  <input
                    type="checkbox"
                    checked={mirrorKeepOriginal}
                    onChange={(e) => setMirrorKeepOriginal(e.target.checked)}
                  />
                  元を残す
                </label>
                <button
                  type="button"
                  onClick={() => {
                    setMirrorArmed(false);
                    setMirrorPoints([]);
                  }}
                  className="rounded border border-rule px-2 py-1 text-ink"
                >
                  キャンセル
                </button>
              </>
            )}

            {tool === 'select' && !mirrorArmed && selection.length === 0 && (
              <span>クリックで選択（Shiftで複数選択）</span>
            )}
            {tool === 'select' && !mirrorArmed && selection.length > 0 && (
              <>
                <span>
                  選択中: {selection.length}件（ドラッグで移動、Alt+ドラッグでコピー）
                  {gripScreens.length > 0 && '、□をドラッグで頂点編集'}
                </span>
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
                  onClick={() => setMirrorArmed(true)}
                  className="rounded border border-rule px-2 py-1 text-ink"
                >
                  ミラー
                </button>
                {selection.length === 1 && (
                  <>
                    <input
                      type="number"
                      value={offsetDistance}
                      onChange={(e) => setOffsetDistance(e.target.value)}
                      className="w-20 rounded border border-rule bg-paper px-2 py-1"
                    />
                    <button
                      type="button"
                      onClick={offsetSelection}
                      className="rounded border border-rule px-2 py-1 text-ink"
                    >
                      オフセット
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

            {tool === 'mep' && (
              <span>
                {mepPath.length === 0
                  ? '経路の点をクリック'
                  : `点 ${mepPath.length} — クリックで追加、確定で自動継手を挿入`}
              </span>
            )}
            {tool === 'mep' && mepPath.length > 0 && (
              <>
                <button
                  type="button"
                  onClick={() => setMepPath((p) => p.slice(0, -1))}
                  className="rounded border border-rule px-2 py-1 text-ink"
                >
                  一点戻す
                </button>
                <button
                  type="button"
                  onClick={() => setMepPath([])}
                  className="rounded border border-rule px-2 py-1 text-ink"
                >
                  クリア
                </button>
                <button
                  type="button"
                  onClick={finishMepRoute}
                  disabled={
                    mepPath.length < 2 ||
                    !effectiveSystem ||
                    !effectiveSpec ||
                    route.isPending
                  }
                  className="rounded border border-accent bg-accent/10 px-2 py-1 text-ink disabled:opacity-40"
                >
                  確定
                </button>
              </>
            )}

            {tool === 'place' && (
              <span>
                {effectivePlacePart ? 'クリックして配置' : '部品を検索・選択してください'}
              </span>
            )}

            {snapPoint && (
              <span className="text-sys-hydronic">
                {snapPoint.portElevation !== undefined
                  ? '接続口にスナップ中'
                  : 'スナップ中'}{' '}
                ({snapPoint.world.x.toFixed(0)}, {snapPoint.world.y.toFixed(0)})
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
            {(edit.isPending || pick.isPending || route.isPending || place.isPending) && (
              <span>処理中…</span>
            )}
          </div>

          {tool === 'mep' && (
            <div className="flex flex-wrap items-end gap-3 rounded border border-rule bg-paper-raised px-3 py-2 text-xs">
              <label className="flex flex-col gap-1">
                系統
                <select
                  value={effectiveSystem}
                  onChange={(e) => setMepSystem(e.target.value)}
                  className="rounded border border-rule bg-paper px-2 py-1"
                >
                  {(systemsQuery.data ?? []).map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name.ja}
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex flex-col gap-1">
                仕様
                <select
                  value={effectiveSpec}
                  onChange={(e) => setMepSpec(e.target.value)}
                  className="rounded border border-rule bg-paper px-2 py-1"
                >
                  {(specsQuery.data ?? []).map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name.ja}
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex flex-col gap-1">
                断面
                <select
                  value={mepProfileKind}
                  onChange={(e) => setMepProfileKind(e.target.value as ProfileKind)}
                  className="rounded border border-rule bg-paper px-2 py-1"
                >
                  <option value="rect">矩形</option>
                  <option value="round">円形</option>
                </select>
              </label>
              {mepProfileKind === 'rect' ? (
                <>
                  <NumberField label="幅 mm" value={mepW} onChange={setMepW} />
                  <NumberField label="高さ mm" value={mepH} onChange={setMepH} />
                </>
              ) : (
                <NumberField label="径 mm" value={mepD} onChange={setMepD} />
              )}
              <NumberField
                label="標高 (Z) mm"
                value={mepElevation}
                onChange={setMepElevation}
              />
            </div>
          )}

          {tool === 'place' && (
            <div className="flex flex-wrap items-end gap-3 rounded border border-rule bg-paper-raised px-3 py-2 text-xs">
              <label className="flex flex-col gap-1">
                部品検索
                <input
                  type="text"
                  value={placeQuery}
                  onChange={(e) => setPlaceQuery(e.target.value)}
                  placeholder="送風機、ダンパー…"
                  className="rounded border border-rule bg-paper px-2 py-1"
                />
              </label>
              <label className="flex flex-col gap-1">
                部品
                <select
                  value={effectivePlacePart}
                  onChange={(e) => setPlacePartId(e.target.value)}
                  className="rounded border border-rule bg-paper px-2 py-1"
                >
                  {(partsQuery.data ?? []).map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name_ja}
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex flex-col gap-1">
                系統
                <select
                  value={effectiveSystem}
                  onChange={(e) => setMepSystem(e.target.value)}
                  className="rounded border border-rule bg-paper px-2 py-1"
                >
                  {(systemsQuery.data ?? []).map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name.ja}
                    </option>
                  ))}
                </select>
              </label>
            </div>
          )}

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
            {gripRubberBand.length > 0 && (
              <svg
                className="pointer-events-none absolute inset-0 h-full w-full"
                aria-hidden
              >
                {gripRubberBand.map((seg, i) => (
                  <line
                    key={i}
                    x1={seg.from.left}
                    y1={seg.from.top}
                    x2={seg.to.left}
                    y2={seg.to.top}
                    className="stroke-accent"
                    strokeWidth={2}
                    strokeDasharray="4 3"
                  />
                ))}
              </svg>
            )}
            {tool === 'select' &&
              gripScreens.map((g, i) => (
                <div
                  key={i}
                  onPointerDown={(e) => onGripPointerDown(e, i)}
                  onPointerUp={(e) => onGripPointerUp(e, i)}
                  className="absolute h-3 w-3 -translate-x-1/2 -translate-y-1/2 cursor-move border-2 border-accent bg-paper-raised"
                  style={{ left: g.left, top: g.top }}
                />
              ))}
            {snapPoint && (
              <div
                className="pointer-events-none absolute h-3.5 w-3.5 -translate-x-1/2 -translate-y-1/2 rotate-45 border-2 border-sys-hydronic bg-paper-raised"
                style={{ left: snapPoint.left, top: snapPoint.top }}
              />
            )}
            {mepPathScreen.length > 0 && (
              // Our own UI chrome, not the drawing's content — safe to draw
              // directly, unlike the rendered SVG itself (CLAUDE.md). May
              // include one extra point past the committed path: the live
              // rubber-band segment to wherever the next click would land.
              <svg
                className="pointer-events-none absolute inset-0 h-full w-full"
                aria-hidden
              >
                <polyline
                  points={mepPathScreen.map((p) => `${p.left},${p.top}`).join(' ')}
                  fill="none"
                  className="stroke-accent"
                  strokeWidth={2}
                  strokeDasharray="4 3"
                />
                {mepPathScreen.slice(0, mepPath.length).map((p, i) => (
                  <circle
                    key={mepPath[i] ? `${mepPath[i]?.x}-${mepPath[i]?.y}-${i}` : i}
                    cx={p.left}
                    cy={p.top}
                    r={4}
                    className="fill-paper-raised stroke-accent"
                    strokeWidth={2}
                  />
                ))}
              </svg>
            )}
            {polylinePathScreen.length > 0 && (
              <svg
                className="pointer-events-none absolute inset-0 h-full w-full"
                aria-hidden
              >
                <polyline
                  points={polylinePathScreen.map((p) => `${p.left},${p.top}`).join(' ')}
                  fill="none"
                  className="stroke-accent"
                  strokeWidth={2}
                  strokeDasharray="4 3"
                />
                {polylinePathScreen.slice(0, polylinePoints.length).map((p, i) => (
                  <circle
                    key={
                      polylinePoints[i]
                        ? `${polylinePoints[i]?.x}-${polylinePoints[i]?.y}-${i}`
                        : i
                    }
                    cx={p.left}
                    cy={p.top}
                    r={4}
                    className="fill-paper-raised stroke-accent"
                    strokeWidth={2}
                  />
                ))}
              </svg>
            )}
            {circlePreview && circlePreview.r > 0 && (
              <svg
                className="pointer-events-none absolute inset-0 h-full w-full"
                aria-hidden
              >
                <circle
                  cx={circlePreview.left}
                  cy={circlePreview.top}
                  r={circlePreview.r}
                  fill="none"
                  className="stroke-accent"
                  strokeWidth={2}
                  strokeDasharray="4 3"
                />
                <circle
                  cx={circlePreview.left}
                  cy={circlePreview.top}
                  r={3}
                  className="fill-paper-raised stroke-accent"
                  strokeWidth={2}
                />
              </svg>
            )}
            {arcPreview && (
              <svg
                className="pointer-events-none absolute inset-0 h-full w-full"
                aria-hidden
              >
                {arcPreview.spokes.map((p, i) => (
                  <line
                    key={i}
                    x1={arcPreview.center.left}
                    y1={arcPreview.center.top}
                    x2={p.left}
                    y2={p.top}
                    className="stroke-accent"
                    strokeWidth={2}
                    strokeDasharray="4 3"
                  />
                ))}
                <circle
                  cx={arcPreview.center.left}
                  cy={arcPreview.center.top}
                  r={3}
                  className="fill-paper-raised stroke-accent"
                  strokeWidth={2}
                />
              </svg>
            )}
            {mirrorLineScreen.length > 0 && (
              <svg
                className="pointer-events-none absolute inset-0 h-full w-full"
                aria-hidden
              >
                <polyline
                  points={mirrorLineScreen.map((p) => `${p.left},${p.top}`).join(' ')}
                  fill="none"
                  className="stroke-accent"
                  strokeWidth={2}
                  strokeDasharray="4 3"
                />
                {mirrorLineScreen.slice(0, mirrorPoints.length).map((p, i) => (
                  <circle
                    key={
                      mirrorPoints[i]
                        ? `${mirrorPoints[i]?.x}-${mirrorPoints[i]?.y}-${i}`
                        : i
                    }
                    cx={p.left}
                    cy={p.top}
                    r={4}
                    className="fill-paper-raised stroke-accent"
                    strokeWidth={2}
                  />
                ))}
              </svg>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function NumberField({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <label className="flex flex-col gap-1">
      {label}
      <input
        type="number"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="w-24 rounded border border-rule bg-paper px-2 py-1 tabular"
      />
    </label>
  );
}

/**
 * Center/start/end (the classic three-click arc) into the
 * center/radius/start_angle/sweep `Geometry::Arc` — and `add_arc` — actually
 * store. Sweep always goes counter-clockwise from start to end, matching the
 * DXF/`Geometry::Arc` convention; a `sweep` of exactly 0 (start and end
 * coincide) is normalised to a full turn rather than "no arc at all", since
 * a degenerate zero-length arc is never what three real clicks meant.
 */
function arcFromThreePoints(
  center: WorldPoint,
  start: WorldPoint,
  end: WorldPoint,
): { radius: number; startAngle: number; sweep: number } {
  const radius = Math.hypot(start.x - center.x, start.y - center.y);
  const startAngle = Math.atan2(start.y - center.y, start.x - center.x);
  const endAngle = Math.atan2(end.y - center.y, end.x - center.x);
  const twoPi = 2 * Math.PI;
  const sweep = (((endAngle - startAngle) % twoPi) + twoPi) % twoPi || twoPi;
  return { radius, startAngle, sweep };
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
