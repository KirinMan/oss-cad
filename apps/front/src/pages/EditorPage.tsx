import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import type { Command, Profile, QueryHit } from '@opendraft/shared';
import {
  editDrawing,
  fetchMepPorts,
  fetchParts,
  fetchSpecs,
  fetchSystems,
  inspectDrawing,
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

type Tool =
  | 'line'
  | 'circle'
  | 'arc'
  | 'polyline'
  | 'text'
  | 'dimension'
  | 'insert'
  | 'select'
  | 'mep'
  | 'place';
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

/**
 * The command line's tool-switching vocabulary — each tool's own Japanese
 * label (what its toolbar button already says, so anything visible is also
 * typeable) plus a short English alias in the AutoCAD command-line tradition
 * (`L`, `C`, `A`, ...). Keys are matched case-insensitively; Japanese text is
 * unaffected by `toLowerCase()`, so one lookup covers both.
 */
const TOOL_ALIASES: Record<string, Tool> = {
  線分: 'line',
  l: 'line',
  line: 'line',
  円: 'circle',
  c: 'circle',
  circle: 'circle',
  円弧: 'arc',
  a: 'arc',
  arc: 'arc',
  ポリライン: 'polyline',
  pl: 'polyline',
  polyline: 'polyline',
  文字: 'text',
  t: 'text',
  text: 'text',
  寸法: 'dimension',
  dim: 'dimension',
  dimension: 'dimension',
  挿入: 'insert',
  i: 'insert',
  insert: 'insert',
  選択: 'select',
  s: 'select',
  select: 'select',
  esc: 'select',
  配管: 'mep',
  m: 'mep',
  mep: 'mep',
  配置: 'place',
  p: 'place',
  place: 'place',
};

/** `x,y` in world millimetres — the command line's point-entry syntax. */
const POINT_PATTERN = /^(-?[\d.]+)\s*,\s*(-?[\d.]+)$/;

/** Below this distance from the reference point, an angle is meaningless — polar tracking stays off rather than guessing one. */
const MIN_POLAR_DISTANCE_MM = 1;

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
  const [commandInput, setCommandInput] = useState('');

  // ── Polar tracking ──────────────────────────────────────────────────────
  // A fallback for when object snap finds nothing: near a multiple of the
  // increment away from whatever point a tool is currently measuring from,
  // the cursor snaps onto that exact angle rather than wherever the pixel
  // grid happened to land. Object snap always wins when both apply — an
  // existing point in the drawing is a stronger signal than an angle guess.
  const [polarEnabled, setPolarEnabled] = useState(true);
  const [polarIncrementDeg, setPolarIncrementDeg] = useState('15');
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

  // ── Text ────────────────────────────────────────────────────────────────
  // A click places the position and opens the settings panel; the entity
  // itself isn't drawn until 配置 is pressed, since there's nothing to draw
  // with an empty string.
  const [textPosition, setTextPosition] = useState<WorldPoint | null>(null);
  const [textValue, setTextValue] = useState('');
  const [textHeight, setTextHeight] = useState('250');

  // ── Dimension ───────────────────────────────────────────────────────────
  // Three clicks: the two measured points, then anywhere to place the
  // dimension line — the third click's perpendicular distance from the
  // point_a–point_b segment becomes the signed offset, computed client-side
  // and sent as a plain number rather than a third point, since od-core's
  // own DimensionEntity stores exactly that (not a placement point).
  const [dimPoints, setDimPoints] = useState<WorldPoint[]>([]);

  // ── Blocks ──────────────────────────────────────────────────────────────
  // ブロック化 groups the current 選択 into a new block, named through a
  // small inline panel rather than a browser prompt() (which blocks the
  // whole extension — see claude-in-chrome's own dialog warning, and this
  // app already avoids native dialogs everywhere else). 挿入 needs to know
  // what block names the open document already defines, which — unlike
  // every other per-document query so far — comes from inspectDrawing
  // rather than a dedicated endpoint, since od inspect already reports
  // exactly this list.
  const [blockCreating, setBlockCreating] = useState(false);
  const [blockNameInput, setBlockNameInput] = useState('');
  const [insertBlockName, setInsertBlockName] = useState('');
  const blockNamesQuery = useQuery({
    queryKey: ['blockNames', current?.url ?? null],
    queryFn: () => inspectDrawing(current!.file),
    enabled: current !== null,
  });
  const effectiveInsertBlock =
    insertBlockName || (blockNamesQuery.data?.block_names[0] ?? '');

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
    setTextPosition(null);
    setTextValue('');
    setDimPoints([]);
    setBlockCreating(false);
    setBlockNameInput('');
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

  /**
   * The command line's own Enter handler — a tool alias switches tools; an
   * `x,y` point is fed to whichever tool is already active exactly like a
   * click would be (`handlePoint`, shared with `onCanvasClick`); `u`/`undo`
   * undoes. Anything else is reported through the same error banner a
   * failed edit already uses, rather than a second UI for "something went
   * wrong."
   */
  function runCommand() {
    const raw = commandInput.trim();
    setCommandInput('');
    if (!raw) return;

    if (raw.toLowerCase() === 'u' || raw.toLowerCase() === 'undo') {
      undo();
      return;
    }

    const pointMatch = POINT_PATTERN.exec(raw);
    if (pointMatch) {
      const [, xText, yText] = pointMatch;
      const x = Number(xText);
      const y = Number(yText);
      if (Number.isFinite(x) && Number.isFinite(y)) {
        handlePoint({ x, y });
        return;
      }
    }

    const nextTool = TOOL_ALIASES[raw.toLowerCase()];
    if (nextTool) {
      switchTool(nextTool);
      return;
    }

    setError(`不明なコマンド: ${raw}`);
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
      tool === 'text' ||
      tool === 'dimension' ||
      tool === 'insert' ||
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
    handlePoint(point);
  }

  /**
   * Everything a click does once a world point is known — factored out so
   * the command line's typed `x,y` entry can feed the active tool the exact
   * same way a mouse click does, rather than duplicating every tool's logic
   * a second time.
   */
  function handlePoint(point: WorldPoint) {
    if (!current || edit.isPending) return;

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

    if (tool === 'text') {
      // Clicking again before 配置 just relocates the pending text, rather
      // than starting a second one — there is only ever one point to place.
      setTextPosition(point);
      return;
    }

    if (tool === 'dimension') {
      if (dimPoints.length < 2) {
        setDimPoints((p) => [...p, point]);
        return;
      }
      const [a, b] = dimPoints;
      if (a && b) {
        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const len = Math.hypot(dx, dy);
        if (len > 0) {
          // Signed perpendicular distance from the third click to the
          // a→b segment — the same (-dy, dx)/len basis
          // DimensionEntity::dimension_line uses on the Rust side, so a
          // positive offset here lands the dimension line on the same side
          // there would compute.
          const offset = ((point.x - a.x) * -dy + (point.y - a.y) * dx) / len;
          edit.mutate({
            kind: 'add_dimension',
            layer,
            point_a: { x: a.x, y: a.y, z: 0 },
            point_b: { x: b.x, y: b.y, z: 0 },
            offset,
          });
        }
      }
      setDimPoints([]);
      return;
    }

    if (tool === 'insert') {
      if (!effectiveInsertBlock || edit.isPending) return;
      edit.mutate({
        kind: 'insert_block',
        layer,
        block_name: effectiveInsertBlock,
        position: { x: point.x, y: point.y, z: 0 },
        rotation: 0,
        scale: { x: 1, y: 1, z: 1 },
      });
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

  function placeText() {
    if (!textPosition || edit.isPending) return;
    const height = Number(textHeight);
    if (!textValue.trim() || !Number.isFinite(height) || height <= 0) return;
    edit.mutate({
      kind: 'add_text',
      layer,
      position: { x: textPosition.x, y: textPosition.y, z: 0 },
      text: textValue,
      height,
      rotation: 0,
    });
    setTextPosition(null);
    setTextValue('');
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

  function createBlockFromSelection() {
    const name = blockNameInput.trim();
    if (!name || selection.length === 0 || edit.isPending) return;
    // The bounding-box centre of the whole selection — a reasonable base
    // point without an extra "pick a point" click this tool doesn't have
    // yet (od-core and commandSchema both accept an arbitrary base_point;
    // only this UI's choice of which one is narrow).
    const minX = Math.min(...selection.map((s) => s.bounds_mm[0]));
    const minY = Math.min(...selection.map((s) => s.bounds_mm[1]));
    const maxX = Math.max(...selection.map((s) => s.bounds_mm[3]));
    const maxY = Math.max(...selection.map((s) => s.bounds_mm[4]));
    edit.mutate({
      kind: 'create_block',
      name,
      layer,
      base_point: { x: (minX + maxX) / 2, y: (minY + maxY) / 2, z: 0 },
      ids: selection.map((s) => s.id),
    });
    setBlockCreating(false);
    setBlockNameInput('');
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
    /** Set when this snap came from polar tracking, not object snap. */
    polar?: boolean;
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
  const [dimPathScreen, setDimPathScreen] = useState<{ left: number; top: number }[]>([]);
  const [gripScreens, setGripScreens] = useState<{ left: number; top: number }[]>([]);
  // Dashed lines from the dragged vertex's immediate neighbours (in the
  // entity's own vertex order — a line's other endpoint, or a polyline
  // vertex's one or two neighbours) to wherever it would land right now.
  // Not a full redraw of the entity: neighbours are the only geometry that
  // actually changes shape while a single vertex moves.
  const [gripRubberBand, setGripRubberBand] = useState<
    { from: { left: number; top: number }; to: { left: number; top: number } }[]
  >([]);
  const [polarGuide, setPolarGuide] = useState<{
    from: { left: number; top: number };
    to: { left: number; top: number };
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
      setMepPathScreen([]);
      setPolylinePathScreen([]);
      setCirclePreview(null);
      setArcPreview(null);
      setMirrorLineScreen([]);
      setGripScreens([]);
      setGripRubberBand([]);
      setDimPathScreen([]);
      setPolarGuide(null);
      return;
    }

    const markerWorld = pendingStart ?? (tool === 'text' ? textPosition : null);
    setMarker(markerWorld ? toScreen(fit, markerWorld.x, markerWorld.y) : null);

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
      polar?: boolean;
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

      // Object snap always wins; polar tracking only ever fills in when it
      // found nothing. The reference point is whatever the active tool is
      // currently measuring from — the same point its own rubber-band
      // preview already draws from.
      if (!snap && polarEnabled) {
        const reference =
          tool === 'line' || tool === 'circle'
            ? pendingStart
            : tool === 'arc'
              ? (arcPoints[arcPoints.length - 1] ?? null)
              : tool === 'polyline'
                ? (polylinePoints[polylinePoints.length - 1] ?? null)
                : tool === 'dimension'
                  ? (dimPoints[dimPoints.length - 1] ?? null)
                  : tool === 'mep'
                    ? (mepPath[mepPath.length - 1] ?? null)
                    : null;
        if (reference) {
          const dx = hoverPoint.x - reference.x;
          const dy = hoverPoint.y - reference.y;
          const dist = Math.hypot(dx, dy);
          const incrementRad = ((Number(polarIncrementDeg) || 15) * Math.PI) / 180;
          // Within ~2° of a multiple of the increment counts as "on" that
          // angle — narrow enough that the exact angle stays intentional, a
          // fixed angular tolerance rather than a screen-pixel one since the
          // whole point is precision independent of zoom level.
          const angleToleranceRad = (2 * Math.PI) / 180;
          if (dist > MIN_POLAR_DISTANCE_MM) {
            const angle = Math.atan2(dy, dx);
            const nearest = Math.round(angle / incrementRad) * incrementRad;
            const angularDiff = Math.abs(
              Math.atan2(Math.sin(angle - nearest), Math.cos(angle - nearest)),
            );
            if (angularDiff <= angleToleranceRad) {
              const world = {
                x: reference.x + dist * Math.cos(nearest),
                y: reference.y + dist * Math.sin(nearest),
              };
              snap = { ...toScreen(fit, world.x, world.y), world, polar: true };
              setPolarGuide({
                from: toScreen(fit, reference.x, reference.y),
                to: toScreen(fit, world.x, world.y),
              });
            }
          }
        }
      }
    }
    if (!snap?.polar) setPolarGuide(null);
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

    if (tool === 'dimension' && dimPoints.length > 0) {
      const rubberBand = snap?.world ?? hoverPoint;
      const points = dimPoints.map((p) => toScreen(fit, p.x, p.y));
      if (rubberBand) points.push(toScreen(fit, rubberBand.x, rubberBand.y));
      setDimPathScreen(points);
    } else {
      setDimPathScreen([]);
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
    textPosition,
    dimPoints,
    polarEnabled,
    polarIncrementDeg,
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
              <ToolButton active={tool === 'text'} onClick={() => switchTool('text')}>
                文字
              </ToolButton>
              <ToolButton
                active={tool === 'dimension'}
                onClick={() => switchTool('dimension')}
              >
                寸法
              </ToolButton>
              <ToolButton active={tool === 'insert'} onClick={() => switchTool('insert')}>
                挿入
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

            {tool === 'text' && (
              <span>{textPosition ? '文字を入力して配置' : '配置位置をクリック'}</span>
            )}

            {tool === 'dimension' && (
              <span>
                {dimPoints.length === 0 && '始点をクリック'}
                {dimPoints.length === 1 && '終点をクリック'}
                {dimPoints.length === 2 && '寸法線の位置をクリック'}
              </span>
            )}

            {tool === 'insert' && (
              <span>
                {effectiveInsertBlock
                  ? 'クリックして挿入'
                  : 'このドキュメントにブロック定義がありません'}
              </span>
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
                {blockCreating ? (
                  <>
                    <input
                      type="text"
                      value={blockNameInput}
                      onChange={(e) => setBlockNameInput(e.target.value)}
                      placeholder="ブロック名"
                      autoFocus
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') createBlockFromSelection();
                      }}
                      className="w-32 rounded border border-rule bg-paper px-2 py-1"
                    />
                    <button
                      type="button"
                      onClick={createBlockFromSelection}
                      disabled={!blockNameInput.trim() || edit.isPending}
                      className="rounded border border-accent bg-accent/10 px-2 py-1 text-ink disabled:opacity-40"
                    >
                      作成
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        setBlockCreating(false);
                        setBlockNameInput('');
                      }}
                      className="rounded border border-rule px-2 py-1 text-ink"
                    >
                      キャンセル
                    </button>
                  </>
                ) : (
                  <button
                    type="button"
                    onClick={() => setBlockCreating(true)}
                    className="rounded border border-rule px-2 py-1 text-ink"
                  >
                    ブロック化
                  </button>
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
                  : snapPoint.polar
                    ? `極トラッキング中 (${polarIncrementDeg}°)`
                    : 'スナップ中'}{' '}
                ({snapPoint.world.x.toFixed(0)}, {snapPoint.world.y.toFixed(0)})
              </span>
            )}

            <label className="flex items-center gap-1 text-ink-muted">
              <input
                type="checkbox"
                checked={polarEnabled}
                onChange={(e) => setPolarEnabled(e.target.checked)}
              />
              極トラッキング
            </label>
            {polarEnabled && (
              <select
                value={polarIncrementDeg}
                onChange={(e) => setPolarIncrementDeg(e.target.value)}
                className="rounded border border-rule bg-paper px-1 py-0.5"
              >
                <option value="90">90°</option>
                <option value="45">45°</option>
                <option value="30">30°</option>
                <option value="15">15°</option>
              </select>
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

          <div className="flex items-center gap-2 text-xs">
            <span className="text-ink-muted">コマンド</span>
            <input
              type="text"
              value={commandInput}
              onChange={(e) => setCommandInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') runCommand();
              }}
              placeholder="l / 線分 / 100,200 / u"
              className="flex-1 rounded border border-rule bg-paper px-2 py-1 font-mono"
            />
          </div>

          {tool === 'text' && textPosition && (
            <div className="flex flex-wrap items-end gap-3 rounded border border-rule bg-paper-raised px-3 py-2 text-xs">
              <label className="flex flex-col gap-1">
                文字
                <input
                  type="text"
                  value={textValue}
                  onChange={(e) => setTextValue(e.target.value)}
                  placeholder="M-DUCT-SA"
                  autoFocus
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') placeText();
                  }}
                  className="w-48 rounded border border-rule bg-paper px-2 py-1"
                />
              </label>
              <NumberField
                label="文字高さ mm"
                value={textHeight}
                onChange={setTextHeight}
              />
              <button
                type="button"
                onClick={placeText}
                disabled={!textValue.trim() || edit.isPending}
                className="rounded border border-accent bg-accent/10 px-2 py-1 text-ink disabled:opacity-40"
              >
                配置
              </button>
            </div>
          )}

          {tool === 'insert' && (blockNamesQuery.data?.block_names.length ?? 0) > 0 && (
            <div className="flex flex-wrap items-end gap-3 rounded border border-rule bg-paper-raised px-3 py-2 text-xs">
              <label className="flex flex-col gap-1">
                ブロック
                <select
                  value={effectiveInsertBlock}
                  onChange={(e) => setInsertBlockName(e.target.value)}
                  className="rounded border border-rule bg-paper px-2 py-1"
                >
                  {(blockNamesQuery.data?.block_names ?? []).map((name) => (
                    <option key={name} value={name}>
                      {name}
                    </option>
                  ))}
                </select>
              </label>
            </div>
          )}

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
            {polarGuide && (
              <svg
                className="pointer-events-none absolute inset-0 h-full w-full"
                aria-hidden
              >
                <line
                  x1={polarGuide.from.left}
                  y1={polarGuide.from.top}
                  x2={polarGuide.to.left}
                  y2={polarGuide.to.top}
                  className="stroke-sys-hydronic"
                  strokeWidth={1}
                  strokeDasharray="6 4"
                />
              </svg>
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
            {dimPathScreen.length > 0 && (
              <svg
                className="pointer-events-none absolute inset-0 h-full w-full"
                aria-hidden
              >
                <polyline
                  points={dimPathScreen.map((p) => `${p.left},${p.top}`).join(' ')}
                  fill="none"
                  className="stroke-accent"
                  strokeWidth={2}
                  strokeDasharray="4 3"
                />
                {dimPathScreen.slice(0, dimPoints.length).map((p, i) => (
                  <circle
                    key={dimPoints[i] ? `${dimPoints[i]?.x}-${dimPoints[i]?.y}-${i}` : i}
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
