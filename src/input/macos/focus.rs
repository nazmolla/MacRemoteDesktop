//! Which app the user is in: the most-recently-used app order, focus and
//! activity tracking, and bundle-id lookups for a pid.

use super::*;

/// Per-bundle "most recently in front" PID. Used during dedup to
/// pick the right instance when multiple processes share a bundle
/// ID (two VSCode windows opened as separate projects, two Firefox
/// profiles, etc.) — without this we'd keep the first-launched
/// instance and the cycle would route the user to the "wrong"
/// VSCode. Distinct from the *bundle-level* MRU in `mru_bundles`
/// (which orders the cycle list): this one only resolves
/// bundle → which-pid.
pub(super) fn mru_map() -> &'static std::sync::Mutex<std::collections::HashMap<String, libc::pid_t>>
{
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static MRU: OnceLock<std::sync::Mutex<HashMap<String, libc::pid_t>>> = OnceLock::new();
    MRU.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Global most-recently-used ordering of bundle identifiers, front
/// of the vec = most recent. Drives the cycle order so Cmd+Tab
/// behaves like native macOS: one press goes to the previous app,
/// two presses to the one before, etc. — independent of process
/// launch order. Updated only when (a) a new bundle is first seen,
/// (b) a fresh cycle session starts (current frontmost bumped to
/// the front), or (c) a cycle session commits on Cmd release
/// (cursor target bumped to the front). Critically NOT updated by
/// each intra-session Cmd+Tab press — that would reshuffle the
/// list and ping-pong the cursor between two apps.
pub(super) fn mru_bundles() -> &'static std::sync::Mutex<Vec<String>> {
    use std::sync::OnceLock;
    static MRU: OnceLock<std::sync::Mutex<Vec<String>>> = OnceLock::new();
    MRU.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

pub(super) fn promote_bundle_to_front(bundle: &str) {
    if bundle == "<no-bundle-id>" {
        return;
    }
    let mru = mru_bundles();
    let mut guard = mru.lock_or_recover();
    guard.retain(|b| b != bundle);
    guard.insert(0, bundle.to_string());
}

/// Active Cmd+Tab session. `snapshot` is the MRU-ordered candidate
/// list frozen at session start; `cursor_pid` advances through it
/// as the user holds Cmd and taps Tab. The snapshot does NOT
/// reshuffle mid-session even though we activate each step —
/// without that freeze, the activation would move the target to
/// MRU front, and the next press would cycle right back to where
/// we started.
///
/// `cmd_release_gen` captures `CMD_RELEASE_GENERATION` at session
/// start. Every observed Cmd release bumps the global counter;
/// if the session's stored value still matches on the next press
/// we know Cmd has been held continuously since this session
/// began (no matter how long the user paused between Tab taps)
/// and the session continues. When it differs, the user released
/// Cmd between presses — commit the cursor to MRU and start fresh.
/// This replaced an earlier 2 s time-based grace that incorrectly
/// split sessions whenever a user paused mid-hold.
///
/// `last_press_at` only serves as a hard upper bound on session
/// lifetime (`CYCLE_RESUME_GRACE`) — a safety net for the rare
/// case where a Cmd-release event is eaten before reaching us
/// (RDP client focus loss taking the keyup with it).
pub(super) struct CycleSession {
    pub(super) snapshot: Vec<(String, String, libc::pid_t)>,
    pub(super) cursor_pid: libc::pid_t,
    pub(super) last_press_at: Instant,
    pub(super) cmd_release_gen: u64,
}

pub(super) static CYCLE_SESSION: std::sync::Mutex<Option<CycleSession>> =
    std::sync::Mutex::new(None);

/// True while an app-switcher cycle is in flight — i.e. the switcher is up
/// and its modifier (Cmd, or Option with `--alt-tab-switch`) is held.
/// Lets `try_symbolic_hotkey` route Left/Right arrows into the switcher only
/// while it's showing, leaving bare arrows to reach the focused app
/// otherwise. A poisoned lock is treated as "no session" (fail open to the
/// app, never swallow the arrow).
pub(super) fn cycle_session_active() -> bool {
    CYCLE_SESSION.lock().is_ok_and(|g| g.is_some())
}

/// Incremented every time both Cmd halves transition to released
/// (see `commit_cycle_session` callers). A `CycleSession` stamps
/// its start-time value; on later presses, a match means "Cmd has
/// not been released since this session started" — so we
/// continue, regardless of pause length.
pub(super) static CMD_RELEASE_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// `NSWorkspace.frontmostApplication()` does NOT update for app
/// activations driven through the Accessibility API — it keeps
/// returning the app that was front BEFORE our first AX activation,
/// even after we've activated several others successfully. So we
/// can't trust workspace alone to know "what app is the user
/// actually in" on session start, which means every fresh Cmd+Tab
/// would re-promote the stale workspace value to MRU front and
/// undo the previous session's commit.
///
/// `LAST_AX_ACTIVATED_PID` is our authoritative "this is what's
/// really front" — updated after every successful AX activation,
/// survives across cycle sessions.
///
/// `WORKSPACE_LIE_FRONT` is the value `frontmostApplication()` was
/// returning at the moment we did our first AX activation. While
/// workspace keeps returning this value we treat it as stale. When
/// workspace returns *anything else*, the OS has observed a focus
/// change through a non-AX path (Dock click, menu, app self-
/// activation) — we reset both statics and trust workspace again.
pub(super) static LAST_AX_ACTIVATED_PID: std::sync::Mutex<Option<libc::pid_t>> =
    std::sync::Mutex::new(None);
pub(super) static WORKSPACE_LIE_FRONT: std::sync::Mutex<Option<libc::pid_t>> =
    std::sync::Mutex::new(None);

/// Enumerate every running process by PID directly from the kernel.
///
/// We can't use `NSWorkspace.runningApplications()` for this — that
/// API returns a cache that's only refreshed by Cocoa notifications
/// delivered through the main thread's runloop, and we don't pump
/// one (tokio owns the main thread). Apps launched after macrdp
/// starts never appear there. `proc_listallpids` reads the proc
/// table directly via the BSD-style libproc API, so it always
/// reflects current kernel state.
///
/// Note on the libproc API shape: `proc_listallpids` returns the
/// **count of PIDs** (XNU wraps `proc_listpids` and divides by
/// `sizeof(int)` before returning), and the buffer size passed in
/// must be in **bytes**. Earlier versions of this function divided
/// the return value by 4 again, producing `count=13` and clipping
/// most processes from view.
pub(super) fn list_all_pids() -> Vec<libc::pid_t> {
    extern "C" {
        fn proc_listallpids(buffer: *mut libc::c_int, buffersize: libc::c_int) -> libc::c_int;
    }
    // SAFETY: the sizing call passes a null buffer with size 0; the second call passes `buffer`
    // with its exact byte size, and the result is truncated to the count written.
    unsafe {
        // Sizing call: NULL buffer / 0 size returns the pid count
        // the kernel would have written. Add headroom for procs
        // that may launch between this call and the populated one.
        let probe = proc_listallpids(std::ptr::null_mut(), 0);
        if probe <= 0 {
            return Vec::new();
        }
        let capacity = probe as usize + 64;
        let mut buffer: Vec<libc::pid_t> = vec![0; capacity];
        let pid_count = proc_listallpids(
            buffer.as_mut_ptr() as *mut libc::c_int,
            (buffer.len() * std::mem::size_of::<libc::c_int>()) as libc::c_int,
        );
        if pid_count <= 0 {
            return Vec::new();
        }
        buffer.truncate(pid_count as usize);
        buffer.retain(|&p| p > 0);
        buffer
    }
}

/// Reconcile NSWorkspace's view of "frontmost app" with our own AX
/// activation history. Returns the pid we believe is actually
/// front (drives cycle decisions, MRU promotions, snapshot
/// exclusion). See LAST_AX_ACTIVATED_PID / WORKSPACE_LIE_FRONT
/// docs above.
///
/// `workspace_pid` is whatever `frontmostApplication()` just
/// returned. `is_alive` validates our tracked AX pid against the
/// caller's notion of "this pid still runs" — `cycle_apps` already
/// has `metas` so passes a `metas`-backed predicate; the Cmd+`
/// window cycler doesn't and passes `kill(pid, 0)` instead.
pub(super) fn effective_front_pid(
    workspace_pid: libc::pid_t,
    is_alive: impl FnOnce(libc::pid_t) -> bool,
) -> libc::pid_t {
    let mut lie = WORKSPACE_LIE_FRONT.lock_or_recover();
    let mut last_ax = LAST_AX_ACTIVATED_PID.lock_or_recover();
    if let (Some(lie_pid), Some(ax_pid)) = (*lie, *last_ax) {
        if lie_pid == workspace_pid && is_alive(ax_pid) {
            return ax_pid;
        }
    }
    // Workspace moved past whatever it was lying about (or never
    // lied yet, or our AX target died) — reset and trust workspace.
    *lie = None;
    *last_ax = None;
    workspace_pid
}

/// Spawn a background thread that polls the Accessibility system-wide
/// focused application and promotes its bundle to `mru_bundles[0]`
/// whenever it changes. Idempotent — only one thread is ever spawned.
///
/// We originally polled `NSWorkspace.frontmostApplication()` for this,
/// but in `--capture-primary` / virtual-display setups it stops tracking
/// dock-click activations entirely (the property pins to whatever was
/// front when the capture engaged and never updates, even though apps
/// really are becoming frontmost when the user clicks the dock on the
/// virtual display). AX's system-wide `kAXFocusedApplicationAttribute`
/// reflects WindowServer's actual focus state directly and tracks
/// these activations correctly.
///
/// Skips promotion when a Cmd+Tab cycle session is active so that the
/// AX activations *we* drive don't get re-promoted (mid-cycle reshuffles
/// would corrupt the frozen snapshot's MRU ordering on the next session
/// start). Our cycle commits already promote the cursor target on Cmd
/// release; intra-cycle targets shouldn't bake into MRU.
pub(super) fn start_workspace_mru_poller() {
    use std::sync::OnceLock;
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("ax-focus-mru-poller".into())
            .spawn(ax_focus_mru_poller_loop);
    });
}

