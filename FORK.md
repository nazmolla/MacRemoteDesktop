# Fork notes

This repository is a fork of [clintcan/macrdp](https://github.com/clintcan/macrdp).
Design: `docs/superpowers/specs/2026-09-29-negotiated-mac-rdp-design.md`.

## Syncing upstream
    git fetch upstream --tags
    git merge upstream/main
    cargo build --locked && cargo test --locked
Resolve conflicts in favor of upstream unless the file appears in the divergence log below.

## Divergence log
| # | File | Change | Reason | Upstreamable? |
|---|------|--------|--------|---------------|
| F1 | `.github/workflows/*.yml` | macOS jobs on PR/dispatch only; release on dispatch only; security weekly | Private-repo CI minutes (macOS bills 10×) | No |
| F2 | `src/h264.rs` | `avcc_to_annex_b` → `pub(crate)` | Reused by the color round-trip harness | Yes (trivial) |
