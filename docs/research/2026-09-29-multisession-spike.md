# Multi-session spike — findings (2026-09-29)

Status: **mechanism identified; background-session probe run pending** (needs the
`rdpspike` test account, which the user has parked for now).

## Question
How do Apple Screen Sharing and Jump Desktop run simultaneous remote sessions for
different macOS users, and can a per-user session agent create virtual displays,
capture, and inject input inside a background (non-console) session? (spec §5.3)

## How Jump Desktop Connect does it
Evidence from this Mac (macOS 27, Jump Desktop Connect installed and running):

| Piece | Evidence | Role in our design |
|---|---|---|
| `JumpConnect --service`, **root**, LaunchDaemon `com.p5sys.jump.connect.service` (KeepAlive, RunAtLoad) | `ps`, `/Library/LaunchDaemons/…service.plist` | = our **broker** |
| `JumpConnect --rtcproxy <sock>`, **root** | `ps` | network/transport proxy (we fold this into the broker) |
| `JumpConnect --desktopproxy <sock>`, **runs as the user** | `ps` (user `mnazmi`) | = our **session agent** |
| LaunchAgent `com.p5sys.jump.connect.agent` with `LimitLoadToSessionType = [Aqua, LoginWindow]`, launched on demand by a `com.apple.notifyd.matching` notification | `/Library/LaunchAgents/…agent.plist` | how the root service starts an agent **inside a specific GUI session, including at the login window** |
| Unix sockets `/var/run/com.p5sys.jump.connect.desktop-server.<uuid>.sock` | `ps` | broker ↔ agent hand-off |
| Private symbols: `CGSCreateLoginSessionWithDataAndVisibility`, `CGSSessionScreenIsLocked`, `kCGSSessionOnConsoleKey`, `kCGSSessionUserIDKey`, "Create-displays request … not permitted by policy", "UnlockSlow" | `strings` on the binary | session creation, lock handling, virtual displays |
| Core Audio drivers `JumpAudio.driver`, `JumpAudioMic.driver` (run by `_coreaudiod`) | `ps` | virtual audio devices (we use ScreenCaptureKit/process taps instead) |

## How Apple Screen Sharing does it
`strings` on `screensharingd` and `ScreensharingAgent` (RemoteManagement bundles):
- `screensharingd` (root) logs `CGSCreateLoginSessionWithDataAndVisibility err = %d session = %d`,
  handles `HandleChangeSessionVisibilityMessage`, tracks `OnConsole`, `maximumVirtualDisplays`,
  `virtualDisplayCount`.
- `ScreensharingAgent` (per session) owns `SSAgentVirtualDisplay`
  (`initVirtualDisplay:displayInfo:blankScreen:dynamicResolution:`), built on SkyLight's
  `SLVirtualDisplay` — the class `CGVirtualDisplay` wraps — and reads
  `CGSSessionScreenIsLocked` / `kCGSSessionOnConsoleKey`.

**Same mechanism as Jump:** a root daemon creates a new login session with
`CGSCreateLoginSessionWithDataAndVisibility` (visibility = background), and a
per-session agent creates virtual displays and streams them.

`CoreGraphics` on macOS 27 exports `CGSCreateLoginSessionWithDataAndVisibility`,
`CGSCreateLoginSession` and `CGSReleaseSession` (checked with `dlsym` via ctypes).

## Probe results
Console session (user `mnazmi`), after granting Screen Recording + Accessibility to the
Claude Code `claude` binary:

    {"capture":"ok 640x360 display=11","console":"1","input":"ok (posted, AX trusted)","user":"mnazmi","virtual_display":"ok id=11"}

Background session: **not yet run** (requires the `rdpspike` account; see "Next").

## Per-user TCC
Not yet measured for a second user. Observed for the console user: TCC grants attach to
the *responsible* process (here Claude Code's `claude` binary, because the desktop app
launches it through a responsibility-disclaiming helper), not to the child binary.
Implication for the product: the signed session agent must be its own responsible
process (launched by launchd as a LaunchAgent, like Jump's), so grants attach to our
app — per user unless an MDM PPPC profile is deployed.

## Verdict
**Phase 3 mechanism:** root broker creates a background login session with
`CGSCreateLoginSessionWithDataAndVisibility`; a LaunchAgent limited to
`Aqua` + `LoginWindow` session types is started in that session on demand
(notifyd-matching launch event) and talks to the broker over a Unix socket.
This is what both Apple and Jump ship, so simultaneous sessions are the Phase 3 plan;
Fast User Switching stays only as the fallback if the function's contract (the
`data` argument — expected to carry login credentials — and the visibility flag)
proves unusable.