/// Millis-since-process-start of the last forwarded RDP input event
/// (`u64::MAX` = none yet). One relaxed store per event from
/// `Inner::{keyboard,mouse}`; read by the MRU focus poller so its 5×/s
/// system-wide AX query parks while the session is input-idle — the MRU /
/// focus data it maintains only matters while a user is actually driving
/// input (Cmd+Tab, Ctrl→Cmd remap gating), and without the gate the
/// poller kept waking the process forever, including after the client
/// disconnected.
pub(super) static LAST_INPUT_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);

/// Monotonic millis since process start (first call anchors the epoch).
pub(super) fn monotonic_ms() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Record that an RDP input event was just forwarded.
pub(super) fn mark_input_activity() {
    LAST_INPUT_MS.store(monotonic_ms(), std::sync::atomic::Ordering::Relaxed);
}

/// True while input was seen within the last minute — the window during
/// which the MRU poller keeps its full 5 Hz cadence. Generous on purpose:
/// the poller's fallback focus data may be consulted by the very first
/// keystroke after a pause, and a poller parked ≤1 s behind (see the loop)
/// combined with the event-driven feeders (NSWorkspace observer + click
/// hit-test) covers that first keystroke fine.
pub(super) fn input_recently_active() -> bool {
    let last = LAST_INPUT_MS.load(std::sync::atomic::Ordering::Relaxed);
    last != u64::MAX && monotonic_ms().saturating_sub(last) < 60_000
}

