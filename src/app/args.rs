//! Command-line arguments, the `config.env` translation, and the negotiated
//! defaults applied on top of them.

use super::*;

#[derive(Parser, Debug)]
#[command(name = "portico", about = "Portico: native RDP server for macOS")]
pub(super) struct Args {
    /// Address to bind. Defaults to loopback only — pass `0.0.0.0:3390`
    /// explicitly to accept LAN connections. Port 3389 (the standard RDP
    /// port) is privileged; 3390 is the unprivileged dev default.
    #[arg(long, default_value = "127.0.0.1:3390")]
    pub(super) bind: SocketAddr,

    /// Desktop width in pixels. Defaults to the primary display's native width
    /// (queried via ScreenCaptureKit). Overriding to a non-native size makes
    /// SCK scale internally; we then disable dirty-rect updates and ship a
    /// full frame every tick, so bandwidth is higher than at native size.
    #[arg(long)]
    pub(super) width: Option<u16>,

    /// Desktop height in pixels. Defaults to the primary display's native height.
    /// Same trade-off as --width when overridden.
    #[arg(long)]
    pub(super) height: Option<u16>,

    /// Capture the primary display at its backing (Retina) pixel resolution
    /// instead of logical points — e.g. 3024×1964 instead of 1512×982 — so
    /// clients render crisp native pixels instead of upscaling a point-density
    /// frame. Off by default: it's ~4× the pixels (heavier on legacy/slow
    /// links), and the win is biggest with --enable-h264 (compresses cleanly,
    /// the client downscales sharply). Ignored when --width/--height are set or
    /// with --virtual-display (already an explicit resolution). macOS-only.
    #[arg(long)]
    pub(super) hidpi: bool,

    /// Frame rate cap. Defaults to 15 for the legacy bitmap path, or 60 with
    /// --enable-h264 (H.264 over mstsc holds a ~2-frame presentation buffer, so
    /// 60fps keeps that buffer's wall-clock latency low enough that typing feels
    /// immediate — at 30fps it lags ~2 keystrokes). Set explicitly to override.
    #[arg(long)]
    pub(super) fps: Option<u32>,

    /// Cursor size multiplier (default 1.0). At 1.0 the pointer is forwarded at
    /// its native macOS size, matching the cursor on the Mac's own screen with
    /// an exact hotspot. Some RDP clients draw the pointer at 1:1 device pixels
    /// while upscaling the desktop image to their window, which can make the
    /// native-size pointer look small; bump this (e.g. 1.5 or 2.0) to enlarge
    /// it for comfort. The hotspot stays accurate at any value.
    #[arg(long, default_value_t = 1.0)]
    pub(super) cursor_scale: f64,

    /// Mac account the client authenticates as. Defaults to $USER. The
    /// password is validated against the local account via PAM (checkpw
    /// service) at startup, so this must be a real Mac user.
    #[arg(long)]
    pub(super) username: Option<String>,

    /// Password to use without an interactive prompt. Discouraged and kept only
    /// for compatibility / scripted tests: a command-line password is visible to
    /// any local user via `ps` and may be saved in shell history. Prefer
    /// `--keychain` (headless) or leaving this unset to enter it at the prompt.
    #[arg(long)]
    pub(super) password: Option<String>,

    /// Skip the PAM check and use the supplied --password verbatim. Useful
    /// for non-macOS dev or scripted tests; never use on a shared network.
    #[arg(long)]
    pub(super) skip_auth: bool,

    /// Read the password from the macOS Keychain instead of prompting.
    /// Expects a generic-password entry: service `portico`, account = the
    /// resolved username. Create it once with:
    ///   security add-generic-password -s portico -a $USER -w
    /// Lets launchd start macrdp without an interactive terminal.
    #[arg(long)]
    pub(super) keychain: bool,

    /// Directory holding cert.pem / key.pem. Generated on first run and
    /// reused thereafter so clients see a stable fingerprint across restarts.
    /// Defaults to ~/Library/Application Support/Portico.
    #[arg(long)]
    pub(super) cert_dir: Option<PathBuf>,

    /// Operator-supplied TLS certificate (PEM; leaf first, then any chain).
    /// Use this to serve a real CA / ACME / Let's Encrypt cert instead of the
    /// self-signed default. Must be given together with --key; when set, macrdp
    /// uses exactly these files and NEVER falls back to self-signed (a missing
    /// file is a hard error). Default (neither flag): auto self-signed in
    /// --cert-dir. A cert change needs a restart.
    #[arg(long)]
    pub(super) cert: Option<PathBuf>,

    /// Operator-supplied TLS private key (PEM) for --cert. Must be chmod 600 and
    /// readable by the macrdp user. Required together with --cert.
    #[arg(long)]
    pub(super) key: Option<PathBuf>,

    /// Directory for the rotating log file (`portico.log`, size-bounded via
    /// MACRDP_LOG_MAX_BYTES / MACRDP_LOG_MAX_FILES). If unset, logs go to a
    /// rotating file in ~/Library/Logs when running headless (stdout is not a
    /// TTY, e.g. under the LaunchAgent) and to stdout when interactive.
    #[arg(long)]
    pub(super) log_dir: Option<PathBuf>,

    /// Additionally write the security **audit** events (connection
    /// accept/reject/disconnect) to this file as one JSON object per line, for a
    /// SIEM/SOC log collector (Vector / Fluent Bit / rsyslog / Splunk UF) to tail
    /// and forward. Off by default; the human-readable audit lines still appear in
    /// the main log. The file self-rotates (MACRDP_AUDIT_LOG_MAX_BYTES /
    /// MACRDP_AUDIT_LOG_MAX_FILES). Setting MACRDP_AUDIT_JSON=1 enables it at the
    /// default path `<log-dir>/portico-audit.log` without naming a file here. See
    /// docs/siem-forwarding.md. Config key: AUDIT_FILE.
    #[arg(long)]
    pub(super) audit_file: Option<PathBuf>,

    /// Show debug-level logs from macrdp and the underlying ironrdp / rustls
    /// crates. Without this, known-noisy lines (TLS SNI warnings, the
    /// "Encoding bitmap took N ms" timer, the cert-acceptance "Broken pipe"
    /// from ironrdp_server) are suppressed.
    #[arg(long, short = 'v')]
    pub(super) verbose: bool,

    /// Don't prevent the Mac from going to sleep / auto-locking while macrdp
    /// is running. By default we spawn `caffeinate` so an idle Mac doesn't
    /// tear down the connection mid-session. Pass this to keep normal power
    /// management on.
    #[arg(long)]
    pub(super) allow_sleep: bool,

    /// Attach a headless virtual display at --width × --height and serve
    /// THAT to the RDP client instead of mirroring the user's primary
    /// panel. Behaves like plugging in an external monitor — the local
    /// screen stays untouched and you can keep working on the Mac
    /// locally while the remote session uses its own desktop.
    ///
    /// Requires --width and --height (the virtual display has no
    /// "native" size to fall back to). Backed by undocumented
    /// CoreGraphics private API (CGVirtualDisplay*); may break on
    /// future macOS releases.
    #[arg(long)]
    pub(super) virtual_display: bool,

    /// Promote the virtual display to be the system's primary display
    /// for the duration of the run — the one with the menu bar, where
    /// new app windows open. Use this when you want to drive the
    /// remote desktop as your "main" workspace (e.g. headless remote
    /// use). Restored on clean exit; the underlying CGConfigure call is
    /// session-scoped so the layout also reverts on user logout if
    /// macrdp exits uncleanly (signal, crash). Only valid with
    /// --virtual-display.
    #[arg(long)]
    pub(super) make_primary: bool,

    /// While an RDP client is connected, disable every physical display
    /// (built-in panel + any external monitors) so the Mac is headless
    /// via the virtual display alone: backlights off, no menu bar, no
    /// windows can be placed there, cursor can't cross onto them. As
    /// soon as the last client disconnects, the displays come back and
    /// the local workspace is restored. Also restored on clean exit
    /// and — even if Drop doesn't run (signal, crash) — at the next
    /// user logout via the session-scoped CGConfigure. Only valid with
    /// --virtual-display.
    #[arg(long)]
    pub(super) detach_primary: bool,

