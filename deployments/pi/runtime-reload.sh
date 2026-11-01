#!/bin/bash
# Runtime deploy on the Raspberry Pi, called by .github/workflows/deploy.yml after the
# cross-compiled binary was rsynced. Nothing is compiled here.
set -euo pipefail
REPO="${SPRACHASSISTENT_REPO:-/root/sprachassistent}"
cd "$REPO"

log() { echo "[sprachassistent-runtime] $*"; }

[ -x bin/sprachassistent ] && [ -f bin/lib/libsherpa-onnx-c-api.so ] \
  || { echo "ERROR: bin/ unvollstaendig, zuerst in CI bauen und rsyncen." >&2; exit 1; }
[ -f .env ] || { echo "ERROR: $REPO/.env fehlt, zuerst deployments/pi/bootstrap.sh ausfuehren." >&2; exit 1; }
API_PORT="$(sed -n 's/^API_PORT=//p' .env)"

log "Sprachmodelle"
bash deployments/pi/download-models.sh

log "nginx-Site"
# A config that fails nginx -t is rolled back, so it never blocks the next reload of the other sites.
NGINX_FILES=(/etc/nginx/snippets/sprachassistent-headers.conf /etc/nginx/sites-available/sprachassistent.conf)
for f in "${NGINX_FILES[@]}"; do cp -f "$f" "$f.bak"; done
install -m0644 deployments/pi/nginx/sprachassistent-headers.conf /etc/nginx/snippets/
install -m0644 deployments/pi/nginx/sprachassistent.conf /etc/nginx/sites-available/
if ! nginx -t; then
  for f in "${NGINX_FILES[@]}"; do mv -f "$f.bak" "$f"; done
  echo "ERROR: nginx -t fehlgeschlagen, alte Konfiguration wiederhergestellt." >&2
  exit 1
fi
rm -f "${NGINX_FILES[@]/%/.bak}"
systemctl reload nginx

log "pm2 reload"
pm2 startOrReload deployments/pi/ecosystem.config.cjs --update-env
pm2 save

log "Health check"
# Loading whisper and piper takes a few seconds on the Pi.
for _ in $(seq 1 60); do
  if HEALTH="$(curl -fsS --max-time 2 "http://127.0.0.1:${API_PORT:-3020}/v1/health" 2>/dev/null)"; then
    log "$HEALTH"
    log "fertig $(date -Is), Commit ${DEPLOY_COMMIT:-unknown}"
    exit 0
  fi
  sleep 1
done
echo "ERROR: Server antwortet nicht auf :${API_PORT:-3020}/v1/health, Logs:" >&2
pm2 logs sprachassistent-api --lines 60 --nostream >&2 || true
exit 1
