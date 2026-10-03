//! The composition root: turns the resolved configuration into a running
//! server. `run` wires the pieces together; each concern lives in its own
//! submodule.

use crate::sync_ext::{LockExt, RwLockExt};
use std::fs;
use std::io::{BufReader, IsTerminal};
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use der::Decode;
use ironrdp_pdu::rdp::capability_sets::{
    BitmapCodecs, Codec, CodecProperty, NsCodec, RemoteFxContainer,
};
use ironrdp_server::{Credentials, RdpServer};
use rcgen::{generate_simple_self_signed, CertifiedKey};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;
use tracing::{error, info, warn};
use x509_cert::Certificate;
use zeroize::Zeroizing;

use crate::capture::{primary_display_size, CaptureDisplay};
use crate::input::{ensure_accessibility_access, MacInputHandler};

use crate::{
    audio, auth, auth_guard, camera, capture, clipboard, credential_monitor, health, input,
    keyboard_layout, lock_activity, logging, multitransport, negotiator, rdpdr, resync, tunables,
    usb_redirect, virtual_display,
};

#[cfg(target_os = "macos")]
use crate::{file_promise, file_promise_lazy, h264};

mod args;
mod credentials;
mod display;
mod helpers;
mod overlay;
mod session_lock;
mod tls;
mod transport;

use args::*;
use display::*;
use helpers::*;
use overlay::*;
use session_lock::*;
use tls::*;

pub(crate) use display::bitmap_codecs;
#[cfg(target_os = "macos")]
pub(crate) use helpers::boost_thread_qos;

