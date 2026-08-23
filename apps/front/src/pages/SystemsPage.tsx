import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { systemKindLabels } from '@opendraft/shared';
import { fetchSpecs, fetchSystems } from '../api.ts';

export function SystemsPage() {
  const systems = useQuery({ queryKey: ['systems'], queryFn: fetchSystems });
  const specs = useQuery({ queryKey: ['specs'], queryFn: fetchSpecs });
  const [openSpec, setOpenSpec] = useState<string | null>(null);

  return (
    <div className="space-y-8">
      <section className="space-y-3">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">系統</h1>
          <p className="mt-1 text-sm text-ink-muted">
            系統ごとに色・レイヤ・標準高さが決まっています。作図時の既定値であり、社内標準に合わせて差し替えられます。
          </p>
        </div>

        {systems.isPending && <p className="text-sm text-ink-muted">読み込み中…</p>}

        {systems.data && (
          <div className="overflow-x-auto rounded border border-rule">
            <table className="w-full text-sm">
              <thead className="bg-rule/30 text-left text-xs text-ink-muted">
                <tr>
                  <th className="px-3 py-2 font-medium">系統</th>
                  <th className="px-3 py-2 font-medium">略号</th>
                  <th className="px-3 py-2 font-medium">区分</th>
                  <th className="px-3 py-2 font-medium">レイヤ</th>
                  <th className="px-3 py-2 text-right font-medium">標準高さ</th>
                </tr>
              </thead>
              <tbody>
                {systems.data.map((s) => (
                  <tr key={s.id} className="border-t border-rule">
                    <td className="px-3 py-2">{s.name.ja}</td>
                    <td className="px-3 py-2 font-mono text-xs">{s.abbreviation}</td>
                    <td className="px-3 py-2 text-xs">{systemKindLabels[s.kind]}</td>
                    <td className="px-3 py-2 font-mono text-xs">{s.layer}</td>
                    <td className="px-3 py-2 text-right font-mono text-xs tabular">
                      {s.default_elevation > 0 ? `FL+${s.default_elevation}` : '—'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      <section className="space-y-3">
        <div>
          <h2 className="text-xl font-semibold tracking-tight">仕様</h2>
          <p className="mt-1 text-sm text-ink-muted">
            材種ごとの継手・曲げ半径・定尺長・規格サイズ。作図した経路はここから継手を自動生成します。
          </p>
        </div>

        {specs.data?.map((spec) => {
          const open = openSpec === spec.id;
          return (
            <div key={spec.id} className="rounded border border-rule bg-paper-raised">
              <button
                type="button"
                onClick={() => setOpenSpec(open ? null : spec.id)}
                aria-expanded={open}
                className="flex w-full items-baseline justify-between gap-4 px-4 py-3 text-left"
              >
                <span>
                  <span className="font-medium">{spec.name.ja}</span>
                  <span className="ml-2 text-xs text-ink-muted">{spec.standard}</span>
                </span>
                <span className="shrink-0 text-xs text-ink-muted tabular">
                  {spec.sizes.length} サイズ · 曲げ {spec.min_bend_radius_ratio}D ·{' '}
                  {open ? '閉じる' : '開く'}
                </span>
              </button>

              {open && (
                <div className="border-t border-rule px-4 py-3">
                  <dl className="mb-3 grid grid-cols-2 gap-x-6 gap-y-1 text-xs sm:grid-cols-4">
                    <Meta label="材質" value={spec.material.ja} />
                    <Meta label="接合" value={spec.joint.ja} />
                    <Meta label="定尺" value={`${spec.stock_length} mm`} />
                    <Meta label="用途" value={systemKindLabels[spec.system_kind]} />
                  </dl>
                  <div className="overflow-x-auto">
                    <table className="w-full text-xs">
                      <thead className="text-left text-ink-muted">
                        <tr>
                          <th className="py-1 font-medium">呼び</th>
                          <th className="py-1 text-right font-medium">呼び径</th>
                          <th className="py-1 text-right font-medium">外径</th>
                          <th className="py-1 text-right font-medium">厚さ</th>
                          <th className="py-1 text-right font-medium">kg/m</th>
                        </tr>
                      </thead>
                      <tbody className="tabular">
                        {spec.sizes.map((s) => (
                          <tr key={s.designation} className="border-t border-rule/60">
                            <td className="py-1 font-mono">{s.designation}</td>
                            <td className="py-1 text-right">{s.nominal}</td>
                            <td className="py-1 text-right">{s.outside}</td>
                            <td className="py-1 text-right">{s.thickness || '—'}</td>
                            <td className="py-1 text-right">{s.mass_per_m || '—'}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </section>
    </div>
  );
}

function Meta({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt className="text-ink-muted">{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}
