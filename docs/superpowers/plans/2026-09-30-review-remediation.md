# Review remediation: security, quality and architecture

**Source:** `docs/reviews/codebase-review.md` (findings S1 to S11, A1 to A7, Q1 to Q7).

**Decisions from the owner (2026-09-30)**

- This fork is the product and will not be merged back into clintcan/macrdp or IronRDP. The vendored crates are owned code. Changing them is fine, and there is no need to keep them close to upstream.
- No CI in the loop. Changes are verified on Linux here, then reviewed and run on a Mac by the owner.

## Verification strategy

| Check | How | Covers |
|---|---|---|
| Linux tests | `cargo test --locked`; `cargo test` in `vendor/ironrdp-rdpeudp` | Cross-platform logic, the real accept loop over loopback TCP, the UDP listener |
| Linux lint | `cargo clippy --all-targets -- -D warnings`; `cargo fmt --check` | Cross-platform code |
| macOS type check on Linux | `tools/macos-typecheck/check.sh` (new). It runs `cargo check` and `cargo clippy` for `aarch64-apple-darwin` with a fake C toolchain and a stub SDK. Nothing is linked. It reproduces the macOS CI dead-code errors exactly. | Every `cfg(target_os = "macos")` path compiles and passes lint |
| On the Mac (owner) | `cargo test --locked`, then a live session with mstsc or Windows App | Runtime behaviour of macOS-only code |

Every commit must pass the first three checks.

## Architecture changes

### 1. One authenticated entry path for every connection (S1, S2, S3, S7, A5)

**Today:** the first connection is served straight away, and its TLS and CredSSP handshake runs inside `run_connection` with no deadline. Only connections that arrive while a session is live go through `negotiate_candidate`, and only one at a time; accepts pause while it runs. The two code paths duplicate the handshake "by hand". The multitransport (UDP) offer is only made on the first path, and the UDP listener admits anyone.

**New design:** a small `HandshakePool` in `vendor/ironrdp-server/src/handshake.rs`.

- **Admission.** Every accepted TCP connection passes `on_accept` (rate limit and lockout), then the bounce-back check, then capacity limits:
  - at most 32 handshakes in flight,
  - at most 4 per source (IPv4 address, or IPv6 /64). Four rather than two because mstsc opens a second, abandoned connection on each attempt, and several clients can share one NAT address.

  Over capacity, the socket is closed at once and reported as `HandshakeFailure::Capacity`.
- **Deadlines.** Each handshake runs under two deadlines:
  - 5 s to receive and answer the X.224 connection request,
  - 30 s total to finish TLS and CredSSP. The 30 s leaves room for the mstsc certificate prompt.
- **Concurrency.** Handshakes run concurrently (`FuturesUnordered`). Accepting never pauses.
- **Result.** A handshake yields either `Authenticated(NegotiatedConnection)` or `Failed(peer, reason)`.
- **Serving.** The accept loop keeps no special case for "first connection":
  - with no live session, the first authenticated connection is served;
  - with a live session, it preempts (existing eviction logic, unchanged);
  - handshakes still in flight when a session ends stay in the pool and are served next. This replaces `CANDIDATE_HANDOFF_GRACE`.
- **One generic sequence.** `negotiate<S: AsyncRead + AsyncWrite>` is the only X.224, TLS and CredSSP sequence. `run_connection` (used by the duplex tests) calls the same function, so the "keep the two in sync by hand" duplication goes away.
- **Post-auth setup.** Channel attachment, the multitransport offer and cookie registration all move after authentication, into `serve_negotiated`, where the authenticated peer IP is known. The acceptor only reads the channel list at the MCS exchange, which comes after CredSSP, so this is in time. As a result the channel factories never run for a peer that has not authenticated. (The first draft kept channel attachment before `accept_begin`; checking the acceptor showed that was not needed.)
- **Link RTT.** It is sampled per accepted socket and carried in the negotiated connection, not written to shared state at accept.

**Handler contract** (`ConnectionHandler`, breaking change, owned code):

