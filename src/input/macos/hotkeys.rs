//! System shortcuts reimplemented in user space: Spotlight and screenshots.

use super::*;

/// Cmd+Space → invoke Spotlight. WindowServer's symbolic-hotkey
/// dispatcher doesn't fire for our CGEvent-posted Cmd+Space, and
/// AppleScript's `key code … using command down` route goes through
/// the same dispatcher so it doesn't help on macOS versions where
/// the dispatcher rejects synthesized events (confirmed empirically:
/// osascript exits cleanly with status 0 but the search UI never
/// opens).
///
/// Working path: AX-press the Spotlight menu-bar icon directly.
/// We already hold the Accessibility TCC grant (required for
/// CGEventPost), so no extra permission needed. Falls back to the
/// osascript route if the menu-bar press fails — and if even that
/// silently no-ops, the user's last resort is to rebind Spotlight
/// in System Settings → Keyboard → Shortcuts (a custom binding
/// goes through the normal app-shortcut path our CGEventPost
/// handles fine).
pub(super) fn invoke_spotlight() {
    debug!("invoke_spotlight: trying AX menu-bar press");
    if ax_press_spotlight_menu_extra() {
        debug!("invoke_spotlight: AX menu-bar press ok");
        return;
    }
    debug!("invoke_spotlight: AX menu-bar press failed, falling back to osascript");
    invoke_spotlight_via_osascript();
}

