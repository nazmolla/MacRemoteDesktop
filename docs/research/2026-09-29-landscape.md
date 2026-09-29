# Mac RDP server landscape (2026-09-29)

Working name: MacRemoteDesktop (rename later).

## Decisions so far
- Host: headless macOS in a server room. Client: stock RDP clients only (mstsc / Windows App / FreeRDP). No custom client; protocol limits accepted.
- Workloads: dev work + photo editing. Priorities: color accuracy, speed. Drive mounting and device passthrough are nice-to-have.
- Personal use now, product-ready design always (own Developer ID, own entitlements, notarized, non-App-Store).
- Everything auto-configured from the RDP handshake + live feedback. Nothing hardcoded; CLI flags become hidden debug overrides.
- Color: handshake carries no gamut/ICC, so the stream is treated as sRGB; virtual displays are created with sRGB primaries so macOS color-manages into sRGB before capture.
- Approach: hard fork of `clintcan/macrdp` (canonical upstream; `islee23520/linardp` is a copy) restructured around a Session Negotiator.
- Local agents (local-delegate MCP) used for doc summarization, flag extraction, mechanical refactors, doc comments and simple tests; design, protocol, color, security and debugging stay with Claude.

## Existing implementations surveyed
| Project | Kind | Notes |
|---|---|---|
| macrdp (clintcan/macrdp) | Native RDP server, Rust/IronRDP, MIT/Apache | v0.9.8. H.264 VideoToolbox, AVC444 module, UDP multitransport (verified mstsc), adaptive bitrate, virtual display, HiDPI, resize, drive redirection, generic USB redirection (entitled build), smart card, camera, audio, PAM+NLA. Missing: multi-monitor, printer, dirty-region encode. Flag-driven config. |
| osxrdp | xrdp 0.10.6.1 + macOS module, Apache-2.0 | H.264, virtual monitor, multi-monitor, clipboard, file transfer. No audio. |
| RDPonMAC | xrdp-based, early | 1080p/5fps cap. |
| FreeRDP shadow server | Mac subsystem | Basic mirroring. |
| Jump Desktop (Fluid) | Proprietary | 120-480 Hz virtual displays, adaptive. No USB. |
| Parsec | Proprietary | 4:4:4 only on Windows hosts. |
| Sunshine/Moonlight | OSS game streaming | HDR/4:4:4 not on macOS hosts. |
| NoMachine | Proprietary | USB forwarding Windows->Mac unreliable. |
| HP Anyware (PCoIP Ultra) | Enterprise | Lossless color on macOS agent. |
| Amazon DCV | Proprietary | macOS server only on EC2 Mac. |
| Apple Screen Sharing HP mode | Apple | Mac clients only. |

## Protocol ceiling with stock clients
Not achievable over RDP: HDR/EDR, reliable 120 Hz+, ICC sync, QUIC, raw precision-touchpad gestures, Touch ID, Bluetooth passthrough.
Partial substitute: WebAuthn redirection (Windows Hello / security key approving Mac passkey prompts) — later phase.
