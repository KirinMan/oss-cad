import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    proxy: {
      // Same-origin in development, so no CORS preflight on every request and
      // no separate base URL to configure.
      '/api': {
        target: process.env.OD_API_URL ?? 'http://localhost:8787',
        changeOrigin: true,
      },
      '/health': {
        target: process.env.OD_API_URL ?? 'http://localhost:8787',
        changeOrigin: true,
      },
    },
  },
  build: {
    target: 'es2023',
    sourcemap: true,
  },
});
