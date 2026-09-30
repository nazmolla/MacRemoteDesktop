//! Window management through Accessibility: gathering stranded windows onto
//! the session display, and raising, unminimizing and cycling windows.

use super::*;

/// Gather "stranded" windows — ones sitting mostly off the display the RDP
/// client sees — back onto that display. Under `--capture-primary` (and the
/// other headless modes) a window opened on the physical panel keeps its old
/// global coordinates, which fall outside the virtual display's region, so
/// it's invisible/unclickable over RDP. Triggered on demand by Ctrl+Option+G,
/// this moves every such window's top-left just inside the target display.
/// A window is left untouched only if at least **half its area** is already on
/// the target, so a window the user positioned on the virtual display is never
/// disturbed, while one straddling the vd/physical boundary (mostly on the
/// blanked panel, poking a sliver onto the vd) is still swept rather than left
/// half-cut. Only *regular* (Dock) GUI apps are considered, so menu-extras /
/// system panels are never yanked around. Best-effort AX — macrdp already holds
/// the Accessibility grant (for `CGEventPost`), so no extra permission/prompt.
/// Returns how many windows were moved. Triggered on demand by the
/// Ctrl+Option+G hotkey (see `try_symbolic_hotkey`).
pub(crate) fn gather_windows_onto_display(display_id: u32) -> usize {
    // CFRelease + CGPoint are already in scope (the mod-level extern block /
    // `use` above); importing them here would shadow.
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::string::CFString;
    use std::ffi::c_void;
    use std::ptr;

    // AXValue type tags (AXValue.h): CGPoint = 1, CGSize = 2.
    const K_AX_VALUE_CGPOINT_TYPE: u32 = 1;
    const K_AX_VALUE_CGSIZE_TYPE: u32 = 2;

    // Target rect in global top-left-origin coords (the display's live bounds).
    let b = CGDisplay::new(display_id).bounds();
    let (tx, ty, tw, th) = (b.origin.x, b.origin.y, b.size.width, b.size.height);
    if tw <= 0.0 || th <= 0.0 {
        return 0;
    }
    // Land stranded windows a little inside the top-left so the title bar is
    // grabbable.
    let (nx, ny) = (tx + 40.0, ty + 40.0);

    // Read an AXValue-typed attribute (AXPosition/AXSize) as a coordinate pair.
    let read_pair = |win: *const c_void, attr: &CFString, tag: u32| -> Option<(f64, f64)> {
        // SAFETY: `win` is an AX window element borrowed from an array that outlives this
        // closure; the copied AXValue is checked for null and type, read into a local, and
        // released once.
        unsafe {
            let mut v: CFTypeRef = ptr::null();
            if AXUIElementCopyAttributeValue(
                win as *mut c_void,
                attr.as_concrete_TypeRef().cast(),
                &mut v,
            ) != AX_ERROR_SUCCESS
                || v.is_null()
            {
                return None;
            }
            let mut pair = [0f64; 2];
            let ok = AXValueGetValue(v, tag, pair.as_mut_ptr().cast());
            CFRelease(v.cast());
            (ok != 0).then_some((pair[0], pair[1]))
        }
    };

    let pos_attr = CFString::from_static_string("AXPosition");
    let size_attr = CFString::from_static_string("AXSize");
    let windows_attr = CFString::from_static_string("AXWindows");

    let mut moved = 0usize;
    for pid in list_all_pids() {
        // Only regular (Dock) GUI apps — skip agents/daemons/accessory apps.
        // SAFETY: NSRunningApplication lookups take a pid by value and return retained objects
        // or None.
        let is_regular = unsafe {
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                .map(|a| a.activationPolicy() == NSApplicationActivationPolicy::Regular)
                .unwrap_or(false)
        };
        if !is_regular {
            continue;
        }
        // SAFETY: AX and CF calls on objects created or copied here: each +1 reference
        // (Create/Copy rule) is checked for null before use and released exactly once on every
        // path, and borrowed array elements are only used while their array is alive.
        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() {
                continue;
            }
            let mut arr: CFTypeRef = ptr::null();
            if AXUIElementCopyAttributeValue(
                app,
                windows_attr.as_concrete_TypeRef().cast(),
                &mut arr,
            ) == AX_ERROR_SUCCESS
                && !arr.is_null()
            {
                let n = CFArrayGetCount(arr.cast());
                for i in 0..n {
                    // Array elements are borrowed (not retained).
                    let win = CFArrayGetValueAtIndex(arr.cast(), i);
                    if win.is_null() {
                        continue;
                    }
                    let Some((px, py)) = read_pair(win, &pos_attr, K_AX_VALUE_CGPOINT_TYPE) else {
                        continue;
                    };
                    // Default a missing size to a point, so a window still
                    // counts as stranded by its origin alone.
                    let (sw, sh) =
                        read_pair(win, &size_attr, K_AX_VALUE_CGSIZE_TYPE).unwrap_or((1.0, 1.0));
                    // How much of the window lies on the target display?
                    // Skip a window only if at least HALF its area is already
                    // there — so a window straddling the vd/physical boundary
                    // (mostly on the blanked panel, poking a sliver onto the vd)
                    // is still swept, instead of being left half-cut. A window
                    // fully off the target overlaps by 0 and is always moved; a
                    // window fully on it overlaps 100% and is always left.
                    let ix = (px + sw).min(tx + tw) - px.max(tx);
                    let iy = (py + sh).min(ty + th) - py.max(ty);
                    let on_target = ix.max(0.0) * iy.max(0.0);
                    let win_area = (sw * sh).max(1.0);
                    if on_target >= win_area * 0.5 {
                        // Mostly (or fully) visible on the target — leave it be.
                        continue;
                    }
                    // Stranded (or mostly off) — move its top-left onto the target.
                    let np = CGPoint::new(nx, ny);
                    let val =
                        AXValueCreate(K_AX_VALUE_CGPOINT_TYPE, (&np as *const CGPoint).cast());
                    if !val.is_null() {
                        if AXUIElementSetAttributeValue(
                            win as *mut c_void,
                            pos_attr.as_concrete_TypeRef().cast(),
                            val,
                        ) == AX_ERROR_SUCCESS
                        {
                            moved += 1;
                        }
                        CFRelease(val.cast());
                    }
                }
                CFRelease(arr.cast());
            }
            CFRelease(app.cast());
        }
    }
    moved
}