/// Resolve the configuration, set up the displays, helpers and channels,
/// then run the RDP server until it stops.
pub(crate) async fn run() -> Result<()> {
    // Multi-session spike: report this process's Screen Recording and
    // Accessibility grants in whatever login session launchd started it in.
    #[cfg(target_os = "macos")]
    if crate::tunables::truthy("MACRDP_SESSION_PROBE") {
        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXIsProcessTrusted() -> bool;
        }
        let screen = core_graphics::access::ScreenCaptureAccess.preflight();
        // SAFETY: AXIsProcessTrusted takes no arguments and only reads TCC state.
        let ax = unsafe { AXIsProcessTrusted() };
        println!(
            "{{\"uid\":{},\"screen_recording\":{screen},\"accessibility\":{ax}}}",
            // SAFETY: getuid has no preconditions and cannot fail.
            unsafe { libc::getuid() }
        );
        std::process::exit(0);
    }
    let (mut args, negotiation_reasons) = args::resolve()?;

    // Research spike (Phase-1b USB-redirection go/no-go): run the UserHCI probe
    // and exit before any server/auth/capture setup. Requires the signed+
    // provisioned entitled build; see src/usb_redirect/.
    if args.usb_spike {
        std::process::exit(usb_redirect::run_spike());
    }

    init_logging(&args, &negotiation_reasons);

    // Multi-user: report which kind of GUI session this agent landed in. On the
    // console the full virtual-display path is available; a background
    // (Fast-User-Switching) session can't activate a virtual display, so the
    // negotiator falls back to native-framebuffer capture + resize (see
    // `virtual_display::virtual_display_available`). Audio still works in a
    // background session — ScreenCaptureKit delivers system audio off-console
    // (verified in the VM) — but SCK audio is system-wide, so with several
    // active users it is NOT per-session isolated. Purely informational.
    #[cfg(target_os = "macos")]
    match virtual_display::session_is_on_console() {
        Some(true) => info!("session: on the physical console — full virtual-display path available"),
        Some(false) => info!(
            "session: background (off-console) — native-framebuffer capture + resize + system audio (not per-session isolated); no virtual display"
        ),
        None => {}
    }

    // Sweep leftovers from a PRIOR macrdp that died uncleanly (SIGKILL / panic /
    // power-loss skip Drop AND the signal handler, stranding NFS mounts + paste
    // temp dirs). Dead-pid-gated so it's safe with another instance live; on a
    // detached thread so a stale `umount` can never delay startup.
    #[cfg(target_os = "macos")]
    std::thread::spawn(|| {
        rdpdr::reap_stale();
        file_promise::reap_stale();
        file_promise_lazy::reap_stale();
    });

    // Arm the health-check watchdog on the long-lived, launchd-watched process
    // so a hung-but-alive runtime — which KeepAlive can't tell from a healthy
    // one — gets bounced into a restart. Skipped by default when interactive
    // (stdout is a TTY, e.g. `cargo run`, where nothing would restart a bounce).
    // MACRDP_HEALTHCHECK=0/1 overrides; see src/health.rs.
    if health::should_arm(
        std::io::stdout().is_terminal(),
        crate::tunables::var("MACRDP_HEALTHCHECK").ok().as_deref(),
    ) {
        health::spawn(
            tokio::runtime::Handle::current(),
            health::HealthConfig::from_env(),
        );
    }

    // Opt-in loopback read-only live-telemetry endpoint (--stats-endpoint,
    // default OFF). When off, stats::global() returns None and every encode-path
    // update site is a no-op, so the default runtime path is unchanged.
    if args.stats_endpoint {
        let stats = crate::stats::enable();
        // The h264 path can't see the audio codec; seed it here from the args.
        stats
            .aac
            .store(args.enable_aac, std::sync::atomic::Ordering::Relaxed);
        let port = crate::stats::default_port();
        tokio::spawn(crate::stats::serve(port, stats));
    }

    args::validate(&args)?;
    if (args.restore_windows_on_disconnect || args.lock_on_disconnect || args.auto_unlock)
        && !(args.detach_primary || args.capture_primary || args.shield_primary)
    {
        warn!(
            "--restore-windows-on-disconnect / --lock-on-disconnect / \
             --auto-unlock have no effect without --detach-primary, \
             --capture-primary or --shield-primary (they need the headless \
             session watcher); ignoring"
        );
    }
    if args.lock_on_disconnect {
        match screen_lock_delay_is_immediate() {
            Some(false) => warn!(
                "--lock-on-disconnect is set, but this account's \"require \
                 password after sleep or screen saver begins\" delay is not \
                 Immediately — `open ScreenSaverEngine.app` will start the \
                 screen saver without actually requiring a password to get \
                 back in, so the Mac won't really be locked. Set it with \
                 System Settings, or `sudo sysadminctl -screenLock \
                 immediate -password <password>`; macrdp does not change \
                 this setting itself."
            ),
            Some(true) => {}
            None => warn!(
                "--lock-on-disconnect is set, but macrdp couldn't confirm \
                 this account's screen-lock delay is Immediately (the \
                 `sysadminctl -screenLock status` check failed) — if it \
                 isn't, the Mac won't really be locked. See \
                 docs/known-quirks.md."
            ),
        }
    }

    // Shared slots so the signal handler and the session-transition
    // watcher can drop the display RAII guards before process::exit and
    // actually restore the user's setup. Populated later if the
    // corresponding flag is set.
    let headless = HeadlessSlots::default();

    // Install signal handling before anything touches ScreenCaptureKit. Once an
    // SCK capture stream is live, macOS framework threads can leave the process
    // unkillable by Ctrl-C; a forced exit on SIGINT/SIGTERM sidesteps that — and
    // any other non-cooperative native threads — instead of relying on the
    // tokio runtime to unwind cleanly.
    //
    // The display RAII guards (PrimaryOverride / DetachedPrimary) are
    // dropped here first so the user's layout is restored before exit.
    // Without this, Ctrl-C would leave the virtual display promoted or
    // the built-in panel disabled until logout.
    let cleanup = headless.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("shutdown signal received — exiting");
        cleanup.release_all();
        // Lazy paste leaves NSFilePresenters registered + a temp dir on
        // disk + URLs on NSPasteboard. Process::exit skips Drop on the
        // cliprdr backend, so flush that state explicitly. No-op if
        // MacCliprdr was never constructed.
        #[cfg(target_os = "macos")]
        file_promise_lazy::shutdown_cleanup();
        // RDPDR NFS volumes are unmounted on disconnect by Surface::Drop, but
        // process::exit (below) skips Drop — so unmount any still-mounted ones
        // here, or a signal stop strands them pointing at the dead server.
        rdpdr::shutdown_cleanup();
        std::process::exit(0);
    });

    #[cfg(target_os = "macos")]
    {
        if !args.allow_sleep {
            prevent_sleep();
        }
        ensure_screen_recording_access();
        if !ensure_accessibility_access() {
            warn!(
                "Accessibility permission NOT granted. Viga will appear in \
                 System Settings → Privacy & Security → Accessibility. Enable \
                 it, then RESTART Viga. Without it, keyboard/mouse input \
                 from RDP clients is silently dropped."
            );
        } else {
            info!("Accessibility permission already granted");
        }
    }
    #[cfg(not(target_os = "macos"))]
    tracing::warn!("Built for a non-macOS target — capture is a static-rectangle stub.");

    // Allocate the virtual display BEFORE anything else queries SCK, so
    // it's visible in SCShareableContent's enumeration when capture
    // resolves the displayID. Held in main's scope so its Drop runs on
    // normal exit (signal-driven exit goes through std::process::exit
    // and skips Drop, but macOS reaps virtual displays when the owning
    // process dies, so cleanup still happens — just not via Drop).
    // Arc<Mutex<>> because the display is shared with `CaptureDisplay` for
    // live client-driven resize: the capture side re-modes it via
    // `VirtualDisplay::resize` when the client's window size changes. This
    // scope still holds a clone for the lifetime/teardown semantics
    // described above.
    // Capture the built-in/physical main display id BEFORE the virtual display
    // exists — at this point CGMainDisplayID is the physical panel (the vd is
    // created next and, under --capture-primary, only becomes main mid-session).
    // Used as the sweep target for --restore-windows-on-disconnect. macOS-only.
    #[cfg(target_os = "macos")]
    let physical_main_id: u32 = core_graphics::display::CGDisplay::main().id;
    #[cfg(not(target_os = "macos"))]
    let physical_main_id: u32 = 0;

    let virtual_display = create_virtual_display(&args)?;

    // --detach-primary / --capture-primary are lazy: the headless
    // mechanism is only engaged once a client actually connects. A
    // session-transition watcher installs the corresponding RAII
    // guard on 0→≥1 client transitions and drops it on ≥1→0. Both
    // flags subsume --make-primary (each puts the virtual display
    // at (0,0)), so if both are set, only the lazy path runs.
    let (username, password) = credentials::obtain(&mut args)?;
    // Shared with the reconnect-time auto-unlock watcher (below), which reuses
    // this exact validated credential without a second Keychain read, and kept
    // current by the credential monitor (a changed or revoked password reaches
    // auto-unlock too).
    let secret: credential_monitor::SecretCell =
        Arc::new(std::sync::RwLock::new(Some(password.clone())));

    let session_tracker = capture::SessionTracker::default();
    // (--lock-on-disconnect) Connection activity the pending lock consults so
    // it doesn't fire under a reconnect's handshake. None when the flag is off.
    let lock_activity = args
        .lock_on_disconnect
        .then(|| Arc::new(lock_activity::ConnectionActivity::default()));
    // Auto-unlock is opt-in (--auto-unlock / config AUTO_UNLOCK=1), and
    // additionally skipped entirely under --skip-auth, since in that mode
    // `password` was never validated by PAM and isn't trustworthy to
    // auto-type (see attempt_auto_unlock's docs).
    let auto_unlock = args.auto_unlock && !args.skip_auth;
    if args.auto_unlock && args.skip_auth {
        warn!("--auto-unlock has no effect under --skip-auth; ignoring");
    }
    engage_headless_mode(
        &args,
        virtual_display.as_ref(),
        &headless,
        WatcherOptions {
            tracker: session_tracker.clone(),
            restore_windows: args.restore_windows_on_disconnect,
            physical_main_id,
            lock_on_disconnect: lock_activity.clone(),
            secret: Arc::clone(&secret),
            auto_unlock,
        },
    )?;

    let TlsMaterial {
        acceptor: tls,
        spki_der,
        config: udp_tls_config,
        cert_der: udp_cert_der,
        key_der: udp_key_der,
    } = tls::load_material(&args)?;

    let (width, height, capture_display_id, screen_size_pts) =
        resolve_desktop(&args, virtual_display.as_ref()).await?;
    // The display capture, input and audio address. Follows the virtual
    // display if it is replaced; fixed (or none) on the mirror-primary path.
    let display_cell = match virtual_display.as_ref() {
        Some(vd) => vd.lock_or_recover().id_cell(),
        None => virtual_display::DisplayIdCell::new(capture_display_id),
    };

    // Frame rate: explicit --fps wins; otherwise 60 for H.264 (mstsc holds a
    // ~2-frame presentation buffer, so 60fps keeps typing latency low) or 15 for
    // the legacy bitmap path (which has no such buffer and is bandwidth-heavier).
    let fps = args.fps.unwrap_or(if args.enable_h264 { 60 } else { 15 });

    // Live session desktop size, shared by capture, input scaling, and the
    // H.264 pipeline. Starts at the size resolved above; when `auto_size`
    // is on, `CaptureDisplay::request_initial_size` overwrites it with the
    // client's requested resolution at connect time.
    let desktop_size = capture::SharedDesktopSize::new(width, height);

    // Adopt the client's requested resolution at connect (and re-mode to
    // match on a live resize). Two cases:
    //   - Virtual display: --width/--height are the display's INITIAL size,
    //     not a pin — adopt the client's request and re-mode the virtual
    //     display to match, so a phone and a laptop each get a crisp 1:1
    //     desktop (and their mouse coords, sent in the client's own size,
    //     map correctly). --hidpi is inert on this path.
    //   - Mirror-primary: adopt only when nothing pins the size
    //     (--width/--height pin a size, --hidpi pins backing pixels).
    // --no-client-resolution opts out in both cases — the virtual display
    // then stays fixed at --width/--height; mirror-primary serves native.
    let auto_size = !args.no_client_resolution
        && if args.virtual_display {
            true
        } else {
            !args.hidpi && args.width.is_none() && args.height.is_none()
        };
    if auto_size {
        info!(
            "client-resolution auto-adopt enabled — the session will be served at \
             the resolution the client requests{} (--no-client-resolution disables)",
            if args.virtual_display {
                ", re-moding the virtual display to match"
            } else {
                ""
            }
        );
    }

    // Video-path heads-up. With --enable-h264 the active vs AVC420-fallback cases
    // are logged later (h264.rs, once the client's EGFX caps are known); the only
    // gap is the h264-disabled case, which never reaches that code — so name the
    // legacy path and point at the crisp option here. Answers the common "the
    // picture is grainy / not sure what codec it negotiated" question.
    if !args.enable_h264 {
        info!(
            "video codec: legacy bitmap path (RemoteFX/QOI/NSCodec/raw, client-negotiated). \
             For crisp H.264 on capable clients (mstsc, Microsoft Remote Desktop, FreeRDP, \
             Thincast) pass --enable-h264 (AVC420 over EGFX)."
        );
    }

    crate::input::set_unminimize_on_switch(args.unminimize_on_switch);
    crate::input::set_alt_tab_switch(args.alt_tab_switch);
    crate::input::set_alt_backtick_switch(args.alt_backtick_switch);
    crate::input::set_app_switcher_hud(args.app_switcher_hud);
    crate::input::set_map_ctrl_to_cmd(args.map_ctrl_to_cmd);
    crate::input::set_no_remap_apps(args.no_remap_apps.clone());
    if args.map_ctrl_to_cmd {
        info!(
            no_remap_apps = ?args.no_remap_apps,
            "Ctrl→Cmd Windows-shortcut remap enabled (--map-ctrl-to-cmd)"
        );
        // Event-driven frontmost-app tracking for the terminal/app suppression
        // (catches click-focus + Electron apps that AX polling misses).
        crate::input::init_focus_observer();
    }

    // App-switcher HUD overlay helper (macOS-only). Tell it which display to
    // center on (the captured one) and spawn it; it idles until a Cmd+Tab. Held
    // for the process lifetime; the helper also self-exits if we die.
    // Shield helper (macOS-only). Spawned eagerly at startup rather than lazily
    // on first connect, so a missing/broken helper is a loud startup failure
    // instead of a silently-visible desktop at the moment a client arrives.
    // Held for the process lifetime; it also self-exits if we die.
    #[cfg(target_os = "macos")]
    let _shield_helper = if args.shield_primary {
        Some(spawn_shield_helper()?)
    } else {
        None
    };

    #[cfg(target_os = "macos")]
    let _hud_helper = if args.app_switcher_hud {
        crate::switcher_hud::set_display_id(if capture_display_id.is_some() {
            display_cell.clone()
        } else {
            virtual_display::DisplayIdCell::new(Some(core_graphics::display::CGDisplay::main().id))
        });
        spawn_hud_helper()
    } else {
        None
    };

    // Shared flag flipped true when EGFX is Soft-Synced onto a LOSSY UDP tunnel
    // (UDPFECL). The H.264 pipeline reads it to arm ack-driven IDR recovery; the
    // server flips it at the migration site. Cross-platform so the UDP listener
    // wiring below (which hands the same clone to the server) compiles on CI.
    let egfx_on_lossy_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Companion flag: EGFX migrated onto ANY UDP tunnel (reliable or lossy). The
    // H.264 pipeline reads it to enable frame-ack-lag backpressure (the UDP tunnel
    // has no socket pacing); the server flips it at the same migration site.
    let egfx_on_udp_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // EGFX-over-UDP → TCP watchdog request: the H.264 pipeline sets it true when the
    // reliable UDP tunnel wedges (acks silent while shipping); the server reads it to
    // flip EGFX routing back to TCP. Reset on reconnect (server side).
    let egfx_demigrate_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Adaptive-bitrate loss signal: cumulative reliable-tunnel retransmit count the
    // UDP listener bumps and the H.264 controller samples (deltas) to drive
    // congestion-responsive bitrate. Cross-platform so the listener wiring compiles
    // on CI; only read on the macOS H.264 path.
    let congestion_retransmits = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    // Kernel-measured TCP RTT (ms) of the most recently accepted connection,
    // written by the vendored server at accept (divergence 15). Drives the
    // link-aware blank-recovery gate + the adaptive-bitrate seed in h264.rs.
    // 0 = unknown.
    let link_rtt_ms = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

    // EGFX/H.264 video pipeline (macOS-only; opt-in via --enable-h264). One
    // clone drives the builder's GfxServerFactory (protocol side); another
    // rides on CaptureDisplay, where the capture loop feeds it BGRA frames.
    #[cfg(target_os = "macos")]
    let gfx = args.enable_h264.then(|| {
        h264::Gfx::new(
            desktop_size.clone(),
            fps,
            args.bitrate.max(1).saturating_mul(1_000_000),
            args.keyframe_interval,
            args.h264_frames_in_flight.max(1),
            egfx_on_lossy_flag.clone(),
            egfx_on_udp_flag.clone(),
            egfx_demigrate_flag.clone(),
            args.adaptive_bitrate,
            congestion_retransmits.clone(),
            link_rtt_ms.clone(),
        )
    });

    // On-change keyframes are off by default; --keyframe-on-change opts in.
    let keyframe_on_change = crate::capture::KeyframeOnChange {
        enabled: args.keyframe_on_change,
        change_pct: args.keyframe_change_pct,
        click_pct: args.keyframe_click_pct,
        click_window: std::time::Duration::from_millis(args.keyframe_click_window_ms),
    };

    // Mouse-click hint shared by the input handler (which records clicks) and
    // the H.264 capture path (which lowers its keyframe threshold briefly after
    // a click). Only allocated when the on-demand-keyframe feature is enabled.
    let click_signal =
        (args.enable_h264 && keyframe_on_change.enabled).then(crate::capture::ClickSignal::new);

    // Shared "client minimized / SuppressOutput" flag — created here so
    // the same `Arc<AtomicBool>` can be handed to both the capture
    // backend (which reads it to gate frame emission) and the vendor
    // server (whose per-connection PDU handler writes it). See
    // `vendor/ironrdp-server` `display_suppressed` plumbing.
    let display_suppressed = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Negotiator (spec §6): what the client advertised at handshake, shared
    // between the connection-handler decorator and the display path.
    let client_advert = std::sync::Arc::new(negotiator::handler::ClientAdvert::default());
    // The Ctrl+Alt+Shift+R resync request: raised by the input handler,
    // consumed by the capture and audio loops.
    let resync = resync::ResyncSignal::default();
    let display = CaptureDisplay {
        desktop_size: desktop_size.clone(),
        auto_size,
        stretch: args.stretch,
        fps,
        display_id: display_cell.clone(),
        screen_size_pts,
        // Virtual-display sessions drive the cursor into the virtual display's
        // off-panel coordinate region; warp it back to the primary on
        // disconnect so the local Mac mouse isn't stranded.
        warp_cursor_home: virtual_display.is_some(),
        cursor_scale: args.cursor_scale,
        // Only attach the tracker when the watchdog needs it. Saves
        // a useless atomic increment/decrement per session otherwise.
        session_tracker: (args.detach_primary || args.capture_primary || args.shield_primary)
            .then(|| session_tracker.clone()),
        #[cfg(target_os = "macos")]
        gfx: gfx.clone(),
        keyframe_on_change,
        click_signal: click_signal.clone(),
        flush_frames: args.flush_frames,
        display_suppressed: Some(display_suppressed.clone()),
        // Live in-session resize (client drags its window, sending an
        // MS-RDPEDISP monitor-layout PDU) — the counterpart to the
        // connect-time auto-adopt above. See `CaptureDisplay::request_layout`.
        pending_resize: capture::PendingResize::new(),
        max_client_size: args
            .max_client_size
            .map(|(width, height)| ironrdp_server::DesktopSize { width, height }),
        suppress_next_adopt: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        // Shared so a live client resize can re-mode the display; None on
        // the mirror-primary path (no virtual display to re-mode).
        virtual_display: virtual_display.clone(),
        // Shared so a live re-mode can re-assert the gamma blanking (a re-mode
        // resets gamma → the panel un-blanks). Only for --capture-primary.
        captured_primary: if args.capture_primary {
            Some(headless.captured.clone())
        } else {
            None
        },
        // Shared so a live re-mode can re-fit the shield windows to the panels'
        // new frames. Only for --shield-primary.
        shielded_primary: if args.shield_primary {
            Some(headless.shielded.clone())
        } else {
            None
        },
        client_advert: Some(client_advert.clone()),
        applied_plan: None,
        resync: resync.clone(),
        #[cfg(target_os = "macos")]
        extra_capture: Vec::new(),
    };

    // Shared cell the server fills with the connecting client's keyboard-layout
    // id, for auto-detecting a non-US layout when --keyboard-layout isn't given.
    let keyboard_layout_klid: crate::input::SharedKeyboardLayout =
        std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let input_handler = MacInputHandler::new(
        desktop_size.clone(),
        display_cell.clone(),
        click_signal,
        args.keyboard_layout.clone(),
        Some(keyboard_layout_klid.clone()),
        resync.clone(),
    )?;
    let cliprdr: Box<dyn ironrdp_server::CliprdrServerFactory> = {
        #[cfg(target_os = "macos")]
        {
            Box::new(clipboard::MacCliprdr::new(
                !args.no_lazy_paste,
                !args.no_rich_clipboard,
            ))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Box::new(clipboard::MacCliprdr::new())
        }
    };
    let sound: Box<dyn ironrdp_server::SoundServerFactory> = Box::new(audio::MacRdpsnd::new(
        Some(display_suppressed.clone()),
        !args.no_mute_on_minimize,
        args.enable_aac,
        args.aac_bitrate,
        // Bind audio to the same display the video captures so --detach-primary
        // / --capture-primary (which disable/capture the physical panel) don't
        // kill the audio stream's content source.
        display_cell.clone(),
        resync,
    ));

    // with_hybrid advertises HYBRID | HYBRID_EX so clients run CredSSP/NLA
    // over TLS. ironrdp_acceptor's accept_credssp reads our set_credentials
    // and validates the client's NTLM response against it.
    #[cfg(target_os = "macos")]
    let gfx_factory: Option<Box<dyn ironrdp_server::GfxServerFactory>> =
        gfx.map(|g| Box::new(g) as Box<dyn ironrdp_server::GfxServerFactory>);
    #[cfg(not(target_os = "macos"))]
    let gfx_factory: Option<Box<dyn ironrdp_server::GfxServerFactory>> = None;

    // RDPDR is opt-in. Drive redirection (--enable-drive-redirection) lets the
    // Mac browse/read the client's redirected drive; smart-card redirection
    // (--enable-smartcard-redirection) lets macOS apps use the client's reader.
    // Both ride the one RDPDR channel, so attach the factory if either is on.
    let rdpdr_factory: Option<Box<dyn ironrdp_server::RdpdrServerFactory>> =
        if args.enable_drive_redirection || args.enable_smartcard_redirection || printers_enabled() {
            Some(Box::new(rdpdr::MacRdpdr::new(
                args.enable_drive_redirection,
                args.enable_smartcard_redirection,
                printers_enabled(),
            )))
        } else {
            None
        };

    // Server-direction USB redirection (--enable-usb-redirection, opt-in). Phase
    // 3.0 installs the observe-only URBDRC DVC processor (capability exchange +
    // device-announce logging). Ships inert when the flag is off.
    let usb_factory: Option<Box<dyn ironrdp_server::UrbdrcServerFactory>> =
        if args.enable_usb_redirection {
            Some(Box::new(usb_redirect::MacUsb::new()))
        } else {
            None
        };

    // Camera redirection (--enable-camera-redirection, opt-in) — Phase 0 protocol
    // gate. Installs the MS-RDPECAM enumeration processor that advertises
    // RDCamera_Device_Enumerator and logs the client's camera announcement. Ships
    // inert when the flag is off.
    let camera_factory: Option<Box<dyn ironrdp_server::RdCameraServerFactory>> =
        if args.enable_camera_redirection {
            Some(Box::new(camera::MacCamera::new()))
        } else {
            None
        };

    // Auth hardening (Tier 1.2): per-IP rate-limit + lockout + audit log via the
    // server's pre-handshake/post-disconnect ConnectionHandler seam. On by default
    // (MACRDP_CONN_GUARD=0 disables).
    let conn_handler: Option<Box<dyn ironrdp_server::ConnectionHandler>> =
        auth_guard::AuthGuardHandler::from_env();
    // With --lock-on-disconnect, wrap it to record reconnect activity (every
    // hook still forwards unchanged); otherwise the handler is untouched.
    let conn_handler = match &lock_activity {
        Some(activity) => Some(lock_activity::ActivityHandler::wrap(
            conn_handler,
            Arc::clone(activity),
        )),
        None => conn_handler,
    };
    let conn_handler = Some(negotiator::handler::NegotiationHandler::wrap(
        conn_handler,
        client_advert.clone(),
        if args.map_ctrl_to_cmd {
            Some(true)
        } else {
            None
        },
        input::set_map_ctrl_to_cmd,
    ));

    let mut server = RdpServer::builder()
        .with_addr(args.bind)
        .with_hybrid(tls, spki_der)
        .with_input_handler(input_handler)
        .with_display_handler(display)
        .with_cliprdr_factory(Some(cliprdr))
        .with_sound_factory(Some(sound))
        .with_rdpdr_factory(rdpdr_factory)
        .with_usb_factory(usb_factory)
        .with_camera_factory(camera_factory)
        .with_bitmap_codecs(bitmap_codecs())
        .with_gfx_factory(gfx_factory)
        .with_connection_handler(conn_handler)
        .build();

    // Hand the shared suppress flag to the server so its per-connection
    // PDU handler writes to the same `AtomicBool` the capture backend
    // reads from. Without this, the server uses an internally-created
    // flag the display never sees.
    server.set_display_suppressed_handle(display_suppressed);

    // The acceptor records the client's announced keyboard-layout id (KLID) in
    // its Client Core Data; the server publishes it here so the input handler
    // can auto-select a matching non-US layout (when --keyboard-layout is unset).
    server.set_keyboard_layout_handle(keyboard_layout_klid);

    // The server samples the kernel's smoothed TCP RTT for each accepted
    // connection (divergence 15) into this cell; the H.264 pipeline reads it
    // for link-aware blank-recovery gating + adaptive-bitrate seeding.
    server.set_link_rtt_handle(link_rtt_ms.clone());

    // Client-resolution auto-adopt: the vendored acceptor reads the desktop
    // size the client requests in its GCC Client Core Data and negotiates
    // the session at that size from the start (Demand Active); the
    // CaptureDisplay's `request_initial_size` then adopts it into the
    // shared `desktop_size` that capture, input scaling, and the H.264
    // pipeline all read.
    server.set_honor_client_desktop_size(auto_size);
    // Optional operator ceiling for the adopted size (--max-client-size,
    // defense-in-depth): a request above the cap is clamped per-dimension in
    // the acceptor. Meaningless off the auto-adopt path (nothing is adopted),
    // so warn rather than silently ignore.
    if let Some((max_w, max_h)) = args.max_client_size {
        if auto_size {
            server.set_honor_client_desktop_size_max(Some(ironrdp_server::DesktopSize {
                width: max_w,
                height: max_h,
            }));
            info!(
                max_w,
                max_h, "client-requested session size capped at the operator maximum"
            );
        } else {
            warn!(
                "--max-client-size has no effect: client-resolution auto-adopt is off \
                 (--no-client-resolution, or an explicit --width/--height/--hidpi \
                 without --virtual-display)"
            );
        }
    }

    // Client microphone (MS-RDPEAI), played by the VigaMic virtual microphone.
    server.add_dvc_factory(Box::new(crate::audin::AudinFactory::new(std::sync::Arc::new(
        crate::audin::RingSink::new(),
    ))));

    transport::provision_auto_reconnect(&mut server);

    // Held for the process lifetime (Drop aborts the listener task).
    let _udp_listener = transport::start_udp_multitransport(
        &mut server,
        &args,
        transport::UdpTls {
            config: &udp_tls_config,
            cert_der: &udp_cert_der,
            key_der: &udp_key_der,
        },
        transport::EgfxTransportFlags {
            on_lossy: &egfx_on_lossy_flag,
            on_udp: &egfx_on_udp_flag,
            demigrate: &egfx_demigrate_flag,
            congestion_retransmits: &congestion_retransmits,
        },
    )
    .await;

    credentials::install(&mut server, &args, &username, &password, &secret);

    info!(
        addr = %args.bind,
        user = %username,
        "Viga listening — connect with: mstsc / xfreerdp at port {} as {}",
        args.bind.port(),
        username,
    );
    server.run().await
}

