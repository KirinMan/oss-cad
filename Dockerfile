# syntax=docker/dockerfile:1
#
# Dev-environment image, built in stages so the API and front end containers
# don't each need a Rust toolchain — they just need the `od` binary the
# rust-builder stage produces. See docker-compose.yml for how these targets
# come together; `docker compose up` is the whole point of this file.

# ── Rust: the od-cli binary everything else shells out to ──────────────────
FROM rust:1-slim-bookworm AS rust-builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# od-parts embeds the catalogue at compile time (include_str!), so the
# source tree has to be present for the build — the resulting binary needs
# none of it at runtime.
COPY parts parts
RUN cargo build --release -p od-cli

# ── JS dependencies, shared by the api and front stages ─────────────────────
FROM oven/bun:1 AS js-deps
WORKDIR /app
COPY package.json bun.lock ./
COPY apps/api/package.json apps/api/package.json
COPY apps/front/package.json apps/front/package.json
COPY packages/shared/package.json packages/shared/package.json
RUN bun install --frozen-lockfile

# ── API dev server ───────────────────────────────────────────────────────
FROM oven/bun:1 AS api
WORKDIR /app
COPY --from=js-deps /app/node_modules ./node_modules
COPY --from=js-deps /app/apps/api/node_modules ./apps/api/node_modules
COPY --from=js-deps /app/packages/shared/node_modules ./packages/shared/node_modules
COPY --from=rust-builder /app/target/release/od /usr/local/bin/od
COPY package.json bun.lock ./
COPY packages/shared packages/shared
COPY apps/api apps/api
ENV OD_BIN=/usr/local/bin/od
WORKDIR /app/apps/api
EXPOSE 8787
CMD ["bun", "--watch", "src/index.ts"]

# ── Front dev server ─────────────────────────────────────────────────────
FROM oven/bun:1 AS front
WORKDIR /app
COPY --from=js-deps /app/node_modules ./node_modules
COPY --from=js-deps /app/apps/front/node_modules ./apps/front/node_modules
COPY --from=js-deps /app/packages/shared/node_modules ./packages/shared/node_modules
COPY package.json bun.lock ./
COPY packages/shared packages/shared
COPY apps/front apps/front
WORKDIR /app/apps/front
EXPOSE 5173
# --host so the dev server is reachable from outside the container, not just
# from within it.
CMD ["bun", "run", "dev", "--", "--host", "0.0.0.0"]
