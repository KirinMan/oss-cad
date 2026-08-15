import { useState } from 'react';
import { Link, useParams } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { categoryLabels, profileLabel, systemKindLabels } from '@opendraft/shared';
import { fetchPart } from '../api.ts';
import { SymbolPreview } from '../components/SymbolPreview.tsx';

export function PartPage() {
  const { partId } = useParams({ from: '/parts/$partId' });
  const [overrides, setOverrides] = useState<Record<string, number>>({});

  const part = useQuery({
    queryKey: ['part', partId, overrides],
    queryFn: () => fetchPart(partId, overrides),
    // Keeping the previous drawing on screen while a new size is fetched stops
    // the preview flashing empty on every slider step.
    placeholderData: (prev) => prev,
  });

  if (part.isError) {
    return (
      <div className="space-y-4">
        <Link to="/" className="text-sm text-accent">
          ← 部品一覧
        </Link>
        <p className="rounded border border-sys-fire/40 bg-sys-fire/10 px-4 py-3 text-sm">
          {part.error.message}
        </p>
      </div>
    );
  }

  if (!part.data) {
    return <p className="text-sm text-ink-muted">読み込み中…</p>;
  }

  const detail = part.data;
  const [, , , maxX, maxY] = detail.bounds_mm;
  const size = {
    x: maxX - detail.bounds_mm[0],
    y: maxY - detail.bounds_mm[1],
    z: detail.bounds_mm[5] - detail.bounds_mm[2],
  };

  return (
    <div className="space-y-6">
      <div>
        <Link to="/" className="text-sm text-accent">
          ← 部品一覧
        </Link>
        <h1 className="mt-2 text-2xl font-semibold tracking-tight">
          {detail.part.name.ja}
        </h1>
        <p className="mt-1 font-mono text-sm text-ink-muted">{detail.part.id}</p>
        <div className="mt-2 flex flex-wrap gap-2 text-xs">
          <Tag>{categoryLabels[detail.part.category]}</Tag>
          <Tag>{detail.part.ifc_class}</Tag>
          {detail.part.source && <Tag>出典: {detail.part.source}</Tag>}
        </div>
      </div>

      <div className="grid gap-6 lg:grid-cols-[1fr_20rem]">
        <div className="rounded border border-rule bg-paper-raised p-4">
          <SymbolPreview detail={detail} className="h-80 w-full text-ink" />
          <p className="mt-2 text-center text-xs text-ink-muted tabular">
            {size.x.toFixed(0)} × {size.y.toFixed(0)} × {size.z.toFixed(0)} mm
          </p>
        </div>

        <div className="space-y-4">
          <section>
            <h2 className="mb-2 text-xs font-semibold tracking-widest text-ink-muted uppercase">
              寸法
            </h2>
            <div className="space-y-3">
              {detail.part.parameters.map((p) => {
                const value = detail.parameters[p.name] ?? 0;
                const min = p.min ?? Math.max(1, value * 0.25);
                const max = p.max ?? Math.max(value * 4, min + 1);
                return (
                  <div key={p.name}>
                    <label
                      htmlFor={`param-${p.name}`}
                      className="flex items-baseline justify-between text-sm"
                    >
                      <span>
                        {p.label.ja}
                        <span className="ml-1.5 font-mono text-xs text-ink-muted">
                          {p.name}
                        </span>
                      </span>
                      <span className="font-mono tabular">
                        {value.toFixed(0)} {p.unit}
                      </span>
                    </label>
                    <input
                      id={`param-${p.name}`}
                      type="range"
                      min={min}
                      max={max}
                      step={max - min > 500 ? 25 : 5}
                      value={value}
                      onChange={(e) =>
                        setOverrides((o) => ({ ...o, [p.name]: Number(e.target.value) }))
                      }
                      className="mt-1 w-full accent-accent"
                    />
                  </div>
                );
              })}
              {Object.keys(overrides).length > 0 && (
                <button
                  type="button"
                  onClick={() => setOverrides({})}
                  className="text-xs text-accent underline-offset-2 hover:underline"
                >
                  既定値に戻す
                </button>
              )}
            </div>
          </section>

          <section>
            <h2 className="mb-2 text-xs font-semibold tracking-widest text-ink-muted uppercase">
              接続口
            </h2>
            <ul className="space-y-1 text-sm">
              {detail.ports.map((p) => (
                <li key={p.name} className="flex items-baseline justify-between gap-2">
                  <span className="font-mono text-xs">{p.name}</span>
                  <span className="text-right">
                    {profileLabel(p.profile)}
                    <span className="ml-2 text-xs text-ink-muted">
                      {systemKindLabels[p.system_kind]}
                    </span>
                  </span>
                </li>
              ))}
              {detail.ports.length === 0 && (
                <li className="text-xs text-ink-muted">接続口なし（支持金物等）</li>
              )}
            </ul>
          </section>
        </div>
      </div>

      {detail.part.properties.length > 0 && (
        <section>
          <h2 className="mb-2 text-xs font-semibold tracking-widest text-ink-muted uppercase">
            属性と IFC 対応
          </h2>
          <div className="overflow-x-auto rounded border border-rule">
            <table className="w-full text-sm">
              <thead className="bg-rule/30 text-left text-xs text-ink-muted">
                <tr>
                  <th className="px-3 py-2 font-medium">属性</th>
                  <th className="px-3 py-2 font-medium">型</th>
                  <th className="px-3 py-2 font-medium">単位</th>
                  <th className="px-3 py-2 font-medium">IFC プロパティ</th>
                </tr>
              </thead>
              <tbody>
                {detail.part.properties.map((p) => (
                  <tr key={p.name} className="border-t border-rule">
                    <td className="px-3 py-2">
                      {p.label.ja}
                      <span className="ml-2 font-mono text-xs text-ink-muted">
                        {p.name}
                      </span>
                    </td>
                    <td className="px-3 py-2 font-mono text-xs">{p.ty}</td>
                    <td className="px-3 py-2 font-mono text-xs">{p.unit ?? '—'}</td>
                    <td className="px-3 py-2 font-mono text-xs break-all">
                      {p.ifc_property}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="mt-2 text-xs text-ink-muted">
            すべての属性に IFC
            の写像が定義されています。写像のない属性は部品として登録できないため、書き出し時に落ちることがありません。
          </p>
        </section>
      )}
    </div>
  );
}

function Tag({ children }: { children: React.ReactNode }) {
  return <span className="rounded bg-rule/50 px-2 py-0.5 font-mono">{children}</span>;
}