/// Activate the target app via AX (kAXFrontmost) AND explicitly
/// raise its focused window. The two-step gesture matters for apps
/// with multiple windows in one process (the canonical case is two
/// VSCode projects opened via File→New Window — same pid, two
/// windows): setting kAXFrontmost activates the *process*, but
/// macOS picks which window pops up, and it doesn't always pick
/// the one the user was last working in. Following up with an
/// AXRaise on `AXFocusedWindow` pins the result to the window AX
/// tracks as focused — i.e. the one the user actually touched
/// last. Without this the user perceives the cycle as "switching
/// between two VSCodes" because the cycle keeps activating the
/// same app but a different window pops up each time.
/// Re-assert a just-un-minimized app to the front after its genie animation
/// finishes (~0.4 s). Setting AXMain/AXRaise synchronously right after
/// AXMinimized=false races the animation and can leave the window behind
/// others; a delayed second raise lands reliably. Runs on a detached thread so
/// it never blocks the input path; by then the window is no longer minimized,
/// so the re-call takes the normal raise+AXMain path. Only scheduled when we
/// actually un-minimized something, so it won't yank focus otherwise.
pub(super) fn schedule_refront(pid: libc::pid_t) {
    std::thread::Builder::new()
        .name("macrdp-refront".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(420));
            ax_make_frontmost(pid, false, false);
        })
        .ok();
}