    /// Alternative to --detach-primary: while a client is connected,
    /// take **exclusive capture** of every physical display via
    /// CGDisplayCapture. The panels go solid black (backlight stays
    /// on) and the cursor can't be moved onto them. Drops the capture
    /// on last disconnect. Captures are process-scoped, so a signal /
    /// crash auto-releases — no logout dance. Use this when
    /// --detach-primary doesn't actually go headless on your machine
    /// (e.g. the disable transaction succeeds but the panel keeps
    /// showing the desktop). Mutually exclusive with --detach-primary.
    /// Only valid with --virtual-display.
    #[arg(long)]
    pub(super) capture_primary: bool,

    /// Third headless mechanism, alternative to --detach-primary and
    /// --capture-primary: while a client is connected, cover every physical
    /// display with an opaque **black shield window** (drawn by the bundled
    /// macrdpshield helper). Two advantages over --capture-primary: (1) the
    /// Mac **can still be locked** — capture-primary silently prevents
    /// locking, because loginwindow cannot draw onto a captured display; and
    /// (2) no ~250 ms desktop flash when the client resizes, because a window
    /// survives a display reconfiguration whereas a gamma LUT is reset by it.
    /// The trade-off: without the capture the local pointer is NOT confined,
    /// so someone at the machine can move it and disturb the remote cursor
    /// (their clicks are swallowed by the shield). Mutually exclusive with
    /// --detach-primary and --capture-primary. Only valid with
    /// --virtual-display.
    #[arg(long)]
    pub(super) shield_primary: bool,

    /// Make windows follow you between the local built-in screen and the
    /// remote virtual display (opt-in; only meaningful with
    /// --detach-primary/--capture-primary). By default the virtual display
    /// is process-lifetime, so on disconnect its windows stay stranded on
    /// the (now off-screen) virtual display — invisible on a laptop's
    /// built-in panel until you reconnect. With this flag, the last-client
    /// disconnect sweeps those windows back onto the built-in display (so
    /// the Mac is usable locally), and a reconnect auto-gathers them onto
    /// the virtual display the client sees (so you don't need Ctrl+Alt+G).
    /// Reuses the same window-gather machinery as the Ctrl+Alt+G hotkey.
    #[arg(long)]
    pub(super) restore_windows_on_disconnect: bool,

    /// Lock the local macOS session when the last RDP client genuinely
    /// disconnects (opt-in; only meaningful with --detach-primary/
    /// --capture-primary/--shield-primary). Fires via `open
    /// ScreenSaverEngine.app` after the existing disconnect-confirmation
    /// grace PLUS an additional tunable safety buffer
    /// (MACRDP_LOCK_ON_DISCONNECT_DELAY_MS / config
    /// LOCK_ON_DISCONNECT_DELAY_MS, default ~25s total) to reduce — not
    /// eliminate — the chance of locking mid a blank-recovery self-heal
    /// reconnect. Only an actual password-required lock if the account has
    /// "require password" set to Immediately; otherwise this just starts
    /// the screensaver. Heuristic, not a guarantee; see
    /// docs/known-quirks.md. Never fires on server shutdown/kill, only on a
    /// genuine watcher-observed last-client-disconnect. macOS-only.
    #[arg(long)]
    pub(super) lock_on_disconnect: bool,

    /// EXPERIMENTAL, opt-in. Type the account password into the lock screen
    /// when an RDP client connects while the local session is locked — any
    /// lock, any cause (a --lock-on-disconnect lock, one set manually, or
    /// macOS's own idle policy), not just one this project set. Off unless
    /// passed (config AUTO_UNLOCK=1); a no-op when the screen isn't locked,
    /// and skipped entirely under --skip-auth (where the password was never
    /// PAM-validated). Off by default because the effect lands on the
    /// PHYSICAL machine: once it fires, anyone standing at that Mac has a
    /// live desktop, and it undoes a lock it did not set — someone may have
    /// locked it deliberately. Reuses the same credential PAM already
    /// validated at startup, so no fresh keychain read. Verified on one
    /// machine, one macOS version, one keyboard layout; rests on typing
    /// real keycode events into a secure field, a private lock-state check,
    /// and a behavior (synthetic input reaching the lock screen) Apple could
    /// change. See docs/known-quirks.md. macOS-only.
    #[arg(long)]
    pub(super) auto_unlock: bool,

    /// Expose a loopback (127.0.0.1) read-only live-telemetry endpoint for the
    /// menu-bar controller's Status pane (bitrate/RTT/fps). Default off. No disk
    /// writes. macOS-only concern but cross-platform code.
    #[arg(long)]
    pub(super) stats_endpoint: bool,

    /// Serve the display as H.264 video over the EGFX virtual channel
    /// (MS-RDPEGFX, AVC420) instead of legacy RemoteFx/QOI BitmapUpdates.
    /// Hardware-encoded via VideoToolbox. Falls back to the legacy path
    /// automatically for clients that don't negotiate EGFX. Experimental;
    /// macOS-only.
    #[arg(long)]
    pub(super) enable_h264: bool,

    /// Target H.264 bitrate in megabits/sec (only with --enable-h264).
    /// Default 6. Raising it sharpens detail at the cost of bigger per-frame
    /// writes, which can fill the socket buffer and delay audio on a
    /// constrained link (e.g. Wi-Fi); try 8–12 if you have headroom.
    #[arg(long, default_value_t = 6)]
    pub(super) bitrate: u32,

    /// H.264 periodic keyframe (IDR) interval in seconds (only with
    /// --enable-h264). Default 2. This is a safety net for transient decode
    /// glitches on small changes (e.g. mstsc's lingering garbled text while
    /// typing); large changes (window-to-front, scroll) can also force an
    /// immediate IDR if --keyframe-on-change is set (off by default). Lower self-heals
    /// faster but frequent IDRs cost bandwidth/quality at a fixed bitrate and
    /// can stutter. Fractional values are allowed. First frame is a keyframe.
    #[arg(long, default_value_t = 2.0)]
    pub(super) keyframe_interval: f32,

    /// Force on-change H.264 keyframes (only with --enable-h264). OFF by
    /// default. When enabled, a keyframe (IDR) is forced when a lot of the
    /// screen changes at once — a window raised to front, a scroll, an app
    /// launch — and briefly after a mouse click, so large updates render
    /// immediately on clients (e.g. mstsc) that apply big P-frames cleanly only
    /// on a keyframe. Left off (the default), the periodic --keyframe-interval
    /// safety net plus the trailing flush-burst (--flush-frames) already drain
    /// mstsc's presentation buffer, so the extra forced IDRs mostly just spend
    /// bitrate/quality at a fixed bitrate for no typing benefit; enable it only
    /// if large updates visibly lag on your client/link.
    #[arg(long = "keyframe-on-change", action = clap::ArgAction::SetTrue)]
    pub(super) keyframe_on_change: bool,

    /// Deprecated/no-op: on-change keyframes are now OFF by default, so this is
    /// already the default. Accepted so existing command lines that pass
    /// --no-keyframe-on-change keep working; use --keyframe-on-change to enable.
    #[arg(long = "no-keyframe-on-change", action = clap::ArgAction::SetTrue, hide = true)]
    #[allow(dead_code)]
    pub(super) no_keyframe_on_change_compat: bool,

    /// Dirty-area threshold (percent of the frame) that triggers an on-change
    /// keyframe (only with --enable-h264 + on-change keyframes). Lower catches
    /// smaller updates (more IDRs); higher is more conservative. Default 20.
    #[arg(long, default_value_t = 20)]
    pub(super) keyframe_change_pct: u64,

    /// Lowered dirty-area threshold (percent) used briefly after a mouse click,
    /// to catch moderate click-driven updates (dropdowns, dialogs). Default 5.
    #[arg(long, default_value_t = 5)]
    pub(super) keyframe_click_pct: u64,

    /// How long (milliseconds) after a click the --keyframe-click-pct threshold
    /// applies. Default 400.
    #[arg(long, default_value_t = 400)]
    pub(super) keyframe_click_window_ms: u64,

