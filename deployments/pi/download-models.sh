#!/bin/bash
# Downloads the speech models into models/ (once, about 280 MB). Existing files are kept.
#   WHISPER_MODEL_NAME=ggml-base-q5_1.bin bash deployments/pi/download-models.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

WHISPER_MODEL_NAME="${WHISPER_MODEL_NAME:-ggml-small-q5_1.bin}"
PIPER_VOICE="${PIPER_VOICE:-vits-piper-de_DE-thorsten-medium}"

log() { echo "[sprachassistent-models] $*"; }

mkdir -p models/whisper
if [ ! -f "models/whisper/$WHISPER_MODEL_NAME" ]; then
  log "Whisper $WHISPER_MODEL_NAME"
  curl -fL --retry 3 -o "models/whisper/$WHISPER_MODEL_NAME.part" \
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$WHISPER_MODEL_NAME"
  mv "models/whisper/$WHISPER_MODEL_NAME.part" "models/whisper/$WHISPER_MODEL_NAME"
fi

if [ ! -d "models/$PIPER_VOICE" ]; then
  log "Piper-Stimme $PIPER_VOICE"
  curl -fL --retry 3 \
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/$PIPER_VOICE.tar.bz2" \
    | tar -xj -C models
fi

log "fertig: $(du -sh models | cut -f1)"