/// Read the pid of the AX system-wide focused application. Returns
/// None if AX can't resolve it (permission missing, focus on a non-
/// AX-bearing element, etc.).
pub(super) fn ax_focused_application_pid() -> Option<libc::pid_t> {
    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::CFString;
    use std::ffi::c_void;
    use std::ptr;

    // SAFETY: AX and CF calls on objects created or copied here: each +1 reference (Create/Copy
    // rule) is checked for null before use and released exactly once on every path, and
    // borrowed array elements are only used while their array is alive.
    unsafe {
        let systemwide = AXUIElementCreateSystemWide();
        if systemwide.is_null() {
            return None;
        }
        let attr = CFString::from_static_string("AXFocusedApplication");
        let mut focused: CFTypeRef = ptr::null();
        let err = AXUIElementCopyAttributeValue(
            systemwide,
            attr.as_concrete_TypeRef().cast(),
            &mut focused,
        );
        CFRelease(systemwide as *const c_void);
        if err != AX_ERROR_SUCCESS || focused.is_null() {
            return None;
        }
        let mut pid: libc::pid_t = 0;
        let pid_err = AXUIElementGetPid(focused as *mut c_void, &mut pid);
        CFRelease(focused);
        if pid_err != AX_ERROR_SUCCESS || pid <= 0 {
            return None;
        }
        Some(pid)
    }
}

