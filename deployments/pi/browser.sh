#!/bin/bash
# Headless Chromium for web_research follow on pages that build their content with JavaScript.
# Called by bootstrap.sh and runtime-reload.sh. Idempotent: installs Chromium and the account it runs
# as only when missing. The voice server starts it with setpriv as that account, never as root,
# and routes all of its traffic through a proxy that only lets https to public hosts through.
set -euo pipefail
USER_NAME="${BROWSER_USER:-sprachassistent-browser}"

log() { echo "[sprachassistent-browser] $*"; }

if ! command -v chromium >/dev/null && ! command -v chromium-browser >/dev/null; then
  log "Chromium installieren"
  apt-get update -qq
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends chromium fonts-dejavu-core
fi

if ! id "$USER_NAME" >/dev/null 2>&1; then
  useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin "$USER_NAME"
  log "Benutzer $USER_NAME angelegt"
fi

command -v setpriv >/dev/null || { echo "ERROR: setpriv fehlt (util-linux)" >&2; exit 1; }

# One render as the browser account through no proxy at all: proves Chromium and its sandbox start.
TMP="$(mktemp -d)"
chmod 777 "$TMP"
if setpriv --reuid "$USER_NAME" --regid "$USER_NAME" --clear-groups -- \
  env HOME="$TMP" "$(command -v chromium || command -v chromium-browser)" --headless=new --disable-gpu \
  --no-first-run --user-data-dir="$TMP/profile" --dump-dom 'data:text/html,<p>bereit</p>' 2>/dev/null \
  | grep -q bereit; then
  log "Chromium läuft als $USER_NAME"
else
  rm -rf "$TMP"
  echo "ERROR: Chromium startet als $USER_NAME nicht" >&2
  exit 1
fi
rm -rf "$TMP"
