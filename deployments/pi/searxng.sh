#!/bin/bash
# Self-hosted SearXNG for web_research as a Docker container on 127.0.0.1:8888 (SEARXNG_URL default).
# Called by bootstrap.sh and runtime-reload.sh. Idempotent: installs Docker and SEARXNG_SECRET only when
# missing and recreates the container only when the image or settings.yml changed.
set -euo pipefail
REPO="${SPRACHASSISTENT_REPO:-/root/sprachassistent}"
IMAGE=searxng/searxng:2026.10.27-b5ed73fcb
NAME=sprachassistent-searxng
PORT=8888
SETTINGS="$REPO/deployments/pi/searxng/settings.yml"

log() { echo "[sprachassistent-searxng] $*"; }

if ! command -v docker >/dev/null; then
  log "Docker installieren"
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq docker.io
  systemctl enable --now docker
fi

if ! grep -q '^SEARXNG_SECRET=.\+' "$REPO/.env" 2>/dev/null; then
  umask 077
  echo "SEARXNG_SECRET=$(openssl rand -hex 32)" >>"$REPO/.env"
  umask 022
  log "SEARXNG_SECRET in .env ergänzt"
fi
SECRET="$(sed -n 's/^SEARXNG_SECRET=//p' "$REPO/.env" | tail -n1)"

WANT="$IMAGE $(sha256sum "$SETTINGS" | cut -c1-64)"
HAVE="$(docker inspect -f '{{index .Config.Labels "sprachassistent.state"}}' "$NAME" 2>/dev/null || true)"
RUNNING="$(docker inspect -f '{{.State.Running}}' "$NAME" 2>/dev/null || echo false)"
if [ "$WANT" = "$HAVE" ] && [ "$RUNNING" = "true" ]; then
  log "Container läuft unverändert"
else
  log "Container starten ($IMAGE)"
  docker pull -q "$IMAGE" >/dev/null
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  docker run -d --name "$NAME" --restart unless-stopped \
    -p "127.0.0.1:${PORT}:8080" \
    -e SEARXNG_SECRET="$SECRET" \
    -v "$SETTINGS:/etc/searxng/settings.yml:ro" \
    --label "sprachassistent.state=$WANT" \
    "$IMAGE" >/dev/null
fi

# The JSON format answers 403 while it is not enabled, so this also checks settings.yml.
for _ in $(seq 1 60); do
  if curl -fsS --max-time 5 "http://127.0.0.1:${PORT}/search?q=wetter&format=json" >/dev/null 2>&1; then
    log "bereit auf http://127.0.0.1:${PORT}"
    exit 0
  fi
  sleep 1
done
echo "ERROR: SearXNG antwortet nicht auf :${PORT}, Logs:" >&2
docker logs --tail 40 "$NAME" >&2 || true
exit 1
