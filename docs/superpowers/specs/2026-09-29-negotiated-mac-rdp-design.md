# Negotiated Mac RDP Server — Design Spec

- **Date:** 2026-09-29
- **Status:** Draft for review
- **Working name:** MacRemoteDesktop (folder name only; the product name must avoid Microsoft/Apple trademarks)
- **Related:** [landscape research](../../research/2026-09-29-landscape.md), [macrdp flag inventory](../../research/macrdp-flags.md)

## 1. Intent

### 1.1 Outcome
A macOS RDP server that lets a headless Mac in a server room be used from Windows through a **stock RDP client** (mstsc / Windows App; FreeRDP as secondary) for **dev work and photo editing**, with **color accuracy** and **speed** as top priorities, while staying **lightweight on both host and client**.

### 1.2 Audience
Personal use first; every decision must keep a path to shipping it as a product.

### 1.3 Success criteria
1. Connecting with an unmodified RDP client produces a correctly sized, correctly scaled, color-correct desktop with **zero server-side configuration** beyond owner policy.
2. Static content is **bit-exact** on the client; moving content has ΔE < 1 when the client supports AVC444.
3. Host and client resource use meet the budgets in §9, and client CPU/GPU is measurably lower than Jump Desktop on the same workload.
4. The user whose credentials were entered in the RDP client gets **their own** macOS session, including when nobody is logged in.
5. Clipboard works both directions for text, rich text, images and files.

### 1.4 Constraints and non-goals
- **No custom client.** Anything the RDP protocol cannot express with stock clients is out of scope: HDR/EDR, reliable >60 Hz, ICC sync, QUIC, raw precision-touchpad gestures, Touch ID, Bluetooth passthrough. Wide-gamut content clips to sRGB.
- **No HEVC:** MS-RDPEGFX defines AVC only.
- Not a Mac App Store product (private APIs, system daemon).
- Drive mounting and device passthrough are nice-to-have, not gating.

## 2. Core principle: negotiated, not configured
Every session parameter is derived from **what the client advertises** during connection, **what the host is** (hardware, displays, entitlements, permissions), and **live feedback** (frame acks, network auto-detect, resize events). The only human-chosen inputs are **owner policy** (what is *permitted*). Negotiation decides what is *used* within policy. macrdp's CLI flags survive only as hidden developer/debug overrides.

## 3. Approach
Hard fork of **`clintcan/macrdp`** (canonical upstream; `islee23520/linardp` is a copy). macrdp v0.9.8 already provides: IronRDP-based server (vendored with ~25 documented divergences), VideoToolbox H.264 over EGFX, an AVC444 split module, UDP multitransport (verified on mstsc), AIMD adaptive bitrate with frame-rate floor, virtual displays, HiDPI, client-resolution adoption, clipboard (text/images/lazy files), drive/smart-card/camera/generic-USB redirection, system audio, PAM + NLA, auth rate-limiting, audit log, menu-bar controller.

Rejected alternatives: upstream-first with a thin product layer (roadmap depends on another maintainer; weak differentiation); clean rewrite on IronRDP (months to regain parity; discards verified interop work).

## 4. Architecture

### 4.1 Process model (privilege separation, sshd-style)
```
                   RDP client
                       │
┌──────────────────────▼───────────────────────┐
│ Broker (LaunchDaemon, root, minimal code)      │
│  TLS · NLA/CredSSP · PAM · policy check        │
│  find-or-create user's GUI session             │
│  hand the live connection to that session's agent │
└───────┬────────────────────────┬───────────────┘
        │ socket handoff          │
┌───────▼─────────┐     ┌────────▼────────┐
│ Session agent    │     │ Session agent    │  one per logged-in user,
│ (runs AS user A) │     │ (runs AS user B) │  inside that user's GUI session
└──────────────────┘     └─────────────────┘
```
- **Broker:** only root component. Handles untrusted input only up to authentication. No capture/encode/input/audio code.
- **Session agent:** runs as the authenticated user inside their GUI session; hosts the Negotiator and all subsystems.
- **Menu-bar app:** status, reasons for negotiated decisions, permission checks, owner-policy editor (admin-authenticated). No per-session settings.

