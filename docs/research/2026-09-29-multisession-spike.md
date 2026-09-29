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
