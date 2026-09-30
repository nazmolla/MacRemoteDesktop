# Reading the macrdp audit log

macrdp emits a security **audit event** for each connection lifecycle step — who
connected, whether the guard let them in, whether they authenticated, and how the
session ended. This page explains **what each event and field means and how to
interpret them**. For getting these events off-box into a SIEM/SOC (collector
configs, JSON stream), see [`siem-forwarding.md`](siem-forwarding.md); for the
knobs, see [`configuration.md`](configuration.md).

## Where the events are, and in what format

Every audit event is a `tracing` record on the dedicated **`macrdp::audit`**
target, written to up to two places:

- **Human-readable** — always in the main log, `~/Library/Logs/macrdp.log`
  (logfmt: `key=value` pairs). Find them with:
  ```bash
  grep 'macrdp::audit' ~/Library/Logs/macrdp.log
  ```
- **Structured JSON** — one JSON object per line, in a dedicated self-rotating
  file, **only when you opt in** with `--audit-file PATH` / `AUDIT_FILE` /
  `MACRDP_AUDIT_JSON=1`. This is the machine-parse target.

> **Parse the JSON stream, not the logfmt line.** The human-readable line's field
> order and spacing come from the `tracing` formatter and are not a stable
> contract; the JSON stream is versioned (`schema_version`) and is what tooling
> should consume.

The same event is written to both sinks, e.g. an accepted connection:

```text
# logfmt (macrdp.log)
2026-07-10T18:22:04.117Z  INFO macrdp::audit: schema_version=2 macrdp_version="0.8.32" host="mac-studio" event="accept" src_ip=203.0.113.5 src_port=54132
```
```json
// JSON (--audit-file)
{"timestamp":"2026-07-10T18:22:04.117Z","level":"INFO","target":"macrdp::audit","schema_version":2,"macrdp_version":"0.8.32","host":"mac-studio","event":"accept","src_ip":"203.0.113.5","src_port":54132}
```

## The six events

A successful login produces four events in order —
**`accept` → `auth` → `fingerprint` → `disconnect`**. A connection that does not
log in ends earlier: at **`auth`** with `outcome="did_not_complete"` for rejected
credentials, or at **`handshake_failed`** for anything else that stopped it before
authentication. **`reject`** covers connections the guard blocks before they ever
handshake.

### `accept` — the guard let the connection through *(INFO)*
Emitted the moment a connection passes the pre-handshake auth guard (per-IP
rate-limit + lockout checks) and is handed to the TLS/CredSSP stack. **It does
not mean the client authenticated** — only that it was allowed to try. Every
accepted connection is followed, with the same `(src_ip, src_port)`, by exactly
one of: an `auth` success (then `fingerprint` and `disconnect`), an `auth`
failure, or a `handshake_failed`.

### `reject` — the guard blocked it before any handshake *(WARN)*
Emitted when the per-IP guard refuses the connection outright; it is dropped with
no TLS, no CredSSP, no `accept`. `reason` says why:

- **`reason="rate_limit"`** — too many connection attempts from this IP inside the
  sliding window (default 10 / 60 s). `window_attempts` is how many were counted.
- **`reason="lockout"`** — this IP is in an escalating cooldown after repeated
  failed logins or failed handshakes (default: after 5 consecutive, 30 s doubling to a 15 min cap).
  `retry_after_secs` is how long until the cooldown expires.

A `reject` carries **`src_ip` but not `src_port`** (the decision is per-IP, made
before the port matters) and has **no matching `accept`** — correlate rejects by
`src_ip` + time, not by the connection tuple.

### `auth` — the CredSSP/NLA login verdict *(INFO on success, WARN on failure)*
Emitted **once per connection**, right after the TLS upgrade, when the credential
exchange resolves. This is the authoritative login result:

- **`outcome="success"`** — the client's credentials validated against the macrdp
  account. *(INFO)*
- **`outcome="did_not_complete"`** — authentication did not finish. Dominated by a
  **wrong username/password**, but also covers a client aborting the credential
  dialog or a rare mid-exchange transport error. `reason` is a short sspi error
  description (sanitized, ≤200 chars, never credential material) that
  disambiguates. *(WARN)*

Because it fires *after* the TLS upgrade, a benign pre-TLS blip (e.g. mstsc's
first-connect certificate-trust prompt reopening the socket) happens **before**
this point and can never produce a false `auth` failure — such a connection has
an `accept` followed by a `handshake_failed`, with **no `auth` event**, which is
itself the tell that it never reached authentication.

> **Scope:** the `auth` event is emitted on the single-process server path. See
> `configuration.md`.

### `fingerprint` — which RDP client connected *(INFO)*
Emitted **once per connection** when the capability exchange completes (after
`auth`; not re-emitted on an in-session reactivation such as a live resize or
blank recovery). Carries the identity the client announced during the handshake:

- **`client_name`** — the client machine's hostname (client-controlled;
  sanitized: control-chars stripped, length-bounded).
- **`rdp_version`** — the announced RDP protocol version, hex (e.g. `0x80011` =
  RDP 10.12).
- **`client_build`** — the client's announced build number.
- **`platform`** — the OS platform from the client's General capability set.

**This is fingerprinting, not authentication** — a client can claim anything.
Live-verified signatures for telling clients apart:

| Client | `client_build` | `platform` |
|---|---|---|
| **mstsc** (real Windows) | the actual Windows build (e.g. `26100` = Win11 24H2, `22621` = 22H2) | `WINDOWS/WINDOWS_NT` |
| **Thincast** | `18363` (a fixed, claimed value — not the host's real build) | `UNSPECIFIED/UNSPECIFIED` |
| **FreeRDP** family | `2600` (hardcoded XP build) | `UNIX/...` |
| **Windows App** (macOS/iOS/Android) | varies | its host platform |

Reading it: mstsc reports the *machine's real* build and a Windows platform;
everyone else reports a fixed/claimed build and (for the FreeRDP family, which
Thincast derives from) a non-Windows or unspecified platform. Use it for "which
client is this?" triage — never as a trust signal.

> **Scope:** the `fingerprint` event is emitted on the single-process server
> path (same as `auth`).

### `handshake_failed` — the connection ended before authenticating *(WARN)*
Emitted when an accepted connection is dropped before the login verdict, for a
reason other than rejected credentials. `reason` says why:

- **`reason="timeout"`** — the client did not finish the handshake in time (5 s
  to send its first RDP request, 30 s for the whole handshake including TLS and
  CredSSP). A port scanner that connects and says nothing ends here.
- **`reason="tls"`** — the TLS handshake failed. mstsc's first-connect certificate
  prompt produces one of these.
- **`reason="protocol"`** — the client did not speak RDP.
- **`reason="capacity"`** — the server had no free handshake slot (too many
  handshakes in flight overall or from this source). Not counted against the IP.

The first three count toward the lockout, the same as a failed login.

### `disconnect` — a logged-in session ended *(INFO)*
Emitted when a session that authenticated ends, with `duration_ms` (wall-clock
lifetime) and `outcome`:

- **`outcome="clean"`** — the session ended normally.
- **`outcome="error"`** — the session ended with an error (for example a network
  drop).

This event does not affect the lockout: only the login step does.

## Field reference

| field | type | on events | meaning |
|---|---|---|---|
| `timestamp` | string | all | RFC3339 UTC when the event was recorded (JSON only; logfmt shows it as the line prefix) |
| `level` | string | all | `INFO`, or `WARN` for `reject` and `auth` failure |
| `target` | string | all | always `macrdp::audit` (the grep key) |
| `schema_version` | int | all | audit contract version (`2`); bumps only on a breaking field change |
| `macrdp_version` | string | all | server build, e.g. `0.8.32` |
| `host` | string | all | server hostname (a collector usually adds its own too) |
| `event` | string | all | `accept` \| `reject` \| `auth` \| `handshake_failed` \| `fingerprint` \| `disconnect` |
| `src_ip` | string | all | client source IP — the primary correlation key |
| `src_port` | int | accept, auth, handshake_failed, fingerprint, disconnect | client source port — completes the per-connection tuple. **Absent on `reject`.** |
| `reason` | string | reject, auth (failure only), handshake_failed | reject: `rate_limit` \| `lockout`. auth: sanitized sspi error text. handshake_failed: `timeout` \| `tls` \| `protocol` \| `capacity` |
| `window_attempts` | int | reject (`rate_limit`) | attempts counted in the current window |
| `retry_after_secs` | int | reject (`lockout`) | seconds until the cooldown expires |
| `outcome` | string | auth, disconnect | auth: `success` \| `did_not_complete`. disconnect: `clean` \| `error` |
| `client_name` | string | fingerprint | client's announced hostname (client-controlled; sanitized) |
| `rdp_version` | string | fingerprint | announced RDP protocol version, hex |
| `client_build` | int | fingerprint | client's announced build number |
| `platform` | string | fingerprint | OS platform from the General capset (server-formatted) |
| `duration_ms` | int | disconnect | connection wall-clock lifetime in milliseconds |

**Correlation:** an `accept`, its `auth`, and its `disconnect` share
`(src_ip, src_port)`. Rejects have no port and no matching accept — group them by
`src_ip`. Ephemeral source ports get reused over time, so bound correlation by a
time window; a monotonic per-connection id is a possible future additive field.

## Reading common patterns

**Normal successful session**
```text
event="accept"      src_ip=203.0.113.5 src_port=54132
event="auth"        src_ip=203.0.113.5 src_port=54132 outcome="success"
event="fingerprint" src_ip=203.0.113.5 src_port=54132 client_name="GENMACWIN" client_build=26100 platform="WINDOWS/WINDOWS_NT"
event="disconnect"  src_ip=203.0.113.5 src_port=54132 duration_ms=216913 outcome="clean"
```
Allowed → authenticated → identified as real mstsc → clean multi-minute session.
The baseline.

**A single wrong password**
```text
event="accept"     src_ip=203.0.113.9 src_port=51020
event="auth"       src_ip=203.0.113.9 src_port=51020 outcome="did_not_complete" reason="logon denied"  (WARN)
```
One bad login: the `auth` WARN is both the authoritative signal and what the
lockout counter records. There is no `disconnect`, since no session started.

**Brute force / password spray → lockout**
```text
… five (or more) accept → auth did_not_complete cycles from 203.0.113.9 …
event="reject" reason="lockout" src_ip=203.0.113.9 retry_after_secs=30   (WARN)
event="reject" reason="lockout" src_ip=203.0.113.9 retry_after_secs=60   (WARN)   ← escalating
```
After the threshold, the guard stops handing the IP to the stack; each further
attempt is a `reject` with a doubling `retry_after_secs`. A burst faster than the
window instead shows `reason="rate_limit"` with `window_attempts`.

**Benign client blip (e.g. mstsc cert prompt)**
```text
event="accept"           src_ip=203.0.113.5 src_port=54120
event="handshake_failed" src_ip=203.0.113.5 src_port=54120 reason="tls"   (WARN)
event="accept"           src_ip=203.0.113.5 src_port=54121
event="auth"             src_ip=203.0.113.5 src_port=54121 outcome="success"
event="disconnect"       src_ip=203.0.113.5 src_port=54121 outcome="clean"
```
The first connection ended **before** `auth` — it never reached authentication,
so it is not a failed login. It counts as one failure toward the lockout, far
below the threshold, and the successful login right after resets the counter.

**Loopback** — `127.0.0.1` / `::1` is exempt from the guard's enforcement but is
**still audited**, so you will see `accept`/`auth`/`disconnect` for local
connections (dev, `--skip-auth`). These are local, not remote logins; filter them
out or treat them as low signal in a SOC.

## Interpretation gotchas

- **No audit events at all?** The audit handler only exists when the connection
  guard is enabled. `MACRDP_CONN_GUARD=0` disables the whole subsystem — including
  audit — even with `MACRDP_AUDIT_LOG=1`. To keep audit while disabling
  *enforcement*, leave `MACRDP_CONN_GUARD` on and zero the thresholds
  (`MACRDP_GUARD_RL_MAX=0`, `MACRDP_GUARD_FAIL_THRESHOLD=0`).
- **`did_not_complete` ≠ always "wrong password."** It is dominated by bad
  credentials but the `reason` string is what tells logon-denied from a client
  abort or transport reset. Do not alert solely on the count without reading it.
- **Volume is bounded.** Rejected connections never reach the stack, so a
  brute-forcer's accepted attempts are capped by the lockout, and the audit file
  self-rotates — the stream can't run away under attack.
- **Schema 2 (2026-09-30).** Version 1 emitted a `disconnect` for every accepted
  connection, with a heuristic `outcome` of `success` or `failure` that fed the
  lockout. Version 2 adds `handshake_failed`, emits `disconnect` only for
  sessions that logged in, and changes its `outcome` to `clean` or `error`.
  Rules keyed on `disconnect.outcome="failure"` should move to `auth`
  `did_not_complete` plus `handshake_failed`.
- **`macrdp_version` / `schema_version`** let you pin detection rules across
  upgrades; key alerts off `schema_version` so an additive field never breaks a
  parser.

## Verifying it locally

[`scripts/test-audit-log.sh`](../scripts/test-audit-log.sh) exercises the whole
path end-to-end with no real password and no GUI: it starts a loopback macrdp
(`--skip-auth` against a throwaway credential), drives one correct- and one
wrong-password `sdl-freerdp +auth-only` connection, and asserts the JSON audit
stream recorded `auth` `success` / `did_not_complete` (with a clean `reason`) plus
the `accept`/`disconnect` correlation. Needs `sdl-freerdp` (`brew install
freerdp`); exit 0 = pass.

## See also
- [`siem-forwarding.md`](siem-forwarding.md) — forwarding the JSON stream to a SIEM (Vector / Fluent Bit / rsyslog).
- [`siem-tutorial.md`](siem-tutorial.md) — a runnable end-to-end walkthrough: OpenSearch SIEM on your Mac detecting an RDP brute-force.
- [`configuration.md`](configuration.md) — `--audit-file`, `MACRDP_AUDIT_*`, and the connection-guard thresholds.