- `on_authenticated(peer, success, reason)`
- `on_client_fingerprint(peer, ...)`
- `on_client_display(peer, ...)`
- new `on_handshake_failed(peer, HandshakeFailure)` with the variants Timeout, Tls, Protocol and Capacity.

These carry the peer explicitly. The audit log no longer needs the `last_peer` guess.

**Credentials** become a shared cell, `CredentialsHandle = Arc<RwLock<Option<Credentials>>>`, read at the start of each handshake. The application can rotate or revoke credentials while a session is live (needed by S9).

### 2. UDP admission tied to authenticated sessions (S2)

- `CookieRegistry::register(cookie, inbound, peer_ip)` records the authenticated TCP peer's IP.
- A SYN from a source with no peer state is admitted only if:
  1. the datagram is at least 1132 bytes, the minimum MTU in MS-RDPEUDP. Real clients pad the SYN to their MTU, so this rule removes the amplification.
  2. the IP belongs to an authenticated TCP session that was offered multitransport (IPv4-mapped addresses normalised). The binding lasts until that session ends, not until the cookie is consumed, because one session opens up to two UDP flows (reliable and lossy).
  3. the peer table has room: at most 64 peers, and at most 4 per IP.
- **The listener stays usable without a registry.** When constructed with no registry (the handshake-only test path), rule 2 is skipped.

**Documentation.** `auth_guard.rs` and `docs/security.md` are corrected to describe this.

### 3. Auth guard accounting from explicit signals (S3, S7, S8, Q4)

- **Failure** is recorded by `on_authenticated(peer, false)` and by `on_handshake_failed` for Timeout, Tls and Protocol. Capacity is audited but not counted.
- **Success** is recorded by `on_authenticated(peer, true)`.
- The fail-fast duration heuristic is removed, and `on_disconnected` is audit only.
- Keys become the IPv4 address, or the IPv6 /64. Loopback stays exempt.
- The stale sweep runs at most once per second, and the cap is enforced by evicting the oldest entries.
- Threshold values from the environment are parsed with `try_from`; out-of-range values are rejected at startup.

### 4. Credential monitor (S9)

New module `src/credential_monitor.rs`:

- **Pure state machine.** It is unit-tested on Linux and decides when to re-check, and what to do on a result.
- **macOS driver.**
  - Every 5 minutes it re-runs PAM with the current password.
  - On a failure it immediately revokes RDP credentials (sets an unguessable random password in the `CredentialsHandle`), logs an error, and never retries that password. Retrying could trip the OS account lockout.
  - With `--keychain`, it polls the Keychain entry every 60 s. When the value changes, it validates the new value once with PAM and installs it on success.

  This means a password change propagates without a restart. A disabled account stops RDP within 5 minutes.
- The auto-unlock path reads the same shared secret.

### 5. Smaller fixes

| ID | Change |
|---|---|
| S4 | `sanitize_label` maps empty, `.`, `..` and a leading `.` to safe names. Mountpoints are created with `create_dir` inside a 0700 per-process directory, and the result must be a direct child of that directory. |
| S5 | The IFD handler caps every read length before allocating (ATR 33, response `min(caller capacity, 65538)`). Every exported `IFDH*` function runs inside `catch_unwind` and uses poison-tolerant locking. |
| S6 | Absolute paths for all external commands. A unit test scans `src/` for `Command::new("` with a bare name. |
| S10 | The PAM conversation frees and zeroes earlier responses on the error path. |
| S11 | New `src/private_dir.rs` creates a fresh 0700 directory with `create_dir` (never `create_dir_all` on the final component) and verifies ownership. It is used by paste, lazy paste and the RDPDR fallback. |

### 6. Code quality

