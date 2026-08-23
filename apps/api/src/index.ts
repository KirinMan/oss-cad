import { createApp } from './app.ts';
import { odAvailable, odBinary } from './od.ts';

const port = Number(process.env.PORT ?? 8787);
const app = createApp();

if (!(await odAvailable())) {
  console.warn(
    `warning: \`${odBinary}\` is not on PATH. Catalogue and drawing endpoints will ` +
      `return 503 until it is. Build it with: cargo build --release -p od-cli`,
  );
}

export default {
  port,
  fetch: app.fetch,
  // Uploads are streamed to disk by the handler, but Bun still buffers the
  // request body, so the server limit has to clear the app's own limit.
  maxRequestBodySize: Number(process.env.OD_MAX_UPLOAD ?? 64 * 1024 * 1024) + 1024 * 1024,
};

console.warn(`OpenDraft API listening on http://localhost:${port}`);
