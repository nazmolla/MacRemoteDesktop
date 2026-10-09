# Phase 3 — Session broker and multi-user

## The constraint that shapes this plan (read first — corrected 2026-10-03)

**Simultaneous multi-user IS achievable without any special entitlement** — the leading
product (Jump Desktop / Fluid) ships it, and holds no private entitlement. macOS provides
**background sessions**: when a second local user logs in while the first is still logged in
(Fast User Switching), that second user gets a *real, logged-in* Aqua session in the
background. An in-session agent can **capture that session's real framebuffer and inject
input** there — no virtual display involved.

The entitlement `com.apple.private.SkyLight.virtualdisplay` is only required to put a
**virtual display** into a background session, which also gets you dynamic client-resolution
matching, HiDPI negotiation and audio there. That entitlement lives in the `com.apple.private.*`
namespace, is AMFI-validated against the signed binary, and is **not issued to third parties**
(and can't be faked below WindowServer: a kext can't inject a userspace entitlement, and
faking one means SIP off = unsignable/un-notarizable). **Nobody third-party has solved
virtual-display-in-background — Jump explicitly documents that "virtual displays and audio
remoting will not work when a user is logged into a background session."** So we match the
market: background (non-console) sessions capture the native framebuffer, no virtual
display, no audio.

**Why the earlier VM spike concluded "not achievable":** it kept trying to create a
*virtual display* inside the background session — the one thing that genuinely needs the
entitlement. It never tested capturing the session's **real** framebuffer, which is what
Jump actually does. `docs/research/2026-10-03-background-sessions.md`'s "needs Apple's
entitlement / not achievable" verdict is therefore corrected: it applies to
virtual-display-in-background only, not to simultaneous sessions.

**Out of scope (hard lines):** bypassing/patching the WindowServer off-console check;
reverse-engineering screensharingd's credentialed-login dictionary; writing our own
remote-login-protocol client. The macOS 14+ supported **"Remote Desktop" permission for
unattended access** is the legitimate path to verify for driving an unattended login,
in place of the private reverse-engineering.

## What this phase delivers (simultaneous multi-user, Jump's model)

**The broker authenticates every connection and routes each user's clients to that user's
own session agent. The console / first user gets full Viga (virtual display, dynamic
resolution, HiDPI, audio). Additional users run in background sessions where the agent
captures the native framebuffer and injects input — no virtual display, no audio there
(the documented limitation Jump also ships).**

### Architecture (the Jump / Apple split, proven in the spike)
- **Broker** — root LaunchDaemon (`ca.nazmi.portico.broker`), `KeepAlive`, `RunAtLoad`.
  Binds the RDP port (3389 privileged is now possible as root; 3390 default stays).
  Terminates TLS + NLA/CredSSP, authenticates via PAM (`auth::check`), checks owner policy
  (allow-list / admin group). Owns the long-lived machine state the current process owns
  today (caffeinate, headless-display policy decisions are delegated to the agent).
- **Session agent** — per-GUI-session LaunchAgent (`LimitLoadToSessionType = Aqua`),
  runs as the logged-in user inside their session. This is the *current Viga server*,
  minus the listen/TLS/auth front end: it receives an already-authenticated TCP socket
  from the broker over a Unix socket via `SCM_RIGHTS` fd passing and runs capture / encode /
  input / clipboard / device redirection. The agent detects whether its session is on the
  **console** (`CGSessionCopyCurrentDictionary` → `kCGSSessionOnConsoleKey`): on the
  console it uses the full virtual-display + audio path as today; in a **background**
  session it uses the degraded path below. This console-vs-background branch is the one
  genuinely new capability to prove in the VM.

### Background-session rendering (no SkyLight entitlement → no virtual display there)
A virtual display can't be created off-console, so the background session renders at its
framebuffer's own geometry. To still serve the client its own resolution, in order of
preference:
1. **Mode-set (best, if available):** set the background session's framebuffer to a display
   mode matching the client via `CGDisplaySetDisplayMode` — a true native-geometry relayout
   (macOS lays out the desktop at that size), **no scaling**. Only if that session has a
   settable display with a suitable mode (unknown on a headless Mac — probed in step 0).
2. **Server-side resize (fallback, already in Viga):** capture the fixed-geometry framebuffer
   and scale frames to the client's size on the encode path — the existing non-`--virtual-display`
   mirror-capture path (letterbox default / `--stretch`), with client-resolution auto-adopt.
   Resolution-*matching*, not native: upscale softens, downscale loses sharpness; same tier
   as VNC / Apple Screen Sharing for additional users. Cost: forces full-frame encodes
   (no dirty-rects while scaling), mitigated by the encoder frame-diff. Acceptable for one
   extra user. This is also the handler for **live client resize** (rescale, since we can't
   re-mode a display we may not own).
3. **Audio: WORKS — verified in the VM.** Contrary to the initial assumption (Jump documents
   "audio … will not work" in background, but that's *their* JumpAudio driver's limit), Viga's
   existing ScreenCaptureKit system-audio capture **does deliver audio off-console** — the VM
   agent logged `SCK audio format rate=48000 channels=2` while a client was connected and sound
   played in the background session. No Core Audio tap needed. **Caveat:** SCK audio is
   *system-wide*, so it is **not per-session isolated** — with several users playing audio at
   once, each client hears the mixed system output. Fine for one active user; a Core Audio
   process tap scoped to the session's processes is the future path to per-session isolation
   if needed.
- **Handoff** — `/var/run/ca.nazmi.portico.agent.<uid>.sock`. Broker → agent passes the
  connection fd plus the negotiated `SessionPlan` seed and the authenticated username.