| ID | Change |
|---|---|
| Q1 | New `src/sync_ext.rs` with `LockExt::lock_or_recover()` (poison-tolerant). All `lock().unwrap()` and `lock().expect(..)` in `src/` switch to it. |
| Q2 | `#![warn(clippy::undocumented_unsafe_blocks)]` in `src/main.rs` and `ifd-handler`, and a `SAFETY:` comment on every unsafe block. |
| Q3 (dead code) | The 7 dead-code items get `#[allow(dead_code, reason = ...)]`. |
| Q3 (test abort) | A module-level lock in `keyboard_layout.rs` serialises every TIS and UCKeyTranslate call. This fixes the intermittent SIGABRT in tests and the same race in production (input thread against auto-unlock). |
| Q4 | Truncating casts on external input replaced with `try_from`. |
| Q5 | The misleading `drop` calls are removed. |
| Q6 | A `fuzz/` crate (cargo-fuzz, not a workspace member) with targets for the RDPEUDP datagram, DVC PDUs, RDPDR PDUs and the acceptor. Plus stable, deterministic garbage-input tests for the same decoders, so `cargo test` exercises them today. |
| Q7 | Guideline for new code: the why lives in code and the history lives in `docs/`. Code that is moved keeps its comments verbatim. There is no mass rewrite. |

### 7. Architecture refactors

- **A6.** A debug-build lock-order check in `h264`: a thread-local flag set while `ctx` is held, which the `server_handle` locking helper asserts is clear.
- **A3.** `ResyncSignal` handle (two `Arc<AtomicBool>`) created in the app and injected into input, capture and audio. It replaces the `RESYNC_*` statics. The remaining statics are module-private caches of OS-global state, such as focus, and are documented as such.
- **A2.** `src/tunables.rs`: one typed, documented registry of every `MACRDP_*` variable.
  - It is loaded once at startup, and every non-default value is logged.
  - Modules read `tunables()`, never `std::env::var`.
  - Vendored crates receive their values through config structs and setters filled from the registry.
- **A4.** Split the large files, keeping the code the same (mostly moves):
  - `src/h264.rs` into `src/h264/{mod.rs, congestion.rs, blank.rs, regions.rs, annexb.rs}`,
  - `src/input.rs` into `src/input/{mod.rs, switcher.rs, focus.rs, hotkeys.rs}` (`src/input/scancodes.rs` already exists).
- **A1.** `src/app/` holds the composition root:
  - `args.rs` (CLI, config file, negotiated defaults),
  - `credentials.rs`,
  - `tls.rs`,
  - `helpers.rs` (helper processes and sleep prevention),
  - `session_lock.rs` (lock on disconnect, auto-unlock),
  - `overlay.rs` (primary display watcher),
  - `server.rs` (builds the `RdpServer` and its factories),
  - `mod.rs` (`run`, a short sequence of those steps).

  `main.rs` keeps module declarations and `main`.
- **A5.** Resolved by the owner's decision: the vendored crates are first-party. `FORK.md` records the new divergences and states that upstream sync is no longer a goal.

## Order of work

Commit after each item. Each commit passes the three checks above.

1. Tooling: `tools/macos-typecheck/`.
2. Vendored server: `HandshakePool`, unified `negotiate`, handler contract, credentials cell, UDP admission. Update `conn_test.rs` and add regression tests:
   - silent peers do not delay a real user,
   - a stalled handshake is closed after its deadline,
   - per-source capacity,
   - a short SYN gets no reply,
   - a SYN from an IP with no offer gets no reply.
3. Auth guard accounting and keying.
4. Credential monitor.
5. S4, S5, S6, S10, S11.
6. Q1 to Q6.
7. A6, A3, A2, A4, A1.
8. Documentation: `docs/security.md`, `docs/architecture.md`, `FORK.md`, and a resolution section in `docs/reviews/codebase-review.md`.

## Behaviour changes the owner should check on the Mac

- **mstsc certificate prompt.** A first connection whose certificate prompt stays open more than 30 s is dropped. mstsc already reconnects after the user accepts, so this should only add one reconnect.
- **UDP multitransport from a different IP.** If the client reaches UDP from a different public IP than TCP (unusual carrier NAT), the tunnel is refused and the session stays on TCP.
- **Credential monitor.** Every 5 minutes it performs one PAM check of the stored password. After a password change, RDP logins stop working until the Keychain entry is updated (with `--keychain`) or macrdp is restarted with the new password.
