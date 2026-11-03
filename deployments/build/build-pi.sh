#!/bin/bash
# Builds bin/sprachassistent and bin/lib/*.so for the Raspberry Pi 5. Runs inside
# deployments/build/Dockerfile.
set -euo pipefail
TARGET=aarch64-unknown-linux-gnu
OUT="target/$TARGET/release"

cargo build --release --locked --target "$TARGET"

rm -rf bin
mkdir -p bin/lib
cp "$OUT/sprachassistent" bin/sprachassistent
# sherpa-onnx (feature "shared") copies its prebuilt libraries next to the binary.
cp -L "$OUT"/libsherpa-onnx-c-api.so "$OUT"/libonnxruntime.so bin/lib/

echo "bin: $(du -sh bin | cut -f1)"
# The Pi needs at least this glibc version (bookworm has 2.36).
for f in bin/sprachassistent bin/lib/*.so; do
  echo "$f: $(aarch64-linux-gnu-objdump -T "$f" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)"
done
echo "rpath: $(aarch64-linux-gnu-readelf -d bin/sprachassistent | grep -E 'RPATH|RUNPATH' | grep -o '\[.*\]')"