    /// Max H.264 frames in the encode/ship pipeline before the capture loop
    /// drops to the latest frame (only with --enable-h264). Bounds interactive
    /// latency under sustained load: lower drops-to-latest sooner (snappier
    /// typing/window-switching during bursts) at the cost of more frame-skips;
    /// higher buffers more (smoother video) but lets a backlog build up. This
    /// throttles at capture, before encoding — encoded frames are never dropped
    /// (that would break the H.264 reference chain). Default 2; raise to 3–4 if
    /// video looks too skippy under heavy motion.
    #[arg(long, default_value_t = 2)]
    pub(super) h264_frames_in_flight: u32,

    /// Number of trailing "flush" frames re-sent after the last on-screen change
    /// (only with --enable-h264). ScreenCaptureKit stops delivering frames on a
    /// static screen, so the last change before a pause (e.g. the final
    /// keystroke) would otherwise sit in mstsc's ~2-frame AVC420 presentation
    /// buffer until the next change or periodic keyframe — the "typing follows
    /// the keyframe" lag. After each change we re-submit the last frame this many
    /// times as cheap skip-P-frames to drain that buffer so the change appears
    /// promptly. mstsc needs ≥2; default 4 gives margin. Raise if a slight
    /// trailing lag remains; set 0 to disable the flush burst entirely.
    #[arg(long, default_value_t = 4)]
    pub(super) flush_frames: u32,

    /// Disable lazy Windows→Mac file paste. Lazy paste is ON by default:
    /// temp files are pre-sized but empty when the Windows copy lands,
    /// and bytes stream only when the user actually pastes in Finder
    /// (macOS shows its native "Preparing to paste" progress dialog).
    /// Handles single files AND folder trees. Uses fewer parallel chunk
    /// requests than the eager path so the RDP session stays responsive
    /// during the on-paste download. Pass this to fall back to the eager
    /// path (downloads every file the moment Windows announces the copy,
    /// then auto-fires Cmd-V into Finder).
    #[cfg(target_os = "macos")]
    #[arg(long = "no-lazy-paste", action = clap::ArgAction::SetTrue)]
    pub(super) no_lazy_paste: bool,

    /// Copy plain text and images only, without rich text. By default macrdp
    /// also carries formatted text across the clipboard in both directions —
    /// the Windows `HTML Format` / `Rich Text Format` clipboard formats mapped
    /// to the Mac's `public.html` / `public.rtf` — so formatting survives a
    /// copy from Word, Outlook or a browser into Mail, Notes or Pages, and back.
    /// A Windows→Mac copy then fetches the formatted version as well as the
    /// plain text, which is extra traffic on every copy; pass this if that
    /// matters on a thin link. Images embedded in rich text generally don't
    /// survive either way (Word's HTML points at local temp files). Config
    /// key: RICH_CLIPBOARD=0.
    #[arg(long = "no-rich-clipboard", action = clap::ArgAction::SetTrue)]
    pub(super) no_rich_clipboard: bool,

    /// Deprecated/no-op: lazy paste is now the default. Accepted so
    /// existing command lines that pass --lazy-paste keep working; use
    /// --no-lazy-paste to disable.
    #[cfg(target_os = "macos")]
    #[arg(long = "lazy-paste", action = clap::ArgAction::SetTrue, hide = true)]
    #[allow(dead_code)]
    pub(super) lazy_paste_compat: bool,

    /// Default-on. While the client has sent `SuppressOutput { None }`
    /// (i.e., mstsc is minimized), stop emitting RDPSND wave PDUs at the
    /// server so the client's audio renderer drains naturally. Without
    /// this, mstsc keeps queueing waves into `audiodg.exe`'s buffer
    /// during the minimize; on refocus that buffer plays out late and
    /// audio drifts by however many seconds were spent minimized. Pass
    /// `--no-mute-on-minimize` to keep audio flowing through a minimize
    /// (preserves "minimized YouTube keeps playing on the Mac speakers")
    /// at the cost of accepting that drift on refocus.
    #[arg(long = "no-mute-on-minimize", action = clap::ArgAction::SetTrue)]
    pub(super) no_mute_on_minimize: bool,

    /// Compress forwarded audio as AAC-LC over RDPSND (MS-RDPEA
    /// WAVE_FORMAT_AAC_MS) instead of raw 16-bit PCM — ~11x less audio
    /// bandwidth (~128 kbps vs ~1.4 Mbps). Off by default: AAC adds ~40–50 ms
    /// of encoder priming latency, so PCM stays the zero-latency LAN default.
    /// Clients that don't advertise AAC decode fall back to PCM automatically.
    /// AudioToolbox-encoded; macOS-only.
    #[arg(long)]
    pub(super) enable_aac: bool,

    /// Target AAC bitrate in bits/sec (only with --enable-aac). Default
    /// 128000. 96000 maximizes savings (audible artifacts on music); 192000
    /// is near-transparent. Advertised to the client and used to configure
    /// the encoder.
    #[arg(long, default_value_t = 128_000)]
    pub(super) aac_bitrate: u32,

    /// Enable RDPDR drive redirection (MS-RDPEFS): the connecting client
    /// redirects its local drive(s) and the Mac mounts each as a real
    /// read-write NFS volume (an in-process NFSv3 server + the built-in
    /// mount_nfs; no root/kext/FUSE), browsable in Finder. Off by default. The
    /// client must opt in too (mstsc: Local Resources → Drives; FreeRDP:
    /// /drive:NAME,PATH). macOS-only.
    #[arg(long)]
    pub(super) enable_drive_redirection: bool,

    /// Enable RDPDR smart-card redirection (MS-RDPESC): the connecting client
    /// redirects its smart-card reader and macOS apps can use the card through
    /// it. Requires macrdp's PC/SC IFD handler bundle to be installed
    /// (`ifd-macrdp.bundle` in /usr/local/libexec/SmartCardServices/drivers) and
    /// a USB device present to trigger its load. Off by default. The client must
    /// opt in too (mstsc: Local Resources → More → Smart cards; FreeRDP:
    /// /smartcard). macOS-only.
    #[arg(long)]
    pub(super) enable_smartcard_redirection: bool,

    /// EXPERIMENTAL, opt-in (default OFF). Generic USB redirection (MS-RDPEUSB /
    /// the URBDRC dynamic channel): the connecting client redirects a physical USB
    /// device and macrdp presents it as a REAL local device via a user-space virtual
    /// USB host controller (IOUSBHost UserHCI) — e.g. a redirected flash drive mounts
    /// in Finder. Needs the entitled (signed+provisioned) build (the
    /// com.apple.developer.usb.host-controller-interface entitlement); a plain build
    /// logs "controller unavailable" and no-ops. Mass storage is verified end-to-end;
    /// other device classes are untested. The client must opt in too (FreeRDP: /usb).
    /// mstsc gates USB behind Group Policy: enable "Allow RDP redirection of other
    /// supported RemoteFX USB devices from this computer" (Computer Config -> Admin
    /// Templates -> Windows Components -> Remote Desktop Services -> Remote Desktop
    /// Connection Client -> RemoteFX USB Device Redirection) + reboot, then select the
    /// device under Local Resources -> More -> USB. On mstsc a device now enumerates,
    /// configures, and negotiates its format, but the client doesn't deliver the actual
    /// video/data frames (a webcam's video rides mstsc's separate camera-redirection
    /// channel, not implemented). macOS-only.
    #[arg(long)]
    pub(super) enable_usb_redirection: bool,

    /// EXPERIMENTAL, opt-in (default OFF). Camera redirection (MS-RDPECAM) —
    /// **Phase 0 protocol gate only**. Advertises the `RDCamera_Device_Enumerator`
    /// DVC and logs the client's camera announcement (`DEVICE_ADDED_NOTIFICATION`)
    /// so we can confirm a modern mstsc/Win11 will hand macrdp a redirected webcam
    /// over MS-RDPECAM. It does NOT present a camera yet (no per-device channel, no
    /// stream, no macOS code). The client must opt in too (mstsc: Local Resources ->
    /// More -> "Video capture devices"). Cross-platform (pure protocol). See
    /// docs/rdp-camera-redirection-feasibility.md.
    #[arg(long)]
    pub(super) enable_camera_redirection: bool,