pub(super) fn ax_make_frontmost(
    pid: libc::pid_t,
    unminimize: bool,
    reopen_if_windowless: bool,
) -> i32 {
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::string::CFString;
    use std::ffi::c_void;
    use std::ptr;

    // SAFETY: AXUIElementCreateApplication takes a pid by value; the +1 result is checked for
    // null here and released once at the end of this function.
    let app_ref = unsafe { AXUIElementCreateApplication(pid) };
    if app_ref.is_null() {
        return AX_ERROR_ILLEGAL_ARGUMENT;
    }

    // Native Cmd+Tab unhides a hidden (Cmd+H) app when you switch to it.
    // AXFrontmost activates the process/menu bar but does NOT surface a
    // window of a hidden app, so it'd appear to "not come forward." Unhide
    // first; no-op when not hidden. Also grab the bundle id for the
    // windowless-reopen fallback below.
    let mut bundle_id: Option<String> = None;
    // SAFETY: NSRunningApplication lookups take a pid by value and return retained objects or
    // None.
    unsafe {
        if let Some(running) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            if running.isHidden() {
                running.unhide();
            }
            bundle_id = running.bundleIdentifier().map(|s| s.to_string());
        }
    }

    let frontmost_attr = CFString::from_static_string("AXFrontmost");
    let true_val = CFBoolean::true_value();
    // SAFETY: `app_ref` is non-null and live; the attribute name and value are CF objects that
    // outlive the call.
    let frontmost_err = unsafe {
        AXUIElementSetAttributeValue(
            app_ref,
            frontmost_attr.as_concrete_TypeRef().cast(),
            true_val.as_CFTypeRef() as CFTypeRef,
        )
    };

    // Raise + main a window of the app. `kAXFrontmost` activates the
    // *process* (menu bar), but a window only surfaces if we raise one —
    // and `AXFocusedWindow` is **null for an app the user hasn't interacted
    // with on this Space** (the common case right after `--detach-primary`
    // evacuates everyone onto the virtual display). When that happens,
    // `kAXFrontmost` succeeds but nothing visibly comes forward, so the
    // cycle appears to "only switch between the 2 recently-touched apps"
    // until a real mouse click populates each app's focused window.
    //
    // Fix: fall back AXFocusedWindow -> AXMainWindow -> AXWindows[0], then
    // both AXRaise it AND set AXMain=true. AXMain is what actually moves
    // window stacking for apps where AXRaise alone is a no-op (Electron —
    // VSCode/Slack/etc.). All best-effort: the caller only cares that the
    // process was activated.
    // Restore a minimized window before raising — AXRaise does NOT
    // de-miniaturize, so without this, landing on an all-minimized app shows
    // nothing. We un-minimize the landed window when EITHER --unminimize-on-
    // switch is set (which also un-minimizes per cycle step) OR this is the
    // commit/landing call (`reopen_if_windowless`): the app you release on
    // should always surface — consistent with reopening a windowless app on
    // landing. Per intermediate cycle steps we still leave minimized windows
    // alone (no flag), so apps cycled *through* don't flicker open.
    // AXMinimized=false is idempotent on a non-minimized window.
    let effective_unminimize = unminimize || reopen_if_windowless;
    // Set when we actually de-miniaturize a window: un-minimize is an async
    // (genie) animation, so the AXRaise/AXMain we do immediately afterward
    // races it and the window can land BEHIND others. We re-assert front once
    // the animation has settled (see schedule_refront below).
    let did_unminimize = std::cell::Cell::new(false);
    let min_attr = CFString::from_static_string("AXMinimized");
    // SAFETY: `win` is an AX window element the caller keeps alive for this call; the copied
    // minimized value is released once.
    let raise_and_main = |win: *mut c_void| unsafe {
        // Is this window minimized?
        let mut mv: CFTypeRef = ptr::null();
        let is_min =
            AXUIElementCopyAttributeValue(win, min_attr.as_concrete_TypeRef().cast(), &mut mv)
                == AX_ERROR_SUCCESS
                && !mv.is_null()
                && CFBooleanGetValue(mv) != 0;
        if !mv.is_null() {
            CFRelease(mv.cast());
        }
        if is_min {
            // On intermediate cycle steps, leave a minimized window alone
            // (no flicker for apps merely cycled through).
            if !effective_unminimize {
                return;
            }
            // De-miniaturize, then fall through to the same raise + AXMain as a
            // normal window. Un-minimizing alone (or AXRaise alone) leaves the
            // window BEHIND others on some apps (e.g. Calendar) — AXMain=true is
            // what actually moves it to the front of the stack.
            let false_val = CFBoolean::false_value();
            AXUIElementSetAttributeValue(
                win,
                min_attr.as_concrete_TypeRef().cast(),
                false_val.as_CFTypeRef() as CFTypeRef,
            );
            did_unminimize.set(true);
        }
        let raise = CFString::from_static_string("AXRaise");
        AXUIElementPerformAction(win, raise.as_concrete_TypeRef().cast());
        let main_attr = CFString::from_static_string("AXMain");
        AXUIElementSetAttributeValue(
            win,
            main_attr.as_concrete_TypeRef().cast(),
            true_val.as_CFTypeRef() as CFTypeRef,
        );
    };

    let mut raised_via: &str = "none";
    // Single-window attributes first (own a +1 retain → release after use).
    for attr in ["AXFocusedWindow", "AXMainWindow"] {
        let a = CFString::new(attr);
        let mut win: CFTypeRef = ptr::null();
        // SAFETY: `app_ref` is live; `a` outlives the call and `win` is a valid out-pointer.
        let err = unsafe {
            AXUIElementCopyAttributeValue(app_ref, a.as_concrete_TypeRef().cast(), &mut win)
        };
        if err == AX_ERROR_SUCCESS && !win.is_null() {
            raise_and_main(win as *mut c_void);
            // SAFETY: `win` is the non-null +1 value copied above, released once.
            unsafe { CFRelease(win.cast()) };
            raised_via = attr;
            break;
        }
    }
    // Fall back to the first window in AXWindows. Array elements are
    // borrowed (not retained), so raise while the array is still alive.
    if raised_via == "none" {
        let a = CFString::new("AXWindows");
        let mut arr: CFTypeRef = ptr::null();
        // SAFETY: `app_ref` is live; `a` outlives the call and `arr` is a valid out-pointer.
        let err = unsafe {
            AXUIElementCopyAttributeValue(app_ref, a.as_concrete_TypeRef().cast(), &mut arr)
        };
        if err == AX_ERROR_SUCCESS && !arr.is_null() {
            // SAFETY: `arr` is the non-null +1 array copied above.
            if unsafe { CFArrayGetCount(arr.cast()) } > 0 {
                // SAFETY: the index is 0 and the count was checked to be above 0; the element
                // is borrowed while `arr` is alive.
                let w = unsafe { CFArrayGetValueAtIndex(arr.cast(), 0) };
                if !w.is_null() {
                    raise_and_main(w as *mut c_void);
                    raised_via = "AXWindows[0]";
                }
            }
            // SAFETY: `arr` is the non-null +1 array copied above, released once.
            unsafe { CFRelease(arr.cast()) };
        }
    }
    debug!(pid, raised_via, "ax_make_frontmost: window raise");

    // If we de-miniaturized the landed window, re-assert front after the
    // un-minimize animation settles — the immediate raise above races the
    // genie animation and can leave the window behind others (Calendar).
    if did_unminimize.get() && reopen_if_windowless {
        schedule_refront(pid);
    }

    // Running app with NO window (Notes/Calendar/Mail et al. with their
    // window closed): AXFrontmost activated the menu bar but there was
    // nothing to raise, so the app appears "not to come forward." Trigger the
    // app's reopen via LaunchServices (like clicking its Dock icon / what
    // native Cmd+Tab does), which creates a window. Gated to the landing app
    // (`reopen_if_windowless`, set only on commit) so apps merely *cycled
    // through* don't pop windows; only fires when no window was found, so
    // apps that already surfaced one are untouched. `open` re-launches a
    // since-died bundle, but the user is switching *to* it, so that's fine.
    if raised_via == "none" && reopen_if_windowless {
        if let Some(b) = &bundle_id {
            let _ = std::process::Command::new("/usr/bin/open")
                .args(["-b", b])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            debug!(pid, bundle = %b, "ax_make_frontmost: reopen windowless app via open -b");
        }
    }

    // SAFETY: `app_ref` was created non-null at the top of this function and is released once
    // here.
    unsafe { CFRelease(app_ref as *const c_void) };
    frontmost_err
}

