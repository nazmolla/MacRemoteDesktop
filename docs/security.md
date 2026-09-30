# Security model

Attack surface, trust boundaries and known limitations of the server. Threat model for local IPC: a trusted single local user (see `docs/macos-gotchas.md`).

## Attack surface
- **Network, pre-auth:** TLS + NLA/CredSSP in vendored `ironrdp-server`/`ironrdp-acceptor`; X.224/MCS/GCC parsing; per-IP rate limit + lockout in `src/auth_guard.rs`. The acceptor now copies more client-controlled GCC fields (`ClientDisplayInfo`) — check they are only used as bounded hints.
- **Client-controlled sizes → allocations:** desktop size, monitor layout (`request_layout`), scale factor → `negotiator::display::plan_display` → virtual display modes, capture sizes, encoder dims, region rects, tracker tiles. Check clamping (200–8192), integer overflow, `as u16` truncation, division by zero.
- **Authentication:** PAM (`src/auth.rs`), Keychain password path (`docs/macos-gotchas.md`), `--skip-auth` (dev only). Auto-unlock types the account password into the lock screen (`--auto-unlock`, `docs/known-quirks.md`).
- **Local IPC (loopback, unauthenticated by design):** stats `:40245`, smart-card bridge `:40242`, HUD `:40243`, shield helper `:40244`, NFS mount for drive redirection. Threat model: trusted single local user (`docs/macos-gotchas.md`, last bullet). Report anything that widens exposure off loopback.
- **Privacy blanking:** `--shield-primary` / negotiated shield (helper process), `--capture-primary` (cannot lock the Mac while engaged — documented). Negotiated shield is skipped with a log line when the helper is missing (`apply_negotiated_defaults`) — check this fail-open is acceptable/visible.
- **Private Apple APIs + unsafe FFI:** `src/virtual_display/private_api.rs`, `src/usb_redirect/usb_spike.m`, `src/cursor/private_api.rs`, CoreGraphics raw FFI (`cg_modes`), `CGRestorePermanentDisplayConfiguration` side effects (resets other app-scoped display configs, e.g. `--detach-primary`).
- **Redirection channels:** RDPDR drives (NFS), smart card (IFD handler cdylib `ifd-handler/`), USB (UserHCI, entitled build only), camera system extension, clipboard (file promises, rich text). Client-supplied data crosses into the Mac here.
- **Supply chain:** `deny.toml` (cargo-deny), `.github/workflows/security.yml`, pinned IronRDP git rev `a5d1c682`.

## Known limitations
Listed in `docs/known-quirks.md`, `docs/macos-gotchas.md`, `docs/research/*` (hidpi, multisession spike, phase verifications), and `docs/reviews/phase-2a-review.md` (resolved findings + deferred minors M1–M3).
