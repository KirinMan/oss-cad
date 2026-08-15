import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider } from '@tanstack/react-router';
import { router } from './router.tsx';
import './styles.css';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // The catalogue is compiled into the binary, so it does not change while
      // the page is open. Not refetching it on every focus keeps the part
      // browser instant.
      staleTime: 5 * 60_000,
      retry: 1,
    },
  },
});

const container = document.getElementById('root');
if (!container) {
  throw new Error('index.html is missing its #root element');
}

createRoot(container).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
);
