import type { PartDetail, ResolvedPort, SymbolGeometry } from '@opendraft/shared';

/**
 * Draws a part's plan symbol.
 *
 * The whole drawing is emitted in millimetres inside a group flipped in Y, so
 * the geometry needs no conversion — what the engine computed is what is drawn,
 * and a mistake here cannot be mistaken for a mistake in the part definition.
 */

interface Props {
  detail: PartDetail;
  className?: string;
  showPorts?: boolean;
}

export function SymbolPreview({ detail, className, showPorts = true }: Props) {
  const box = boundsOf(detail);
  if (!box) {
    return (
      <div
        className={`grid place-items-center text-sm text-ink-muted ${className ?? ''}`}
      >
        この部品には平面記号がありません
      </div>
    );
  }

  const pad = Math.max(box.width, box.height) * 0.12 + 20;
  const viewBox = [
    box.minX - pad,
    -(box.maxY + pad),
    box.width + pad * 2,
    box.height + pad * 2,
  ].join(' ');

  // One stroke width for the whole drawing, in drawing units, so lines stay
  // even at any zoom.
  const stroke = Math.max(box.width, box.height) / 160;

  return (
    <svg
      viewBox={viewBox}
      className={className}
      role="img"
      aria-label={`${detail.part.name.ja} の平面記号`}
      preserveAspectRatio="xMidYMid meet"
    >
      <g transform="scale(1 -1)" fill="none" strokeLinecap="round" strokeLinejoin="round">
        {detail.symbol.map((g, i) => (
          <Shape key={i} geom={g} stroke={stroke} />
        ))}
        {showPorts &&
          detail.ports.map((p) => <Port key={p.name} port={p} size={stroke * 6} />)}
      </g>
    </svg>
  );
}

function Shape({ geom, stroke }: { geom: SymbolGeometry; stroke: number }) {
  const common = {
    stroke: 'currentColor',
    strokeWidth: stroke,
  };

  switch (geom.kind) {
    case 'line':
      return <line x1={geom.a.x} y1={geom.a.y} x2={geom.b.x} y2={geom.b.y} {...common} />;

    case 'circle':
      return <circle cx={geom.center.x} cy={geom.center.y} r={geom.radius} {...common} />;

    case 'arc': {
      const { center, radius, start_angle: start, sweep } = geom;
      // A full turn has no two distinct endpoints, so it is drawn as a circle.
      if (Math.abs(sweep) >= Math.PI * 2 - 1e-9) {
        return <circle cx={center.x} cy={center.y} r={radius} {...common} />;
      }
      const x1 = center.x + radius * Math.cos(start);
      const y1 = center.y + radius * Math.sin(start);
      const x2 = center.x + radius * Math.cos(start + sweep);
      const y2 = center.y + radius * Math.sin(start + sweep);
      const largeArc = Math.abs(sweep) > Math.PI ? 1 : 0;
      // Inside the Y-flipped group, a mathematically counter-clockwise sweep is
      // SVG's positive direction.
      const sweepFlag = sweep > 0 ? 1 : 0;
      return (
        <path
          d={`M ${x1} ${y1} A ${radius} ${radius} 0 ${largeArc} ${sweepFlag} ${x2} ${y2}`}
          {...common}
        />
      );
    }

    case 'polyline': {
      const d = polylinePath(geom.polyline);
      return d ? <path d={d} {...common} /> : null;
    }
  }
}

/** Builds a path, turning each bulge into an arc segment. */
function polylinePath(pl: {
  vertices: { point: { x: number; y: number }; bulge: number }[];
  closed: boolean;
}): string {
  const v = pl.vertices;
  if (v.length < 2) return '';

  const first = v[0];
  if (!first) return '';
  let d = `M ${first.point.x} ${first.point.y}`;

  const spans = pl.closed ? v.length : v.length - 1;
  for (let i = 0; i < spans; i++) {
    const from = v[i];
    const to = v[(i + 1) % v.length];
    if (!from || !to) continue;

    if (Math.abs(from.bulge) < 1e-12) {
      d += ` L ${to.point.x} ${to.point.y}`;
      continue;
    }
    // bulge = tan(sweep / 4).
    const sweep = 4 * Math.atan(from.bulge);
    const chord = Math.hypot(to.point.x - from.point.x, to.point.y - from.point.y);
    const radius = Math.abs(chord / (2 * Math.sin(sweep / 2)));
    const largeArc = Math.abs(sweep) > Math.PI ? 1 : 0;
    const sweepFlag = sweep > 0 ? 1 : 0;
    d += ` A ${radius} ${radius} 0 ${largeArc} ${sweepFlag} ${to.point.x} ${to.point.y}`;
  }
  if (pl.closed) d += ' Z';
  return d;
}

/** A port is drawn as a tick pointing the way a run leaves the part. */
function Port({ port, size }: { port: ResolvedPort; size: number }) {
  const [x, y] = port.origin;
  const [dx, dy] = port.direction;
  const length = Math.hypot(dx, dy);
  // A port facing straight up or down has no direction to draw in plan.
  const scale = length > 1e-9 ? size / length : 0;

  return (
    <g className={portColor(port.system_kind)}>
      <circle cx={x} cy={y} r={size * 0.35} fill="currentColor" stroke="none" />
      {scale > 0 && (
        <line
          x1={x}
          y1={y}
          x2={x + dx * scale}
          y2={y + dy * scale}
          stroke="currentColor"
          strokeWidth={size * 0.22}
        />
      )}
    </g>
  );
}

function portColor(kind: ResolvedPort['system_kind']): string {
  switch (kind) {
    case 'air':
      return 'text-sys-air';
    case 'water':
      return 'text-sys-water';
    case 'drainage':
      return 'text-sys-drainage';
    case 'hydronic':
      return 'text-sys-hydronic';
    case 'power':
    case 'signal':
      return 'text-sys-power';
    case 'fire_protection':
      return 'text-sys-fire';
    case 'gas':
    case 'generic':
      return 'text-ink-muted';
  }
}

function boundsOf(detail: PartDetail): {
  minX: number;
  maxY: number;
  width: number;
  height: number;
} | null {
  const [minX, minY, , maxX, maxY] = detail.bounds_mm;
  const width = maxX - minX;
  const height = maxY - minY;
  if (!Number.isFinite(width) || !Number.isFinite(height)) return null;
  if (width <= 0 && height <= 0) return null;
  // A part that is long and thin still needs a box with area.
  const w = width > 0 ? width : height * 0.1;
  const h = height > 0 ? height : width * 0.1;
  return { minX, maxY: minY + h, width: w, height: h };
}
