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
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY frontend/dist ./frontend/dist
RUN cargo build --bin oxo-flow-web --release

# ===== Runtime =====
FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=backend-builder /app/target/release/oxo-flow-web /app/oxo-flow-web
COPY --from=frontend-builder /app/frontend/dist /app/frontend/dist
RUN mkdir -p /app/data
ENV OXO_FLOW_FRONTEND_DIR=/app/frontend/dist
EXPOSE 3000
CMD ["/app/oxo-flow-web", "--host", "0.0.0.0", "--port", "3000"]
