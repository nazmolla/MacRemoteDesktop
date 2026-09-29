# Phase 0 baselines

## Build/Test (upstream at import)
- Upstream commit: dd1b6d9
- Toolchain: rustc 1.98.1 (48a229cea 2026-09-01) (plus Homebrew cmake, needed by boring-sys)
- `cargo test --locked`: 247 passed, 0 failed, 3 ignored (macOS 27, Apple M6)

## Color (AVC420, VideoToolbox, 50 Mbps, 30 static frames, ffmpeg decode, full-range BT.709)
| Size | Patch mean ΔE00 | Patch max ΔE00 | Chroma-edge mean ΔE00 |
|------|-----------------|----------------|-----------------------|
| 1920×1080 | 0.239 | 0.615 | 31.900 |
| 1714×1288 | 0.239 | 0.615 | 31.939 |

Command: `cargo test --locked --release color_roundtrip -- --ignored --nocapture`

Reading: flat colour areas already survive well (ΔE < 1 is invisible). Fine alternating
colour detail is destroyed (ΔE ≈ 32: 1-px red/blue columns turn purple) — the 4:2:0 loss
that AVC444 + lossless refinement (Phase 2) must fix. This is what makes coloured text
and thin UI lines look smeared today.

**Finding (Phase 2):** an odd frame height (legal in RDP; only widths must be even) loses
its last pixel row — `1714×1287` decodes as `1714×1286`, because the encoder is handed
the odd size. Production must pad to even/16 and let the client crop.
