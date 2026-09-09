import { useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import type {
  CheckReport,
  Inspection,
  MepCheckReport,
  MepTakeoffReport,
} from '@opendraft/shared';
import {
  checkDrawing,
  checkMep,
  inspectDrawing,
  saveDrawing,
  takeoffMep,
  type SaveFormat,
} from '../api.ts';
import { DrawingViewer } from '../components/DrawingViewer.tsx';

/**
 * Open a drawing, see what is in it, check it, and save it.
 *
 * Anyone can open a drawing without installing anything, and files are not
 * stored — the service deletes each upload as soon as the report is produced.
 *
 * Saving defaults to `.odc` because it is the only format that keeps the whole
 * document. Choosing DXF is choosing an exchange copy, and the UI says what
 * that costs before the file is downloaded rather than after.
 */
export function DrawingPage() {
  const [file, setFile] = useState<File | null>(null);
  const [rules, setRules] = useState<'basic' | 'jp'>('jp');
  const [dark, setDark] = useState(false);
  const [hiddenLayers, setHiddenLayers] = useState<Set<string>>(new Set());

  const analysis = useMutation({
    mutationFn: async (f: File) => {
      const [inspection, check, takeoff, mepCheck] = await Promise.all([
        inspectDrawing(f),
        checkDrawing(f, rules),
        takeoffMep(f),
        checkMep(f),
      ]);
      return { inspection, check, takeoff, mepCheck };
    },
  });

  const save = useMutation({
    mutationFn: async ({ file: f, to }: { file: File; to: SaveFormat }) => {
      const saved = await saveDrawing(f, to);
      // Hand the bytes to the browser. Revoking the URL immediately after the
      // click would race the download in some browsers, so it is deferred.
      const url = URL.createObjectURL(saved.blob);
      const link = document.createElement('a');
      link.href = url;
      link.download = saved.filename;
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 60_000);
      return saved;
    },
  });

  function onSelect(f: File | null) {
    setFile(f);
    setHiddenLayers(new Set());
    analysis.reset();
    if (f) analysis.mutate(f);
  }

  const allLayers = analysis.data?.inspection.layer_names ?? [];
  const visibleLayers = new Set(allLayers.filter((n) => !hiddenLayers.has(n)));

  function toggleLayer(name: string) {
    setHiddenLayers((hidden) => {
      const next = new Set(hidden);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
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
            accept=".dxf,.odc"
            className="sr-only"
            onChange={(e) => onSelect(e.target.files?.[0] ?? null)}
          />
          図面を選択（DXF / ODC）
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

      {analysis.data && file && (
        <div className="space-y-6">
          <section className="space-y-2">
            <div className="flex flex-wrap items-baseline justify-between gap-3">
              <h2 className="text-xs font-semibold tracking-widest text-ink-muted uppercase">
                図面
              </h2>
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={dark}
                  onChange={(e) => setDark(e.target.checked)}
                />
                暗い背景
              </label>
            </div>

            <DrawingViewer
              file={file}
              layers={allLayers}
              visibleLayers={visibleLayers}
              dark={dark}
            />

            {allLayers.length > 1 && (
              <div className="flex flex-wrap gap-1">
                {allLayers.map((name) => {
                  const shown = !hiddenLayers.has(name);
                  return (
                    <button
                      key={name}
                      type="button"
                      onClick={() => toggleLayer(name)}
                      aria-pressed={shown}
                      className={`rounded border px-2 py-0.5 font-mono text-xs transition-colors ${
                        shown
                          ? 'border-rule bg-paper-raised text-ink'
                          : 'border-transparent bg-rule/30 text-ink-muted line-through'
                      }`}
                    >
                      {name}
                    </button>
                  );
                })}
              </div>
            )}
          </section>

          <SaveBar
            busy={save.isPending}
            result={save.data ?? null}
            error={save.error}
            onSave={(to) => save.mutate({ file, to })}
          />
          <InspectionView data={analysis.data.inspection} />
          <CheckView data={analysis.data.check} />
          <TakeoffView data={analysis.data.takeoff} />
          <MepCheckView data={analysis.data.mepCheck} />
        </div>
      )}
    </div>
  );
}

function SaveBar({
  busy,
  result,
  error,
  onSave,
}: {
  busy: boolean;
  result: { filename: string; losses: string[] } | null;
  error: Error | null;
  onSave: (to: SaveFormat) => void;
}) {
  return (
    <section className="space-y-2 rounded border border-rule bg-paper-raised px-4 py-3">
      <div className="flex flex-wrap items-center gap-3">
        <span className="text-sm font-medium">保存</span>
        <button
          type="button"
          disabled={busy}
          onClick={() => onSave('odc')}
          className="rounded bg-accent px-3 py-1.5 text-sm text-paper-raised disabled:opacity-50"
        >
          .odc で保存
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => onSave('dxf')}
          className="rounded border border-rule px-3 py-1.5 text-sm disabled:opacity-50"
        >
          DXF で書き出し
        </button>
        <span className="text-xs text-ink-muted">
          .odc は属性・スキーマ・階・通り芯まで保持します
        </span>
      </div>

      {busy && <p className="text-sm text-ink-muted">変換中…</p>}

      {error && (
        <p className="text-sm text-sys-fire">保存できませんでした: {error.message}</p>
      )}

      {result && (
        <div className="text-sm">
          <p>
            <span className="font-mono">{result.filename}</span> をダウンロードしました。
          </p>
          {result.losses.length > 0 && (
            <div className="mt-1 rounded border border-sys-drainage/40 bg-sys-drainage/10 px-3 py-2">
              <p className="font-medium">この形式では保持できなかったもの</p>
              <ul className="mt-1 list-disc pl-5">
                {result.losses.map((l) => (
                  <li key={l}>{l}</li>
                ))}
              </ul>
              <p className="mt-1 text-xs text-ink-muted">
                これらを残すには .odc で保存してください。
              </p>
            </div>
          )}
        </div>
      )}
    </section>
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
        <Stat label="形式" value={data.format.toUpperCase()} />
        <Stat
          label="図面範囲"
          value={
            width > 0 || height > 0
              ? `${(width / 1000).toFixed(1)} × ${(height / 1000).toFixed(1)} m`
              : '—'
          }
        />
      </div>

      {data.unsupported_features.length > 0 && (
        <p className="rounded border border-rule bg-paper-raised px-4 py-3 text-sm">
          この図面には、より新しいバージョンが書き込んだ内容（
          {data.unsupported_features.join('、')}
          ）が含まれています。編集はできませんが、
          <strong className="font-medium">保存しても失われません</strong>。
        </p>
      )}

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

function TakeoffView({ data }: { data: MepTakeoffReport }) {
  const hasEquipment = Object.keys(data.equipment).length > 0;
  const hasFittings = Object.keys(data.fittings).length > 0;
  if (data.routes.length === 0 && !hasEquipment && !hasFittings) {
    // Nothing routed or placed — most drawings this page sees are plain
    // architectural/duct backgrounds, and an empty MEP section would just be
    // noise on every one of them.
    return null;
  }

  return (
    <section className="space-y-3">
      <h2 className="text-xs font-semibold tracking-widest text-ink-muted uppercase">
        拾い出し
      </h2>

      {data.routes.length > 0 && (
        <div className="overflow-x-auto rounded border border-rule">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-rule bg-paper-raised text-left text-xs text-ink-muted">
                <th className="px-3 py-2 font-medium">系統</th>
                <th className="px-3 py-2 font-medium">仕様</th>
                <th className="px-3 py-2 text-right font-medium">延長 (m)</th>
                <th className="px-3 py-2 text-right font-medium">本数</th>
                <th className="px-3 py-2 text-right font-medium">定尺本数</th>
                <th className="px-3 py-2 text-right font-medium">継手数</th>
              </tr>
            </thead>
            <tbody>
              {data.routes.map((r) => (
                <tr
                  key={`${r.system}/${r.spec}`}
                  className="border-b border-rule/50 last:border-0"
                >
                  <td className="px-3 py-1.5 font-mono text-xs">{r.system}</td>
                  <td className="px-3 py-1.5 font-mono text-xs">{r.spec}</td>
                  <td className="px-3 py-1.5 text-right tabular">
                    {(r.length_mm / 1000).toFixed(1)}
                  </td>
                  <td className="px-3 py-1.5 text-right tabular">{r.count}</td>
                  <td className="px-3 py-1.5 text-right tabular">
                    {r.stock_pieces ?? '—'}
                  </td>
                  <td className="px-3 py-1.5 text-right tabular">{r.joints ?? '—'}</td>
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr className="border-t border-rule bg-paper-raised font-medium">
                <td className="px-3 py-1.5" colSpan={2}>
                  合計
                </td>
                <td className="px-3 py-1.5 text-right tabular">
                  {(data.total_length_mm / 1000).toFixed(1)}
                </td>
                <td className="px-3 py-1.5" colSpan={3} />
              </tr>
            </tfoot>
          </table>
        </div>
      )}

      {(hasFittings || hasEquipment) && (
        <div className="grid gap-4 sm:grid-cols-2">
          {hasFittings && <PartCountList title="継手" counts={data.fittings} />}
          {hasEquipment && <PartCountList title="機器" counts={data.equipment} />}
        </div>
      )}
    </section>
  );
}

function PartCountList({
  title,
  counts,
}: {
  title: string;
  counts: Record<string, number>;
}) {
  const entries = Object.entries(counts).sort(([, a], [, b]) => b - a);
  return (
    <div>
      <h3 className="mb-1 text-sm font-medium">{title}</h3>
      <ul className="space-y-0.5 text-sm tabular">
        {entries.map(([id, count]) => (
          <li
            key={id}
            className="flex justify-between gap-4 border-b border-rule/50 py-0.5"
          >
            <span className="font-mono text-xs">{id}</span>
            <span>{count}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function MepCheckView({ data }: { data: MepCheckReport }) {
  if (data.ports === 0) {
    // No MEP objects in this drawing at all — nothing to say.
    return null;
  }

  return (
    <section className="space-y-2">
      <h2 className="text-xs font-semibold tracking-widest text-ink-muted uppercase">
        MEP接続チェック
      </h2>
      <p className="text-sm text-ink-muted">
        {data.ports} 接続口、{data.connections} 接続済み
      </p>

      {data.passed ? (
        <p className="text-sm">未接続の口はありません。</p>
      ) : (
        <ul className="space-y-1">
          {data.unconnected.map((p, i) => (
            <li
              key={`${p.owner}-${p.name}-${i}`}
              className="flex gap-3 rounded border border-sys-drainage/40 bg-sys-drainage/10 px-3 py-2 text-sm"
            >
              <span className="shrink-0 rounded bg-sys-drainage/15 px-1.5 py-0.5 text-xs font-medium text-sys-drainage">
                未接続
              </span>
              <span>
                {p.owner}
                <span className="ml-2 font-mono text-xs text-ink-muted">
                  {p.name} @ ({p.position_mm[0].toFixed(0)}, {p.position_mm[1].toFixed(0)}
                  , {p.position_mm[2].toFixed(0)})
                </span>
              </span>
            </li>
          ))}
          {data.skipped.map((id) => (
            <li
              key={id}
              className="flex gap-3 rounded border border-sys-fire/40 bg-sys-fire/10 px-3 py-2 text-sm"
            >
              <span className="shrink-0 rounded bg-sys-fire/15 px-1.5 py-0.5 text-xs font-medium text-sys-fire">
                部品未解決
              </span>
              <span className="font-mono text-xs">{id}</span>
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
