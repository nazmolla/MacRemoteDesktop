# Security model

Attack surface, trust boundaries and known limitations of the server. Threat model for local IPC: a trusted single local user (see `docs/macos-gotchas.md`).

The full review this is based on, with measurements and the fixes made, is `docs/reviews/codebase-review.md`.

## Attack surface

- **Network, before authentication.** TLS and NLA/CredSSP in the vendored `ironrdp-server` / `ironrdp-acceptor`, plus X.224/MCS/GCC parsing. Every connection takes the same bounded path (`vendor/ironrdp-server`, divergence 25-fork):
  - the auth guard (`src/auth_guard.rs`) applies a per-source rate limit and lockout before anything is parsed. A source is an IPv4 address or an IPv6 /64, so one IPv6 prefix gets one budget, not one per address. Loopback is exempt;
  - at most 32 handshakes run at once, and at most 4 per source. Over capacity the socket is closed straight away;
  - a handshake has 5 s to send the X.224 request and 30 s to finish TLS and CredSSP. Handshakes run concurrently, so silent connections cannot hold a real user off;
  - channel factories run and the multitransport offer is made only after authentication.
- **Failure accounting.** The guard counts a failure when CredSSP rejects the password, or when a handshake times out or fails TLS or protocol checks. It counts a success only on a real authentication. A connection refused for capacity is audited but not counted. Every outcome is attributed to the peer that produced it, not to the most recently accepted connection.
- **UDP multitransport** (only with `--enable-udp-multitransport` or `--enable-lossy-audio`). The listener starts a flow only for a SYN of at least 1132 bytes (so the reply is never larger than the request) from an IP that holds a live offer from an authenticated TCP session, with at most 64 peers and 4 per IP. A client whose UDP leaves from a different public IP than its TCP stays on TCP.
- **Client-controlled sizes into allocations.** Desktop size, monitor layout (`request_layout`) and scale factor flow through `negotiator::display::plan_display` into virtual display modes, capture sizes, encoder dimensions, region rectangles and tracker tiles. Sizes are clamped to 200 to 8192. Casts of external input use `try_from`, and the wire decoders get random and truncated input on every `cargo test` (`src/garbage_input_test.rs`) plus cargo-fuzz targets (`fuzz/`).
- **Authentication and the account password.**
  - PAM (`src/auth.rs`). The conversation callback zeroes and frees earlier responses if it fails part way.
  - The Keychain path runs `/usr/bin/security` by absolute path; every external command is named by absolute path, and a test fails the build if one is not.
  - The password is re-checked with PAM every 5 minutes (`src/credential_monitor.rs`). If PAM rejects it (password changed, account disabled), RDP credentials are revoked at once and that password is never tried again. With `--keychain`, a changed Keychain entry is picked up within 60 s after one PAM check. A PAM service error changes nothing.
  - `--skip-auth` is refused unless the bind address is loopback.
  - Auto-unlock types the account password into the lock screen (`--auto-unlock`, `docs/known-quirks.md`). It reads the same credential cell the monitor maintains, so a revoked password is never typed.
- **Local IPC** (loopback, unauthenticated by design): stats `:40245`, smart-card bridge `:40242`, HUD `:40243`, shield helper `:40244`, the NFS mount for drive redirection. Threat model: trusted single local user (`docs/macos-gotchas.md`, last bullet). The smart-card IFD handler runs inside the system `slotd` daemon, so it caps every length it reads from its socket before allocating, and every exported function catches panics instead of unwinding into `slotd`.
- **Files and directories.** Temporary directories (paste, lazy paste, drive-redirection mountpoints) are created fresh with mode 0700 by `src/private_dir.rs`, which refuses a path that already exists and checks the owner. Client drive labels are sanitised so that `..`, `.`, an empty label or a leading dot cannot escape the mount directory.
- **Privacy blanking.** `--shield-primary` / negotiated shield (helper process) and `--capture-primary` (cannot lock the Mac while engaged, documented). The negotiated shield is skipped with a log line when the helper is missing (`apply_negotiated_defaults`).
- **Private Apple APIs and unsafe FFI.** `src/virtual_display/private_api.rs`, `src/usb_redirect/usb_spike.m`, `src/cursor/private_api.rs`, CoreGraphics raw FFI (`cg_modes`), and `CGRestorePermanentDisplayConfiguration` side effects (it resets other app-scoped display configurations, for example `--detach-primary`). Every `unsafe` block in Rust carries a `SAFETY:` comment, enforced by `clippy::undocumented_unsafe_blocks` in `src/` and `ifd-handler/`.
- **Redirection channels.** RDPDR drives (NFS), smart card (`ifd-handler/`), USB (UserHCI, entitled build only), camera system extension, clipboard (file promises, rich text). Client-supplied data crosses into the Mac here.
- **Supply chain.** `deny.toml` (cargo-deny) and the pinned IronRDP git revision. The vendored IronRDP crates are owned code in this fork and are not synced with upstream.

## Audit trail

`--audit-file` / `MACRDP_AUDIT_JSON=1` writes one JSON line per security event (schema v2, `docs/audit-log.md`): accept, reject (rate limit or lockout), auth, handshake failures with their reason, the client fingerprint and disconnects.

## Known limitations

- Not yet verified on a Mac: the macOS-only paths above were type-checked and linted on Linux, not run. See the checklist in the review's resolution section.
- The fuzz targets exist but no long fuzzing campaign has been run.
- The Swift helpers (`gui/`) and the Objective-C USB shim were read for their IPC surface only, not audited line by line.
- Other limitations are listed in `docs/known-quirks.md`, `docs/macos-gotchas.md`, `docs/research/*` and `docs/reviews/phase-2a-review.md`.