    /// EXPERIMENTAL, opt-in (default OFF). Offer RDP UDP multitransport
    /// (MS-RDPEMT over reliable RDPEUDP) to clients that advertise it, and bind a
    /// UDP listener on the same address/port as TCP. On its own, EGFX stays on TCP
    /// (the proven safe spike) — pass --udp-migrate-egfx to actually move the
    /// H.264 video onto the reliable UDP tunnel. Input, audio (RDPSND), and
    /// clipboard always ride TCP. macOS-only build; see
    /// docs/rdp-udp-multitransport-feasibility.md.
    #[arg(long)]
    pub(super) enable_udp_multitransport: bool,

    /// EXPERIMENTAL, opt-in (default OFF; requires --enable-udp-multitransport).
    /// Migrate the EGFX (H.264) channel onto the reliable UDP tunnel via MS-RDPEDYC
    /// Soft-Sync (verified rendering on mstsc). Without it, EGFX stays on TCP even
    /// when multitransport is offered. **Caveat — clean-link feature only:** the
    /// reliable tunnel is an ordered stream, so under packet loss it head-of-line-
    /// blocks like TCP and, once the client abandons the tunnel, EGFX freezes with
    /// no recovery until reconnect (audio survives on TCP). Use only on a low-loss
    /// link. (Promoted from the MACRDP_UDP_MIGRATE_EGFX env var, which still works
    /// as a fallback.) macOS-only build; see docs/rdp-udp-multitransport-feasibility.md.
    #[arg(long)]
    pub(super) udp_migrate_egfx: bool,

    /// EXPERIMENTAL, opt-in (default OFF). Congestion-responsive H.264 bitrate for
    /// EGFX-over-UDP: when the reliable UDP tunnel shows packet loss (retransmits),
    /// lower the VideoToolbox bitrate toward a floor (AIMD multiplicative-decrease),
    /// and climb back toward the --bitrate ceiling when the link clears — so video
    /// degrades to "choppy but alive" under loss instead of wedging. Only acts while
    /// EGFX is on a UDP tunnel (no-op on TCP). Tunables: MACRDP_UDP_ADAPTIVE_FLOOR_BPS,
    /// _INCREASE_BPS, _DECREASE, _INTERVAL_MS. macOS-only build.
    #[arg(long)]
    pub(super) adaptive_bitrate: bool,

    /// EXPERIMENTAL, opt-in (default OFF; implies --enable-udp-multitransport,
    /// requires --enable-aac and --enable-h264). Stream RDPSND audio over a LOSSY
    /// UDP/DTLS tunnel with 1+1 redundancy instead of TCP — the loss-resilient audio
    /// path. The MS-RDPEA format handshake runs on a reliable DVC over TCP; AAC Wave2
    /// data is Soft-Synced onto a lossy (UdpFecL) RDPEUDP flow (deliver-on-arrival, no
    /// retransmit) and each datagram is sent TWICE (the client's DTLS anti-replay
    /// dedups, so audio never double-plays), so an independent-loss link of rate p
    /// drops a payload only at p² — verified smooth on real mstsc at 5/10/15% loss
    /// where the single-send path glitches. Collapses the
    /// MACRDP_UDP_{OFFER_FECL,LOSSY_DELIVERY,LOSSY_AUDIO,LOSSY_AUDIO_DUP} env gates
    /// (which still work as a fallback). macOS-only build; see
    /// docs/rdp-udp-multitransport-feasibility.md.
    #[arg(long)]
    pub(super) enable_lossy_audio: bool,

    /// Don't adopt the client's requested desktop resolution. By default
    /// macrdp reads the resolution the client asked for while connecting
    /// (e.g. mstsc full-screen on a 1920×1080 monitor) and serves the session
    /// at exactly that size, so the client presents the video 1:1 instead of
    /// rescaling it (client-side rescaling on mstsc costs typing latency and,
    /// with --enable-h264, contributes to audio drift). Applies when mirroring
    /// the primary display without --width/--height/--hidpi, and on
    /// --virtual-display (where the virtual display is re-moded to the
    /// client's size — --width/--height are its initial size, not a pin).
    /// Pass this to always serve the size resolved at startup (the Mac
    /// display's native size, or the --width/--height virtual display size)
    /// and let the client scale.
    #[arg(long = "no-client-resolution", action = clap::ArgAction::SetTrue)]
    pub(super) no_client_resolution: bool,

    /// On the auto-size path, stretch the Mac screen to fill the client frame
    /// instead of preserving the Mac's aspect ratio. By default, when the client
    /// requests a resolution with a different aspect ratio than the Mac display,
    /// macrdp letterboxes/pillarboxes (keeps the picture undistorted, adds black
    /// bars) and maps mouse input into the centered content area. Pass this to
    /// get the old fill-and-distort behavior (no bars). No effect with
    /// --width/--height (those already stretch) or at a matching aspect ratio.
    #[arg(long)]
    pub(super) stretch: bool,

    /// Cap the resolution a client can request on the auto-adopt path
    /// (defense-in-depth resource bound; e.g. 2560x1440). A request above the
    /// cap is clamped per-dimension and the session is served at the clamped
    /// size. Without it, an authenticated client can request up to the
    /// protocol maximum 8192x8192 — a ~256 MB BGRA framebuffer per frame.
    /// Each dimension must be in the RDP band [200, 8192]. No effect with
    /// --no-client-resolution, or with an explicit --width/--height/--hidpi
    /// on the mirror-primary path (those pin the size; --virtual-display
    /// adopts, so the cap applies there). Config: MAX_CLIENT_SIZE.
    #[arg(long = "max-client-size", value_name = "WxH", value_parser = parse_max_client_size)]
    pub(super) max_client_size: Option<(u16, u16)>,

    /// On Cmd+Tab, un-minimize the target app's window (bring it back from the
    /// Dock) instead of just activating the app. Off by default, which matches
    /// native macOS — Cmd+Tab activates a minimized app but doesn't restore its
    /// window. macOS-only.
    #[arg(long = "unminimize-on-switch")]
    pub(super) unminimize_on_switch: bool,

    /// Also accept Option+Tab (Alt+Tab from the client) as a trigger for the app
    /// switcher, in addition to Cmd+Tab. Off by default. Useful for clients/
    /// configs that forward Alt+Tab to the session but gate Win+Tab (e.g. mstsc's
    /// "Apply Windows key combinations" when windowed). Same MRU cycle, committing
    /// on Option release; Option+Shift+Tab cycles backward. When off, Option+Tab
    /// reaches remote apps as a normal key. macOS-only.
    #[arg(long = "alt-tab-switch")]
    pub(super) alt_tab_switch: bool,

    /// Also accept Option+` (Alt+backtick from the client) as a trigger for the
    /// within-app window cycle, in addition to Cmd+`. Off by default. Mirrors
    /// --alt-tab-switch's relationship to Cmd+Tab, for clients/configs that
    /// forward Alt+` but gate the Windows-key combo. Option+Shift+` cycles
    /// backward. When off, Option+` reaches remote apps as a normal key.
    /// macOS-only.
    #[arg(long = "alt-backtick-switch")]
    pub(super) alt_backtick_switch: bool,

    /// Show a visual app-switcher overlay (icon row) on the remote during Cmd+Tab
    /// / Option+Tab, like macOS's native switcher. Off by default. macrdp spawns a
    /// small helper process that draws a real on-screen panel; ScreenCaptureKit
    /// captures it, so the client sees it. The switch behaves identically with or
    /// without this — it only adds the HUD. macOS-only.
    #[arg(long = "app-switcher-hud")]
    pub(super) app_switcher_hud: bool,

