#!/bin/zsh
# WARNING: do not run on a machine in use. Rapid display churn crashed WindowServer
# (logging out every session) and, serialized, wedged the Mac until a hard restart.
# See docs/research/2026-10-01-virtual-display-stability.md.
# Unattended display-host stress test. Writes results-<timestamp>.txt next to this script.
cd "$(dirname "$0")"
out="results-$(date +%Y%m%d-%H%M%S).txt"
export DH_VENDOR=$(printf '0x%X' $(( 0x50000000 + RANDOM * 65536 + RANDOM )))  # fresh ID: no remembered prefs
{
echo "vendor=$DH_VENDOR  started=$(date)"
echo "--- displays + remote-display processes at start"
ps -axo user,command | grep -iE "JumpConnect --desktopproxy|ScreensharingAgent" | grep -v grep | cut -c1-110
echo
echo "=== A: create in a fresh process (20 runs, mixed 1x/2x)"
okA=0
for i in $(seq 1 20); do
  if (( i % 2 )); then c="create $((5000+i)) 1920 1080 1"; else c="create $((5000+i)) 3840 2160 2"; fi
  r=$(printf '%s\nquit\n' "$c" | ./displayhost 2>/dev/null); echo "  $c -> $r"; [[ $r == ok* ]] && okA=$((okA+1)); sleep 1
done
echo "A: $okA/20"
echo
echo "=== B: re-modes inside one process (10 processes x 8 modes)"
okB=0; totB=0
for i in $(seq 1 10); do
  cmds="create $((6000+i)) 1920 1080 1
mode 3440 1440 1
mode 1714 1288 1
mode 3840 2160 2
mode 1920 1080 1
mode 5120 2880 2
mode 1714 1287 1
mode 2560 1440 1
mode 3428 2576 2
quit"
  res=$(printf '%s\n' "$cmds" | ./displayhost 2>/dev/null)
  n=$(echo "$res" | grep -c '^ok'); t=$(echo "$res" | grep -c .)
  okB=$((okB+n)); totB=$((totB+t)); echo "  run $i: $n/$t"; echo "$res" | grep '^err' | sed 's/^/     /'
  sleep 1
done
echo "B: $okB/$totB"
echo
# Start a host whose stdin stays open (fd 3), so it lives until we quit or kill it.
start_host() { rm -f /tmp/dh.in /tmp/dh.out; mkfifo /tmp/dh.in; ./displayhost < /tmp/dh.in > /tmp/dh.out 2>/dev/null & hostpid=$!; exec 3>/tmp/dh.in; }
first_reply() { for k in $(seq 1 60); do [[ -s /tmp/dh.out ]] && { head -1 /tmp/dh.out; return; }; sleep 0.1; done; echo "timeout"; }
echo "=== C: crash + restart with the same serial (10 cycles)"
okC=0
for i in $(seq 1 10); do
  s=$((7000+i))
  start_host; print -u 3 "create $s 2560 1440 1"; r1=$(first_reply)
  kill -9 $hostpid 2>/dev/null; exec 3>&-; wait $hostpid 2>/dev/null; sleep 0.5
  r2=$(printf 'create %s 2560 1440 1\nquit\n' $s | ./displayhost 2>/dev/null)
  echo "  cycle $i: first=[$r1] after-crash=[$r2]"; [[ $r2 == ok* ]] && okC=$((okC+1)); sleep 1
done
echo "C: $okC/10"
echo
echo "=== D: two displays at once, one per process (5 runs)"
okD=0
for i in $(seq 1 5); do
  start_host; print -u 3 "create $((8000+i)) 1920 1080 1"; ra=$(first_reply)
  rb=$(printf 'create %s 2560 1440 2\nquit\n' $((8100+i)) | ./displayhost 2>/dev/null)
  print -u 3 "quit"; exec 3>&-; wait $hostpid 2>/dev/null
  echo "  run $i: A=[$ra] B=[$rb]"; [[ $ra == ok* && $rb == ok* ]] && okD=$((okD+1)); sleep 1
done
echo "D: $okD/5"
echo
echo "SUMMARY  A=$okA/20  B=$okB/$totB  C=$okC/10  D=$okD/5  finished=$(date)"
} > "$out" 2>&1
echo "$out"