/// Look up the bundle identifier of a running app by pid. Used by
/// the AX poller to translate the focused-app pid into the bundle
/// string our MRU is keyed by. Uses
/// `NSRunningApplication.runningApplicationWithProcessIdentifier`
/// directly — that path does a fresh per-call lookup against
/// LaunchServices and sees newly-launched apps, unlike
/// `NSWorkspace.runningApplications()` which is cached and stale
/// in our (non-Cocoa-runloop) process.
pub(super) fn bundle_identifier_for_pid(pid: libc::pid_t) -> Option<String> {
    // Fast path: a normal app resolves directly via NSRunningApplication.
    // SAFETY: NSRunningApplication lookups take a pid by value and return retained objects or
    // None.
    let direct = unsafe {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .and_then(|a| a.bundleIdentifier())
            .map(|s| s.to_string())
    };
    if let Some(bid) = direct {
        if !bid.is_empty() {
            return Some(bid);
        }
    }
    // Helper path: AXFocusedApplication hands us Electron *helper* process
    // pids (VSCode's "Code Helper (Renderer)", Slack, etc.) that
    // NSRunningApplication returns no bundle id for — so a focused Electron
    // app would otherwise be invisible to both the focus poller and the
    // Ctrl→Cmd exclusion check. Derive the OWNING app's bundle id from the
    // process executable path's OUTERMOST `.app` bundle, so e.g. a focused
    // VSCode resolves to `com.microsoft.VSCode` regardless of which helper
    // holds focus.
    bundle_id_from_pid_path(pid)
}

/// Resolve a pid to the bundle id of the outermost `.app` containing its
/// executable (via `proc_pidpath` + `NSBundle`). Used as the helper-process
/// fallback in `bundle_identifier_for_pid`. Returns None if the path can't be
/// read or isn't inside an app bundle.
pub(super) fn bundle_id_from_pid_path(pid: libc::pid_t) -> Option<String> {
    extern "C" {
        fn proc_pidpath(
            pid: libc::c_int,
            buffer: *mut libc::c_void,
            buffersize: u32,
        ) -> libc::c_int;
    }
    // PROC_PIDPATHINFO_MAXSIZE = 4 * MAXPATHLEN.
    let mut buf = vec![0u8; 4096];
    // SAFETY: `buf` is 4096 bytes (PROC_PIDPATHINFO_MAXSIZE), which is the size passed, and
    // only the returned length is read.
    let n = unsafe {
        proc_pidpath(
            pid as libc::c_int,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len() as u32,
        )
    };
    if n <= 0 {
        return None;
    }
    let path = std::str::from_utf8(&buf[..n as usize]).ok()?;
    // The outermost app bundle is the FIRST ".app" component in the path
    // (e.g. ".../Visual Studio Code.app/Contents/Frameworks/Code Helper.app/
    // .../Code Helper" → ".../Visual Studio Code.app").
    let idx = path.find(".app/")?;
    let app_path = &path[..idx + 4];
    // SAFETY: `ns` is a valid NSString for the call; NSBundle returns retained objects or None.
    unsafe {
        let ns = objc2_foundation::NSString::from_str(app_path);
        let bundle = objc2_foundation::NSBundle::bundleWithPath(&ns)?;
        bundle
            .bundleIdentifier()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    }
}

/// Standalone-terminal bundle ids where the Ctrl→Cmd remap is always
/// suppressed, so `Ctrl+C` keeps reaching the shell as SIGINT. Embedded
/// terminals (VSCode's integrated TTY etc.) can't be auto-detected — the
/// frontmost app is the IDE, not a terminal — so they're covered by the
/// user-extensible --no-remap-apps list instead.
pub(super) const BUILTIN_TERMINAL_BUNDLES: &[&str] = &[
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "io.alacritty",
    "net.kovidgoyal.kitty",
    "com.github.wez.wezterm",
    "com.mitchellh.ghostty",
    "co.zeit.hyper",
    "dev.warp.Warp-Stable",
];

