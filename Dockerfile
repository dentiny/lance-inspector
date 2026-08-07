FROM node:24-bookworm-slim AS frontend
WORKDIR /app/frontend
COPY frontend/package*.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1.97-bookworm AS backend
WORKDIR /app
COPY backend/Cargo.toml backend/Cargo.lock ./backend/
RUN mkdir -p backend/src && echo 'fn main() {}' > backend/src/main.rs
RUN cargo build --release --manifest-path backend/Cargo.toml
COPY backend/src ./backend/src
RUN touch backend/src/main.rs && cargo build --release --manifest-path backend/Cargo.toml

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=backend /app/backend/target/release/lance-inspector-backend /usr/local/bin/lance-inspector
COPY --from=frontend /app/frontend/dist /opt/lance-inspector/ui
ENV LANCE_INSPECTOR_BIND=0.0.0.0:8080 \
    LANCE_INSPECTOR_UI_DIR=/opt/lance-inspector/ui
EXPOSE 8080
USER 65532:65532
ENTRYPOINT ["lance-inspector"]
