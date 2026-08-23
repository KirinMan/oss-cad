import { useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import type { CheckReport, Inspection } from '@opendraft/shared';
import { checkDrawing, inspectDrawing } from '../api.ts';

/**
 * Upload a DXF, see what is in it, and check it against a rule set.
 *
 * This is the Phase 1 promise in its smallest useful form: anyone can open a
 * drawing without installing anything. Files are not stored — the service
 * deletes each upload as soon as the report is produced.
 */
export function DrawingPage() {
  const [file, setFile] = useState<File | null>(null);
  const [rules, setRules] = useState<'basic' | 'jp'>('jp');

  const analysis = useMutation({
    mutationFn: async (f: File) => {
      const [inspection, check] = await Promise.all([
        inspectDrawing(f),
        checkDrawing(f, rules),
      ]);
      return { inspection, check };
    },
  });

  function onSelect(f: File | null) {
    setFile(f);
    analysis.reset();
    if (f) analysis.mutate(f);
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">図面を調べる</h1>
        <p className="mt-1 max-w-2xl text-sm text-ink-muted">
          DXF
          を読み込んで内容と検査結果を表示します。ファイルはサーバに保存されず、処理後すぐ削除されます。
        </p>
      </div>

      <div className="flex flex-wrap items-center gap-4">
        <label className="cursor-pointer rounded border border-rule bg-paper-raised px-4 py-2 text-sm transition-colors hover:border-accent">
          <input
            type="file"
            accept=".dxf"
            className="sr-only"
            onChange={(e) => onSelect(e.target.files?.[0] ?? null)}
          />
          DXF を選択
        </label>
        {file && (
          <span className="text-sm text-ink-muted">
            {file.name}
            <span className="ml-2 tabular">{(file.size / 1024).toFixed(0)} KB</span>
          </span>
        )}
        <label className="ml-auto flex items-center gap-2 text-sm">
          検査ルール
          <select
            value={rules}
            onChange={(e) => {
              const next = e.target.value as 'basic' | 'jp';
              setRules(next);
              if (file) analysis.mutate(file);
            }}
            className="rounded border border-rule bg-paper-raised px-2 py-1"
          >
            <option value="basic">基本</option>
            <option value="jp">国内実務（レイヤ名・文字高さ）</option>
          </select>
        </label>
      </div>

      {analysis.isPending && <p className="text-sm text-ink-muted">解析中…</p>}

      {analysis.isError && (
        <p className="rounded border border-sys-fire/40 bg-sys-fire/10 px-4 py-3 text-sm">
          {analysis.error.message}
        </p>
      )}

      {analysis.data && (
        <div className="space-y-6">
          <InspectionView data={analysis.data.inspection} />
          <CheckView data={analysis.data.check} />
        </div>
      )}
    </div>
  );
}

function InspectionView({ data }: { data: Inspection }) {
  const [minX, minY, , maxX, maxY] = data.extents_mm;
  const width = maxX - minX;
  const height = maxY - minY;

  return (
    <section className="space-y-3">
      <h2 className="text-xs font-semibold tracking-widest text-ink-muted uppercase">
        図面の内容
      </h2>

      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <Stat label="エンティティ" value={data.entities.toLocaleString()} />
        <Stat label="レイヤ" value={String(data.layers)} />
        <Stat label="ブロック" value={String(data.blocks)} />
        <Stat
          label="図面範囲"
          value={
            width > 0 || height > 0
              ? `${(width / 1000).toFixed(1)} × ${(height / 1000).toFixed(1)} m`
              : '—'
          }
        />
      </div>

      {(data.preserved_entities > 0 || data.preserved_sections > 0) && (
        <p className="rounded border border-rule bg-paper-raised px-4 py-3 text-sm">
          <strong className="font-medium">
            {data.preserved_entities} 個のエンティティ
          </strong>
          （{data.unsupported_types.join('、')}）はこのビルドが解釈できない形式ですが、
          <strong className="font-medium">原形のまま保持</strong>
          されます。保存し直しても失われません。
        </p>
      )}

      <div className="grid gap-4 sm:grid-cols-2">
        <div>
          <h3 className="mb-1 text-sm font-medium">種別</h3>
          <ul className="space-y-0.5 text-sm tabular">
            {Object.entries(data.entities_by_type)
              .sort(([, a], [, b]) => b - a)
              .map(([type, count]) => (
                <li
                  key={type}
                  className="flex justify-between gap-4 border-b border-rule/50 py-0.5"
                >
                  <span className="font-mono text-xs">{type}</span>
                  <span>{count.toLocaleString()}</span>
                </li>
              ))}
          </ul>
        </div>
        <div>
          <h3 className="mb-1 text-sm font-medium">レイヤ</h3>
          <ul className="max-h-64 space-y-0.5 overflow-y-auto text-sm">
            {data.layer_names.map((name) => (
              <li key={name} className="border-b border-rule/50 py-0.5 font-mono text-xs">
                {name}
              </li>
            ))}
          </ul>
        </div>
      </div>
    </section>
  );
}

function CheckView({ data }: { data: CheckReport }) {
  return (
    <section className="space-y-2">
      <h2 className="text-xs font-semibold tracking-widest text-ink-muted uppercase">
        検査結果
      </h2>
      {data.findings.length === 0 ? (
        <p className="text-sm">指摘はありません。</p>
      ) : (
        <ul className="space-y-1">
          {data.findings.map((f) => (
            <li
              key={f.rule}
              className="flex gap-3 rounded border border-rule bg-paper-raised px-3 py-2 text-sm"
            >
              <span
                className={`shrink-0 rounded px-1.5 py-0.5 text-xs font-medium ${severityClass(f.severity)}`}
              >
                {severityLabel(f.severity)}
              </span>
              <span>
                {f.message}
                <span className="ml-2 font-mono text-xs text-ink-muted">{f.rule}</span>
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded border border-rule bg-paper-raised px-4 py-3">
      <div className="text-xs text-ink-muted">{label}</div>
      <div className="mt-0.5 text-xl font-semibold tabular">{value}</div>
    </div>
  );
}

function severityLabel(s: CheckReport['findings'][number]['severity']): string {
  switch (s) {
    case 'error':
      return 'エラー';
    case 'warning':
      return '警告';
    case 'info':
      return '情報';
  }
}

function severityClass(s: CheckReport['findings'][number]['severity']): string {
  switch (s) {
    case 'error':
      return 'bg-sys-fire/15 text-sys-fire';
    case 'warning':
      return 'bg-sys-drainage/15 text-sys-drainage';
    case 'info':
      return 'bg-rule/60 text-ink-muted';
  }
}