    /// Remap a curated set of Windows editing shortcuts from Ctrl to Cmd so
    /// Windows muscle memory drives macOS: Ctrl+C/V/X/A/Z/S/F/N/T/W/O/P/R/G (and
    /// Shift variants, e.g. Ctrl+Shift+Z → Cmd+Shift+Z redo) fire as the Cmd
    /// equivalent. Off by default (then Ctrl reaches remote apps unchanged, so
    /// macOS shortcuts need Cmd, i.e. the client's Windows/Super key). Cmd+Q is
    /// deliberately NOT produced (Q is excluded); nav keys are untouched. Always
    /// suppressed when a terminal is frontmost so Ctrl+C stays SIGINT. macOS-only.
    #[arg(long = "map-ctrl-to-cmd")]
    pub(super) map_ctrl_to_cmd: bool,

    /// Bundle ids (comma-separated) where --map-ctrl-to-cmd is suppressed, in
    /// addition to the built-in terminal list. Use this for apps with an embedded
    /// terminal that can't be auto-detected (the frontmost app is the IDE, not a
    /// TTY), e.g. `--no-remap-apps com.microsoft.VSCode`. In a listed app, Ctrl
    /// stays Ctrl (its integrated-terminal Ctrl+C = SIGINT works; editor copy is
    /// Cmd+C, the app's native macOS copy). No effect without --map-ctrl-to-cmd.
    #[arg(long = "no-remap-apps", value_delimiter = ',')]
    pub(super) no_remap_apps: Vec<String>,

    /// Keyboard layout to interpret the client's keystrokes as, for non-US
    /// clients. By default the layout is **auto-detected** from the client's
    /// announced KLID (US/unknown keep the plain positional-keycode path, so
    /// the US majority is unaffected); pass this only to force a specific
    /// layout, or `none`/`off` to disable translation entirely. macrdp
    /// translates each key against the layout via `UCKeyTranslate` and posts
    /// the resulting character — without changing the Mac's own input source.
    /// Cmd/Ctrl shortcuts stay on the keycode path. Accepts a macOS
    /// input-source id (`com.apple.keylayout.French`), a short name (`french`,
    /// `de`, `azerty`, `swissgerman`), or a Windows KLID (`0x040C`). The layout
    /// must be installed on the Mac. macOS-only.
    #[arg(long)]
    pub(super) keyboard_layout: Option<String>,

    /// Read all settings from a `key=value` config file (the same `config.env`
    /// the menu-bar controller writes) instead of from individual flags. When
    /// present, every other flag is ignored and the effective settings come
    /// entirely from the file — this is what the LaunchAgent passes so launchd
    /// runs THIS signed binary directly (stable Background-Task-Management
    /// identity, no unsigned wrapper script). See packaging/config.env.example
    /// for the recognized keys; unknown keys are ignored.
    #[arg(long)]
    pub(super) config: Option<PathBuf>,

    /// EXPERIMENTAL research spike (not a real feature): instantiate a user-space
    /// USB host controller via IOUSBHostControllerInterface to prove the
    /// generic-USB-redirection route works, print GO/NO-GO, then exit. Requires a
    /// signed+provisioned build carrying the com.apple.developer.usb.host-controller-
    /// interface entitlement (packaging/make-app.sh PROVISION_PROFILE=...). macOS-only.
    #[arg(long = "usb-spike")]
    pub(super) usb_spike: bool,
}

/// Parse a `--max-client-size` spec ("WxH", e.g. "2560x1440") into a
/// per-dimension cap, validating each dimension against the RDP desktop band
/// [200, 8192] (MS-RDPBCGR) — the same band the acceptor's sanity check uses,
/// so a clamped size can never fall below the protocol minimum.
pub(super) fn parse_max_client_size(spec: &str) -> Result<(u16, u16), String> {
    let lower = spec.trim().to_ascii_lowercase();
    let (w, h) = lower
        .split_once('x')
        .ok_or_else(|| format!("expected WxH (e.g. 2560x1440), got '{spec}'"))?;
    let parse_dim = |s: &str, name: &str| -> Result<u16, String> {
        let v: u16 = s
            .trim()
            .parse()
            .map_err(|_| format!("{name} '{}' is not a number in 0..=65535", s.trim()))?;
        if !(200..=8192).contains(&v) {
            return Err(format!(
                "{name} {v} is outside the RDP desktop band 200..=8192 (MS-RDPBCGR)"
            ));
        }
        Ok(v)
    };
    Ok((parse_dim(w, "width")?, parse_dim(h, "height")?))
}

