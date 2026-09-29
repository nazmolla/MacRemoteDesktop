# Performance harness

## Host (this Mac)
1. `cargo build --release --locked && tools/workload/build.sh`
2. `scripts/perf/run-host-perf.sh <label> <idle|typing|scroll|motion> <seconds> [process-name]`
3. Results land in `perf-results/` (git-ignored); copy the summary into `docs/research/phase0-baseline.md`.

The default target launches macrdp on a virtual display with a loopback FreeRDP
client. To measure another server (e.g. Jump Desktop Connect), connect to it from
Windows first, then pass its process name.

## Client (Windows PC)
`scripts/perf/client-perf.ps1 -ProcessName mstsc -Seconds 60` while the same
workload runs on the Mac. Use `-ProcessName JumpDesktop` for Jump Desktop.

## Workloads
`target/workload <mode> <seconds> [screen-name-substring]` — idle, typing (10 chars/s),
scroll (60 px/s), motion (full-screen animated gradient at 60 Hz).

## Preconditions
The Mac's session must be **unlocked**: a locked session never draws the workload
window, so `target/workload` exits 4 and the runner refuses to measure. (Phase 0
lost a full baseline run to an auto-lock before this guard existed.)
