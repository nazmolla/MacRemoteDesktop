# Review & security-audit guide

For an agent or person doing a **full code review** or **security audit** of this repository. Read this first; it tells you what the code is, what is ours vs upstream, how to build/test, and where the risk is.

## 1. What this is
A native RDP server for macOS in Rust, built on IronRDP. Stock RDP clients (mstsc, Windows App, FreeRDP) connect to a (typically headless) Mac. This repo is a **fork of [clintcan/macrdp](https://github.com/clintcan/macrdp)** restructured so sessions are **negotiated from the client handshake instead of configured with flags** (spec: `docs/superpowers/specs/2026-09-29-negotiated-mac-rdp-design.md`).

## 2. What is ours vs upstream
- Upstream history is merged in; `git log upstream/main` is macrdp. **The fork's own changes:** `git diff upstream/main...main` (remote `upstream` = `https://github.com/clintcan/macrdp.git`; add it if missing).
- Every deliberate difference is listed in `FORK.md` → *Divergence log* (F1…). Vendored IronRDP forks (`vendor/ironrdp-*`) each keep a divergence log in their own `CLAUDE.md`.
- Upstream code is in scope for a security audit (it ships in the product) but review findings should say whether a defect is **upstream** or **fork-introduced**.

## 3. Fork-introduced modules (highest review value)
| Area | Files | What it does |
|---|---|---|
| Session Negotiator | `src/negotiator/{display,session,host,handler,video}.rs` | Pure decisions: display scale plan, Ctrl→Cmd, privacy shield, codec ladder; `handler.rs` is a `ConnectionHandler` decorator recording client scale/platform |
| Zero-flag startup | `src/main.rs` (`apply_negotiated_defaults`) | Turns features on from host caps; `MACRDP_NEGOTIATE=0` restores flag-only |
| Handshake plumbing | `vendor/ironrdp-acceptor` div (5), `vendor/ironrdp-server` `on_client_display` | Carries GCC Core Data scale/physical size to the app |
| Retina virtual display | `src/virtual_display/{mod,private_api}.rs` | Private `CGVirtualDisplay` API, hiDPI modes, raw CoreGraphics mode FFI (`cg_modes`), sRGB primaries |
| Dirty-region H.264 | `src/h264.rs` (`FrameRegions`, `RegionDebt`, `avc_regions`, region queue by PTS), `src/capture.rs` | AVC frames repaint only changed rects |
| Lossless refinement | `src/refine.rs`, `src/lossless.rs`, `src/h264.rs` (`refine_tick`) | Idle tiles re-sent as ClearCodec via `send_mixed_frame` |
| AVC444 | `src/avc444.rs`, `src/h264.rs` (`Avc444Buffers`, aux encoder, PTS pairing in `ship_loop`), `src/videotoolbox.rs` (`encode_yuv420`, odd-height padding) | 4:4:4 as main + auxiliary H.264 streams |
| Test/perf tooling | `src/color_*`, `scripts/perf/*`, `tools/workload`, `scripts/verify/*` | Harnesses (not shipped) |

## 4. Security-relevant surface (audit focus)
- **Network, pre-auth:** TLS + NLA/CredSSP in vendored `ironrdp-server`/`ironrdp-acceptor`; X.224/MCS/GCC parsing; per-IP rate limit + lockout in `src/auth_guard.rs`. The acceptor now copies more client-controlled GCC fields (`ClientDisplayInfo`) — check they are only used as bounded hints.
- **Client-controlled sizes → allocations:** desktop size, monitor layout (`request_layout`), scale factor → `negotiator::display::plan_display` → virtual display modes, capture sizes, encoder dims, region rects, tracker tiles. Check clamping (200–8192), integer overflow, `as u16` truncation, division by zero.
- **Authentication:** PAM (`src/auth.rs`), Keychain password path (`docs/macos-gotchas.md`), `--skip-auth` (dev only). Auto-unlock types the account password into the lock screen (`--auto-unlock`, `docs/known-quirks.md`).
- **Local IPC (loopback, unauthenticated by design):** stats `:40245`, smart-card bridge `:40242`, HUD `:40243`, shield helper `:40244`, NFS mount for drive redirection. Threat model: trusted single local user (`docs/macos-gotchas.md`, last bullet). Report anything that widens exposure off loopback.
- **Privacy blanking:** `--shield-primary` / negotiated shield (helper process), `--capture-primary` (cannot lock the Mac while engaged — documented). Negotiated shield is skipped with a log line when the helper is missing (`apply_negotiated_defaults`) — check this fail-open is acceptable/visible.
- **Private Apple APIs + unsafe FFI:** `src/virtual_display/private_api.rs`, `src/usb_redirect/usb_spike.m`, `src/cursor/private_api.rs`, CoreGraphics raw FFI (`cg_modes`), `CGRestorePermanentDisplayConfiguration` side effects (resets other app-scoped display configs, e.g. `--detach-primary`).
- **Redirection channels:** RDPDR drives (NFS), smart card (IFD handler cdylib `ifd-handler/`), USB (UserHCI, entitled build only), camera system extension, clipboard (file promises, rich text). Client-supplied data crosses into the Mac here.
- **Supply chain:** `deny.toml` (cargo-deny), `.github/workflows/security.yml`, pinned IronRDP git rev `a5d1c682`.

## 5. Concurrency invariants to check
- Lock order in `src/h264.rs`: **`server_handle` → `ctx`**, never the reverse (`GfxDvcBridge::process` holds the server mutex while calling handler callbacks that lock `ctx`). `refine_tick` prepares under `ctx`, releases it, then locks `server_handle`.
- Region queue is keyed by encoder PTS and reset on every encoder rebuild; `RegionDebt` carries dropped frames' regions.
- AVC444: main and aux encoders get the same keyframe decision; the ship thread pairs by PTS (250 ms timeout; unpaired main ships as AVC420).
- One active RDP session per process (newer authenticated connection preempts).

## 6. Build & test
- Toolchain: stable Rust (`rust-toolchain.toml`), `cmake` (boring-sys). macOS-only code is `#[cfg(target_os = "macos")]` with Linux stubs.
- **Linux (cloud):** `cargo test --locked` must pass (upstream CI does this). macOS APIs (VideoToolbox, ScreenCaptureKit, CGVirtualDisplay) are stubbed and cannot run.
- **macOS:** `cargo test --locked`; ignored integration tests need a WindowServer / ffmpeg:
  - `cargo test --locked --release color_roundtrip -- --ignored --nocapture` (color ΔE harness)
  - `cargo test --locked planned_tests::<name> -- --ignored` (virtual display; **one test per process** — a second virtual display in the same process never comes online)
  - `scripts/verify/phase1-loopback.sh` (negotiation across sizes/scales)
- Lint: `cargo clippy --all-targets -- -D warnings` currently fails on **pre-existing upstream** code under rustc 1.98 (new lints); fork files are kept clean.

## 7. Known limitations (don't report as new findings unless you find a new angle)
Listed in `docs/known-quirks.md`, `docs/macos-gotchas.md`, `docs/research/*` (hidpi, multisession spike, phase verifications), and `docs/reviews/phase-2a-review.md` (resolved findings + deferred minors M1–M3).

## 8. Reporting format
Per finding: **severity** (Critical/Important/Minor), **file:line**, **origin** (fork / upstream / vendored-IronRDP), **concrete failure or attack scenario**, **fix**. No style nits. For the security audit also give a short threat-model summary and an overall risk rating. Write results to `docs/reviews/<date>-<full-review|security-audit>.md` on a branch and open a PR against `main`.
