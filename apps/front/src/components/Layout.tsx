import { Link, Outlet } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { fetchHealth } from '../api.ts';

export function Layout() {
  const health = useQuery({
    queryKey: ['health'],
    queryFn: fetchHealth,
    refetchInterval: 30_000,
  });

  const engineDown = health.data?.engine.available === false;

  return (
    <div className="min-h-dvh">
      <header className="border-b border-rule bg-paper-raised">
        <div className="mx-auto flex max-w-6xl flex-wrap items-center gap-x-8 gap-y-2 px-6 py-3">
          <Link to="/" className="text-lg font-semibold tracking-tight">
            OpenDraft
          </Link>
          <nav className="flex gap-1 text-sm">
            <NavLink to="/">部品</NavLink>
            <NavLink to="/systems">系統・仕様</NavLink>
            <NavLink to="/drawing">図面</NavLink>
            <NavLink to="/editor">編集</NavLink>
          </nav>
        </div>
      </header>

      {engineDown && (
        <div className="border-b border-rule bg-sys-drainage/10 px-6 py-2 text-sm">
          <div className="mx-auto max-w-6xl">
            描画エンジン（<code className="font-mono">{health.data?.engine.binary}</code>
            ）が見つかりません。
            <code className="mx-1 font-mono">cargo build --release -p od-cli</code>
            でビルドし、PATH に追加してください。
          </div>
        </div>
      )}

      <main className="mx-auto max-w-6xl px-6 py-8">
        <Outlet />
      </main>
    </div>
  );
}

function NavLink({ to, children }: { to: string; children: React.ReactNode }) {
  return (
    <Link
      to={to}
      className="rounded px-3 py-1.5 text-ink-muted transition-colors hover:bg-rule/40 hover:text-ink"
      activeProps={{ className: 'bg-rule/60 text-ink font-medium' }}
      activeOptions={{ exact: to === '/' }}
    >
      {children}
    </Link>
  );
}