/// The app the user is currently interacting with (bundle id), as the Ctrl→Cmd
/// suppression's primary "what's focused right now" signal. Updated, last-wins,
/// by TWO sources because no single macOS API covers every case in this
/// headless/virtual-display context:
///   - the `NSWorkspace` activation observer (`init_focus_observer`) — fires on
///     Cmd+Tab and on clicking apps that activate normally; and
///   - a **click hit-test** (`update_focus_from_click`) — resolves the app whose
///     window is under the cursor at mouse-down, which is the ONLY way to catch
///     an Electron app (VSCode) clicked into in this setup: it takes key focus
///     (keystrokes land in it) WITHOUT emitting an activation, so neither
///     `AXFocusedApplication` nor `NSWorkspace` ever reports it.
pub(super) static LAST_FOCUS_BUNDLE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Register an `NSWorkspaceDidActivateApplicationNotification` observer so we
/// always know the current frontmost app. It fires on **every** activation
/// (including mouse-click focus and Electron apps), and the activated app is
/// carried in the notification's `userInfo` as an `NSRunningApplication`, whose
/// `bundleIdentifier` is the **main app** id (not an Electron helper). Runs on
/// the dedicated CFRunLoop thread because `NSWorkspace` notifications need a
/// live, pumped runloop — macrdp's main thread (owned by tokio) has none. The
/// observer token + block are leaked intentionally (process-lifetime).
///
/// This replaces the unreliable AX-poll / MRU-front guess that the Ctrl→Cmd
/// suppression used before: AX polling never reported VSCode focused by click,
/// so the remap wrongly fired in its integrated terminal.
pub(crate) fn init_focus_observer() {
    // SAFETY: runs on the dedicated run-loop thread, which pumps the CFRunLoop NSWorkspace
    // notifications need; the observer block and every object it captures are retained by the
    // notification center for the process lifetime.
    crate::runloop_thread::submit(|| unsafe {
        use objc2::rc::Retained;
        use objc2::runtime::AnyObject;
        use objc2_app_kit::{
            NSWorkspace, NSWorkspaceApplicationKey, NSWorkspaceDidActivateApplicationNotification,
        };
        use objc2_foundation::{NSNotification, NSString};
        use std::ptr::NonNull;

        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let block = block2::RcBlock::new(move |notif: NonNull<NSNotification>| {
            let notif: &NSNotification = notif.as_ref();
            let Some(info) = notif.userInfo() else { return };
            let key: &AnyObject = NSWorkspaceApplicationKey.as_ref();
            // The value is the activated NSRunningApplication; read its
            // bundleIdentifier directly (the main app id, not a helper).
            let Some(obj) = info.objectForKey(key) else {
                return;
            };
            let bid: Option<Retained<NSString>> = objc2::msg_send_id![&*obj, bundleIdentifier];
            if let Some(bid) = bid {
                let s = bid.to_string();
                if !s.is_empty() {
                    debug!(bundle = %s, "ctrl→cmd activation focus");
                    if let Ok(mut g) = LAST_FOCUS_BUNDLE.lock() {
                        *g = Some(s);
                    }
                }
            }
        });
        let token = center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidActivateApplicationNotification),
            None,
            None,
            &block,
        );
        std::mem::forget(token);
        std::mem::forget(block);
        debug!("ctrl→cmd: NSWorkspace activation observer registered");
    });
}

/// True when the frontmost app is a built-in terminal or appears in the user's
/// --no-remap-apps list — in which case the Ctrl→Cmd remap is suppressed (so
/// Ctrl+C stays SIGINT). Called on a remappable Ctrl-shortcut key-down.
///
/// Frontmost resolution priority: (1) `LAST_FOCUS_BUNDLE` — the last-wins merge
/// of the `NSWorkspace` activation observer AND the click hit-test, which
/// together cover Cmd+Tab, normal click-activation, AND Electron click-focus;
/// (2) a direct `AXFocusedApplication` lookup (returns None on this input
/// thread, but kept as a belt-and-suspenders); (3) the MRU front
/// (`mru_bundles()[0]`) the focus poller maintains.
///
/// Electron apps focused via a helper can surface a `.helper[.Renderer]`
/// suffixed id, so we match an exclusion entry by **equality OR a dotted-prefix**
/// (`<entry>.`) — listing `com.microsoft.VSCode` also covers its helpers; the
/// trailing dot keeps it from matching a sibling like `…VSCodeInsiders`.
pub(super) fn frontmost_is_excluded() -> bool {
    let bid = LAST_FOCUS_BUNDLE
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .or_else(|| ax_focused_application_pid().and_then(bundle_identifier_for_pid))
        .or_else(|| mru_bundles().lock().ok().and_then(|m| m.first().cloned()));
    let Some(bid) = bid else {
        return false;
    };
    let matches = |entry: &str| bid == entry || bid.starts_with(&format!("{entry}."));
    let excluded = BUILTIN_TERMINAL_BUNDLES.iter().any(|e| matches(e))
        || super::super::NO_REMAP_APPS
            .get()
            .is_some_and(|list| list.iter().any(|e| matches(e)));
    debug!(bundle = %bid, excluded, "ctrl→cmd frontmost check");
    excluded
}

