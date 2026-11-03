#!/bin/bash
# Downloads the speech models into models/ (once, about 400 MB). Existing files are kept.
# Each device picks one of the voices in the admin area.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

# German streaming recognizer by Kroko (Banafo), CC-BY-SA:
# https://huggingface.co/Banafo/Kroko-ASR
STT_MODEL="sherpa-onnx-streaming-zipformer-de-kroko-2025-08-06"
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

mkdir -p models/stt
if [ ! -f "models/stt/$STT_MODEL/tokens.txt" ]; then
  log "Spracherkennung $STT_MODEL"
  rm -rf "models/stt/$STT_MODEL"
  curl -fL --retry 3 "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$STT_MODEL.tar.bz2" \
    | tar -xj -C models/stt
fi
# Replaced by the streaming recognizer.
rm -rf models/whisper

for voice in "${VOICES[@]}"; do
  dir="vits-piper-$voice"
  [ -f "models/$dir/tokens.txt" ] && continue
  log "Stimme $voice"
  rm -rf "models/$dir"
  curl -fL --retry 3 "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/$dir.tar.bz2" \
    | tar -xj -C models
done

log "fertig: $(du -sh models | cut -f1)"
