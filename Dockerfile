# ===== Frontend builder =====
FROM node:22-alpine AS frontend-builder
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ .
RUN npm run build

# ===== Backend builder =====
FROM rust:1.98-slim AS backend-builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
# The root manifest is both workspace and package: without its src/lib.rs
# cargo rejects the manifest ("no targets specified") inside the container.
COPY src/lib.rs src/
COPY crates/ ./crates/
# -p is required: the workspace root's default package (oxo-flow) has no bin
# targets, so `--bin oxo-flow-web` aborts with "no bin target named".
# oxo-flow-cli provides the `oxo-flow` binary: web-triggered runs shell out to
# it (executor.rs find_oxo_flow_binary falls back to PATH), and it keeps the
# CLI usable inside the image.
RUN cargo build --release -p oxo-flow-web -p oxo-flow-cli

# ===== Runtime =====
# Must match the builder's Debian release: rust:1.98-slim is trixie-based
# (glibc 2.41); a bookworm runtime (glibc 2.36) aborts with
# "GLIBC_2.39 not found" (issue #276 docker-build gate).
FROM debian:trixie-slim
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=backend-builder /app/target/release/oxo-flow-web /app/oxo-flow-web
COPY --from=backend-builder /app/target/release/oxo-flow /app/oxo-flow
# vite emits to ../crates/oxo-flow-web/static (vite.config.ts outDir),
# NOT frontend/dist — copy from there.
COPY --from=frontend-builder /app/crates/oxo-flow-web/static /app/frontend/dist

RUN mkdir -p /app/data && \
    chown -R 1000:1000 /app

USER 1000:1000

# Both entrypoints default their SQLite DB to ./oxo-flow.db in the CWD
# (the standalone binary honors DATABASE_URL, but `serve` hardcodes the
# relative path) — so run from /app/data: state lands on the mounted
# volume and survives container restarts. Binary paths stay absolute.
WORKDIR /app/data

ENV OXO_FLOW_FRONTEND_DIR=/app/frontend/dist \
    OXO_FLOW_MODE=team

EXPOSE 3000

LABEL org.opencontainers.image.title="oxo-flow" \
      org.opencontainers.image.description="Bioinformatics pipeline engine" \
      org.opencontainers.image.version="0.16.0" \
      org.opencontainers.image.licenses="Apache-2.0 AND LicenseRef-OxoFlow-Commercial" \
      org.opencontainers.image.vendor="Traitome" \
      org.opencontainers.image.authors="Shixiang Wang <w_shixiang@163.com>"

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -sf http://localhost:3000/api/health | grep -q '"status":"ok"' || exit 1

CMD ["/app/oxo-flow-web", "--host", "0.0.0.0", "--port", "3000", "--mode", "team"]