/// Translate a `config.env` (plain `key=value`, the format the menu-bar
/// controller writes and `packaging/config.env.example` documents) into the
/// equivalent CLI argv and parse it back into `Args`. This lets the LaunchAgent
/// launch the *signed* `macrdp` binary directly with a single `--config <file>`
/// argument — giving macOS Background Task Management a stable Developer-ID
/// identity to approve once — instead of going through an unsigned wrapper
/// script that BTM re-flags on every rebuild. Mirrors the old
/// `packaging/macrdp-launch` translation exactly (same keys, same defaults).
pub(super) fn args_from_config(path: &Path) -> Result<Args> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))?;

    let mut cfg: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Tolerate a leading `export ` (the file used to be shell-sourced).
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        let mut val = val.trim();
        // Strip one layer of matching surrounding quotes.
        for q in ['"', '\''] {
            if val.len() >= 2 && val.starts_with(q) && val.ends_with(q) {
                val = &val[1..val.len() - 1];
                break;
            }
        }
        cfg.insert(key, val.to_string());
    }

    let on = |key: &str, default: bool| cfg.get(key).map(|v| v == "1").unwrap_or(default);
    let get =
        |key: &str, default: &str| cfg.get(key).cloned().unwrap_or_else(|| default.to_string());

    let mut argv: Vec<String> = vec!["macrdp".to_string()];
    argv.push("--bind".into());
    argv.push(get("BIND", "127.0.0.1:3390"));

    // USERNAME defaults to $USER, matching the wrapper.
    let username = cfg
        .get("USERNAME")
        .cloned()
        .unwrap_or_else(|| std::env::var("USER").unwrap_or_default());
    if !username.is_empty() {
        argv.push("--username".into());
        argv.push(username);
    }

    if on("USE_KEYCHAIN", true) {
        argv.push("--keychain".into());
    }
    if on("ENABLE_H264", false) {
        argv.push("--enable-h264".into());
    }
    if on("ENABLE_AAC", false) {
        argv.push("--enable-aac".into());
    }
    if on("HIDPI", false) {
        argv.push("--hidpi".into());
    }
    if on("UNMINIMIZE", false) {
        argv.push("--unminimize-on-switch".into());
    }
    if on("ALT_TAB_SWITCH", false) {
        argv.push("--alt-tab-switch".into());
    }
    if on("ALT_BACKTICK_SWITCH", false) {
        argv.push("--alt-backtick-switch".into());
    }
    if on("APP_SWITCHER_HUD", false) {
        argv.push("--app-switcher-hud".into());
    }
    if on("RESTORE_WINDOWS_ON_DISCONNECT", false) {
        argv.push("--restore-windows-on-disconnect".into());
    }
    if on("LOCK_ON_DISCONNECT", false) {
        argv.push("--lock-on-disconnect".into());
    }
    if on("AUTO_UNLOCK", false) {
        argv.push("--auto-unlock".into());
    }
    if !on("RICH_CLIPBOARD", true) {
        argv.push("--no-rich-clipboard".into());
    }
    if on("STATS_ENDPOINT", false) {
        argv.push("--stats-endpoint".into());
    }
    if on("MAP_CTRL_TO_CMD", false) {
        argv.push("--map-ctrl-to-cmd".into());
    }
    if let Some(list) = cfg.get("NO_REMAP_APPS") {
        if !list.is_empty() {
            argv.push("--no-remap-apps".into());
            argv.push(list.clone());
        }
    }
    if on("ENABLE_DRIVE_REDIRECTION", false) {
        argv.push("--enable-drive-redirection".into());
    }
    if on("ENABLE_SMARTCARD_REDIRECTION", false) {
        argv.push("--enable-smartcard-redirection".into());
    }
    if on("ENABLE_USB_REDIRECTION", false) {
        argv.push("--enable-usb-redirection".into());
    }
    if on("ENABLE_CAMERA_REDIRECTION", false) {
        argv.push("--enable-camera-redirection".into());
    }
    if on("ENABLE_UDP_MULTITRANSPORT", false) {
        argv.push("--enable-udp-multitransport".into());
    }
    if on("UDP_MIGRATE_EGFX", false) {
        argv.push("--udp-migrate-egfx".into());
    }
    if on("ADAPTIVE_BITRATE", false) {
        argv.push("--adaptive-bitrate".into());
    }
    if on("ENABLE_LOSSY_AUDIO", false) {
        argv.push("--enable-lossy-audio".into());
    }
    if on("VIRTUAL_DISPLAY", false) {
        argv.push("--virtual-display".into());
        argv.push("--width".into());
        argv.push(get("VD_WIDTH", "1920"));
        argv.push("--height".into());
        argv.push(get("VD_HEIGHT", "1080"));
        // Back-compat: old configs set CAPTURE_PRIMARY=1 with no PRIMARY_MODE.
        let mut primary_mode = get("PRIMARY_MODE", "none");
        if primary_mode == "none" && on("CAPTURE_PRIMARY", false) {
            primary_mode = "capture".to_string();
        }
        match primary_mode.as_str() {
            "detach" => argv.push("--detach-primary".into()),
            "capture" => argv.push("--capture-primary".into()),
            "shield" => argv.push("--shield-primary".into()),
            _ => {}
        }
    }
    if let Some(layout) = cfg.get("KEYBOARD_LAYOUT") {
        if !layout.is_empty() {
            argv.push("--keyboard-layout".into());
            argv.push(layout.clone());
        }
    }
    if let Some(dir) = cfg.get("LOG_DIR") {
        if !dir.is_empty() {
            argv.push("--log-dir".into());
            argv.push(dir.clone());
        }
    }
    if let Some(path) = cfg.get("AUDIT_FILE") {
        if !path.is_empty() {
            argv.push("--audit-file".into());
            argv.push(path.clone());
        }
    }
    if let Some(path) = cfg.get("TLS_CERT") {
        if !path.is_empty() {
            argv.push("--cert".into());
            argv.push(path.clone());
        }
    }
    if let Some(path) = cfg.get("TLS_KEY") {
        if !path.is_empty() {
            argv.push("--key".into());
            argv.push(path.clone());
        }
    }
    if let Some(sz) = cfg.get("MAX_CLIENT_SIZE") {
        if !sz.is_empty() {
            argv.push("--max-client-size".into());
            argv.push(sz.clone());
        }
    }
    // Env-only tunables (auth guard, health-check watchdog, blank recovery,
    // auto-reconnect cookie — each read lazily after this point). Bridge the
    // friendly config.env keys to their MACRDP_* env vars here so menu-bar /
    // LaunchAgent users can tune them via config.env without editing the
    // plist's EnvironmentVariables. Unset keys keep the on-by-default defaults.
    for (cfg_key, env_var) in [
        ("CONN_GUARD", "MACRDP_CONN_GUARD"),
        ("AUDIT_LOG", "MACRDP_AUDIT_LOG"),
        ("GUARD_RL_MAX", "MACRDP_GUARD_RL_MAX"),
        ("GUARD_RL_WINDOW_SECS", "MACRDP_GUARD_RL_WINDOW_SECS"),
        ("GUARD_FAIL_THRESHOLD", "MACRDP_GUARD_FAIL_THRESHOLD"),
        (
            "GUARD_COOLDOWN_BASE_SECS",
            "MACRDP_GUARD_COOLDOWN_BASE_SECS",
        ),
        ("GUARD_COOLDOWN_MAX_SECS", "MACRDP_GUARD_COOLDOWN_MAX_SECS"),
        // Health-check watchdog (env-driven like the auth guard; on by default
        // when headless). See src/health.rs.
        ("HEALTH_CHECK", "MACRDP_HEALTHCHECK"),
        (
            "HEALTHCHECK_INTERVAL_SECS",
            "MACRDP_HEALTHCHECK_INTERVAL_SECS",
        ),
        (
            "HEALTHCHECK_TIMEOUT_SECS",
            "MACRDP_HEALTHCHECK_TIMEOUT_SECS",
        ),
        ("HEALTHCHECK_FAILURES", "MACRDP_HEALTHCHECK_FAILURES"),
        // Blank recovery (the mstsc reconnect-blank auto-heal, needs
        // --enable-h264) + the auto-reconnect cookie. Env-driven like the
        // guard (read in h264.rs per connection / main.rs at server build,
        // both after this bridge runs). BLANK_RECOVERY=0 is the ask that
        // motivated bridging: disabling the detector for an overlay-link
        // profile used to need a plist EnvironmentVariables edit.
        ("BLANK_RECOVERY", "MACRDP_BLANK_RECOVERY"),
        (
            "BLANK_RECOVERY_REACTIVATE",
            "MACRDP_BLANK_RECOVERY_REACTIVATE",
        ),
        (
            "BLANK_RECOVERY_MAX_RTT_MS",
            "MACRDP_BLANK_RECOVERY_MAX_RTT_MS",
        ),
        ("BLANK_RECOVERY_MIN_QOE", "MACRDP_BLANK_RECOVERY_MIN_QOE"),
        (
            "BLANK_RECOVERY_MIN_RENDER_REPORTS",
            "MACRDP_BLANK_RECOVERY_MIN_RENDER_REPORTS",
        ),
        (
            "BLANK_RECOVERY_ESTABLISHED_REPORTS",
            "MACRDP_BLANK_RECOVERY_ESTABLISHED_REPORTS",
        ),
        (
            "BLANK_RECOVERY_ESTABLISHED_MIN_QOE",
            "MACRDP_BLANK_RECOVERY_ESTABLISHED_MIN_QOE",
        ),
        (
            "BLANK_RECOVERY_ESTABLISHED_MAX_WAIT_MS",
            "MACRDP_BLANK_RECOVERY_ESTABLISHED_MAX_WAIT_MS",
        ),
        (
            "BLANK_RECOVERY_ESTABLISHED_WALL_REPORTS",
            "MACRDP_BLANK_RECOVERY_ESTABLISHED_WALL_REPORTS",
        ),
        (
            "BLANK_RECOVERY_MAX_WAIT_MS",
            "MACRDP_BLANK_RECOVERY_MAX_WAIT_MS",
        ),
        (
            "BLANK_RECOVERY_MIN_WALL_REPORTS",
            "MACRDP_BLANK_RECOVERY_MIN_WALL_REPORTS",
        ),
        (
            "BLANK_RECOVERY_HEAL_CONFIRM_MS",
            "MACRDP_BLANK_RECOVERY_HEAL_CONFIRM_MS",
        ),
        ("BLANK_RECOVERY_ARM_MS", "MACRDP_BLANK_RECOVERY_ARM_MS"),
        ("BLANK_RECOVERY_RETRY_MS", "MACRDP_BLANK_RECOVERY_RETRY_MS"),
        (
            "BLANK_RECOVERY_MAX_ATTEMPTS",
            "MACRDP_BLANK_RECOVERY_MAX_ATTEMPTS",
        ),
        (
            "BLANK_RECOVERY_MAX_CONSECUTIVE_DROPS",
            "MACRDP_BLANK_RECOVERY_MAX_CONSECUTIVE_DROPS",
        ),
        ("AUTO_RECONNECT", "MACRDP_AUTO_RECONNECT"),
        // USB-redirection read-ahead tunables (--enable-usb-redirection). These
        // are env-only, read via getenv() in usb_spike.m; bridging them lets the
        // webcam knobs be set from config.env instead of the plist's
        // EnvironmentVariables (which a `kickstart -k` won't even pick up).
        ("USB_PREFETCH_DEPTH", "MACRDP_USB_PREFETCH_DEPTH"),
        ("USB_STREAM_STALL_MS", "MACRDP_USB_STREAM_STALL_MS"),
        // Camera-redirection decode diagnostics (--enable-camera-redirection): raw
        // H.264 + PNG frame dumps to $TMPDIR. Env-only like the USB knobs above;
        // bridged so it can be flipped in config.env when debugging the camera.
        ("CAMERA_DUMP", "MACRDP_CAMERA_DUMP"),
        // #168 stopgap: when --detach-primary can't re-enable the physical panel
        // on disconnect (macOS 26.x won't do it in-process), restart under
        // launchd to restore it. Env-read at the disconnect edge.
        ("DETACH_RESTART_ON_STUCK", "MACRDP_DETACH_RESTART_ON_STUCK"),
        // Extra safety-buffer delay (--lock-on-disconnect) on top of the
        // overlay watcher's REACTIVATION_GRACE before actually locking the
        // screen. Env-read at the disconnect edge via lock_on_disconnect_delay_ms().
        (
            "LOCK_ON_DISCONNECT_DELAY_MS",
            "MACRDP_LOCK_ON_DISCONNECT_DELAY_MS",
        ),
        // Loopback live-telemetry endpoint port (--stats-endpoint). Read via
        // getenv() in stats::default_port(); bridged like the USB/camera knobs
        // above so STATS_PORT can be set from config.env.
        ("STATS_PORT", "MACRDP_STATS_PORT"),
        // Virtual display in a replaceable helper process, and Retina modes
        // for high-DPI clients. Read when the display is created / re-moded.
        ("DISPLAY_HOST", "MACRDP_DISPLAY_HOST"),
        ("RETINA_VD", "MACRDP_RETINA_VD"),
        ("AVC444", "MACRDP_AVC444"),
    ] {
        if let Some(val) = cfg.get(cfg_key) {
            if !val.is_empty() {
                std::env::set_var(env_var, val);
            }
        }
    }

    // BITRATE (H.264 ceiling, Mbit/s), the dedicated key. When set it is
    // AUTHORITATIVE: any `--bitrate N` left in EXTRA_FLAGS is dropped below, so
    // clap never sees a duplicated `--bitrate` (which it rejects with an error
    // rather than taking the last). Unset/empty → EXTRA_FLAGS or the default (6).
    let bitrate = cfg.get("BITRATE").filter(|b| !b.is_empty());

    // EXTRA_FLAGS: space-separated escape hatch, appended verbatim — except a
    // `--bitrate <val>` pair is skipped when the dedicated BITRATE key is set.
    if let Some(extra) = cfg.get("EXTRA_FLAGS") {
        let mut toks = extra.split_whitespace();
        while let Some(tok) = toks.next() {
            if bitrate.is_some() && tok == "--bitrate" {
                toks.next(); // consume its value so it isn't pushed either
                continue;
            }
            argv.push(tok.to_string());
        }
    }
    if let Some(bitrate) = bitrate {
        argv.push("--bitrate".into());
        argv.push(bitrate.clone());
    }

    // Re-parse through clap so every value is validated exactly as a CLI arg
    // would be (SocketAddr, numeric ranges, mutually-exclusive checks).
    Args::try_parse_from(&argv)
        .with_context(|| format!("config file {} produced invalid settings", path.display()))
}

