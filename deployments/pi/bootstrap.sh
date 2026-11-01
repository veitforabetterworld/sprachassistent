#!/bin/bash
# One-time setup of sprachassistent.plenivo.de on the Raspberry Pi (next to plenivo and
# hochwasserdienst). Run as root from a checkout or copy of the repo:
#   DEPLOY_PUBKEY="ssh-ed25519 AAAA... github-actions-sprachassistent" bash deployments/pi/bootstrap.sh
# Idempotent: existing .env, data, models, DNS record, certificate and keys are kept.
# Afterwards every push to the deploy branch deploys via .github/workflows/deploy.yml.
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
apt-get install -y -qq rsync curl bzip2

mkdir -p "$REPO/data" "$REPO/bin"
if [ "$SCRIPT_DIR" != "$REPO/deployments/pi" ]; then
  rsync -a "$SCRIPT_DIR/../../deployments/" "$REPO/deployments/"
fi

if [ ! -f "$REPO/.env" ]; then
  log ".env anlegen"
  API_KEY="$(openssl rand -hex 24)"
  umask 077
  cat >"$REPO/.env" <<EOF
API_HOST=127.0.0.1
API_PORT=3020
API_KEY=${API_KEY}
DATA_DIR=${REPO}/data
WHISPER_MODEL=${REPO}/models/whisper/ggml-small-q5_1.bin
WHISPER_THREADS=4
WHISPER_BEAM_SIZE=1
WHISPER_LANGUAGE=de
PIPER_MODEL=${REPO}/models/vits-piper-de_DE-thorsten-medium/de_DE-thorsten-medium.onnx
PIPER_THREADS=2
PIPER_SPEED=1.0
LLM_PROVIDERS=groq,gemini
GROQ_API_KEY=
GROQ_MODEL=llama-3.3-70b-versatile
GEMINI_API_KEY=
GEMINI_MODEL=gemini-2.5-flash-lite
HISTORY_MESSAGES=12
MEMORY_EXTRACTION=true
RUST_LOG=info
EOF
  umask 022
  log "API_KEY fuer den ESP32 (firmware/include/secrets.h): ${API_KEY}"
  log "GROQ_API_KEY und GEMINI_API_KEY in $REPO/.env eintragen."
fi

log "Sprachmodelle"
bash "$REPO/deployments/pi/download-models.sh"

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
log "Danach Push auf den Branch deploy, das baut und startet den Server."