/// Open Spotlight by synthesizing a real mouse click on its
/// menu-bar magnifier icon. We tried AXPress on the icon first —
/// it returned `kAXErrorSuccess` but didn't actually open the
/// search UI on the macOS versions we tested, so the "press"
/// action seems to be a no-op for menu extras on those releases.
///
/// Mouse-click route: read the icon's screen-space AXPosition +
/// AXSize, compute its center, and CGEventPost a left
/// mouseDown/mouseUp pair there. Mouse events don't go through
/// the symbolic-hotkey dispatcher that rejects our synthesized
/// Cmd+Space, so this drives normal click-handling and actually
/// opens Spotlight.
pub(super) fn ax_press_spotlight_menu_extra() -> bool {
    use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex};
    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::CFString;
    use core_graphics::geometry::{CGPoint, CGSize};
    use std::ffi::c_void;
    use std::ptr;

    // AXValue type tags from <HIServices/AXValue.h>.
    const K_AX_VALUE_CG_POINT_TYPE: u32 = 1;
    const K_AX_VALUE_CG_SIZE_TYPE: u32 = 2;

    let Some(pid) = pid_for_bundle("com.apple.Spotlight") else {
        debug!("ax_press_spotlight: com.apple.Spotlight not running");
        return false;
    };
    // SAFETY: AX and CF calls on objects created or copied here: each +1 reference (Create/Copy
    // rule) is checked for null before use and released exactly once on every path, and
    // borrowed array elements are only used while their array is alive.
    unsafe {
        let app_ref = AXUIElementCreateApplication(pid);
        if app_ref.is_null() {
            debug!(pid, "ax_press_spotlight: null app_ref");
            return false;
        }

        // Accessory apps put their menu-bar icon under
        // `AXExtrasMenuBar`. Its children are the AXMenuBarItem
        // entries — for Spotlight there's just one (the magnifier).
        let extras_attr = CFString::from_static_string("AXExtrasMenuBar");
        let mut extras: CFTypeRef = ptr::null();
        let copy_err = AXUIElementCopyAttributeValue(
            app_ref,
            extras_attr.as_concrete_TypeRef().cast(),
            &mut extras,
        );
        if copy_err != AX_ERROR_SUCCESS || extras.is_null() {
            debug!(
                pid,
                copy_err, "ax_press_spotlight: AXExtrasMenuBar copy failed"
            );
            CFRelease(app_ref as *const c_void);
            return false;
        }

        let children_attr = CFString::from_static_string("AXChildren");
        let mut children: CFTypeRef = ptr::null();
        let children_err = AXUIElementCopyAttributeValue(
            extras as *mut c_void,
            children_attr.as_concrete_TypeRef().cast(),
            &mut children,
        );
        if children_err != AX_ERROR_SUCCESS || children.is_null() {
            debug!(
                pid,
                children_err, "ax_press_spotlight: AXChildren copy failed"
            );
            CFRelease(extras);
            CFRelease(app_ref as *const c_void);
            return false;
        }

        let count = CFArrayGetCount(children.cast());
        debug!(pid, count, "ax_press_spotlight: extras menu-bar children");
        if count < 1 {
            CFRelease(children);
            CFRelease(extras);
            CFRelease(app_ref as *const c_void);
            return false;
        }

        let item = CFArrayGetValueAtIndex(children.cast(), 0) as *mut c_void;

        // Pull AXPosition (top-left of the icon, screen-space).
        let pos_attr = CFString::from_static_string("AXPosition");
        let mut pos_value: CFTypeRef = ptr::null();
        let pos_err = AXUIElementCopyAttributeValue(
            item,
            pos_attr.as_concrete_TypeRef().cast(),
            &mut pos_value,
        );
        if pos_err != AX_ERROR_SUCCESS || pos_value.is_null() {
            debug!(pid, pos_err, "ax_press_spotlight: AXPosition copy failed");
            CFRelease(children);
            CFRelease(extras);
            CFRelease(app_ref as *const c_void);
            return false;
        }
        let mut pos = CGPoint { x: 0.0, y: 0.0 };
        let pos_ok = AXValueGetValue(
            pos_value,
            K_AX_VALUE_CG_POINT_TYPE,
            &mut pos as *mut _ as *mut c_void,
        );
        CFRelease(pos_value);
        if pos_ok == 0 {
            debug!(pid, "ax_press_spotlight: AXValueGetValue(point) failed");
            CFRelease(children);
            CFRelease(extras);
            CFRelease(app_ref as *const c_void);
            return false;
        }

        // Pull AXSize so we can click the icon center.
        let size_attr = CFString::from_static_string("AXSize");
        let mut size_value: CFTypeRef = ptr::null();
        let size_err = AXUIElementCopyAttributeValue(
            item,
            size_attr.as_concrete_TypeRef().cast(),
            &mut size_value,
        );
        let size = if size_err == AX_ERROR_SUCCESS && !size_value.is_null() {
            let mut s = CGSize {
                width: 0.0,
                height: 0.0,
            };
            let ok = AXValueGetValue(
                size_value,
                K_AX_VALUE_CG_SIZE_TYPE,
                &mut s as *mut _ as *mut c_void,
            );
            CFRelease(size_value);
            if ok != 0 {
                s
            } else {
                // Best-effort fallback — typical menu-bar icon is
                // ~24x24 points.
                CGSize {
                    width: 24.0,
                    height: 24.0,
                }
            }
        } else {
            CGSize {
                width: 24.0,
                height: 24.0,
            }
        };

        let center = CGPoint {
            x: pos.x + size.width / 2.0,
            y: pos.y + size.height / 2.0,
        };
        debug!(
            pid,
            center_x = center.x,
            center_y = center.y,
            "ax_press_spotlight: clicking icon center"
        );

        CFRelease(children);
        CFRelease(extras);
        CFRelease(app_ref as *const c_void);

        // Build a CGEvent source — fresh for this click so we
        // don't tangle with the keyboard-input source state.
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) else {
            debug!("ax_press_spotlight: CGEventSource::new failed");
            return false;
        };
        // Move the cursor to the icon BEFORE the click. Without
        // this, menu-bar items often reject the synthesized click
        // (they hit-test against actual cursor position, not the
        // event's CG point). The mouseMoved + mouseDown + mouseUp
        // sequence mirrors what a real cursor-driven click sends.
        // Also clear modifier flags on every event — Cmd is still
        // held from the user's Cmd+Space chord, and a Cmd-modified
        // menu-bar click has different semantics than a plain one.
        let moved = CGEvent::new_mouse_event(
            source.clone(),
            CGEventType::MouseMoved,
            center,
            CGMouseButton::Left,
        );
        let down = CGEvent::new_mouse_event(
            source.clone(),
            CGEventType::LeftMouseDown,
            center,
            CGMouseButton::Left,
        );
        let up = CGEvent::new_mouse_event(
            source,
            CGEventType::LeftMouseUp,
            center,
            CGMouseButton::Left,
        );
        match (moved, down, up) {
            (Ok(m), Ok(d), Ok(u)) => {
                m.set_flags(CGEventFlags::empty());
                d.set_flags(CGEventFlags::empty());
                u.set_flags(CGEventFlags::empty());
                m.post(CGEventTapLocation::HID);
                d.post(CGEventTapLocation::HID);
                u.post(CGEventTapLocation::HID);
                debug!("ax_press_spotlight: mouse move+click posted");
                true
            }
            _ => {
                debug!("ax_press_spotlight: CGEvent::new_mouse_event failed");
                false
            }
        }
    }
}

