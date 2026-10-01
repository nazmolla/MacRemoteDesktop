# Fork notes

This repository is a fork of [clintcan/macrdp](https://github.com/clintcan/macrdp).
Design: `docs/superpowers/specs/2026-09-29-negotiated-mac-rdp-design.md`.

## Upstream
This fork is the product. It will not be merged back into clintcan/macrdp or
IronRDP, and upstream sync is no longer a goal (owner decision, 2026-09-30).
The vendored IronRDP crates are owned code: change them as needed and record
the change in the crate's own `CLAUDE.md` divergence log and here.

Pulling a specific upstream fix is still possible (`git fetch upstream`,
then cherry-pick), but expect conflicts in the files listed below.

## Divergence log
| # | File | Change | Reason | Upstreamable? |
|---|------|--------|--------|---------------|
| F1 | `.github/workflows/*.yml` | macOS jobs on PR/dispatch only; release on dispatch only; security weekly | Private-repo CI minutes (macOS bills 10×) | No |
| F2 | `src/h264/annexb.rs` | `avcc_to_annex_b` → `pub(crate)` | Reused by the color round-trip harness | Yes (trivial) |
| F3 | `vendor/ironrdp-acceptor` | divergence (5): `ClientDisplayInfo` on `AcceptorResult` | Negotiator needs client scale/physical size | Yes |
| F4 | `vendor/ironrdp-server` | `ConnectionHandler::on_client_display` hook (24-fork) | Deliver `ClientDisplayInfo` to the app | Yes |
| F5 | `src/virtual_display/*` | sRGB primaries; global dispatch queue; per-display serial; modes are registered and left to become current on their own (never selected explicitly, never `CGRestorePermanentDisplayConfiguration`); at most one display change per second; Retina modes opt-in (`MACRDP_RETINA_VD`), 1× at client pixels by default | Spec §7.2, §8.1; findings in docs/research/2026-09-29-hidpi-virtual-display.md and docs/research/2026-10-01-virtual-display-stability.md | Yes |
| F6 | `vendor/ironrdp-server` | divergence (25-fork): `HandshakePool` with deadlines and per-source caps, one `negotiate` for every connection, channels attached after auth, `ConnectionHandler` methods take the peer plus `on_handshake_failed`, shared `CredentialsHandle`, UDP admission tied to authenticated offers | Review S1, S2, S3, S7 | No (product policy) |
| F7 | `vendor/ironrdp-server` | divergence (26-fork): no environment reads; UDP knobs are `ListenerConfig` fields and `RdpServer` setters | Review A2 | No |
| F8 | `src/auth_guard.rs` | Accounting from explicit handshake signals; IPv6 keyed by /64; audit schema v2 (`handshake_failed`, disconnect `clean`/`error`) | Review S3, S7, S8 | No |
| F9 | `src/credential_monitor.rs`, `src/auth.rs` | PAM re-check every 5 min, Keychain follow, revoke on rejection; `auth::check` returns a verdict | Review S9 | No |
| F10 | `src/private_dir.rs`, `src/rdpdr/surface.rs`, `src/file_promise*.rs`, `src/auth.rs`, `ifd-handler/` | 0700 fresh temp dirs, drive-label sanitising, absolute command paths, IFD length caps and panic guards, PAM response zeroing | Review S4, S5, S6, S10, S11 | No |
| F11 | `src/sync_ext.rs`, all of `src/` | Poison-tolerant locking; `SAFETY:` comments enforced; lint clean with `-D warnings` on both targets | Review Q1 to Q5 | No |
| F12 | `src/tunables.rs` | Registry of every `MACRDP_*` variable, logged at startup | Review A2 | No |
| F13 | `src/resync.rs` | `ResyncSignal` handle replaces the `RESYNC_*` statics | Review A3 | No |
| F14 | `src/main.rs` → `src/app/`; `src/h264.rs` → `src/h264/`; `src/input.rs` → `src/input/` | Module splits; startup as phases | Review A1, A4 | No |
| F15 | `src/h264/mod.rs` | `lock_server` / `lock_ctx` with a debug-build lock-order check; fixed `setup_locked` taking the server lock under `ctx` | Review A6 | No |
| F16 | `src/garbage_input_test.rs`, `fuzz/`, `tools/macos-typecheck/` | Decoder robustness tests, cargo-fuzz targets, macOS type check on Linux | Review Q6, Q3 | No |
| F17 | `src/brand.rs`, `Cargo.toml`, `packaging/*`, `gui/Sources/macrdptray`, `gui/make-tray-app.sh`, `branding/` | Product is **Portico**: `Portico.app`, executable `portico`, bundle and LaunchAgent id `ca.nazmi.portico`, Keychain service `portico` (falls back to `macrdp`), `~/Library/Application Support/Portico`, `~/Library/Logs/portico.log`, new icon. Internal names (crate, `MACRDP_*`, helper executables) unchanged | Own product identity | No |
| F18 | `vendor/ironrdp-egfx` | Divergence (1): AVC420/AVC444 region rectangles written exclusive, as RECT16 requires; one-pixel regions were invalid and Windows clients showed black | Black screen on real clients after per-region updates (Phase 2a) | Yes |
| F19 | `src/capture.rs` | Client-requested sizes rounded down to even (H.264 4:2:0 codes in 2-pixel units) | Odd window sizes from windowed clients | No |
