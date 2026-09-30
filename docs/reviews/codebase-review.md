# Codebase review: architecture, code quality, SAST and DAST

| | |
|---|---|
| Date | 2026-09-30 |
| Commit | `2584904` (`main`, after merging #4) |
| Scope | `src/` (Rust server, about 38k lines), `vendor/ironrdp-*` (five forked crates, about 24k lines), `ifd-handler/`, `src/usb_redirect/usb_spike.m`, `gui/Sources/*` (Swift helpers, read for their IPC contracts only) |
| Reviewer environment | Linux container. macOS-only code compiles to stubs here and cannot run, so it was reviewed by reading. Everything cross-platform was built, linted and exercised. |

## Contents

1. [How the review was done](#1-how-the-review-was-done)
2. [Summary of findings](#2-summary-of-findings)
3. [Architecture and design practices](#3-architecture-and-design-practices)
4. [Code quality](#4-code-quality)
5. [SAST (static security analysis)](#5-sast-static-security-analysis)
6. [DAST (dynamic testing and estimates)](#6-dast-dynamic-testing-and-estimates)
7. [Dependencies and supply chain](#7-dependencies-and-supply-chain)
8. [Remediation plan](#8-remediation-plan)
9. [Appendix: commands and raw numbers](#9-appendix-commands-and-raw-numbers)
10. [Resolution](#10-resolution)

## 1. How the review was done

**Tooling run on the tree**

- `cargo clippy --all-targets` (default lints), plus a second pass with `clippy::pedantic`, `too_many_lines` and `cognitive_complexity`.
- `cargo test --locked` (main crate) and `cargo test` in `vendor/ironrdp-rdpeudp` (a standalone crate).
- `cargo audit` and `cargo deny check advisories bans sources licenses`.
- An inventory of `unsafe`, `unwrap`, `panic!`, external process launches, network listeners, global mutable state and environment-variable reads, done with `grep` and a small script.

**Manual reading** covered every network-facing path (TCP accept loop, TLS and CredSSP, auth guard, UDP multitransport listener), every redirection channel (drives over NFS, clipboard files and images, smart card, USB, camera), the FFI boundaries (PAM, IFD handler, ObjC UserHCI) and the startup composition in `main.rs`.

**Dynamic testing** used the repository's own loopback harness (`src/conn_test.rs`), which drives the real `RdpServer::run` accept loop over real TCP with a real IronRDP client, and the real `UdpMultitransportListener`. The probes were temporary test functions. They were run and then removed, and are not part of this change. Section 6 describes each probe and its measured result.

**What could not be run:** anything that needs macOS frameworks (ScreenCaptureKit, VideoToolbox, CGEvent input, Keychain, PAM, NFS mounting, UserHCI, CoreMediaIO). Findings in those areas come from reading the code and are marked as such.

**Severity scale**

| Rating | Meaning |
|---|---|
| High | Exploitable by a remote party in a supported deployment, or loses integrity of a security control. |
| Medium | Needs an authenticated client, a local process, or a non-default setting. The impact is real, but the path is narrower. |
| Low | Hardening, defence in depth, or an unlikely precondition. |
| Info | Observation with no direct exploit path. |

The project's documented posture is VPN or trusted LAN, bound to `127.0.0.1` by default. Several network findings only apply once an operator binds to a routable address (`BIND=0.0.0.0:3390`), which the README describes as the normal way to serve remote clients. The ratings assume that setting.

## 2. Summary of findings

| ID | Area | Rating | Finding |
|---|---|---|---|
| S1 | Network, pre-auth | High | Silent pre-auth connections can keep a real user out of the single session: each one holds the only preemption slot for 10 s while accepts are paused. Measured: 3 silent connections delayed a real login by 30 s. |
| S2 | Network, UDP | High (when UDP is on) | The UDP multitransport listener answers any source before any cookie check. Measured 77x reflection amplification; per-peer state is unbounded (2000 fake peers accepted in 184 ms). The docs state it is unreachable before auth, which is not true. |
| S3 | Auth guard | Medium | Password failures made through the preemption path never count toward lockout. Only the 10-per-minute rate limit applies. A comment in the vendored server claims the opposite. |
| S4 | Drive redirection | Medium | A client drive named `..` is NFS-mounted over the user's `$TMPDIR` for the life of the session. |
| S5 | Smart card | Medium | The IFD handler, which runs inside the system `slotd` daemon, allocates a length read from an unauthenticated loopback socket (up to 4 GiB) before checking it. |
| S6 | Secrets | Medium | The account password is read by running `security` through `PATH`, not `/usr/bin/security`. |
| S7 | Audit log | Low | Auth results can be logged against the wrong IP when a second connection arrives during the first one's NLA. |
| S8 | Auth guard | Low | IPv6 peers are keyed by full address, so a /64 holder gets an unlimited number of fresh rate-limit budgets. `evict_stale` walks the whole table (up to 50,000 entries) on every accept. |
| S9 | Auth | Low | The password is checked by PAM once at startup and cached. A changed password or disabled account keeps working over RDP until restart. |
| S10 | PAM FFI | Low | The conversation callback leaks earlier password copies on a mid-loop allocation failure, and never zeroes them. |
| S11 | Temp files | Low | Predictable temp directory names created with `create_dir_all`, which accepts a directory that already exists. |
| A1 | Architecture | High (maintainability) | `async_main` is 791 lines with cognitive complexity 249. It is the whole composition root and much of the policy. |
| A2 | Architecture | Medium | Configuration sprawl: 62 CLI flags and 82 `MACRDP_*` environment variables, many read deep inside modules. |
| A3 | Architecture | Medium | 43 process-wide mutable statics, including cross-module signalling through `crate::RESYNC_*` atomics. |
| A4 | Architecture | Medium | God modules: `h264.rs` 5,263 lines, `main.rs` 4,200, `input.rs` 3,437. |
| A5 | Architecture | Medium | The `ironrdp-server` fork carries 22 numbered divergences and a 1,934-line divergence log. It now holds product policy such as preemption and eviction. |
| A6 | Architecture | Medium | Lock ordering is enforced by comments only. A real deadlock of this kind was found in the Phase 2a review. |
| Q1 | Quality | Medium | 317 non-test `unwrap()` calls, 79 of them on `Mutex::lock()`. One poisoned lock turns into a cascade of panics. |
| Q2 | Quality | Medium | 232 `unsafe` sites with 24 `SAFETY` comments. |
| Q3 | Quality | Medium | CI on `main` is red for the macOS jobs: 7 dead-code errors in the release build, and an intermittent SIGABRT in the macOS test binary. |
| Q4 | Quality | Low | 41 truncating or sign-losing casts outside tests, including `u64 as u32` on security thresholds read from the environment. |
| Q5 | Quality | Low | Four `std::mem::drop` calls on values that do not implement `Drop` (`main.rs:2820-2837`). |
| Q6 | Quality | Low | No fuzzing of the wire parsers, and no coverage measurement. |

Section 5.4 lists the things checked and found sound, so they do not need re-checking.

## 3. Architecture and design practices

### 3.1 What is done well

- **Dependency inversion at the protocol seams.** The vendored server exposes optional factories (`SoundServerFactory`, `CliprdrServerFactory`, `RdpdrServerFactory`, `GfxServerFactory`, `UrbdrcServerFactory`, `RdCameraServerFactory`) and handler traits (`RdpServerInputHandler`, `RdpServerDisplay`, `ConnectionHandler`). Each feature plugs in through a trait object built in `main.rs`, and the default is `None`. This is the open/closed and dependency-inversion principles applied properly: new channels add implementations and leave the core loop alone.
- **Pure decision cores.** Policy with real branching is kept free of I/O and platform types, and is unit-tested on Linux:
  - `AuthGuardCore` (rate limit and lockout),
  - `negotiator::{session, display, video}`,
  - `refine::Tracker`,
  - the blank-recovery predicates in `h264.rs` (`should_blank_recover`, `blank_rtt_gate` and others).

  This is the most valuable design habit in the codebase. Section 8 recommends extending it.
- **Quarantine of risky code.** Private Apple APIs sit behind `virtual_display/private_api.rs` and `cursor/private_api.rs`. The IOUSBHost SPI is in one ObjC file. Smart-card code runs in a separate cdylib. That keeps review and upgrade effort focused.
- **Cross-platform stubs.** Every macOS module has a non-macOS stub, so protocol logic compiles and tests on Linux.
- **Documentation.** Design decisions, failed experiments and live verification results are written down: `docs/known-quirks.md`, `docs/macos-gotchas.md`, the vendor divergence logs, and `FORK.md`.

### 3.2 Findings

#### A1 (High, maintainability): `async_main` is the whole program

- **Where:** `src/main.rs:2593` (`async_main`, 791 lines, cognitive complexity 249 against a threshold of 25). Its neighbours are also large:
  - `args_from_config` at `:2228` (269 lines),
  - `spawn_primary_overlay_watcher` at `:1749` (231 lines),
  - `attempt_auto_unlock` at `:1453`.
- **Why it matters:** this function parses configuration, applies negotiated defaults, authenticates, creates the virtual display, spawns helper processes, builds every factory, wires the auth guard, binds the UDP listener, installs signal handlers and runs the server. Any change touches it, it cannot be unit-tested, and its local variables act as global state for everything built inside it. The single responsibility principle is broken at the top of the program.
- **Fix:** split it into a small set of assembly steps, each returning a value the next step takes as input. For example:
  - `Config::resolve()`,
  - `Credentials::obtain(&Config)`,
  - `Displays::create(&Config)`,
  - `Channels::build(&Config, &Displays)`,
  - `Server::build(...)`.

  Keep `async_main` as a sequence of about ten lines. The pieces then become testable with fakes, which the factory traits already allow.

#### A2 (Medium): configuration is spread across 144 knobs, many invisible at startup

- **Where:** 62 `#[arg]` flags in `main.rs`, and 82 distinct `MACRDP_*` environment variables across `src/` and `vendor/`. Environment reads happen inside feature modules: 14 in `h264.rs`, 15 in `main.rs`, and others in `auth_guard.rs`, `logging.rs`, `health.rs` and the vendored listener.
- **Why it matters:**
  - It works against the fork's own guiding rule in `CLAUDE.md`: "sessions are configured by negotiation with the client, not flags".
  - Values read with `std::env::var` inside a module are hidden inputs. They are not in `--help`, not in `config.env` validation, and not visible to tests without mutating process state.
  - Some are security controls (`MACRDP_CONN_GUARD`, `MACRDP_GUARD_*`), yet they can be switched off by an environment variable that no configuration file shows.
- **Fix:** resolve all tunables once, at startup, into one typed `Config`. Log the effective non-default values at `info`, and pass the relevant sub-structs into each factory, which is dependency injection. Retire tunables that were only for experiments (for example the blank-recovery constants now that they are settled).

#### A3 (Medium): process-wide mutable state

- **Where:** 43 `static` items with interior mutability in `src/`, 16 of them in `input.rs`. Examples:
  - `RESYNC_VIDEO` and `RESYNC_AUDIO` (`main.rs:63-65`), set by `input.rs:1112` and consumed by `capture.rs:1386` and `audio.rs:531`;
  - `LIVE_MOUNTS` (`rdpdr/surface.rs`);
  - `LAST_FOCUS_BUNDLE` and the focus cursors in `input.rs`;
  - the auto-unlock budget counters in `main.rs`.
- **Why it matters:** modules talk to each other through hidden global channels, so no one can see the coupling from function signatures. Tests cannot run two instances side by side. Reasoning about a reconnect has to include state that outlives the connection.
- **Fix:** replace signal atomics with explicit handles passed at construction (a small `ResyncHandle` holding two `Arc<AtomicBool>`), or an event channel. Keep `static` only for true process singletons such as logging.

#### A4 (Medium): god modules

| File | Lines | Mixed responsibilities |
|---|---|---|
| `src/h264.rs` | 5,263 | VideoToolbox driving, EGFX surface lifecycle, ship thread, congestion control (AIMD, IDR backoff), UDP migration watchdog, blank-presentation detection and recovery, lossless refinement, QoE handling, 69 tests |
| `src/main.rs` | 4,200 | CLI, config file parser, Keychain, PAM wiring, sleep prevention, lock and auto-unlock, overlay watcher, helper process spawning, server assembly |
| `src/input.rs` | 3,437 | scancode mapping, CGEvent posting, app switcher, focus tracking, Spotlight and screenshot shortcuts, hotkeys, gather-windows |
| `src/clipboard.rs` | 2,295 | CLIPRDR state machine, pasteboard FFI, image conversion, file lists |
| `src/capture.rs` | 2,119 | ScreenCaptureKit stream, legacy bitmap path, EGFX path, flush burst, resize, refinement ticking |

- **Why it matters:** these files change for many unrelated reasons, which makes conflicts and regressions likely. The Phase 2a review found three separate defects in code paths that interleave in `h264.rs`.
- **Fix:** move the already-pure pieces into submodules, one file per concern: `h264/{congestion.rs, blank_recovery.rs, refine_ship.rs, surface.rs}`, `input/{switcher.rs, focus.rs, hotkeys.rs}` and `main/{config.rs, unlock.rs, helpers.rs}`. This is mostly moving code, not rewriting it.

#### A5 (Medium): the vendored server fork holds product policy

- **Where:** `vendor/ironrdp-server`. Its `CLAUDE.md` divergence log is 1,934 lines and lists 22 numbered divergences. The other forks are smaller: acceptor 201 lines, rdpdr 133, rdpeudp 260, dvc 139.
- **Why it matters:** preemption, eviction, audio lag control, multitransport routing and the TCP RTT probe live in the vendored crate. Every IronRDP pin bump becomes a large manual merge. The security-relevant accept loop (S1, S3, S7) is in code that upstream does not review.
- **Fix:**
  - Keep pushing seams upstream, as was done for ARC in #1405.
  - Move policy out of the fork behind trait hooks (for example a `SessionPolicy` trait with `on_candidate`, `should_preempt` and `on_auth_result`), so the fork only carries mechanism.
  - Track the divergence count as a number that should go down over time.

#### A6 (Medium): lock ordering exists only in comments

- **Where:** `h264.rs` documents "`server_handle` is never nested under `ctx`" in several comments. The Phase 2a review found `refine_tick` violating it (C1, since fixed).
- **Why it matters:** the rule protects against a deadlock that freezes the whole session. A comment cannot stop the next contributor from breaking it.
- **Fix:** make the order structural. Either keep `ConnectionContext` and the `GraphicsPipelineServer` behind one owner task driven by messages, or wrap them in a helper that only hands out `server_handle` after `ctx` has been dropped (a token type that `ctx`'s guard cannot coexist with). A lighter step is a debug-build lock-order checker.

#### A7 (Info): object orientation and SOLID in Rust terms

The request mentioned OOP and SOLID. In a Rust codebase these translate as follows.

| Principle | Assessment |
|---|---|
| Encapsulation | Good in the pure cores and the private-API boundaries. Weak where state is global (A3) or where large structs expose public fields, for example `ConnectionContext` in `h264.rs` with 49 fields. |
| Single responsibility | Weak at module level (A1, A4). |
| Open/closed | Good at the channel seams. |
| Liskov substitution | Not a concern with trait objects here. The stubs honour the same contracts. |
| Interface segregation | Mostly good. The factory traits are small. `GraphicsPipelineHandler` implementations carry many no-op methods. |
| Dependency inversion and injection | Good for channels (constructor injection through the builder). Weak for configuration (A2) and cross-module signals (A3). |

## 4. Code quality

### 4.1 Measurements

| Metric | Value |
|---|---|
| `cargo clippy --all-targets` (default) | 6 warnings (`clipboard.rs:1204`, `main.rs:2820-2837` four times, `input.rs:190`) |
| `clippy::pedantic` | 350 warnings: 186 doc backticks; 41 truncation, sign or wrap casts; 16 similar names; 10 unused arguments; other style |
| Functions over 100 lines | 65 (13 flagged by clippy on the Linux build, which excludes macOS-only code) |
| Functions over 200 lines | 23 |
| `unsafe` sites | 232 (input.rs 44, videotoolbox.rs 23, file_promise_lazy.rs 21, auth.rs 16, clipboard.rs 16, private_api.rs 16) |
| `SAFETY` comments | 24 |
| Non-test `unwrap()` | about 317, of which 79 are `lock().unwrap()` |
| Non-test `panic!` | 15 |
| External process launches | about 20 |
| Tests | 199 in the main crate and 53 in `ironrdp-rdpeudp`, all passing on Linux |
| Formatting | `cargo fmt --check` clean |

### 4.2 Findings

#### Q1 (Medium): panics used as error handling on shared state

- **Where:** 79 non-test `lock().unwrap()` calls, for example `h264.rs`, `rdpdr/surface.rs:227` and `:241`, and `ifd-handler/src/lib.rs:266` and `:340`.
- **Why it matters:**
  - A panic on any thread holding one of these mutexes poisons it, and every later `unwrap()` panics too.
  - In the tokio worker threads that ends the connection.
  - Inside the IFD handler (an `extern "C"` function running in `slotd`) a panic aborts the system smart-card daemon.
- **Fix:** use `lock().unwrap_or_else(PoisonError::into_inner)` where the protected data stays valid after a panic, which is true of most of these caches and counters. In `extern "C"` entry points, wrap the body in `std::panic::catch_unwind` and return an error code instead.

#### Q2 (Medium): `unsafe` without written invariants

- **Where:** 232 `unsafe` sites and 24 `SAFETY` comments. The largest concentrations are:
  - CGEvent and AX FFI in `input.rs`,
  - CoreVideo and VideoToolbox in `videotoolbox.rs`,
  - Foundation in `file_promise_lazy.rs`,
  - PAM in `auth.rs`.
- **Why it matters:** the safety argument for each block (pointer validity, lifetime, thread) is what a reviewer or a future macOS API change needs, and it is mostly not written down.
- **Fix:**
  - Enable `#![warn(clippy::undocumented_unsafe_blocks)]` crate-wide.
  - Add the comments in the files listed above first.
  - Where possible, replace raw FFI with the maintained `objc2-*` or `core-foundation` wrappers the crate already depends on.

#### Q3 (Medium): CI on `main` is not green

- **Where:**
  - The `audit log (macos integration)` job runs `cargo build` with `-D warnings`. It fails on 7 dead-code errors in phase-1 code: `ClientAdvert::platform`; `ClientCaps`, `Overrides`, `SessionPlan` and `negotiate` in `negotiator/session.rs`; `current_mode` in `virtual_display/private_api.rs:508`; and `VirtualDisplay::new_planned` and `backing_pixels` in `virtual_display/mod.rs`.
  - The `test (macos)` job sometimes aborts with SIGABRT and no panic message, right after `keyboard_layout::macos::tests::spec_resolution_prefers_names_then_klids`. This was seen on PR #1 and PR #2 and passed on PR #3.
- **Why it matters:** a permanently red check trains everyone to ignore CI, and new real failures hide behind it.
- **Fix:**
  - Add `#[allow(dead_code)]` with a reason on those items until they are wired in, or wire them.
  - For the abort, run the `keyboard_layout` tests single-threaded, or on the main thread, on macOS. TIS and UCKeyTranslate are main-thread-affine APIs and a common cause of silent aborts.

#### Q4 (Low): truncating casts on inputs

- **Where:** examples from `clippy::cast_possible_truncation`:
  - `auth_guard.rs:119-120`: `env_u64(...) as usize` and `as u32` for `MACRDP_GUARD_RL_MAX` and `MACRDP_GUARD_FAIL_THRESHOLD`;
  - `capture.rs:692`: `cur_w as u16`;
  - `clipboard.rs:227-232`: image dimensions to `i32`;
  - `avc444.rs:127-129`.
- **Why it matters:** the auth-guard case turns a large configured value into a small or zero one. For example, `MACRDP_GUARD_FAIL_THRESHOLD=4294967296` becomes 0, which silently disables lockout.
- **Fix:** use `u32::try_from(...)` and reject out-of-range configuration at startup.

#### Q5 (Low): `drop` on values that are not `Drop`

- **Where:** `main.rs:2820`, `:2827`, `:2830` and `:2837` (clippy `drop_non_drop`).
- **Why it matters:** the calls read as if they release a resource at that point, but they do nothing. If the intent was to end a borrow or release a lock, the code does not do what it says.
- **Fix:** remove them, or drop the guard that actually holds the resource.

#### Q6 (Low): testing gaps

- No fuzzing of the wire parsers that take untrusted input:
  - the vendored acceptor's X.224, MCS and GCC handling,
  - the RDPEUDP datagram decoder,
  - the RDPDR and ESC PDUs (4,114 and 2,959 lines),
  - the MS-RDPEUSB and camera parsers.

  `cargo fuzz` targets for `Datagram::decode`, the acceptor state machine and the RDPDR decoders are cheap to add, and they would have found malformed-input issues before users did.
- No coverage measurement. The macOS-only modules (input, capture, clipboard FFI, virtual display) have no automated tests in Linux CI, and only a few run in the macOS job.
- The pure cores are well tested. Keep extracting logic into them.

#### Q7 (Info): comment volume

Many functions carry long historical narratives inline (for example the blank-recovery and flush-burst history in `h264.rs` and `capture.rs`, which repeats `docs/known-quirks.md`). They are accurate and valuable, but they bury the current contract. Keep the why-now explanation in code and move the history to `docs/` with a link.

## 5. SAST (static security analysis)

### 5.1 Attack surface map

| Surface | Reachable by | Before auth? | Code |
|---|---|---|---|
| RDP TCP listener (`--bind`, default `127.0.0.1:3390`) | Network | Yes: X.224, TLS, CredSSP | `vendor/ironrdp-server/src/server.rs:1779` (`run`), `vendor/ironrdp-acceptor` |
| UDP multitransport listener (same address, opt-in) | Network | Yes: RDPEUDP SYN, then TLS or DTLS, then the cookie check | `vendor/ironrdp-server/src/multitransport/listener.rs` |
| Virtual channels (clipboard, RDPDR, smart card, USB, camera, audio, EGFX, display control, input) | Authenticated RDP client | No | `src/*`, vendored server |
| Stats `:40245`, smart-card bridge `:40242`, HUD `:40243`, shield `:40244`, NFS for each drive | Any local process | Yes (no authentication, by design) | `stats.rs`, `rdpdr/smartcard.rs`, `switcher_hud.rs`, `shield.rs`, `rdpdr/surface.rs` |
| IFD handler inside `slotd` | Anything that answers on `:40242` | Yes | `ifd-handler/src/lib.rs` |
| External processes | Anything that controls `PATH` or the binaries | Not applicable | 20 `Command::new` sites |

### 5.2 Findings

#### S1 (High): pre-auth connections can lock a real user out of the only session

- **Where:**
  - `vendor/ironrdp-server/src/server.rs:1984`: `accepted = listener.accept(), if !probing`. While a candidate is negotiating, no other connection is accepted.
  - `:83`: `CANDIDATE_NEGOTIATION_TIMEOUT` is 10 s.
  - `:1574`: `run_connection` has no handshake timeout at all for the first connection.
  - `src/auth_guard.rs:118-119`: the default rate limit is 10 attempts per 60 s per IP.
- **How it plays out:**
  1. The server serves one session.
  2. An incoming connection while a session is live becomes a preemption candidate, and it gets up to 10 s to finish TLS and CredSSP.
  3. During those 10 s, accepts are paused, so every other connection, including the real user, waits in the backlog.
  4. A peer that opens TCP and sends nothing therefore costs the real user 10 s. It needs no password.
- **Measured (section 6, probe P2):** three silent connections queued ahead of a real client delayed its login by 30.08 s.
- **Impact at default limits:** 10 attempts per minute per IP buy 100 s of blocking per 60 s window, so a single source address can keep a legitimate user out indefinitely. The stalls also end after the 3 s fail-fast window and never reach `on_disconnected`, so they never count as lockout failures. With several addresses, or with IPv6 (S8), the rate limit stops mattering.
- **Fix, in order of value:**
  1. Negotiate candidates concurrently, up to a small bound, instead of pausing `accept()`.
  2. Apply a short pre-TLS deadline (a few seconds to receive the X.224 Connection Request and the TLS ClientHello) to both the first connection and candidates.
  3. Count a timed-out or abandoned handshake as a failure for the auth guard.
  4. Give `run_connection` an overall handshake deadline.

#### S2 (High when `--enable-udp-multitransport` or lossy audio is on): unauthenticated UDP reflector and state exhaustion

- **Where:**
  - `vendor/ironrdp-server/src/multitransport/listener.rs:832`: `peers.entry(peer_addr).or_insert_with(...)` creates a full reliability state machine for any source address, with no size cap on `peers`.
  - `:235-243`: `send_datagrams` pads every SYN+ACK to the MTU (1232 bytes).
  - Nothing checks the size of the inbound SYN.
  - The cookie that binds UDP to a TCP session is checked much later, at the MS-RDPEMT tunnel create request, after TLS.
  - Idle peers are only reclaimed after 60 s (`:290`).
- **Measured (section 6, probe P4):**
  - a 16-byte SYN produced one 1232-byte reply, a 77x amplification;
  - 2000 SYNs from 2000 distinct source ports were all answered in 184 ms, each creating per-peer state.
- **Why it matters:**
  - From a spoofed source address, the server becomes a UDP amplifier aimed at a third party.
  - Without spoofing, an attacker can grow the peer table (state machine, receive buffers, and TLS or DTLS objects once the handshake starts) at thousands of entries per second, each kept for 60 s.
- **The documentation is wrong about this.** The `src/auth_guard.rs` module docs (lines 38-41) say the UDP listener is "unreachable without first passing this guarded TCP accept", and `docs/security.md` does not list UDP as a pre-auth surface. Both are incorrect, and the auth guard does not cover UDP at all.
- **Fix:**
  - Drop inbound SYNs shorter than the MTU. MS-RDPEUDP requires the SYN to be padded for path MTU discovery, so real clients already comply, and this alone removes the amplification.
  - Accept a SYN only from an IP that currently holds a TCP session with an outstanding multitransport offer. The cookie registry already knows which offers exist.
  - Cap `peers` and apply a per-IP creation rate.
  - Correct both documents.

#### S3 (Medium): failed passwords through the preemption path never trigger lockout

- **Where:**
  - `vendor/ironrdp-server/src/server.rs:1110-1121`: a candidate's CredSSP result is reported through `on_authenticated(false, ...)`.
  - `src/auth_guard.rs:637-642`: `AuthGuardHandler::on_authenticated` only writes an audit line.
  - Lockout accounting happens only in `on_disconnected` (`auth_guard.rs:664-674`), which is called for served connections (`server.rs:2219`) and never for candidates.
- **Why it matters:** whenever a session is live, which for a remote desktop is most of the time, an attacker can guess passwords through the candidate path and never be locked out. Only the rate limit applies (10 per minute per IP, and unlimited across IPv6 addresses, S8). The comment at `server.rs:1113-1115` claims the result "still shows up in the audit log / AuthGuardHandler lockout accounting". The audit part is true; the lockout part is not.
- **Fix:** pass the peer into `on_authenticated`, and have `AuthGuardHandler` call `record_outcome(Failure)` on a failed authentication. That is a more direct signal than the fail-fast duration heuristic, which can stay as a fallback. Add a unit test that drives a failed candidate through the handler and asserts a cooldown.

#### S4 (Medium, authenticated client): drive label `..` mounts the client drive over `$TMPDIR`

- **Where:**
  - `src/rdpdr/surface.rs:814` (`sanitize_label`) replaces `/`, `\`, `:` and control characters, but keeps `.` and `..`.
  - `:793-811` (`prepare_mountpoint`): when `/Volumes` is not writable, which is the normal case when not running as root, it uses `$TMPDIR/macrdp-rdpdr-<pid>/<label>` and calls `create_dir_all`.
  - The label is the client's own device name (`src/rdpdr/mod.rs:178`, `dev.name`).
- **Why it matters:** with the label `..` the path resolves to `$TMPDIR` itself, and `mount_nfs` mounts the client's share over the user's temporary directory for the whole session. Temporary files written by other apps then go to the remote client's disk, and files the client places there are read by local apps. The client is authenticated, so this is not a remote takeover, but it is a way for a compromised or hostile client machine to read and inject local temporary data. On disconnect, `unmount_at` tries `remove_dir` on `..`, which fails harmlessly.
- **Fix:**
  - Reject labels that are empty, `.`, `..`, or start with `.`.
  - Create the mountpoint with `create_dir` (not `create_dir_all`) and refuse a path that already exists.
  - Check that the canonical mountpoint is a direct child of the per-pid directory before calling `mount_nfs`.

#### S5 (Medium, local): the IFD handler trusts a length from a squattable socket

- **Where:** `ifd-handler/src/lib.rs:151-155` (`read_vec`), called at `:276` with a u8 length (bounded) and at `:353-354` in `IFDHTransmitICC` with a u32 length. The u32 length is allocated before it is compared with the caller's buffer at `:359`. The server side of the same protocol caps its input (`MAX_APDU_LEN`, checked at `src/rdpdr/smartcard.rs:195`); this side does not.
- **Why it matters:** the handler runs inside `slotd`, the macOS smart-card daemon. Any local process that binds `127.0.0.1:40242` before macrdp, or while it is not listening, can answer a transmit with a length of `0xFFFFFFFF`. That makes `slotd` try to allocate 4 GiB and abort, which takes down smart-card service for every reader on the machine. The documented threat model trusts local processes, but crashing a system daemon goes beyond "a local process could misuse the redirected card".
- **Fix:**
  - Reject `len > recv_cap` (or `> 65538`, the maximum extended APDU response) before allocating.
  - Wrap each exported `IFDH*` function in `catch_unwind` so no panic can cross into `slotd` (see Q1).

#### S6 (Medium): the account password is fetched through `PATH`

- **Where:** `src/main.rs:945`, `Command::new("security")`. The same pattern appears for `caffeinate` (`:802`), `afplay` and `osascript` (`src/file_promise.rs:309` and `:329`). Other call sites already use absolute paths (`/usr/bin/osascript`, `/usr/bin/open`, `/sbin/mount_nfs`).
- **Why it matters:**
  - The password is whatever this process prints. A `security` earlier in `PATH` (a writable directory such as `~/bin` or a Homebrew prefix placed first) can return a password of its choosing or record the real one. It would do the recording by calling the real binary itself.
  - `docs/macos-gotchas.md` states that the Keychain entry's trusted application must be `/usr/bin/security` for the headless read to work without a prompt. Resolving through `PATH` also undermines that design.
- **Fix:** use `/usr/bin/security`, `/usr/bin/caffeinate`, `/usr/bin/afplay` and `/usr/bin/osascript` everywhere, and add a test that greps for bare `Command::new("` names.

#### S7 (Low): audit events can be attributed to the wrong IP

- **Where:** `src/auth_guard.rs:600-607` and `:623-642`. `last_peer` is set in `on_accept` and read in `on_authenticated`. The comment says this is reliable "because the single-process accept loop is serial". Since the preemption redesign it is not: a candidate is accepted (`on_accept(B)`) while the first connection may still be inside CredSSP, so the first connection's `on_authenticated` is logged against B's address.
- **Why it matters:** the auth audit log is the main forensic record, and the project ships SIEM forwarding for it (`docs/siem-forwarding.md`).
- **Fix:** pass the peer address through `on_authenticated` and `on_client_fingerprint`, as `on_disconnected` already does. This is the same change as the S3 fix.

#### S8 (Low): auth-guard keying and cost

- **Where:** `src/auth_guard.rs:79-87` (`canonical_ip`) keys IPv6 by the full 128-bit address. `evict_stale` (`:318`) runs `retain` over every tracked IP on every `decide` call, with a cap of 50,000.
- **Why it matters:** a normal IPv6 allocation gives an attacker 2^64 addresses, and each one gets a fresh rate-limit and lockout budget, which undoes S1's and S3's only remaining control. The full scan per accept makes the guard itself O(n) per connection, about 50,000 map entries per accept when the table is full.
- **Fix:**
  - Key IPv6 by its /64 prefix, and consider /56 for aggregate limits.
  - Replace the full scan with a time-ordered queue, or run eviction on a timer.

#### S9 (Low): credentials are validated once and then cached

- **Where:** `src/main.rs:2946` runs PAM at startup. The password is then handed to the server as a static `Credentials` value for NLA comparison.
- **Why it matters:** if the account password changes, or the account is disabled, the old password keeps working over RDP until macrdp restarts. The auto-unlock notes in `docs/known-quirks.md` mention the password-change case, but only as it affects auto-unlock, not as an authentication weakness.
- **Fix:** either re-validate with PAM after each successful NLA, when the client's plaintext is available in CredSSP, or watch for password-change notifications and restart. At minimum, record this in `docs/security.md`.

#### S10 (Low): PAM conversation hygiene

- **Where:** `src/auth.rs:88-128`.
  - If `malloc` fails for message i, the function frees the response array but not the password copies already written for messages before i. Those copies are leaked.
  - All copies handed to libpam are freed by libpam without being zeroed.
- **Fix:** free the earlier responses on the error path, after overwriting them with `explicit_bzero` or a volatile write. The `checkpw` service does not call the conversation at present, so this is low impact.

#### S11 (Low): predictable temporary directories

- **Where:**
  - `src/file_promise.rs:353-362` and `src/file_promise_lazy.rs:585-593`: `macrdp-paste-<pid>-<nanos>`, created with `create_dir_all`;
  - `src/rdpdr/surface.rs:804`;
  - the camera debug dumps.
- **Why it matters:** `create_dir_all` succeeds if the directory already exists, even when someone else created it. On macOS `$TMPDIR` is a per-user 0700 directory, so this is only exploitable if `TMPDIR` points somewhere shared, such as `/tmp` in a custom launch setup.
- **Fix:** use `tempfile::Builder::tempdir_in`, or at least `create_dir` plus a check that the new directory is owned by the current user.

### 5.3 Documented and accepted risks (re-confirmed)

These are already documented. They were re-checked and still match the code.

- **Unauthenticated loopback IPC** (stats, smart-card bridge, HUD, shield helper, NFS). See `docs/macos-gotchas.md`, last bullet. The shield helper is the sharpest case: one byte lowers the privacy shield.
- **RUSTSEC-2023-0071** (Marvin timing attack in `rsa`, reached through picky and sspi). No upstream fix exists; it is accepted in `deny.toml` with a written reason.
- **`--capture-primary` prevents the Mac from locking** while engaged. This is logged at startup; `--shield-primary` is the mitigation.
- **Auto-unlock types the account password** into the lock screen, with a submission budget and a Caps Lock check.
- **`--password` on the command line** is visible in `ps`, and the program warns about it.

### 5.4 Checked and found sound

These were reviewed specifically and need no action.

- **NLA is mandatory.** Only `RdpServerSecurity::Hybrid` is used (`main.rs:3478`); plain TLS logon is not offered.
- **`--skip-auth` is refused on any non-loopback bind** (`main.rs:2955`).
- **The TLS private key** is written with mode 0600 and its permissions are checked on load (`main.rs:995` and `:1036-1048`).
- **No password, key or token appears in any log statement.** All logging macros were searched.
- **Clipboard image decoding** (`clipboard.rs:250-345`) checks header sizes, uses `checked_mul` for buffer sizes, and is capped by `MAX_INCOMING_PAYLOAD` (50 MB).
- **Clipboard file paste** (`file_promise.rs:266-284`, reused by the lazy path) rejects `.`, `..` and embedded `/` in every path component.
- **Client-supplied display size and scale** reach `negotiator::display::plan_display` as `u16` values already clamped to 200 to 8192 by the acceptor, so `w * 100` cannot overflow, and a huge scale takes the `div_ceil(2)` branch.
- **The smart-card bridge (server side)** bounds APDU length before allocating (`smartcard.rs:195`).
- **USB transfer completions** (`usb_spike.m:700-712` and `:841-848`) copy `min(data length, buffer length)`.
- **The RDPEUDP and DVC decoders** check sizes before reading or allocating (`pdu.rs:452`, `dvc/pdu.rs:824-827`).
- **The auth audit log** strips control characters and bounds reason length (`auth_guard.rs:484`).

## 6. DAST (dynamic testing and estimates)

### 6.1 Probes that were run

All probes ran against the real accept loop or listener on loopback, using the project's test harness. Loopback is exempt from the auth guard, so these measure the server mechanism. Section 6.2 extrapolates to production limits.

| Probe | Setup | Result |
|---|---|---|
| P1: silent first connection | No live session; a peer connects and sends nothing; then a real client connects | Real client served after 0.93 s (it preempts the silent one). The silent connection stays open indefinitely, because there is no handshake timeout. |
| P2: silent candidates | A live session; 3 peers connect and send nothing; then a real client connects | Real client served after **30.08 s** (3 x 10 s candidate timeout). Supports S1. |
| P3: malformed pre-auth input | 7 malformed inputs, then a real client: random bytes, TPKT length 0, empty TPKT, TPKT claiming 65,535 bytes then stalling, X.224 with a bad length indicator, an oversized routing cookie without CRLF, fast-path junk | The server never crashed. Four inputs were closed immediately. Random bytes, the truncated TPKT and fast-path junk were held open with no reply for the full 3 s observation window (consistent with P1: no deadline). A real client was served afterwards. |
| P4: UDP SYN reflection | `UdpMultitransportListener` on loopback; minimal 16-byte SYN (FEC header plus SYNDATA) | One reply of **1232 bytes**, 77x amplification. Then 2000 SYNs from 2000 source ports: **all 2000 answered in 184 ms**, each creating peer state. Supports S2. |

### 6.2 Estimates for production and for macOS-only surfaces

| Surface | Estimate from code | Confidence |
|---|---|---|
| RDP TCP, bound off-loopback | With the default guard (10 attempts per minute per IP), one IPv4 address can hold the candidate slot continuously (S1). With IPv6 or several addresses, rate limits do not help (S8). Password guessing during a live session is limited to 10 per minute per IP with no lockout (S3). | High (code path read, mechanism measured) |
| UDP multitransport, bound off-loopback | Reflection at 77x from spoofed sources. Peer table growth of thousands per second, each entry held 60 s. | High (measured) |
| TLS and CredSSP | rustls with aws-lc-rs, default cipher suites, self-signed certificate that users trust on first use. The Marvin advisory applies to the RSA operations in CredSSP; exploiting it needs many timed handshakes, which S1's per-IP limits only partly constrain. | Medium |
| Virtual channels (authenticated) | The main risks are S4 (drive label) and resource use from large but bounded payloads (clipboard cap 50 MB, file ranges 4 MiB, file list 10,000 entries). No unbounded allocations were found in the channel decoders that were read. | Medium (read, not fuzzed) |
| Loopback IPC | Reachable by any local process, as documented. S5 raises the smart-card case from "misuse of the card" to "crash `slotd`". | High |
| macOS input and display paths | Not network-parsers; they consume already-decoded PDUs. The risk is logic errors (focus, lock handling), not memory safety. | Medium |

### 6.3 Recommended DAST to add

1. Keep P2, P3 and P4 as permanent regression tests with assertions ("served within 5 s", "SYN shorter than MTU gets no reply") once S1 and S2 are fixed.
2. Add `cargo fuzz` targets:
   - `ironrdp_rdpeudp::datagram::Datagram::decode`,
   - the acceptor fed with arbitrary bytes through a duplex stream,
   - `ironrdp_rdpdr` PDU decode,
   - `ironrdp_dvc` PDU decode.
3. Run a TLS scanner (`testssl.sh` or `sslyze`) against a real macOS instance to confirm protocol versions and ciphers.
4. Run a real client matrix (mstsc, Windows App, FreeRDP) through a proxy that injects malformed virtual-channel PDUs after login.

## 7. Dependencies and supply chain

| Check | Result |
|---|---|
| `cargo deny check advisories bans sources` | Passes |
| `cargo deny check licenses` | Passes (one unused allowance: `Unicode-DFS-2016`) |
| `cargo audit` | 1 vulnerability (RUSTSEC-2023-0071, `rsa 0.10.0-rc.18`, accepted and documented); 2 unmaintained (`atomic-polyfill 1.0.3`, RUSTSEC-2023-0089; `rustls-pemfile 2.2.0`, RUSTSEC-2025-0134) |
| Duplicate crate versions | 22 warnings (for example three versions of `windows-sys` and of `getrandom`) |
| Pinned sources | IronRDP pinned by git revision `a5d1c682`; five crates vendored |

**Recommendations**

- Replace `rustls-pemfile`. Its functionality now lives in `rustls-pki-types` (`PemObject`).
- Find which crate pulls in `atomic-polyfill`, and upgrade or drop it.
- Keep the daily advisory scan (`.github/workflows/security.yml`), which is a good practice.

## 8. Remediation plan

Ordered by risk reduction per unit of effort.

| Order | Item | Effort | Findings closed |
|---|---|---|---|
| 1 | Drop RDPEUDP SYNs shorter than the MTU; cap `peers`; allow a UDP SYN only from IPs with a live offer; fix `auth_guard.rs` and `docs/security.md` wording | Small | S2 |
| 2 | Pass the peer to `on_authenticated`; count failures there; audit with the correct IP | Small | S3, S7 |
| 3 | Absolute paths for all external commands | Small | S6 |
| 4 | Reject `.`/`..` drive labels; use `create_dir` for mountpoints | Small | S4 |
| 5 | Bound `read_vec` in the IFD handler; `catch_unwind` in exported functions | Small | S5, part of Q1 |
| 6 | Pre-TLS deadline for all connections; concurrent, bounded candidate negotiation; count timeouts as failures | Medium (vendored accept loop) | S1 |
| 7 | IPv6 /64 keying; timer-based eviction | Small | S8 |
| 8 | Fix the 7 dead-code errors and the `keyboard_layout` test threading so macOS CI is green | Small | Q3 |
| 9 | Split `async_main` into assembly steps; introduce a typed `Config` that owns all environment reads | Medium | A1, A2 |
| 10 | Replace `RESYNC_*` and other signal statics with injected handles | Medium | A3 |
| 11 | Split `h264.rs`, `input.rs`, `main.rs` into submodules (mostly moves) | Medium | A4 |
| 12 | `undocumented_unsafe_blocks` lint and SAFETY comments; poison-tolerant locks | Medium, ongoing | Q1, Q2 |
| 13 | Fuzz targets for the four wire decoders | Medium | Q6 |
| 14 | Move preemption and eviction policy out of the vendored server behind a trait | Large | A5 |

## 9. Appendix: commands and raw numbers

```
cargo clippy --locked --all-targets --message-format=short
cargo clippy --locked --all-targets --message-format=short -- -W clippy::pedantic
cargo clippy --locked --message-format=short -- -A clippy::all -W clippy::too_many_lines -W clippy::cognitive_complexity
cargo test --locked                      # test result: ok. 199 passed; 0 failed
(cd vendor/ironrdp-rdpeudp && cargo test)  # test result: ok. 53 passed; 0 failed
cargo fmt --check                        # clean
cargo audit                              # 1 vulnerability (accepted), 2 unmaintained
cargo deny check advisories bans sources # ok
cargo deny check licenses                # ok
```

**Largest functions** (clippy `too_many_lines` on the Linux build; macOS-only code is not included):

| Function | Lines |
|---|---|
| `main.rs:2593` `async_main` | 791 |
| `main.rs:2228` `args_from_config` | 269 |
| `main.rs:1749` `spawn_primary_overlay_watcher` | 231 |
| `capture.rs:661` `sync_virtual_display` | 131 |

**Highest cognitive complexity:**

| Function | Complexity |
|---|---|
| `main.rs:2593` `async_main` | 249 |
| `clipboard.rs:1167` `on_format_data_response` | 74 |
| `main.rs:2228` `args_from_config` | 52 |
| `main.rs:2037` (thread closure in the overlay watcher) | 40 |
| `capture.rs:569` `request_layout` | 38 |

**DAST probe source:** the four probes were appended temporarily to `src/conn_test.rs` as `mod dast_probe` and run with `cargo test --locked dast_probe -- --nocapture --test-threads=1`. They reuse `build_test_server_full`, `connect_with_retry` and `connect_client` from that file, and `UdpMultitransportListener::bind` from the vendored server. They were removed after the run. Section 6.3 recommends making them permanent once the fixes land.

## 10. Resolution

Fixed on branch `fix/review-remediation` (2026-09-30), following
`docs/superpowers/plans/2026-09-30-review-remediation.md`. The owner decided
that the fork is the product and will not be merged upstream, so the vendored
crates were changed freely (this also settles A5).

Verified on Linux: `cargo test --locked` (239 tests), `cargo clippy
--all-targets -- -D warnings`, `cargo fmt --check`, and the macOS type check
and clippy (`tools/macos-typecheck/check.sh`, `-D warnings`). Nothing that
only runs on macOS has been executed yet.

| Finding | Status | Change | Commit |
|---|---|---|---|
| S1 | Fixed | `HandshakePool`: 5 s to X.224, 30 s total, 32 in flight, 4 per source; concurrent handshakes, accepts never pause; channels attached after auth | `38aa6dc` |
| S2 | Fixed | UDP SYN must be at least 1132 bytes and come from an IP holding a live offer; 64 peers, 4 per IP | `38aa6dc` |
| S3 | Fixed | Failures counted from explicit signals (`on_authenticated`, `on_handshake_failed`) on every path | `38aa6dc`, `a0ca594` |
| S4 | Fixed | Drive labels sanitised; mountpoints created fresh inside a 0700 per-process directory | `8a3ae58` |
| S5 | Fixed | IFD handler caps lengths before allocating; `catch_unwind` on every export; poison-tolerant lock | `8a3ae58` |
| S6 | Fixed | Absolute paths for every external command, with a test | `8a3ae58` |
| S7 | Fixed | Every handler call carries the peer; audit schema v2 | `38aa6dc`, `a0ca594` |
| S8 | Fixed | IPv6 keyed by /64; stale sweep at most once a second; oldest evicted at the cap | `a0ca594` |
| S9 | Fixed | Credential monitor: PAM re-check every 5 min, Keychain follow, revoke on rejection | `0db1db0` |
| S10 | Fixed | PAM conversation zeroes and frees earlier responses on failure | `8a3ae58` |
| S11 | Fixed | `src/private_dir.rs`: fresh 0700 directories, owner checked | `8a3ae58` |
| A1 | Partly | `main.rs` is the module list; startup is `src/app/` phases; `run` 976 to 633 lines. Channel and capture wiring still inline (see the plan's as-built notes) | `c5357a3`, `c064d01` |
| A2 | Fixed | `src/tunables.rs` registry of all 77 `MACRDP_*` variables, logged at startup; the vendored server reads no environment | `75b6b92` |
| A3 | Fixed for signalling | `ResyncSignal` replaces the `RESYNC_*` statics. Remaining statics are module-private caches of OS-global state | `bbce56c` |
| A4 | Fixed | `src/h264/`, `src/input/`, `src/app/` | `d36f396`, `c5357a3` |
| A5 | Closed by decision | Vendored crates are owned code; divergences recorded in `FORK.md` | |
| A6 | Fixed | `lock_server` / `lock_ctx` with a debug-build order check; one real ABBA risk in `setup_locked` fixed | `b2609bf` |
| Q1 | Fixed | `lock_or_recover` everywhere in `src/` (the one `lock().unwrap()` left is in a test that poisons a lock on purpose) | `f38afb3` |
| Q2 | Fixed | `undocumented_unsafe_blocks` enforced; a `SAFETY:` comment on every block and impl | `43acaa6` |
| Q3 | Fixed | Clippy clean with `-D warnings` on Linux and macOS; TIS calls serialised (the intermittent SIGABRT) | `b97c871` |
| Q4 | Fixed | Casts of external input use `try_from` | `b97c871` |
| Q5 | Fixed | The misleading `drop` calls replaced | `b97c871` |
| Q6 | Partly | Garbage-input tests on every `cargo test`; 7 cargo-fuzz targets written, no campaign run | `5104350` |

Also fixed along the way: two tests that captured tracing events failed about
one run in 40 (`0596568`).

### Still to check on a Mac

1. `cargo test --locked`, which adds the macOS-only tests (among them the
   three lock-order tests in `src/h264`).
2. mstsc first connection with the certificate prompt left open more than
   30 s: expect one extra reconnect, not a failure.
3. UDP multitransport from a client whose UDP leaves from a different IP than
   its TCP: expect the session to stay on TCP.
4. Credential monitor: change the account password while connected. RDP
   logins should stop within 5 minutes; with `--keychain`, updating the
   Keychain entry should restore them within a minute.
5. `--auto-unlock` after a password change (it should stop typing the old
   password once the monitor revokes it).
6. The audit stream (`MACRDP_AUDIT_JSON=1`) against a SIEM parser: schema v2
   adds `handshake_failed` and changes disconnect outcomes to `clean` /
   `error`.
7. Drive redirection, paste and lazy paste, which now use the new private
   temporary directories.
8. Smart-card redirection end to end (the IFD handler changed).
9. A short `cargo fuzz run` of each target (see `fuzz/README.md`).

### Not covered by this pass

- No runtime testing on macOS (above).
- No fuzzing campaign.
- The Swift helpers under `gui/` and the Objective-C USB shim were reviewed
  for their IPC surface only, not line by line.