/// Turn on the features the Session Negotiator chooses by default (spec §2,
/// §6.3). Flags can only switch features ON, so an explicit flag is never
/// overridden; `MACRDP_NEGOTIATE=0` keeps upstream's flag-only behaviour.
/// Returns the reasons to log.
pub(super) fn apply_negotiated_defaults(
    args: &mut Args,
    host: &negotiator::session::HostCaps,
    shield_helper_available: bool,
) -> Vec<String> {
    let d = negotiator::session::connect_defaults(host);
    let mut reasons = d.reasons.clone();
    // MACRDP_H264=0 keeps sessions on legacy bitmaps (field diagnosis: tells a
    // client's H.264 decode problem apart from everything else).
    if crate::tunables::var("MACRDP_H264").as_deref() == Ok("0") {
        reasons.push("video: H.264 off (MACRDP_H264=0) — legacy bitmaps".into());
    } else {
        args.enable_h264 |= d.enable_h264;
    }
    args.adaptive_bitrate |= d.adaptive_bitrate;
    args.enable_udp_multitransport |= d.udp_multitransport;
    let size_pinned = args.width.is_some() || args.height.is_some() || args.hidpi;
    if d.virtual_display && !size_pinned {
        args.virtual_display = true;
        // Placeholder until a client connects; its requested size replaces it
        // (client-resolution auto-adopt re-modes the virtual display).
        args.width = Some(1920);
        args.height = Some(1080);
    }
    let mode_chosen = args.detach_primary || args.capture_primary || args.shield_primary;
    if args.virtual_display
        && host.physical_displays > 0
        && !mode_chosen
        && !shield_helper_available
    {
        // A negotiated default must never stop the server from starting.
        reasons.push(format!(
            "privacy: NOT shielding {} physical display(s) — the macrdpshield helper is missing \
             (build gui/make-shield-helper.sh or install the app bundle)",
            host.physical_displays
        ));
    } else if args.virtual_display && host.physical_displays > 0 && !mode_chosen {
        args.shield_primary = true;
        reasons.push(format!(
            "privacy: shielding {} physical display(s) while a client is connected",
            host.physical_displays
        ));
    }
    reasons
}

/// Read the arguments (or the `--config` file that replaces them), then apply
/// the negotiated defaults. Returns the arguments and the reasons the
/// negotiator gave, which are logged once logging is up.
pub(super) fn resolve() -> Result<(Args, Vec<String>)> {
    let mut args = Args::parse();
    // If launched with `--config <file>` (the LaunchAgent path), the file is the
    // sole source of truth — rebuild Args from it.
    if let Some(cfg_path) = args.config.clone() {
        args = args_from_config(&cfg_path)?;
    }
    let negotiation_reasons = if crate::tunables::var("MACRDP_NEGOTIATE").as_deref() == Ok("0") {
        vec!["negotiation disabled (MACRDP_NEGOTIATE=0): flags only".to_owned()]
    } else {
        let host = negotiator::session::HostCaps {
            physical_displays: negotiator::host::physical_displays(
                &virtual_display::online_display_facts(),
            )
            .len(),
            virtual_display_available: virtual_display::virtual_display_available(),
        };
        #[cfg(target_os = "macos")]
        let shield_helper_available = locate_shield_helper().is_some();
        #[cfg(not(target_os = "macos"))]
        let shield_helper_available = false;
        apply_negotiated_defaults(&mut args, &host, shield_helper_available)
    };
    Ok((args, negotiation_reasons))
}

