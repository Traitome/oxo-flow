# ===== Frontend builder =====
FROM node:22-alpine AS frontend-builder
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ .
RUN npm run build

# ===== Backend builder =====
FROM rust:1.92-slim AS backend-builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
RUN cargo build --bin oxo-flow-web --release

# ===== Runtime =====
FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=backend-builder /app/target/release/oxo-flow-web /app/oxo-flow-web
COPY --from=frontend-builder /app/frontend/dist /app/frontend/dist

RUN mkdir -p /app/data && \
    chown -R 1000:1000 /app

USER 1000:1000

ENV OXO_FLOW_FRONTEND_DIR=/app/frontend/dist
EXPOSE 3000

LABEL org.opencontainers.image.title="oxo-flow" \
      org.opencontainers.image.description="Bioinformatics pipeline engine" \
      org.opencontainers.image.version="0.8.0" \
      org.opencontainers.image.licenses="Apache-2.0 AND LicenseRef-OxoFlow-Commercial" \
      org.opencontainers.image.vendor="Traitome" \
      org.opencontainers.image.authors="Shixiang Wang <w_shixiang@163.com>"

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:3000/api/health || exit 1

CMD ["/app/oxo-flow-web", "--host", "0.0.0.0", "--port", "3000"]