/// Update `LAST_FOCUS_BUNDLE` from a mouse-down at the given GLOBAL screen
/// point (top-left origin — the same space `input.rs` posts mouse events in).
/// Hit-tests the window under the cursor via `AXUIElementCopyElementAtPosition`
/// and resolves its owning app's bundle id. This is the only signal that
/// catches an Electron app (VSCode) clicked into in a headless/virtual-display
/// session: it takes key focus without emitting an activation, so the
/// activation observer and `AXFocusedApplication` never see it. Best-effort —
/// runs on the dedicated CFRunLoop thread (where AX is reliable, unlike the
/// input thread) and updates the cell before the user's next keystroke. Only
/// armed when `--map-ctrl-to-cmd` is on (the only consumer). No-op if AX can't
/// resolve a window/app at the point (menu bar, empty space).
pub(super) fn update_focus_from_click(x: f64, y: f64) {
    // SAFETY: runs on the run-loop thread. AX and CF calls on objects created or copied here:
    // each +1 reference (Create/Copy rule) is checked for null before use and released exactly
    // once on every path, and borrowed array elements are only used while their array is alive.
    crate::runloop_thread::submit(move || unsafe {
        use std::ffi::c_void;
        let systemwide = AXUIElementCreateSystemWide();
        if systemwide.is_null() {
            return;
        }
        let mut element: *mut c_void = std::ptr::null_mut();
        let err = AXUIElementCopyElementAtPosition(systemwide, x as f32, y as f32, &mut element);
        CFRelease(systemwide as *const c_void);
        if err != AX_ERROR_SUCCESS || element.is_null() {
            return;
        }
        let mut pid: libc::pid_t = 0;
        let pid_err = AXUIElementGetPid(element, &mut pid);
        CFRelease(element as *const c_void);
        if pid_err != AX_ERROR_SUCCESS || pid <= 0 {
            return;
        }
        if let Some(bundle) = bundle_identifier_for_pid(pid) {
            if !bundle.is_empty() {
                debug!(pid, bundle = %bundle, "ctrl→cmd click focus");
                if let Ok(mut g) = LAST_FOCUS_BUNDLE.lock() {
                    *g = Some(bundle);
                }
            }
        }
    });
}

/// Find the pid of the first running app matching `bundle`. Walks
/// the kernel pid list and queries each via
/// NSRunningApplication; both calls bypass the stale NSWorkspace
/// cache. Returns None if the bundle isn't running.
pub(super) fn pid_for_bundle(bundle: &str) -> Option<libc::pid_t> {
    for pid in list_all_pids() {
        // SAFETY: NSRunningApplication lookups take a pid by value and return retained objects
        // or None.
        let app = unsafe { NSRunningApplication::runningApplicationWithProcessIdentifier(pid) };
        let Some(app) = app else { continue };
        // SAFETY: `app` is a retained NSRunningApplication.
        let bid = unsafe { app.bundleIdentifier() };
        let Some(b) = bid else { continue };
        if b.to_string() == bundle {
            return Some(pid);
        }
    }
    None
}

pub(super) fn ax_focus_mru_poller_loop() {
    let mut last_seen_pid: libc::pid_t = 0;
    loop {
        // Park while input-idle: no RDP input in the last minute (or ever)
        // means nobody is Cmd+Tab-ing or typing, so the system-wide AX
        // focus query 5×/s is pure wasted wakeups — including forever
        // after a disconnect. Check a cheap atomic at 1 Hz instead; the
        // first event after a pause restores full cadence within ~1 s.
        if !input_recently_active() {
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        std::thread::sleep(Duration::from_millis(200));
        let Some(pid) = ax_focused_application_pid() else {
            continue;
        };
        if pid == last_seen_pid {
            continue;
        }
        last_seen_pid = pid;

        // Skip while a Cmd+Tab cycle is in flight — our own AX
        // activations are causing the focus changes the poller is
        // seeing, and we don't want them double-counted into MRU.
        // (Cmd-release commit already promotes the final target.)
        let cycle_active = CYCLE_SESSION.lock_or_recover().is_some();
        if cycle_active {
            continue;
        }

        let Some(bundle) = bundle_identifier_for_pid(pid) else {
            continue;
        };
        if bundle.is_empty() {
            continue;
        }
        promote_bundle_to_front(&bundle);
        debug!(
            pid,
            bundle = %bundle,
            "AX focus MRU poller promoted bundle"
        );
    }
}
