# Phase 2a — Color: lossless refinement, AVC444, codec ladder

**Spec:** `docs/superpowers/specs/2026-09-29-negotiated-mac-rdp-design.md` §8 (color, codec ladder, lossless refinement), §1.3 criterion 2.
**Execution:** tasks go to the local agent (`gpt-oss:20b`) as goal + acceptance test; Claude reviews diff stat + test output only.
**Baseline to beat** (`docs/research/phase0-baseline.md`): flat patches ΔE 0.24; chroma-edge ΔE ≈ 32.

## Facts (verified 2026-09-29, IronRDP rev a5d1c682)
- `ironrdp-egfx` `GraphicsPipelineServer`: `send_avc420_frame`, `send_avc444_frame(surface, luma, luma_regions, chroma, chroma_regions, ts)`, `send_planar_frame`, `send_clearcodec_frame`, `send_mixed_frame`, `supports_avc444()`.
- `ironrdp-graphics::rdp6::bitmap_stream::encoder::BitmapStreamEncoder` = Planar (lossless) encoder.
- `src/avc444.rs` has the YUV444 → main/aux split (+ decoder-side combine for tests); unused in production.
- `src/h264.rs` ships via `send_avc420_frame(surface_id, &payload, &[region], ts)` with one full-surface region.

## Global constraints
- Hot path (`h264.rs` ship/encode) must not allocate per frame in steady state; reuse buffers.
- Every new behavior is negotiated (client caps), never a user flag. Old behavior reachable via `MACRDP_NEGOTIATE=0`.
- Color truth is the round-trip harness; performance truth is `scripts/perf`.

## Tasks
1. **Static-region tracker (pure).** New `src/refine.rs`: grid of 64×64 tiles; `mark_dirty(rects, now)`, `take_ready(now, idle_ms, budget_tiles) -> Vec<Rect>` returns tiles unchanged for ≥ `idle_ms` and not yet refined, at most `budget_tiles`, merged into row runs. Tests: idle timing, re-dirty cancels refinement, budget cap, edge tiles clipped to surface size (odd sizes).
2. **Planar region encoder.** `src/planar.rs`: `encode_region(bgra, stride, rect) -> Vec<u8>` via `BitmapStreamEncoder` (RLE on). Test: decode with ironrdp's planar decoder → bit-exact vs source for patterns incl. the ColorChecker pattern and odd rect sizes.
3. **Refinement on the EGFX path.** In `h264.rs`, when the client is on EGFX: feed capture dirty rects (or full-frame change) into the tracker; when frames go idle, send ready tiles with `send_planar_frame` after the last AVC frame; budget adapts to bandwidth (skip when adaptive-bitrate reports congestion). Acceptance: extended color harness decodes AVC stream **then** applies planar tiles (ironrdp decoder) → static patches **bit-exact** (ΔE 0.000), edge strip bit-exact.
4. **Odd-height padding.** Encoder receives even (16-aligned) dims; region crops to the true size. Acceptance: harness case 1714×1287 passes (previously lost last row).
5. **AVC444.** Two VideoToolbox encoders (main + aux from `avc444::split_yuv444_to_yuv420_v1`), shipped with `send_avc444_frame` when `supports_avc444()`. Acceptance: harness decodes both streams with ffmpeg, combines with `avc444::combine_*` → moving-content chroma-edge ΔE < 1 (was ≈ 32); perf harness `motion` CPU recorded vs AVC420.
6. **Codec ladder (pure, negotiator).** `negotiator::video::choose(caps) -> VideoPlan {Avc444+refine | Avc420+refine | Lossless}` with reason; logged per connection. Tests over cap combinations incl. `AVC_DISABLED`.
7. **Verify + record.** Loopback + color harness numbers into `docs/research/phase2a-verification.md`.

## Deferred to Phase 2b (speed)
Dirty-region H.264 encode, zero-copy IOSurface/Metal conversion, client-load feedback (AVC444 → AVC420 when acks lag), low-latency VT rate control, perf budgets as gates.
