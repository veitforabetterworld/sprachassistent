#!/bin/bash
# Downloads the speech models into models/ (once, about 450 MB). Existing files are kept.
# Each device picks one of the voices in the admin area.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

WHISPER_MODEL_NAME="${WHISPER_MODEL_NAME:-ggml-small-q5_1.bin}"
# German Piper voices as sherpa-onnx archives (vits-piper-<name>.tar.bz2).
VOICES=(
  de_DE-thorsten-medium
  de_DE-karlsson-low
  de_DE-pavoque-low
  de_DE-kerstin-low
  de_DE-eva_k-x_low
  de_DE-ramona-low
)

log() { echo "[sprachassistent-models] $*"; }

mkdir -p models/whisper
if [ ! -f "models/whisper/$WHISPER_MODEL_NAME" ]; then
  log "Whisper $WHISPER_MODEL_NAME"
  curl -fL --retry 3 -o "models/whisper/$WHISPER_MODEL_NAME.part" \
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$WHISPER_MODEL_NAME"
  mv "models/whisper/$WHISPER_MODEL_NAME.part" "models/whisper/$WHISPER_MODEL_NAME"
fi

for voice in "${VOICES[@]}"; do
  dir="vits-piper-$voice"
  [ -f "models/$dir/tokens.txt" ] && continue
  log "Stimme $voice"
  rm -rf "models/$dir"
  curl -fL --retry 3 "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/$dir.tar.bz2" \
    | tar -xj -C models
done

log "fertig: $(du -sh models | cut -f1)"