pub(super) fn invoke_spotlight_via_osascript() {
    debug!("invoke_spotlight: spawning osascript");
    let script = r#"tell application "System Events" to key code 49 using {command down}"#;
    let res = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    match res {
        Ok(child) => {
            let child_pid = child.id();
            debug!(child_pid, "invoke_spotlight: osascript spawned");
            // Reap on a background thread and log the exit status +
            // any stderr so we can distinguish TCC denials, missing
            // automation grants, etc. from a real launch.
            std::thread::Builder::new()
                .name("invoke-spotlight-wait".into())
                .spawn(move || match child.wait_with_output() {
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        debug!(
                            child_pid,
                            status = ?out.status,
                            stdout = %stdout.trim(),
                            stderr = %stderr.trim(),
                            "invoke_spotlight: osascript exited"
                        );
                    }
                    Err(e) => {
                        warn!(
                            child_pid,
                            error = %e,
                            "invoke_spotlight: wait on osascript failed"
                        );
                    }
                })
                .ok();
        }
        Err(e) => {
            warn!(error = %e, "invoke_spotlight: osascript spawn failed");
        }
    }
}

/// Run /usr/sbin/screencapture with the given args. `-x` = silent
/// (no shutter sound), `-i` = interactive region. Output path is
/// the default (~/Desktop) when not provided.
pub(super) fn screencapture(args: &[&str]) {
    // Default to the same path Cmd+Shift+3 produces natively.
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let Some(home) = home else {
        warn!("screencapture: $HOME unset");
        return;
    };
    let now = chrono_like_filename();
    let path = home.join("Desktop").join(format!("Screenshot {now}.png"));
    let res = Command::new("/usr/sbin/screencapture")
        .args(args)
        .arg(&path)
        .spawn();
    if let Err(e) = res {
        warn!(error = %e, "screencapture spawn failed");
    }
}

pub(super) fn open_screenshot_app() {
    let res = Command::new("/usr/bin/open")
        .arg("-a")
        .arg("/System/Applications/Utilities/Screenshot.app")
        .spawn();
    if let Err(e) = res {
        warn!(error = %e, "open Screenshot.app failed");
    }
}

/// "2026-05-18 at 14.32.05" — matches macOS's native screenshot
/// filename suffix closely enough that the result feels indigenous
/// in Finder's sort order.
pub(super) fn chrono_like_filename() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Local-time conversion without pulling in chrono: use libc.
    // SAFETY: `libc::tm` is a plain C struct for which all-zero bytes are a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = secs as libc::time_t;
    // SAFETY: `t` and `tm` are valid for reading and writing; localtime_r is the re-entrant
    // form.
    unsafe {
        libc::localtime_r(&t, &mut tm);
    }
    format!(
        "{:04}-{:02}-{:02} at {:02}.{:02}.{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}