### Session routing (broker decision table, maps to spec §5.2)
| # | Situation | Broker behavior |
|---|---|---|
| 1 | Nobody logged in | Broker authenticates A, then creates A's login session (console). Mechanism to verify in the VM: the macOS 14+ supported unattended-login path, else designated auto-login, else manual/`fdesetup authrestart` for FileVault (scenario 8). |
| 2 | A connects, A's session exists | Route to A's agent; attach (apps as left). |
| 3 | A connects from a 2nd client | Route to A's agent; agent takes over, old client dropped with reason (existing ARC/takeover path). |
| 4 | B connects while A owns console | **Separate simultaneous session.** B is logged into a background session (FUS); B's agent captures the native framebuffer there (no virtual display / no audio — documented). A keeps full features on the console. |
| 5 | Bad creds / not allowed | Rejected at broker (PAM verdict / policy); rate-limit + lockout (existing `auth_guard`). |
| 6 | Client disconnects | Agent persists; lock-on-disconnect per policy; reconnect re-routes to same agent. |
| 7 | User logs out | Agent exits with its session; broker drops the route. |
| 8 | Reboot + FileVault | Unreachable until unlocked; broker status + docs explain `fdesetup authrestart` / SSH unlock. |
| 9 | Someone at the Mac | Console behavior is macOS's own; the console user is whoever macOS says; the agent follows the console. |

## Owner policy & menu-bar UI (added per user, 2026-10-03)
Two policy knobs in the menu-bar app (root-owned policy file, edited after admin auth,
per spec §12 config model):
- **Primary user** — a designated account that ALWAYS gets the full console treatment
  (virtual display, dynamic resolution, HiDPI, audio). The broker, when the primary user
  connects, ensures they land on the **console** session (they are the one user who gets the
  virtual-display deal); everyone else is a background user on the degraded (native-capture)
  path. If unset, the console goes to whoever connects first.
- **Enable / disable multi-user** — a master toggle. Off: Viga serves only the primary
  (or first) user and additional users are refused with a reason (the pre-Phase-3 behaviour).
  On: additional users get background sessions per this plan. Default off until Phase 3 is
  verified on real mstsc.
Both are negotiator/broker inputs, not client-derived: they live in `Policy` (spec §6.1)
and the broker consults them when routing a connection (decision table above).

## Steps
0. **VM proof of the one new capability (do FIRST — it gates everything else).** Manually
   log a second user into the VM via the console (Fast User Switching; no credential
   automation), confirm their session is a background session (`console=0`), then run a
   **bundle-signed** Viga agent as a LaunchAgent inside it and verify: (a) SCK captures
   *that session's* real framebuffer (not the console's — the 2026-10-02 `launchctl asuser`
   run captured the console because it wasn't a true in-session agent), (b) input injects
   into that session, (c) the TCC grants on `Viga.app` carry (the standalone probe failed
   here). If (a)–(c) pass, simultaneous multi-user is proven and the rest is engineering.
   Also probe the rendering levers for the degraded path: (d) does the background session
   have a framebuffer at all and at what base resolution (headless Mac / no physical
   display — the real unknown); (e) `CGDisplayCopyAllDisplayModes` / `CGDisplaySetDisplayMode`
   on that session's display — is a client-matching mode settable (the mode-set option)?;
   (f) does a Core Audio process tap scoped to a background-session process capture its audio?
   (d)–(f) decide render strategy but don't gate the proof — server-side resize (already in
   Viga) covers resolution if mode-set fails.
1. **Spec edit** — §5.2 scenario 4 reworded to the simultaneous-session behaviour. (Done this commit.)
2. **`src/broker/` module skeleton + two entry modes** — `--broker` (daemon) and
   `--session-agent <handoff-sock>` added to args; `main`/`app::run` dispatch. Non-broker
   default path stays byte-identical (single-process, as today) so nothing regresses while
   the broker is built behind a flag. (This commit.)
3. **fd-passing transport** — `SCM_RIGHTS` send/recv over the Unix socket, unit-tested with
   a socketpair + a loopback TCP fd round-trip. Keep it in one quarantined module.
4. **Agent front-end refactor** — factor the current listen/TLS/auth out of `app::run` so
   the capture/encode/input core can be driven by a handed-off, pre-authenticated socket.
   The in-process (non-broker) path reuses the same core with its own front end.
5. **Broker front-end** — listen + TLS + NLA + PAM + policy + `auth_guard`; resolve the
   target console session (`CGSessionCopyCurrentDictionary` → `kCGSSessionUserIDKey`,
   `kCGSSessionOnConsoleKey`); start/locate the agent; hand off.
6. **Agent lifecycle** — LaunchAgent plist (`Aqua`), launchd-triggered; broker starts it
   in the console session via `launchctl asuser <uid> launchctl bootstrap`/notifyd match;
   TCC grants attach to `Viga.app`'s agent (verified-needed in the spike — standalone
   binary did not inherit grants; agent must be inside the bundle or an MDM PPPC profile).
7. **Scenario tests in the VM** — 1, 2, 3, 4 (simultaneous, background capture), 5, 6, 7,
   9 end-to-end on real mstsc/RDM; 8 = FileVault messaging. Session-creation churn only in
   the VM (`[[virtual-display-churn-crashes-windowserver]]`).
8. **Entitlement request (optional upside, not a dependency)** — file a Feedback Assistant
   request for the SkyLight virtual-display entitlement; if ever granted, a runtime check
   lights up virtual display + audio in background sessions too. The feature ships without
   it, matching Jump's documented limitation.

## Out of scope (hard lines, not revisited)
Bypassing/patching the WindowServer off-console check; reverse-engineering screensharingd's
credentialed-login dictionary to log a user in; writing a remote-login-protocol client.