/// Set up logging (the operational log, plus the JSON audit stream when an
/// operator asks for it), snapshot the tunables, and log why the negotiator
/// chose what it did.
fn init_logging(args: &Args, negotiation_reasons: &[String]) {
    // RUST_LOG (if set) always wins. Otherwise: --verbose turns on debug
    // everywhere; without it we apply a targeted filter that quiets known
    // noisy modules:
    //   - rustls SNI warnings (clients legally put IPs there)
    //   - the ironrdp_server::encoder "took N ms" timer (fires above 10ms)
    //   - ironrdp_server::server WARN ("Unexpected share data pdu"); ERRORs
    //     are still shown — that's where the cert-acceptance "Broken pipe"
    //     comes from, which we can't suppress without losing real errors.
    let filter = if let Ok(env) = tracing_subscriber::EnvFilter::try_from_default_env() {
        env
    } else if args.verbose {
        tracing_subscriber::EnvFilter::new("debug")
    } else {
        tracing_subscriber::EnvFilter::new(
            "info,rustls=error,ironrdp_server::encoder=error,ironrdp_server::server=error",
        )
    };
    // Resolve the JSON audit-stream sink (SIEM/SOC): an explicit --audit-file /
    // AUDIT_FILE wins; otherwise MACRDP_AUDIT_JSON=1 enables it at the default
    // `<log-dir-or-~/Library/Logs>/macrdp-audit.log`. Off (None) by default, so the
    // logging setup is byte-identical unless an operator opts in.
    let audit_json_env = crate::tunables::var("MACRDP_AUDIT_JSON")
        .ok()
        .is_some_and(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "no" | "off"
            )
        });
    let audit_file: Option<PathBuf> = args.audit_file.clone().or_else(|| {
        if audit_json_env {
            let dir = args
                .log_dir
                .clone()
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Logs")))
                .unwrap_or_else(|| PathBuf::from("."));
            Some(dir.join("macrdp-audit.log"))
        } else {
            None
        }
    });
    logging::init(filter, args.log_dir.as_deref(), audit_file.as_deref());
    // Snapshot every MACRDP_* tunable now that config.env has been bridged into
    // the environment, and log the ones that are set.
    tunables::load();
    for reason in negotiation_reasons {
        tracing::info!(target: "macrdp::negotiator", "{reason}");
    }
}

/// Resolves when the process receives SIGINT (Ctrl-C) or, on Unix, SIGTERM.
pub(super) async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = sigterm.recv() => {}
                }
            }
            Err(e) => {
                warn!("could not install SIGTERM handler: {e}");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Client printers become local queues unless `MACRDP_PRINTERS=0`.
fn printers_enabled() -> bool {
    std::env::var("MACRDP_PRINTERS").map_or(true, |v| v != "0")
}