### 4.2 Layers inside the session agent
```
Protocol layer    IronRDP (vendored): TLS, channels, TCP/UDP transport
Session Negotiator  ClientCaps + HostCaps + Policy + live events → SessionPlan
Subsystems        Displays · Video · Input · Audio · Clipboard · Redirection
macOS adapters    ScreenCaptureKit · VideoToolbox · CGVirtualDisplay · CGEvent
                  · Core Audio · FSKit · IOUSBHost · CUPS
```
Rules:
1. Subsystems never read flags or config; they consume their slice of the `SessionPlan` and emit events.
2. Private/fragile Apple APIs live only in adapters, behind traits, each able to report "unavailable" so the Negotiator can fall back.
3. New protocol code is written in IronRDP-shaped crates suitable for upstreaming; product logic stays in our crates.

## 5. Session broker and multi-user

### 5.1 Identity
The user is whoever authenticated via NLA/CredSSP. The broker validates credentials with PAM and checks owner policy (allow-list or group; default: local admins).

### 5.2 Scenarios
| # | Situation | Behavior |
|---|---|---|
| 1 | User A connects; nobody logged in | Broker authenticates A, creates A's session headless, hands off |
| 2 | A connects; A's session exists | Attach; apps remain as left |
| 3 | A connects from a second client | Take over; older connection closed with a reason |
| 4 | B connects while A is active | **Goal:** separate simultaneous session. **Guaranteed fallback:** B's session brought forward, A's keeps running in background (Fast User Switching) |
| 5 | Bad credentials / user not allowed | Rejected at NLA; rate-limit + lockout (inherited) |
| 6 | Client disconnects | Session persists; lock-on-disconnect per policy; reconnect resumes |
| 7 | User logs out in-session | Session ends; connection closes cleanly |
| 8 | Reboot with FileVault on | Unreachable until unlocked; status app + docs explain `fdesetup authrestart` and SSH FileVault unlock |

### 5.3 Risks to resolve in the Phase 0 spike
- Creating simultaneous non-console GUI sessions (private API, as used by Apple Screen Sharing).
- Creating `CGVirtualDisplay`s and injecting input inside a background session.
- Per-user TCC (Screen Recording, Accessibility) and whether a product needs an MDM PPPC profile.
- Apple macOS license terms on remote/multi-user access (legal review before sale).

## 6. Session Negotiator

### 6.1 Inputs and outputs
```
ClientCaps  core data, monitor layout (incl. physical size, scale), DisplayControl,
            EGFX caps, audio formats, multitransport flags, auto-detect RTT/bandwidth,
            offered RDPDR/URBDRC/camera devices, keyboard layout
HostCaps    Mac model/encoder abilities, physical displays online, entitlements present,
            TCC grants, private-API adapter availability
Policy      allowed users, permitted redirections, clipboard rules, lock-on-disconnect
      │
negotiate(client, host, policy) → SessionPlan (+ reason per decision)
      │
live events (resize, scale change, network change, client lag, device plug, suppress-output)
      → renegotiate → plan diff applied by subsystems
```

### 6.2 Properties
- `negotiate()` is **pure** (no macOS calls) and unit-tested against **recorded real handshakes** (mstsc, Windows App, FreeRDP).
- **Staged:** connect-time (displays, input, security) → graphics-channel-time (codec) → post-auto-detect (transport, bitrate). No stage waits on data not yet received.
- **Explainable:** every decision carries a reason string, logged and shown in the status app.
- **One declared heuristic:** Ctrl→Cmd remapping defaults on, off when the handshake looks like a macOS client; one toggle in the menu-bar app.

### 6.3 Disposition of macrdp's 43 settings
| Group | Examples | New behavior |
|---|---|---|
| Derived from client (~25) | width/height, hidpi, client-resolution, stretch, keyboard-layout, enable-h264, fps, bitrate, adaptive-bitrate (always on), keyframe/flush tuning, AAC, UDP multitransport/EGFX migration, lossy audio, cursor-scale | Negotiated |
| Derived from host (~5) | virtual-display, detach/capture/shield-primary | Headless detection |
| Owner policy (~10) | users, cert, audit file, lock-on-disconnect/auto-unlock, permitted redirections | Menu-bar policy editor |
| Diagnostics | log-dir, stats-endpoint | Hidden overrides |

## 7. Displays

### 7.1 Layout
- **Windowed client (the primary case):** exactly one virtual display equal to the client **window** size (arbitrary sizes, e.g. 1713×1288; encoder pads to 16-px internally, invisible to the user). Never assumes the client's full screen.
- **Full-screen spanning chosen in the client:** one virtual display per client monitor, same arrangement.
- **Stable serials** derived from (user, client monitor index) so macOS restores arrangement and window positions on reconnect.

