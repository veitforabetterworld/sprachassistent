# Self-Hosting: alle Dienste aus einem Dockerfile, gestartet von compose.yaml im Repo-Root.
# Laeuft auf x86_64 und aarch64 (z. B. Raspberry Pi 5), gebaut wird jeweils nativ.

# ---------- Rust-Sprachserver ----------
FROM rust:1-bookworm AS voice-build
RUN apt-get update -qq \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends clang libclang-dev cmake pkg-config \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo
COPY src src
COPY tests tests
RUN cargo build --release --locked \
 && mkdir -p /out/lib \
 && cp target/release/sprachassistent /out/ \
 && cp -L target/release/libsherpa-onnx-c-api.so target/release/libonnxruntime.so /out/lib/

FROM debian:bookworm-slim AS voice
# Chromium fuer Seiten mit JavaScript; der Sprachserver startet es per setpriv als eigenes Konto.
# Dessen Sandbox braucht User-Namespaces, die erlaubt seccomp-chromium.json (compose.yaml).
RUN apt-get update -qq \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends ca-certificates curl bzip2 \
      chromium fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin sprachassistent-browser
WORKDIR /app
COPY --from=voice-build /out/ ./
COPY deployments/pi/download-models.sh deployments/pi/download-models.sh
COPY db/migrations db/migrations
ENV LD_LIBRARY_PATH=/app/lib API_HOST=0.0.0.0
# Die Sprachmodelle (~400 MB) landen beim ersten Start im Volume /app/models
CMD ["sh", "-c", "bash deployments/pi/download-models.sh && exec ./sprachassistent"]

# ---------- Web-API und Adminbereich ----------
FROM node:22-bookworm-slim AS web-build
RUN corepack enable
WORKDIR /app
COPY . .
RUN pnpm install --frozen-lockfile && pnpm build:deps && pnpm build

FROM web-build AS web
ENV NODE_ENV=production WEB_API_HOST=0.0.0.0
CMD ["sh", "-c", "node apps/api/dist/migrate.js && exec node apps/api/dist/index.js"]

# ---------- Reverse Proxy mit dem Adminbereich ----------
FROM caddy:2-alpine AS proxy
COPY deployments/docker/Caddyfile /etc/caddy/Caddyfile
COPY --from=web-build /app/apps/web/dist /srv
