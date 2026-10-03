#!/bin/bash
# Build (and sign, when CODESIGN_IDENTITY is set) target/VigaMic.driver.
# Install: sudo cp -R target/VigaMic.driver /Library/Audio/Plug-Ins/HAL/ && sudo killall coreaudiod
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/target/VigaMic.driver"
rm -rf "$OUT"; mkdir -p "$OUT/Contents/MacOS"
cp "$ROOT/mic-driver/Info.plist" "$OUT/Contents/Info.plist"
clang -bundle -O2 -Wall -Wextra -Wno-unused-parameter -framework CoreAudio -framework CoreFoundation \
    -o "$OUT/Contents/MacOS/VigaMic" "$ROOT/mic-driver/VigaMic.c"
if [ -n "${CODESIGN_IDENTITY:-}" ]; then
    codesign --force --options runtime --timestamp -s "$CODESIGN_IDENTITY" "$OUT"
fi
echo "$OUT"
