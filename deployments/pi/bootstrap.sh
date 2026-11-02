#!/bin/bash
# One-time setup of sprachassistent.plenivo.de on the Raspberry Pi (next to plenivo and
# hochwasserdienst). Run as root from a checkout or copy of the repo:
#   DEPLOY_PUBKEY="ssh-ed25519 AAAA... github-actions-sprachassistent" bash deployments/pi/bootstrap.sh
# Idempotent: existing database, data, models, DNS record, certificate and keys are kept.
# Afterwards every push to the deploy branch deploys via .github/workflows/deploy.yml.
# Only the database connection goes into .env; everything else is set in the admin area.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="${SPRACHASSISTENT_REPO:-/root/sprachassistent}"
HOST=sprachassistent.plenivo.de
ZONE=plenivo.de
CNAME_TARGET=dev.plenivo.de.
CERT_EMAIL="${CERT_EMAIL:-kontakt@veitgoetz.de}"
DESEC_CREDENTIALS=/etc/desec/credentials.ini

log() { echo "[sprachassistent-bootstrap] $*"; }

[ "$(id -u)" -eq 0 ] || { echo "Als root ausfuehren." >&2; exit 1; }
[ -n "${DEPLOY_PUBKEY:-}" ] || { echo "DEPLOY_PUBKEY fehlt (oeffentlicher GitHub-Actions-Key)." >&2; exit 1; }

log "Pakete"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq rsync curl bzip2 postgresql docker.io

mkdir -p "$REPO/data" "$REPO/bin" /var/www/sprachassistent/app
if [ "$SCRIPT_DIR" != "$REPO/deployments/pi" ]; then
  rsync -a "$SCRIPT_DIR/../../deployments/" "$REPO/deployments/"
fi

log "Datenbank"
if grep -q '^DATABASE_URL=' "$REPO/.env" 2>/dev/null; then
  log ".env hat DATABASE_URL, Datenbank-Passwort bleibt"
else
  if sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='sprachassistent'" | grep -q 1; then
    echo "Rolle sprachassistent existiert, aber DATABASE_URL fehlt in $REPO/.env. Von Hand eintragen." >&2
    exit 1
  fi
  DB_PASSWORD="$(openssl rand -hex 24)"
  sudo -u postgres psql -v ON_ERROR_STOP=1 -q -c "CREATE ROLE sprachassistent LOGIN PASSWORD '${DB_PASSWORD}'"
  # Settings of the first version (API keys, providers) now live in the database; the old file stays as backup.
  [ -f "$REPO/.env" ] && mv "$REPO/.env" "$REPO/.env.v1.bak"
  umask 077
  cat >"$REPO/.env" <<EOF
DATABASE_URL=postgres://sprachassistent:${DB_PASSWORD}@127.0.0.1:5432/sprachassistent
EOF
  umask 022
fi
if ! sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='sprachassistent'" | grep -q 1; then
  sudo -u postgres createdb -O sprachassistent sprachassistent
fi
if ! grep -q '^CREDENTIALS_KEY=.\+' "$REPO/.env" 2>/dev/null; then
  umask 077
  echo "CREDENTIALS_KEY=$(openssl rand -hex 32)" >>"$REPO/.env"
  umask 022
  log "CREDENTIALS_KEY in .env ergänzt"
fi

log "Sprachmodelle"
bash "$REPO/deployments/pi/download-models.sh"

log "Websuche (SearXNG)"
bash "$REPO/deployments/pi/searxng.sh"

log "Browser (Chromium)"
bash "$REPO/deployments/pi/browser.sh"

log "Deploy-Key fuer root"
mkdir -p /root/.ssh
chmod 700 /root/.ssh
touch /root/.ssh/authorized_keys
chmod 600 /root/.ssh/authorized_keys
grep -qF "$DEPLOY_PUBKEY" /root/.ssh/authorized_keys || echo "$DEPLOY_PUBKEY" >>/root/.ssh/authorized_keys

log "DNS: ${HOST} -> ${CNAME_TARGET} (deSEC)"
TOKEN="$(sed -n 's/^dns_desec_token *= *//p' "$DESEC_CREDENTIALS")"
[ -n "$TOKEN" ] || { echo "Kein Token in $DESEC_CREDENTIALS" >&2; exit 1; }
SUB="${HOST%.$ZONE}"
STATUS="$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Token ${TOKEN}" \
  "https://desec.io/api/v1/domains/${ZONE}/rrsets/${SUB}/CNAME/")"
if [ "$STATUS" = "200" ]; then
  log "CNAME existiert bereits"
else
  curl -fsS -X POST -H "Authorization: Token ${TOKEN}" -H "Content-Type: application/json" \
    -d "{\"subname\":\"${SUB}\",\"type\":\"CNAME\",\"ttl\":3600,\"records\":[\"${CNAME_TARGET}\"]}" \
    "https://desec.io/api/v1/domains/${ZONE}/rrsets/" >/dev/null
  log "CNAME angelegt"
fi

log "Zertifikat"
if [ ! -f "/etc/letsencrypt/live/${HOST}/fullchain.pem" ]; then
  # A fresh rrset needs longer than certbot's 80 s default to reach every deSEC secondary.
  certbot certonly --authenticator dns-desec --dns-desec-credentials "$DESEC_CREDENTIALS" \
    --dns-desec-propagation-seconds "${DNS_PROPAGATION_SECONDS:-180}" --email "$CERT_EMAIL" --agree-tos --non-interactive -d "$HOST"
fi

log "nginx"
install -m0644 "$REPO/deployments/pi/nginx/sprachassistent-headers.conf" /etc/nginx/snippets/
install -m0644 "$REPO/deployments/pi/nginx/sprachassistent.conf" /etc/nginx/sites-available/
ln -sf /etc/nginx/sites-available/sprachassistent.conf /etc/nginx/sites-enabled/sprachassistent.conf
nginx -t
systemctl reload nginx

log "Fertig. GitHub Environment production: SSH_HOST=${CNAME_TARGET%.} SSH_USER=root DEPLOY_PATH=${REPO}"
log "Danach Push auf den Branch deploy; dann https://${HOST} oeffnen, das erste Konto wird Admin."
