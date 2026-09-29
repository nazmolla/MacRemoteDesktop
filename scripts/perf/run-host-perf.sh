#!/usr/bin/env bash
# Measure an RDP server process on this Mac while a workload runs.
#   macrdp (default): builds nothing; launches target/release/macrdp on a
#     1920x1080 virtual display plus a loopback sdl-freerdp client. The workload
#     runs on the virtual display, so the client window (on the physical
#     screen) is never captured (no mirror feedback).
#   other process name (e.g. "Jump Desktop Connect"): connect from Windows first,
#     make sure the remote view shows the display the workload will use, then run.
# Usage: run-host-perf.sh <label> <idle|typing|scroll|motion> <seconds> [process-name]
set -euo pipefail
label=$1 workload=$2 seconds=$3 target=${4:-macrdp}
root=$(cd "$(dirname "$0")/../.." && pwd)
out="$root/perf-results/$(date +%Y%m%d-%H%M%S)-$label-$workload"
mkdir -p "$out"
server_pid="" client_pid=""
cleanup() {
  [[ -n "$client_pid" ]] && kill "$client_pid" 2>/dev/null || true
  [[ -n "$server_pid" ]] && kill "$server_pid" 2>/dev/null || true
}
trap cleanup EXIT

screen_arg=()
if [[ "$target" == macrdp ]]; then
  [[ -x "$root/target/release/macrdp" ]] || { echo "build first: cargo build --release --locked" >&2; exit 2; }
  "$root/target/release/macrdp" --bind 127.0.0.1:3390 --skip-auth --password perf \
    --virtual-display --width 1920 --height 1080 --enable-h264 --adaptive-bitrate \
    >"$out/server.log" 2>&1 &
  server_pid=$!
  sleep 3
  kill -0 "$server_pid" 2>/dev/null || { echo "macrdp exited; see $out/server.log" >&2; exit 1; }
  frdp=$(command -v sdl-freerdp || command -v sdl3-freerdp)
  "$frdp" /v:127.0.0.1:3390 /u:"$USER" /p:perf /cert:ignore /gfx:avc444 /size:1920x1080 \
    >"$out/client.log" 2>&1 &
  client_pid=$!
  sleep 5
  pid=$server_pid
  screen_arg=(macrdp)
else
  pid=$(pgrep -x "$target" | head -1) || { echo "no running process named '$target'" >&2; exit 1; }
fi

"$root/target/workload" "$workload" "$((seconds + 3))" "${screen_arg[@]}" &
workload_pid=$!
sleep 2
kill -0 "$workload_pid" 2>/dev/null || { echo "workload failed to start (locked screen or missing display); no measurement taken" >&2; exit 1; }
python3 "$root/scripts/perf/perfsample.py" sample "$pid" "$seconds" "$out/samples.csv"
python3 "$root/scripts/perf/perfsample.py" summarize "$out/samples.csv" | tee "$out/summary.txt"
echo "results: $out"
