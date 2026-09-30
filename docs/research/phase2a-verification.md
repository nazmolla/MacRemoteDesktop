# Phase 2a verification (2026-09-30)

## Color (encoder harness, 1920×1080, 50 Mbps, ffmpeg decode)
| Path | Patch mean ΔE00 | Chroma-edge mean ΔE00 |
|---|---|---|
| AVC420 (Phase 0 baseline) | 0.239 | 31.900 |
| AVC444 v1 (main + aux) | 0.238 | **0.180** |
| Lossless ClearCodec tile (refinement) | 0 (bit-exact) | 0 (bit-exact) |
Odd height 1714×1287 now round-trips without losing the last row.

## Live path (loopback FreeRDP `/gfx:avc444`, zero server flags)
- Negotiator log: `video: AVC444 on (client advertises it)`.
- 282/282 shipped frames sent as AVC444, no server or client errors.
- AVC frames carry dirty-rect regions; refinement tiles sent as positioned ClearCodec tiles.
- Idle refinement could not be observed in loopback on this Mac (virtual display mirrored onto the screen showing the client → full-screen change every frame).

## Review
Cloud review (docs/reviews/phase-2a-review.md): 3 Critical + 5 Important fixed; 3 Minor deferred to 2b.

## Pending (needs unlocked Mac / real mstsc)
- Host CPU: AVC444 vs AVC420 under the motion workload (`scripts/perf/run-host-perf.sh`, `MACRDP_AVC444=0` for AVC420).
- mstsc: colored text sharp (AVC444), static photo/document sharpens after ~0.2 s (refinement), no stale leftovers after windows move/close (dirty regions).
