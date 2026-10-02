# Audit, 2026-10-02

| Audit | Tool | Result |
|---|---|---|
| Dependency licences | `cargo deny check licenses` | Pass. Every crate is MIT, Apache-2.0, BSD, ISC, Zlib or Unicode-3.0; no copyleft. MPL-2.0 and Unicode-DFS-2016 are allowed in `deny.toml` but unused. |
| Known vulnerabilities | `cargo deny check advisories` (RustSec) | Pass, no advisories. |
| Bans and sources | `cargo deny check bans sources` | 22 duplicate-version warnings (build size only). |
| Attribution | `cargo about generate` | `THIRD-PARTY-NOTICES.html`. Ship it with the root `LICENSE-MIT` / `LICENSE-APACHE`. |
| Code review (first pass) | local model over auth, credentials, TLS, auth guard, helpers, smart-card bridge, stats | 23 items flagged, 0 confirmed after manual check (`review-*.md`). A small model's pass is a weak signal; a deeper review is still worth doing. |
| DAST | `dast.py`, `tls_probe.py`, sdl-freerdp, lsof (2026-10-02) | Pass, see below. |
| Trademark ("Portico") | not yet run | Check USPTO / CIPO. |

Rejected findings, with the reason:
- TLS downgrade: rustls supports only TLS 1.2 and 1.3.
- `private_dir` symlink race: `verify_ours` uses `symlink_metadata` and checks owner and mode.
- Shield helper allocation: the exclude count is a u16 of 4-byte reads.
- Stats endpoint stall: writes run under a 2 s timeout.
- PAM `calloc` overflow: count is checked positive and comes from libpam; `calloc` checks the multiplication.
- `macrdpdisplay` `fgets`/`sscanf`: bounded buffer, input only from Portico.
- Log "leaks" of timing or display configuration: not sensitive.

## DAST results (2026-10-02)

- **Security negotiation:** only NLA (CredSSP) is accepted; legacy RDP security and TLS without NLA are refused (`tls-results.txt`).
- **TLS:** 1.2 and 1.3 only, AEAD ciphers (ECDHE-ECDSA with AES-GCM or ChaCha20-Poly1305, TLS_AES_256_GCM_SHA384). TLS 1.0 and 1.1 are refused by the server.
- **Robustness:** 300 random-garbage connections, 100 truncated handshakes, 200 idle connections held 20 s (all closed by the server's handshake deadline) and 20 slow-drip connections: the server stayed up and kept answering new connections (`dast-results.jsonl`).
- **Brute force:** from a LAN address, 4 wrong passwords lock that address out; later connections are rejected before the password exchange with an escalating cooldown (`bruteforce.txt`). Loopback is exempt by design (`auth_guard.rs`).
- **Unauthenticated mode:** `--skip-auth` is refused on any non-loopback bind.
- **Listening sockets:** the RDP port plus the shield helper's control port, which is bound to 127.0.0.1 only (`listeners.txt`); UDP is off by default.

Note: `dast.py` was written by a local model; its `tls_versions` and negotiation parsing were wrong and are superseded by `tls_probe.py`. Its fuzz and flood tests only check that the server stays alive, which they measure correctly.
