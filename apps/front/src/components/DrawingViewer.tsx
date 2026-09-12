import { useEffect, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { renderDrawing } from '../api.ts';

/**
 * Shows a drawing.
 *
 * The engine renders to SVG and the result is displayed in an `<img>`, not
 * inlined into the page. A drawing is someone else's file: inlining it would
 * make its text content a script vector, and an image element runs nothing.
 *
 * Pan and zoom are CSS transforms over the rendered image rather than a
 * re-render per frame — interaction stays smooth, and a round trip to the
 * engine is reserved for the things that actually change the drawing, like
 * turning a layer off.
 */

interface Props {
  file: File;
  layers: string[];
  visibleLayers: Set<string>;
  dark: boolean;
}

interface View {
  scale: number;
  x: number;
  y: number;
}

const HOME: View = { scale: 1, x: 0, y: 0 };

export function DrawingViewer({ file, layers, visibleLayers, dark }: Props) {
  const [view, setView] = useState<View>(HOME);
  const [dragging, setDragging] = useState(false);
  const dragOrigin = useRef<{ x: number; y: number } | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  // Only a filter change re-renders; panning and zooming do not.
  const filter =
    visibleLayers.size === layers.length ? '' : [...visibleLayers].sort().join(',');

  const render = useQuery({
    queryKey: ['render', file.name, file.size, file.lastModified, dark, filter],
    queryFn: () =>
      renderDrawing(file, { dark, ...(filter ? { layers: filter.split(',') } : {}) }),
    // The result is an object URL owned by this component, so it must not be
    // handed out again after the effect below has revoked it.
    gcTime: 0,
    staleTime: Infinity,
    retry: false,
  });

  // Release the previous image when a new one replaces it, and on unmount.
  const url = render.data;
  useEffect(() => {
    if (!url) return;
    return () => URL.revokeObjectURL(url);
  }, [url]);

  // A native listener, not React's `onWheel` prop: React attaches wheel
  // handlers at the root as passive since v17, so `e.preventDefault()`
  // inside a synthetic handler is silently ignored (and logs a console
  // warning) — the page behind the viewer scrolls right along with the
  // zoom. `{ passive: false }` here is what actually stops that.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    function handleWheel(e: WheelEvent) {
      e.preventDefault();
      // Zoom towards the cursor, which is what every CAD application does
      // and what makes zooming feel like moving rather than jumping.
      const rect = (e.currentTarget as HTMLDivElement).getBoundingClientRect();
      const px = e.clientX - rect.left;
      const py = e.clientY - rect.top;
      const factor = Math.exp(-e.deltaY * 0.0015);

      setView((v) => {
        const scale = Math.min(Math.max(v.scale * factor, 0.05), 200);
        const actual = scale / v.scale;
        return {
          scale,
          x: px - (px - v.x) * actual,
          y: py - (py - v.y) * actual,
        };
      });
    }

    el.addEventListener('wheel', handleWheel, { passive: false });
    return () => el.removeEventListener('wheel', handleWheel);
  }, []);

  return (
    <div className="space-y-2">
      <div
        ref={containerRef}
        className="relative h-[32rem] overflow-hidden rounded border border-rule bg-paper-raised"
        style={{ cursor: dragging ? 'grabbing' : 'grab', touchAction: 'none' }}
        onPointerDown={(e) => {
          dragOrigin.current = { x: e.clientX - view.x, y: e.clientY - view.y };
          setDragging(true);
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          const start = dragOrigin.current;
          if (!start) return;
          setView((v) => ({ ...v, x: e.clientX - start.x, y: e.clientY - start.y }));
        }}
        onPointerUp={() => {
          dragOrigin.current = null;
          setDragging(false);
        }}
      >
        {render.isPending && (
          <p className="absolute inset-0 grid place-items-center text-sm text-ink-muted">
            描画中…
          </p>
        )}

        {render.isError && (
          <p className="absolute inset-0 grid place-items-center px-6 text-center text-sm text-sys-fire">
            {render.error.message}
          </p>
        )}

        {url && !render.isError && (
          <img
            src={url}
            alt="図面"
            draggable={false}
            className="absolute top-0 left-0 h-full w-full origin-top-left object-contain select-none"
            style={{
              transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})`,
              transition: dragging ? 'none' : 'transform 60ms linear',
            }}
          />
        )}
      </div>

      <div className="flex flex-wrap items-center gap-3 text-xs text-ink-muted">
        <button
          type="button"
          onClick={() => setView(HOME)}
          className="rounded border border-rule px-2 py-1 text-ink"
        >
          全体表示
        </button>
        <span className="tabular">{(view.scale * 100).toFixed(0)}%</span>
        <span>ドラッグで移動、ホイールで拡大縮小</span>
      </div>
    </div>
  );
}
