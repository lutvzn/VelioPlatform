# The Velio sync server as a distroless image: one static binary, no shell.
#
#   docker build -t velio-server .
#   docker run -p 8787:8787 -v velio-data:/data --env-file velio.env velio-server
#
# See README.md for the settings and putting a reverse proxy in front.

# --- What the browser loads: vault.js, ops.js, HTMX and the styles ---
FROM node:26-slim AS client
WORKDIR /src
RUN npm install --global --no-fund --no-audit pnpm@12.14.0
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/server/package.json apps/server/
COPY packages/sync-core/package.json packages/sync-core/
RUN pnpm install --frozen-lockfile --ignore-scripts --filter "@velio/server..."
COPY packages/sync-core/src packages/sync-core/src
COPY apps/server/scripts/build-client.mjs apps/server/scripts/
COPY apps/server/client apps/server/client
RUN node apps/server/scripts/build-client.mjs

# --- The server, linked statically against musl ---
FROM rust:1.99-alpine AS server
RUN apk add --no-cache musl-dev cmake make perl clang
WORKDIR /src/apps/server
COPY apps/server/Cargo.toml apps/server/Cargo.lock ./
COPY apps/server/migrations migrations
COPY apps/server/src src
# Embedded in the binary: the static files (src/assets.rs), and the probe's
# test account (src/routes/probe.rs).
COPY --from=client /src/apps/server/public/static public/static
COPY apps/server/probe/test-account.json probe/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/apps/server/target \
    cargo build --release --locked \
    && install -D target/release/velio-server /out/velio-server \
    && mkdir -p /out/data

# --- The image ---
FROM gcr.io/distroless/static-debian13:nonroot
COPY --from=server /out/velio-server /velio-server
COPY --from=server --chown=nonroot:nonroot /out/data /data
ENV VELIO_LISTEN=0.0.0.0:8787 \
    VELIO_DATABASE=/data/velio.db \
    VELIO_BACKUP_DIR=/data/backups
VOLUME ["/data"]
EXPOSE 8787
USER nonroot
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["/velio-server", "health"]
ENTRYPOINT ["/velio-server"]