/// Cycle through windows of the currently-frontmost app, the way
/// native Cmd+` does. Pulls the app's window list via
/// `kAXWindowsAttribute`, locates `kAXFocusedWindow` in that list,
/// and AXRaises the next (or previous) one. Returns `false` if the
/// app has fewer than two windows so the caller can fall back to a
/// no-op rather than re-raising the same window.
pub(super) fn ax_cycle_windows_of_front(reverse: bool) -> bool {
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::string::CFString;
    use std::ffi::c_void;
    use std::ptr;

    // Source-of-truth for "what app is the user actually in": prefer the
    // AX system-wide focused-application pid (`ax_focused_application_pid`,
    // the same primitive the MRU poller uses) over
    // `NSWorkspace.frontmostApplication`. NSWorkspace stops tracking
    // plain click-driven focus changes (Dock/window clicks) under
    // `--virtual-display`/`--capture-primary` — it pins to whatever was
    // front when headless engaged — while AX's system-wide focus follows
    // real WindowServer focus (and our own AX activations) directly. Fall
    // back to workspace + the LAST_AX_ACTIVATED_PID/WORKSPACE_LIE_FRONT
    // reconciliation (`effective_front_pid`) only if AX can't resolve a
    // focused app at all (e.g. a transient AX hiccup).
    // SAFETY: `ax_focused_application_pid` only makes balanced AX calls (see its own safety
    // note).
    let pid = unsafe {
        match ax_focused_application_pid() {
            Some(ax_pid) => ax_pid,
            None => {
                let ws = objc2_app_kit::NSWorkspace::sharedWorkspace();
                let workspace_pid = match ws.frontmostApplication() {
                    Some(app) => app.processIdentifier(),
                    None => return false,
                };
                effective_front_pid(workspace_pid, |p| libc::kill(p, 0) == 0)
            }
        }
    };
    // SAFETY: AXUIElementCreateApplication takes a pid by value; the +1 result is checked for
    // null and released on every return path below.
    let app_ref = unsafe { AXUIElementCreateApplication(pid) };
    if app_ref.is_null() {
        debug!(pid, "ax_cycle_windows_of_front: null app_ref (bad pid)");
        return false;
    }

    let windows_attr = CFString::from_static_string("AXWindows");
    let mut windows: CFTypeRef = ptr::null();
    // SAFETY: `app_ref` is non-null; the attribute outlives the call and `windows` is a valid
    // out-pointer.
    let copy_err = unsafe {
        AXUIElementCopyAttributeValue(
            app_ref,
            windows_attr.as_concrete_TypeRef().cast(),
            &mut windows,
        )
    };
    if copy_err != AX_ERROR_SUCCESS || windows.is_null() {
        debug!(
            pid,
            copy_err,
            windows_null = windows.is_null(),
            "ax_cycle_windows_of_front: AXWindows copy failed"
        );
        // SAFETY: `app_ref` is non-null and released once on this early return.
        unsafe { CFRelease(app_ref as *const c_void) };
        return false;
    }
    // SAFETY: `windows` is the non-null +1 array copied above.
    let count = unsafe { CFArrayGetCount(windows.cast()) };
    if count < 2 {
        debug!(
            pid,
            count, "ax_cycle_windows_of_front: app has <2 windows — nothing to cycle"
        );
        // SAFETY: both are non-null +1 references, released once on this early return.
        unsafe {
            CFRelease(windows.cast());
            CFRelease(app_ref as *const c_void);
        }
        return false;
    }

    // Find the focused window's index in the array. If we can't
    // resolve it, fall back to position 0 — pressing Cmd+` ought to
    // do *something* visible.
    let focused_attr = CFString::from_static_string("AXFocusedWindow");
    let mut focused: CFTypeRef = ptr::null();
    // SAFETY: `app_ref` is non-null; the attribute outlives the call and `focused` is a valid
    // out-pointer.
    let _ = unsafe {
        AXUIElementCopyAttributeValue(
            app_ref,
            focused_attr.as_concrete_TypeRef().cast(),
            &mut focused,
        )
    };
    let mut focused_idx: isize = 0;
    if !focused.is_null() {
        for i in 0..count {
            // SAFETY: `i` is below the array's count; the element is borrowed while `windows`
            // is alive.
            let w = unsafe { CFArrayGetValueAtIndex(windows.cast(), i) };
            // SAFETY: both are live CF objects.
            if unsafe { CFEqual(w, focused) } != 0 {
                focused_idx = i;
                break;
            }
        }
        // SAFETY: `focused` is the non-null +1 value copied above, released once.
        unsafe { CFRelease(focused.cast()) };
    }
    let next_idx = if reverse {
        (focused_idx + count - 1) % count
    } else {
        (focused_idx + 1) % count
    };
    // SAFETY: `next_idx` is a remainder of the count, so it is in range; the element is
    // borrowed while `windows` is alive.
    let next_window = unsafe { CFArrayGetValueAtIndex(windows.cast(), next_idx) };
    // Multi-strategy window raise. Native Cocoa apps (Terminal,
    // Finder) honor AXRaise on a window — that alone is enough.
    // Electron apps (VSCode, Slack, Discord) expose AXRaise but
    // the implementation is a no-op on most versions; for those
    // we need to:
    //   - Set AXMain=true on the target window (Electron's window
    //     controller activates the window when this transitions).
    //   - Set the app's AXMainWindow attribute to point at the
    //     target window (canonical "make this the main window"
    //     gesture that some AX bridges only honor at the app level).
    // We apply all three and log which succeeded so it's obvious
    // from the trace which path actually moved the window.
    let raise_action = CFString::from_static_string("AXRaise");
    let main_attr = CFString::from_static_string("AXMain");
    let main_window_attr = CFString::from_static_string("AXMainWindow");
    let true_val = CFBoolean::true_value();
    // SAFETY: `next_window` is borrowed from `windows`, which is alive; the action name
    // outlives the call.
    let raise_err = unsafe {
        AXUIElementPerformAction(
            next_window as *mut c_void,
            raise_action.as_concrete_TypeRef().cast(),
        )
    };
    // SAFETY: `next_window` is borrowed from `windows`, which is alive; attribute and value
    // outlive the call.
    let set_main_err = unsafe {
        AXUIElementSetAttributeValue(
            next_window as *mut c_void,
            main_attr.as_concrete_TypeRef().cast(),
            true_val.as_CFTypeRef() as CFTypeRef,
        )
    };
    // SAFETY: `app_ref` is live and `next_window` is borrowed from `windows`, which is alive.
    let set_main_window_err = unsafe {
        AXUIElementSetAttributeValue(
            app_ref,
            main_window_attr.as_concrete_TypeRef().cast(),
            next_window.cast(),
        )
    };
    debug!(
        pid,
        count,
        focused_idx,
        next_idx,
        raise_err,
        set_main_err,
        set_main_window_err,
        reverse,
        "ax_cycle_windows_of_front"
    );

    // SAFETY: both are the +1 references created above, released once.
    unsafe {
        CFRelease(windows.cast());
        CFRelease(app_ref as *const c_void);
    }
    raise_err == AX_ERROR_SUCCESS
}
