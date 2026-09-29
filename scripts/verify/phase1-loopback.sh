#!/usr/bin/env bash
# Phase 1 loopback verification: zero server feature flags; FreeRDP clients at
# several sizes/scales; assert the negotiated display plan in the server log.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
log="$HOME/Library/Logs/macrdp.log"
frdp=$(command -v sdl-freerdp || command -v sdl3-freerdp)
# size  scale  expected substring of the "display: plan applied" reason
cases=(
  "1920x1080 100 1× at 1920×1080"
  "3840x2160 200 Retina 1920×1080 pt"
  "3000x2000 150 Retina 2000×1333 pt"
  "1714x1288 100 1× at 1714×1288"
)
fail=0
for c in "${cases[@]}"; do
  read -r size scale expect <<<"$c"
  : > "$log"
  "$root/target/release/macrdp" --bind 127.0.0.1:3390 --skip-auth --password perf >/dev/null 2>&1 &
  srv=$!
  sleep 4
  "$frdp" /v:127.0.0.1:3390 /u:"$USER" /p:perf /cert:ignore /gfx:avc444 /size:"$size" /scale-desktop:"$scale" >/dev/null 2>&1 &
  cli=$!
  sleep 10
  line=$(grep "display: plan applied" "$log" | tail -1 || true)
  kill "$cli" "$srv" 2>/dev/null || true
  wait "$cli" "$srv" 2>/dev/null || true
  if [[ "$line" == *"$expect"* ]]; then
    echo "PASS $size@$scale%"
  else
    echo "FAIL $size@$scale%: expected '$expect', got: ${line:-<no plan line>}"
    fail=1
  fi
done
exit $fail