### 7.2 Scaling (macOS renders only 1× or 2×)
| Client scale | Mode | Result |
|---|---|---|
| 100% | 1× at client pixels | Pixel-exact |
| 200% | 2× HiDPI, logical = pixels ÷ 2 | Pixel-exact Retina |
| 125/150/175% | 2× HiDPI at logical = pixels ÷ scale, GPU-downscaled to client pixels during color conversion | Correct UI size, sharp text |
Moving the client window to a monitor with a different DPI triggers re-selection.

### 7.3 Resize
Debounce ~250 ms during drags; keep streaming the old surface until the new mode's surface is ready; swap without reconnect or black frame. Monitor add/remove handled the same way.

### 7.4 Lifecycle
Created on first connect; **kept while the session exists, including while disconnected**; destroyed on logout. 60 Hz. Minimized client (Suppress Output) pauses capture and encode entirely.

### 7.5 Physical displays and fallback
If a physical display/dummy plug is attached, the remote session still uses virtual displays and the physical screen is blanked while a remote session is active (policy, default on). If `CGVirtualDisplay` is unavailable, fall back to capturing a physical display scaled to fit, with the reason surfaced.

## 8. Video and color pipeline

### 8.1 Color
- Virtual displays created with **sRGB primaries**; ColorSync converts all app output to sRGB.
- ScreenCaptureKit captures **sRGB, 8-bit BGRA** (no double conversion).
- **Full-range BT.709** YUV (retains macrdp's mstsc fix).

### 8.2 Codec ladder (from EGFX caps)
| Client advertises | Motion regions | Static regions |
|---|---|---|
| AVC444 v2/v1 | H.264 4:4:4 (main + auxiliary AVC420) | Lossless refinement |
| AVC420 only | H.264 4:2:0 | Lossless refinement |
| No H.264 | Lossless codecs | Lossless |

### 8.3 Lossless refinement
A region unchanged for ~150–300 ms is re-sent once with a lossless EGFX codec (Planar or ClearCodec) over the same surface. Delay adapts to available bandwidth; refinement is skipped under congestion.

### 8.4 Speed
- Dirty-region encoding from ScreenCaptureKit dirty rects (replaces macrdp's full-frame encode).
- VideoToolbox low-latency rate control, no B-frames.
- Zero-copy IOSurface capture → encoder; 4:4:4 split, scaling and color conversion on Metal, vImage fallback.
- Frame pacing from EGFX frame acks; skip frames rather than queue.
- One EGFX surface per virtual display.
- **Client-load feedback:** rising ack latency without network congestion → drop AVC444 → AVC420 + refinement; restore when it clears.

## 9. Performance budgets (release gates)
| Scenario | CPU | Memory |
|---|---|---|
| Broker idle | ~0% | < 20 MB |
| Session agent, idle desktop | < 1% of one core | < 150 MB |
| Session agent, active 4K @ 60 fps | < 15% of one performance core | < 300 MB |
| Client mstsc, active 4K @ 60 fps | Below Jump Desktop on same PC and workload | — |

Client-lightness levers: 0 fps when idle, dirty-region-only updates, native cursor via pointer PDUs (never baked into video), GPU-decodable H.264, one-shot lossless refinement, client-load feedback. Host levers: event-driven (no polling), zero-copy, no per-frame allocation on the hot path, correct QoS classes.

Enforcement: criterion micro-benchmarks on hot paths (CI fails on >~10% regression); scripted workload harness (idle, editor typing, scrolling, Lightroom slider drags, 4K video) recording host CPU/RAM/GPU/energy (`powermetrics`) and client mstsc CPU/GPU (Windows performance counters); one-time Jump Desktop baseline on the same workloads.

## 10. Clipboard and redirection

### 10.1 Clipboard (always on, per-session)
Both directions: plain text, rich text/HTML, images, files and folders (lazy, streamed on paste). Each session's clipboard is isolated. Policy may restrict formats or direction; default is everything both ways.

### 10.2 Redirection (automatic when offered, within policy)
| Feature | Plan |
|---|---|
| Drives | Auto-mount; move backend to FSKit where possible |
| Audio out | Always on; format from client support (AAC when bandwidth-tight) |
| Microphone | Finish upstream work; appears as a normal Mac mic |
| Webcam | Virtual camera (inherited) |
| Smart cards | Inherited |
| USB | Inherited; requires our own `com.apple.developer.usb.host-controller-interface` entitlement |
| Printers | New: RDPDR printer devices → CUPS printers |
| WebAuthn / Windows Hello | Later phase |
Default in personal mode: all on. Any row can be disabled by policy.

## 11. Failure handling
| Failure | Behavior |
|---|---|
| Private API broken after macOS update | Adapter reports unavailable → Negotiator falls back (physical capture / fast-user-switching); reason surfaced |
| Client rejects codec | Step down the codec ladder |
| Network collapse | Cut bitrate → fps → refinement (never to zero); UDP → TCP fallback (inherited) |
| Session agent crash | Broker restarts it; client auto-reconnects to the same macOS session |
| Broker crash | launchd restarts; sessions persist; clients reconnect |
| Redirection failure | Disable that feature for the session, log it; never tear down the session |
| Missing TCC permission | Pre-flight before accepting; clear disconnect reason; status app shows what to grant |

## 12. Testing
1. Unit: Negotiator vs recorded handshakes; color math; 4:4:4 split; dirty-region logic.
2. Protocol: FreeRDP as automated CI client — connect, resize, clipboard both ways, redirection, reconnect.
3. Color: known sRGB patches + fine text through the full pipeline to a frame-dumping FreeRDP client; assert bit-exact after refinement and ΔE < 1 in motion (AVC444).
4. Performance gates per §9.
5. Real-mstsc release checklist: windowed resize on an ultrawide, mixed-DPI move, multi-user scenarios 1–8, FileVault messaging (manual first, scripted later).
6. Upstream soak after each macrdp pull, under simulated loss (existing harness).

## 13. Productization
- **Licensing:** macrdp/IronRDP MIT/Apache-2.0 → commercial fork allowed; retain notices in `NOTICE`. New code in separate crates (keeps open-source/open-core/closed options). cargo-deny blocks GPL. RDP specs covered by Microsoft's Open Specifications Promise. Apple license terms reviewed before sale.
- **Apple:** Developer ID, hardened runtime, notarization. Entitlements to request: USB host controller interface, System Extension (camera), FSKit. Distribution as signed `.pkg`, not the App Store.
- **Install:** broker daemon, session agent, menu-bar app, extensions. First-run assistant with live TCC checks; MDM PPPC profile for managed deployments.
- **Configuration:** owner policy in a root-owned file, edited via menu-bar app after admin authentication.
- **Updates:** Sparkle, signed.
- **Telemetry:** none; opt-in crash reports only.
- **Upstream:** track `clintcan/macrdp`, upstream generic fixes to macrdp/IronRDP, keep a divergence log.

## 14. Delivery phases (each gets its own implementation plan)
| Phase | Scope | Exit criterion |
|---|---|---|
| 0 Foundation + spike | Fork into this repo; working branding; CI; color harness; performance harness with baselines incl. Jump Desktop; **throwaway spike** for §5.3 risks; submit Apple entitlement requests | Baselines recorded; spike verdict on simultaneous sessions |
| 1 Negotiator + headless single user | Negotiator replaces flags; sRGB virtual displays; windowed resize; mixed-DPI scaling; headless detection; clipboard verified both ways | Daily use from the ultrawide windowed setup with no config |
| 2 Color + speed | Dirty-region encode; AVC444 end-to-end; lossless refinement; low-latency encoder; zero-copy GPU path; client-load feedback; color + perf gates in CI | Bit-exact static content; budgets met; lighter than Jump on the client |
| 3 Broker + multi-user | Root broker; PAM login as credentialed user; socket handoff; scenarios 1–8 (simultaneous or fallback per spike) | All §5.2 scenarios pass on real mstsc |
| 4 Devices | Multi-monitor spanning; printers; microphone; FSKit drives; entitled USB | Each redirection verified on mstsc |
| 5 Product packaging | Signed/notarized `.pkg`; first-run assistant; Sparkle; MDM profile; license review; product name | Installable by a third party |
| Later | WebAuthn / Windows Hello redirection | — |

### 14.1 Local-agent usage
Delegated to local models (local-delegate MCP), always reviewed via diff: doc summarization, rebranding sweeps, flag→`SessionPlan` mechanical refactors, Negotiator unit-test generation, doc comments, benchmark scaffolding. Kept by Claude: architecture, Negotiator decision logic, color pipeline, codec/protocol work, broker/auth/TLS/entitlements, debugging against real clients.
