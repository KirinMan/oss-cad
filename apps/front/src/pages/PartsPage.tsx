import { useMemo, useState } from 'react';
import { Link } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { categoryLabels, type Category, type PartSummary } from '@opendraft/shared';
import { fetchParts } from '../api.ts';

export function PartsPage() {
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState<Category | ''>('');

  const parts = useQuery({
    queryKey: ['parts', query, category],
    queryFn: () => fetchParts(query, category || undefined),
  });

  const grouped = useMemo(() => groupByCategory(parts.data ?? []), [parts.data]);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">部品ライブラリ</h1>
        <p className="mt-1 max-w-2xl text-sm text-ink-muted">
          寸法は固定されていません。すべての部品がパラメトリックで、あらゆるサイズを 1
          つの定義でカバーします。メーカーとの契約なしに、この場で作図に使えます。
        </p>
      </div>

      <div className="flex flex-wrap gap-3">
        <input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="エルボ、valve、ダンパー…"
          className="min-w-64 flex-1 rounded border border-rule bg-paper-raised px-3 py-2 text-sm"
          aria-label="部品を検索"
        />
        <select
          value={category}
          onChange={(e) => setCategory(e.target.value as Category | '')}
          className="rounded border border-rule bg-paper-raised px-3 py-2 text-sm"
          aria-label="分類で絞り込む"
        >
          <option value="">すべての分類</option>
          {Object.entries(categoryLabels).map(([value, label]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
      </div>

      {parts.isPending && <p className="text-sm text-ink-muted">読み込み中…</p>}

      {parts.isError && (
        <p className="rounded border border-sys-fire/40 bg-sys-fire/10 px-4 py-3 text-sm">
          部品を読み込めませんでした: {parts.error.message}
        </p>
      )}

      {parts.data?.length === 0 && (
        <p className="text-sm text-ink-muted">「{query}」に一致する部品はありません。</p>
      )}

      {grouped.map(([cat, items]) => (
        <section key={cat} className="space-y-2">
          <h2 className="border-b border-rule pb-1 text-xs font-semibold tracking-widest text-ink-muted uppercase">
            {categoryLabels[cat]} <span className="ml-2 font-normal">{items.length}</span>
          </h2>
          <ul className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
            {items.map((p) => (
              <li key={p.id}>
                <Link
                  to="/parts/$partId"
                  params={{ partId: p.id }}
                  className="block h-full rounded border border-rule bg-paper-raised p-3 transition-colors hover:border-accent"
                >
                  <div className="font-medium">{p.name_ja}</div>
                  <div className="mt-0.5 font-mono text-xs text-ink-muted">{p.id}</div>
                  <div className="mt-2 flex flex-wrap gap-1 text-xs text-ink-muted">
                    {p.parameters.map((param) => (
                      <span
                        key={param}
                        className="rounded bg-rule/50 px-1.5 py-0.5 font-mono"
                      >
                        {param}
                      </span>
                    ))}
                    <span className="ml-auto">{p.ports} ポート</span>
                  </div>
                </Link>
              </li>
            ))}
          </ul>
        </section>
      ))}

      {parts.data && parts.data.length > 0 && (
        <p className="text-sm text-ink-muted">{parts.data.length} 部品</p>
      )}
    </div>
  );
}

function groupByCategory(parts: PartSummary[]): [Category, PartSummary[]][] {
  const map = new Map<Category, PartSummary[]>();
  for (const p of parts) {
    const list = map.get(p.category);
    if (list) list.push(p);
    else map.set(p.category, [p]);
  }
  // Catalogue order groups by trade; sorting keys keeps the page stable as the
  // search narrows.
  return [...map.entries()].sort(([a], [b]) => a.localeCompare(b));
}
