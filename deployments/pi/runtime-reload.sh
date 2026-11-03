#!/bin/bash
# Runtime deploy on the Raspberry Pi, called by .github/workflows/deploy.yml after the CI build was rsynced
# (Rust binary in bin/, built apps and framework packages). Nothing is compiled here.
set -euo pipefail
REPO="${SPRACHASSISTENT_REPO:-/root/sprachassistent}"
WEB_ROOT=/var/www/sprachassistent/app
cd "$REPO"

log() { echo "[sprachassistent-runtime] $*"; }

for f in bin/sprachassistent bin/lib/libsherpa-onnx-c-api.so apps/api/dist/index.js apps/web/dist/index.html; do
  [ -f "$f" ] || { echo "ERROR: $f fehlt, zuerst in CI bauen und rsyncen." >&2; exit 1; }
done
grep -q '^DATABASE_URL=' .env 2>/dev/null \
  || { echo "ERROR: DATABASE_URL fehlt in $REPO/.env, zuerst deployments/pi/bootstrap.sh ausfuehren." >&2; exit 1; }
# Without it API keys and tool tokens would be stored in plain text.
grep -q '^CREDENTIALS_KEY=.\+' .env 2>/dev/null \
  || { echo "ERROR: CREDENTIALS_KEY fehlt in $REPO/.env, zuerst deployments/pi/bootstrap.sh ausfuehren." >&2; exit 1; }

log "Sprachmodelle"
bash deployments/pi/download-models.sh

log "Websuche (SearXNG)"
bash deployments/pi/searxng.sh

log "Browser (Chromium)"
bash deployments/pi/browser.sh

log "pnpm install (Web-API)"
export CI=1
corepack enable
# Not --prod: the framework packages list express as optional peer plus dev dependency.
pnpm install --frozen-lockfile --filter "@sprachassistent/api..."

log "Datenbank sichern"
# data/ is excluded from the rsync; the last 10 dumps are kept.
mkdir -p data/backups
DATABASE_URL="$(sed -n 's/^DATABASE_URL=//p' .env | tail -n1)"
pg_dump -Fc --dbname="$DATABASE_URL" -f "data/backups/sprachassistent-$(date +%Y%m%d-%H%M%S).dump"
ls -1t data/backups/*.dump | tail -n +11 | xargs -r rm -f
log "$(ls -1t data/backups/*.dump | head -1) ($(du -h "$(ls -1t data/backups/*.dump | head -1)" | cut -f1))"

log "Datenbank migrieren"
# Before the restart: the servers refuse to start while a migration is missing.
node apps/api/dist/migrate.js

log "Web -> $WEB_ROOT"
mkdir -p "$WEB_ROOT"
rsync -a --delete apps/web/dist/ "$WEB_ROOT/"

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
# The first version ran the voice server under this name.
pm2 delete sprachassistent-api >/dev/null 2>&1 || true
pm2 startOrReload deployments/pi/ecosystem.config.cjs --update-env
pm2 save

log "Health check"
# Loading the speech models takes a few seconds on the Pi.
for _ in $(seq 1 90); do
  VOICE="$(curl -fsS --max-time 2 http://127.0.0.1:3020/v1/health 2>/dev/null || true)"
  WEB="$(curl -fsS --max-time 2 http://127.0.0.1:3022/v1/ready 2>/dev/null || true)"
  if [ -n "$VOICE" ] && [ -n "$WEB" ]; then
    log "voice: $VOICE"
    log "web: $WEB"
    log "fertig $(date -Is), Commit ${DEPLOY_COMMIT:-unknown}"
    exit 0
  fi
  sleep 1
done
echo "ERROR: Server antworten nicht (voice :3020 '${VOICE}', web :3022 '${WEB}'), Logs:" >&2
pm2 logs sprachassistent-voice --lines 40 --nostream >&2 || true
pm2 logs sprachassistent-web --lines 40 --nostream >&2 || true
exit 1