Private symbols Phase 3 depends on: `CGSCreateLoginSessionWithDataAndVisibility`,
`CGSReleaseSession`, `CGSessionCopyCurrentDictionary` keys (`kCGSSessionOnConsoleKey`,
`kCGSSessionUserIDKey`, `CGSSessionScreenIsLocked`), `CGVirtualDisplay`.

## Next (before Phase 3 design)
1. Run the probe inside a background session (`rdpspike` logged in via Fast User
   Switching, then `sudo launchctl asuser <uid> sudo -u rdpspike /Users/Shared/probe`).
2. Determine the signature and `data` format of
   `CGSCreateLoginSessionWithDataAndVisibility` (disassemble `screensharingd`'s call
   site), then create a background session from a root test tool and run the probe in it.
3. Measure per-user TCC for the second user.
4. **Locked-session case** (found in Phase 0 perf work): a headless Mac spends most of its
   time at the lock screen. The broker must handle "session exists but locked" — unlock on
   successful RDP authentication, as Jump's `UnlockSlow` / `CGSSessionScreenIsLocked`
   handling suggests — not only "no session".

## Results, 2026-10-02 (Mac mini, macOS 27)

1. **Fast User Switching background session (`rdpspike`, probe run via `launchctl asuser` + `sudo -u`, launched by root):** `console=0`; the probe's own virtual display never appeared in ScreenCaptureKit within 5 s; SCK listed only the console's displays (the physical 3440×1440 and Viga's 1718×1334) and captured the *console user's* desktop; the posted mouse event landed on the console. A Fast-User-Switching background session does not render on its own. (Launching from root also bypassed per-user TCC, so this run says nothing about rdpspike's grants.)
2. **Call recovered from `screensharingd` (arm64e):** `err = CGSCreateLoginSessionWithDataAndVisibility(bytes, len, 0, &session, NULL)` where `bytes` is a **binary plist** (format 200). `CreateOffConsoleLoginWindowSession` sends `{SessionStartedBy: "ScreenSharing"}`; `LoginUser` adds `username` and `UserPasswordKey` (the account password) to the caller's dictionary. Both pass visibility 0 and run as root.
3. **Off-console login-window session, created from a root tool** (`spikes/multisession/loginsession/`): `err=0`, a new `loginwindow` (running as root) appears; `CGSReleaseSession` returns 0 and that `loginwindow` exits within seconds. Three create/release cycles, no effect on WindowServer or the console session.
4. **Probe inside that session via `launchctl bsexec <loginwindow pid>`:** runs as root in the session (`id` works) but the probe traps (exit 133) at virtual display creation. Joining the bootstrap namespace from outside is not enough; Apple's `ScreensharingAgent` and Jump's agent are LaunchAgents limited to `LoginWindow`/`Aqua` session types that launchd starts inside the session.

**Next:** a test LaunchAgent with `LimitLoadToSessionType = LoginWindow`, loaded into the off-console session (find how screensharingd triggers it: `launchctl bootstrap` into the session's domain, or the notifyd-matching launch event Jump uses), running the probe there; then the logged-in-user path (`LoginUser` with credentials).
5. **LaunchAgent inside the session** (`/Library/LaunchAgents/ca.nazmi.viga.spike.plist`, `LimitLoadToSessionType = LoginWindow`, `RunAtLoad`): launchd starts the probe inside each new off-console login session. There it **creates its virtual display** (`console=0`, ids 225 and 227). Capture fails with "The user declined TCCs … display capture" and input with "not AX-trusted", both for an unsigned probe and for the probe signed as `ca.nazmi.portico` (Developer ID). A standalone executable does not inherit the app bundle's grants; the next test needs the agent inside `Viga.app` (or an MDM PPPC profile). Test agent removed afterwards.
6. Side effect seen once: after the first create/release cycles the console went to a `root` login window, leaving the remote user's session off-console (RDP froze until the user logged in again). Session creation must not be run on a machine whose console session is in use without expecting this.
7. **Moved to a macOS 27.0.1 VM** (`spikes/vm/vmhost`, Apple Virtualization.framework, 2 CPUs / 4 GB) after a credentialed visibility-0 create crashed the host's WindowServer (all users logged out, 2026-10-02 14:44). In the VM, with a generated-password test user `vmspike` and an agent limited to `Aqua`+`LoginWindow` that logs its uid: credentialed creates with visibility 1 and 0 both return `err=0` but start only a **root login-window session** — the user is not logged in (no `vmspike` Aqua agent ran) — and **visibility 1 moved the console from the logged-in user to that login window**. No WindowServer crash in the VM. So `{username, UserPasswordKey, SessionStartedBy}` alone does not log a user in on macOS 27; `LoginUser` builds its dictionary from a mutable copy of a caller-supplied one, whose other keys are the next thing to recover (from `screensharingd`'s callers of `LoginUser`). Neither visibility value leaves the console alone, which scenario 9 needs.
