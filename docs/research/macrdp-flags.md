| Flag | Default | One-line purpose |
| :--- | :--- | :--- |
| `--bind` | `0.0.0.0:3390` | Listen address |
| `--username` | `$USER` | Default username |
| `--password` | N/A | Avoid interactive prompt (logs are warned) |
| `--skip-auth` | OFF | Bypass PAM and password validation |
| `--width` | N/A | Override autodetected display size |
| `--height` | N/A | Override autodetected display size |
| `--hidpi` | OFF | Capture primary display at backing (Retina) pixels instead of logical points |
| `--fps` | 60 or 15 | Frame rate (depends on `--enable-h264`) |
| `--cursor-scale` | 1.0 | Pointer size multiplier |
| `--keyboard-layout` | Auto-detect | Force a non-US layout instead of auto-detecting from the client |
| `--map-ctrl-to-cmd` | OFF | Remap Windows editing shortcuts to their Cmd equivalents |
| `--no-remap-apps` | N/A | List bundle ids where --map-ctrl-to-cmd is suppressed |
| `--no-client-resolution` | OFF | Don't adopt the resolution the client requests at connect |
| `--stretch` | OFF | Fill the client frame instead of using aspect-preserving letterbox/pillarbox |
| `--max-client-size` | N/A | Cap the resolution a client can request on the auto-adopt path |
| `--virtual-display` | OFF | Create a virtual display for the session (headless; local screen untouched) |
| `--detach-primary` | OFF | Headless mode: detach the physical primary display (needs --virtual-display) |
| `--capture-primary` | OFF | Headless mode: capture the primary display (needs --virtual-display) |
| `--shield-primary` | OFF | Use third headless blanking mechanism with opaque BLACK WINDOW drawn by macrdpshield helper |
| `--restore-windows-on-disconnect` | OFF | Make windows follow you between local and remote virtual display |
| `--lock-on-disconnect` | OFF (EXPERIMENTAL) | Lock the local macOS session when the last RDP client disconnects |
| `--auto-unlock` | OFF (EXPERIMENTAL) | Try to unlock the locked local screen when an RDP client reconnects |
| `--enable-h264` | OFF | Stream H.264 over EGFX instead of legacy bitmaps |
| `--bitrate` | 6 | H.264 bitrate ceiling in Mbps |
| `--keyframe-interval` | 2 | Periodic IDR safety net seconds |
| `--flush-frames` | 4 | Trailing skip-P-frames re-sent after each change |
| `--enable-aac` | OFF | Compress RDPSND audio as AAC-LC instead of raw PCM |
| `--aac-bitrate` | 128000 | AAC target bitrate |
| `--enable-drive-redirection` | OFF | Enable RDPDR drive redirection (opt-in) |
| `--enable-smartcard-redirection` | OFF | Enable RDPDR smart-card redirection (opt-in) |
| `--enable-usb-redirection` | OFF (EXPERIMENTAL) | Generic USB redirection (opt-in) |
| `--enable-camera-redirection` | OFF (EXPERIMENTAL) | Camera redirection as a REAL macOS camera (opt-in) |
| `--no-lazy-paste` | OFF | Opt out of lazy Windows→Mac file paste |
| `--enable-udp-multitransport` | OFF (EXPERIMENTAL) | Enable RDP UDP multitransport over reliable RDPEUDP |
| `--udp-migrate-egfx` | OFF (EXPERIMENTAL) | Migrate the EGFX (H.264) channel onto the reliable UDP tunnel |
| `--adaptive-bitrate` | OFF | Congestion-responsive H.264 rate control on both UDP and TCP paths |
| `--enable-lossy-audio` | OFF (EXPERIMENTAL) | Stream RDPSND audio over a LOSSY UDP/DTLS tunnel instead of TCP |
| `--cert-dir` | `~/Library/Application Support/macrdp` | Directory for TLS certificate |
| `--cert` | N/A | Operator-supplied TLS certificate (PEM; leaf first, then any intermediate chain) |
| `--key` | N/A | Private key (PEM) for --cert |
| `--log-dir` | `~/Library/Logs` (or stdout when interactive) | Directory for the rotating log file |
| `--audit-file` | OFF | Write security AUDIT events to PATH as JSON per line |
| `--stats-endpoint` | OFF | Expose a loopback READ-ONLY live-telemetry endpoint |
