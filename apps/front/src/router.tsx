import { createRootRoute, createRoute, createRouter } from '@tanstack/react-router';
import { Layout } from './components/Layout.tsx';
import { PartsPage } from './pages/PartsPage.tsx';
import { PartPage } from './pages/PartPage.tsx';
import { SystemsPage } from './pages/SystemsPage.tsx';
import { DrawingPage } from './pages/DrawingPage.tsx';
import { EditorPage } from './pages/EditorPage.tsx';

const rootRoute = createRootRoute({ component: Layout });

const partsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/',
  component: PartsPage,
});

const partRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/parts/$partId',
  component: PartPage,
});

const systemsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/systems',
  component: SystemsPage,
});

const drawingRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/drawing',
  component: DrawingPage,
});

const editorRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/editor',
  component: EditorPage,
});

const routeTree = rootRoute.addChildren([
  partsRoute,
  partRoute,
  systemsRoute,
  drawingRoute,
  editorRoute,
]);

export const router = createRouter({
  routeTree,
  defaultNotFoundComponent: () => (
    <p className="text-sm text-ink-muted">ページが見つかりません。</p>
  ),
});

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router;
  }
}