/// Reject flag combinations that cannot work. Pure: it reads only `args`, so
/// it runs before anything touches a display.
pub(super) fn validate(args: &Args) -> Result<()> {
    if args.make_primary && !args.virtual_display {
        return Err(anyhow!(
            "--make-primary requires --virtual-display (it promotes the \
             virtual display to be the system's primary)"
        ));
    }
    if args.detach_primary && !args.virtual_display {
        return Err(anyhow!(
            "--detach-primary requires --virtual-display (you'd be left \
             with no usable display otherwise)"
        ));
    }
    if args.capture_primary && !args.virtual_display {
        return Err(anyhow!(
            "--capture-primary requires --virtual-display (you'd be left \
             with no usable display otherwise)"
        ));
    }
    if args.shield_primary && !args.virtual_display {
        return Err(anyhow!(
            "--shield-primary requires --virtual-display (you'd be left \
             with no usable display otherwise)"
        ));
    }
    // All three headless mechanisms fight over the same display arrangement,
    // so exactly one may be active. Checked pairwise so the error names the
    // actual conflict.
    if args.capture_primary && args.detach_primary {
        return Err(anyhow!(
            "--capture-primary and --detach-primary are mutually exclusive \
             (pick one mechanism for going headless)"
        ));
    }
    if args.shield_primary && args.detach_primary {
        return Err(anyhow!(
            "--shield-primary and --detach-primary are mutually exclusive \
             (pick one mechanism for going headless)"
        ));
    }
    if args.shield_primary && args.capture_primary {
        return Err(anyhow!(
            "--shield-primary and --capture-primary are mutually exclusive \
             (pick one mechanism for going headless)"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod validate_tests {
    use super::*;

    fn parse(flags: &[&str]) -> Args {
        let mut argv = vec!["macrdp"];
        argv.extend_from_slice(flags);
        Args::try_parse_from(argv).expect("test flags parse")
    }

    fn rejected(flags: &[&str]) -> String {
        validate(&parse(flags))
            .expect_err("combination should be rejected")
            .to_string()
    }

    #[test]
    fn plain_and_single_headless_modes_pass() {
        assert!(validate(&parse(&[])).is_ok());
        for mode in [
            "--detach-primary",
            "--capture-primary",
            "--shield-primary",
            "--make-primary",
        ] {
            assert!(
                validate(&parse(&["--virtual-display", mode])).is_ok(),
                "{mode}"
            );
        }
    }

    #[test]
    fn headless_modes_need_a_virtual_display() {
        for mode in [
            "--detach-primary",
            "--capture-primary",
            "--shield-primary",
            "--make-primary",
        ] {
            let err = rejected(&[mode]);
            assert!(
                err.contains(mode) && err.contains("--virtual-display"),
                "{err}"
            );
        }
    }

    #[test]
    fn two_headless_modes_are_rejected_by_name() {
        let pairs = [
            ("--capture-primary", "--detach-primary"),
            ("--shield-primary", "--detach-primary"),
            ("--shield-primary", "--capture-primary"),
        ];
        for (a, b) in pairs {
            let err = rejected(&["--virtual-display", a, b]);
            assert!(
                err.contains(a) && err.contains(b) && err.contains("mutually exclusive"),
                "{err}"
            );
        }
    }
}

#[cfg(test)]
mod max_client_size_tests {
    use super::parse_max_client_size;

    #[test]
    fn parses_wxh() {
        assert_eq!(parse_max_client_size("2560x1440"), Ok((2560, 1440)));
        // Case-insensitive separator + surrounding whitespace tolerated.
        assert_eq!(parse_max_client_size(" 1920X1080 "), Ok((1920, 1080)));
        // Band edges are inclusive.
        assert_eq!(parse_max_client_size("200x200"), Ok((200, 200)));
        assert_eq!(parse_max_client_size("8192x8192"), Ok((8192, 8192)));
    }

    #[test]
    fn rejects_out_of_band_and_garbage() {
        // Below the protocol minimum (would let the clamp produce an
        // un-servable size) and above the protocol maximum (meaningless).
        assert!(parse_max_client_size("199x1080").is_err());
        assert!(parse_max_client_size("1920x8193").is_err());
        // Not WxH at all.
        assert!(parse_max_client_size("1920").is_err());
        assert!(parse_max_client_size("axb").is_err());
        assert!(parse_max_client_size("1920x").is_err());
        assert!(parse_max_client_size("-1x1080").is_err());
    }
}
#[cfg(test)]
mod config_tests {
    use super::*;

    fn write_temp(name: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "macrdp-cfgtest-{}-{}.env",
            std::process::id(),
            name
        ));
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn parses_full_config_into_flags() {
        let path = write_temp(
            "full",
            "# a comment, then a blank line\n\
             \n\
             export BIND=0.0.0.0:3390\n\
             USERNAME=alice\n\
             ENABLE_H264=1\n\
             ENABLE_AAC=0\n\
             HIDPI=1\n\
             UNMINIMIZE=1\n\
             VIRTUAL_DISPLAY=1\n\
             VD_WIDTH=2560\n\
             VD_HEIGHT=1440\n\
             PRIMARY_MODE=detach\n\
             ENABLE_UDP_MULTITRANSPORT=1\n\
             UDP_MIGRATE_EGFX=1\n\
             MAX_CLIENT_SIZE=2560x1440\n\
             EXTRA_FLAGS=\"--fps 30\"\n",
        );
        let args = args_from_config(&path).unwrap();
        fs::remove_file(&path).ok();

        assert_eq!(args.bind.to_string(), "0.0.0.0:3390");
        assert_eq!(args.username.as_deref(), Some("alice"));
        assert!(args.enable_h264);
        assert!(!args.enable_aac);
        assert!(args.hidpi);
        assert!(args.unminimize_on_switch);
        assert!(args.virtual_display);
        assert_eq!(args.width, Some(2560));
        assert_eq!(args.height, Some(1440));
        assert!(args.detach_primary);
        assert!(!args.capture_primary);
        assert!(args.enable_udp_multitransport);
        assert!(args.udp_migrate_egfx);
        assert_eq!(args.max_client_size, Some((2560, 1440)));
        // USE_KEYCHAIN defaults on (matches the old wrapper).
        assert!(args.keychain);
        // EXTRA_FLAGS is parsed as real CLI tokens.
        assert_eq!(args.fps, Some(30));
    }

    #[test]
    fn bitrate_config_bridge() {
        // BITRATE alone maps to --bitrate.
        let p = write_temp("br1", "BITRATE=8\n");
        assert_eq!(args_from_config(&p).unwrap().bitrate, 8);
        fs::remove_file(&p).ok();

        // BITRATE wins over a stale `--bitrate` in EXTRA_FLAGS (which is dropped
        // so clap never sees a duplicate) WITHOUT eating the other EXTRA_FLAGS.
        let p = write_temp("br2", "EXTRA_FLAGS=\"--fps 24 --bitrate 6\"\nBITRATE=12\n");
        let a = args_from_config(&p).unwrap();
        assert_eq!(a.bitrate, 12);
        assert_eq!(a.fps, Some(24)); // the non-bitrate flag survived the strip
        fs::remove_file(&p).ok();

        // Unset → the server default (6).
        let p = write_temp("br3", "ENABLE_H264=1\n");
        assert_eq!(args_from_config(&p).unwrap().bitrate, 6);
        fs::remove_file(&p).ok();

        // Empty BITRATE is ignored (no arg pushed) → default, not an error.
        let p = write_temp("br4", "BITRATE=\n");
        assert_eq!(args_from_config(&p).unwrap().bitrate, 6);
        fs::remove_file(&p).ok();
    }

    #[test]
    fn empty_config_matches_wrapper_defaults() {
        let path = write_temp("empty", "\n# nothing set\n");
        let args = args_from_config(&path).unwrap();
        fs::remove_file(&path).ok();

        assert_eq!(args.bind.to_string(), "127.0.0.1:3390");
        assert!(args.keychain);
        assert!(!args.virtual_display);
        assert!(!args.enable_h264);
        assert!(!args.enable_udp_multitransport);
        assert!(!args.udp_migrate_egfx);
    }

    #[test]
    fn capture_primary_backcompat() {
        let path = write_temp(
            "bc",
            "VIRTUAL_DISPLAY=1\n\
             VD_WIDTH=1920\n\
             VD_HEIGHT=1080\n\
             CAPTURE_PRIMARY=1\n\
             USE_KEYCHAIN=0\n",
        );
        let args = args_from_config(&path).unwrap();
        fs::remove_file(&path).ok();

        assert!(args.capture_primary);
        assert!(!args.detach_primary);
        assert!(!args.keychain);
    }

    #[test]
    fn primary_mode_wins_over_legacy_capture_primary() {
        let path = write_temp(
            "winner",
            "VIRTUAL_DISPLAY=1\nVD_WIDTH=1920\nVD_HEIGHT=1080\nPRIMARY_MODE=detach\nCAPTURE_PRIMARY=1\n",
        );
        let args = args_from_config(&path).unwrap();
        fs::remove_file(&path).ok();

        assert!(args.detach_primary);
        assert!(!args.capture_primary);
    }

    #[test]
    fn config_maps_tls_cert_and_key() {
        let path = write_temp(
            "tls",
            "TLS_CERT=/etc/ssl/macrdp.pem\nTLS_KEY=/etc/ssl/macrdp.key\n",
        );
        let args = args_from_config(&path).unwrap();
        fs::remove_file(&path).ok();
        assert_eq!(args.cert.as_deref(), Some(Path::new("/etc/ssl/macrdp.pem")));
        assert_eq!(args.key.as_deref(), Some(Path::new("/etc/ssl/macrdp.key")));
    }
}
