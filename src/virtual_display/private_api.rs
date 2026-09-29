//! All touches to undocumented `CGVirtualDisplay*` private API live here.
//!
//! When Apple renames or restructures the surface in a future macOS,
//! this is the only file that should need to change. The public side
//! (in `mod.rs`) talks to this layer through stable Rust types only —
//! no `*mut AnyObject`, no private class names leak out.
//!
//! ## API surface
//!
//! On macOS 26+ the surface is a fully Obj-C class `CGVirtualDisplay`
//! with `-initWithDescriptor:` / `-applySettings:` / `-displayID`. The
//! older C functions `CGVirtualDisplayCreate` /
//! `CGVirtualDisplayApplySettings` / `CGVirtualDisplayGetDisplayID`
//! that BetterDisplay's wrappers cover were removed/renamed at some
//! point during the macOS 16/26 cycle. We don't currently support the
//! older path — if you need it, add a fallback in `create()` that
//! detects class absence via `AnyClass::get` and reaches for `dlsym`
//! on the C symbols instead.
//!
//! ## Maintenance contract
//!
//! - All Obj-C classes resolved via `AnyClass::get(...)`. A missing
//!   class surfaces as an `Err` naming what's gone.
//! - Selectors used: `alloc`, `init`, `release`, `setName:`,
//!   `setMaxPixelsWide:`, `setMaxPixelsHigh:`, `setSizeInMillimeters:`,
//!   `setProductID:`, `setVendorID:`, `setSerialNum:`,
//!   `initWithDescriptor:`, `initWithWidth:height:refreshRate:`,
//!   `arrayWithObject:`, `setHiDPI:`, `setModes:`, `applySettings:`,
//!   `displayID`. Confirmed against macOS 26.4 (`dyld_info -exports`
//!   on `/System/Library/Frameworks/CoreGraphics.framework/...`).

#![cfg(target_os = "macos")]

use std::ffi::{c_void, CString};

use anyhow::{anyhow, Result};
use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::string::CFString;
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{CGPoint, CGSize, NSString};

type CGDirectDisplayID = u32;

extern "C" {
    fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFBooleanGetValue(boolean: CFTypeRef) -> u8;
}

/// `CGSessionCopyCurrentDictionary()` — undocumented CoreGraphics/SkyLight
/// symbol, the same "CGS" private-API family as `CGSConfigureDisplayEnabled`
/// above. Returns a dictionary describing the current login-session state;
/// the also-undocumented `CGSSessionScreenIsLocked` key reads `1` while the
/// screen is locked. Used only as a READ gate before the reconnect-time
/// auto-unlock attempt in main.rs.
///
/// Returns `None` when the lookup itself couldn't be completed (missing
/// symbol, or a null dictionary) — genuinely "unknown," not "not locked."
/// **This distinction is load-bearing at the call site:** `attempt_auto_unlock`
/// treats a confirmed `Some(false)` as a lock-cycle boundary and resets the
/// shared per-lock submission budget there, but must NOT do that on a mere
/// lookup failure — an intermittently-flaky symbol would otherwise reset the
/// budget on every failed read, defeating the cap `AUTO_UNLOCK_MAX_SUBMISSIONS`
/// exists to enforce. Deciding *whether to attempt* an unlock can still fail
/// toward "skip" on `None` (safe, same as before); it's specifically the
/// budget-reset side that needed the tri-state.
///
/// **If Apple ever removes this** (as happened to the `CGSession` *binary*
/// on macOS 26 — see docs/known-quirks.md, the `--lock-on-disconnect`
/// entry), the documented longer-lived alternative is watching the
/// `com.apple.screenIsLocked` / `com.apple.screenIsUnlocked` distributed
/// notifications (`NSDistributedNotificationCenter`) instead — an older
/// mechanism with a longer track record, not a drop-in replacement for this
/// function's shape.
pub(super) fn screen_is_locked() -> Option<bool> {
    unsafe {
        let rtld_default = -2isize as *mut c_void;
        let name = CString::new("CGSessionCopyCurrentDictionary").unwrap();
        let sym = libc::dlsym(rtld_default, name.as_ptr());
        if sym.is_null() {
            return None;
        }
        let copy_dict: unsafe extern "C" fn() -> CFTypeRef = std::mem::transmute(sym);
        let dict = copy_dict();
        if dict.is_null() {
            return None;
        }
        // CGSessionCopyCurrentDictionary follows the Copy naming convention
        // (+1 owned reference) — release it once we're done reading it.
        let key = CFString::new("CGSSessionScreenIsLocked");
        let value = CFDictionaryGetValue(dict, key.as_concrete_TypeRef() as CFTypeRef);
        let locked = !value.is_null() && CFBooleanGetValue(value) != 0;
        CFRelease(dict);
        Some(locked)
    }
}

/// `CGSConfigureDisplayEnabled(config, display, enabled)` — undocumented
/// SkyLight symbol re-exported from CoreGraphics on macOS 26. Called
/// between `CGBeginDisplayConfiguration` / `CGCompleteDisplayConfiguration`
/// to enable or disable a display from the WindowServer's perspective
/// (backlight off, no menu bar, no windows can be placed there).
pub(super) struct DisplayEnabler(unsafe extern "C" fn(*mut c_void, CGDirectDisplayID, bool) -> i32);

impl DisplayEnabler {
    pub(super) fn load() -> Result<Self> {
        unsafe {
            let rtld_default = -2isize as *mut c_void;
            let name = CString::new("CGSConfigureDisplayEnabled").unwrap();
            let p = libc::dlsym(rtld_default, name.as_ptr());
            if p.is_null() {
                return Err(anyhow!(
                    "private CoreGraphics symbol `CGSConfigureDisplayEnabled` \
                     not found — display detach is unavailable on this macOS"
                ));
            }
            Ok(Self(std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(*mut c_void, CGDirectDisplayID, bool) -> i32,
            >(p)))
        }
    }

    /// Apply the enabled/disabled state inside an active CG display config.
    /// `config` is the `CGDisplayConfigRef` returned by
    /// `CGDisplay::begin_configuration` — its layout is `*mut c_void` in
    /// the core-graphics crate, which is what the FFI expects.
    pub(super) fn set(&self, config: *mut c_void, display: u32, enabled: bool) -> Result<()> {
        let err = unsafe { (self.0)(config, display, enabled) };
        if err == 0 {
            Ok(())
        } else {
            Err(anyhow!(
                "CGSConfigureDisplayEnabled(display={display}, enabled={enabled}) \
                 returned CGError {err}"
            ))
        }
    }
}

fn class_required(name: &str) -> Result<&'static AnyClass> {
    AnyClass::get(name).ok_or_else(|| {
        anyhow!(
            "private Obj-C class `{name}` not registered in the runtime — \
             virtual-display support is unavailable on this macOS"
        )
    })
}

/// Owned `CGVirtualDisplay` instance. Released via Obj-C `release` on
/// Drop, which is what un-registers the display from the system.
pub(super) struct Handle {
    raw: *mut AnyObject,
    display_id: u32,
}

// SAFETY: CGVirtualDisplay is a CF-bridged Obj-C class; CF types are
// thread-safe. Our usage is single-threaded but the wrapper crosses
// thread boundaries in main's task setup.
unsafe impl Send for Handle {}

impl Handle {
    pub(super) fn display_id(&self) -> u32 {
        self.display_id
    }

    /// Re-apply settings with a single new mode — the live-resize primitive.
    /// Same `applySettings:` call that registered the display in `create`;
    /// calling it again on the live instance switches the active mode (the
    /// WindowServer relays out windows like a physical monitor changing
    /// resolution). The new mode must fit the descriptor's max dimensions
    /// (8192×8192, set in `create`).
    pub(super) fn apply_mode(&self, width: u32, height: u32, refresh_hz: u32) -> Result<()> {
        apply_single_mode(self.raw, width, height, refresh_hz)?;
        select_mode(self.display_id, width, height, false).map(|_| ())
    }

    /// Upstream's proven re-mode: hiDPI=0, the single mode becomes current by
    /// itself. Used as the fallback when a hiDPI re-mode doesn't publish.
    pub(super) fn apply_mode_one_x_legacy(
        &self,
        width: u32,
        height: u32,
    ) -> Result<(u32, u32, u32, u32)> {
        apply_single_mode_hidpi(self.raw, width, height, 60, false)?;
        for _ in 0..60 {
            if let Some(m) = cg_modes::current(self.display_id) {
                if (m.0, m.1) == (width, height) {
                    return Ok(m);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Err(anyhow!(
            "legacy 1× re-mode to {width}×{height} never became active"
        ))
    }

    /// Register `points_w × points_h` and select its 1× or Retina variant.
    /// Returns the active mode as `(points_w, points_h, pixels_w, pixels_h)`.
    pub(super) fn apply_points_mode(
        &self,
        points_w: u32,
        points_h: u32,
        hidpi: bool,
    ) -> Result<(u32, u32, u32, u32)> {
        apply_single_mode(self.raw, points_w, points_h, 60)?;
        select_mode(self.display_id, points_w, points_h, hidpi)
    }
}

/// Build a one-mode `CGVirtualDisplaySettings` and `applySettings:` it onto
/// `display`. Shared by `create` (initial registration) and
/// `Handle::apply_mode` (live resize). The temporaries are leaked, same
/// rationale as in `create` — tiny, rare, and obviously double-free-proof.
fn apply_single_mode(
    display: *mut AnyObject,
    width: u32,
    height: u32,
    refresh_hz: u32,
) -> Result<()> {
    apply_single_mode_hidpi(display, width, height, refresh_hz, true)
}

fn apply_single_mode_hidpi(
    display: *mut AnyObject,
    width: u32,
    height: u32,
    refresh_hz: u32,
    hidpi: bool,
) -> Result<()> {
    // Drop any app-scoped mode override from a previous `select_mode`; while it
    // exists the WindowServer ignores new settings for the display.
    extern "C" {
        fn CGRestorePermanentDisplayConfiguration();
    }
    unsafe { CGRestorePermanentDisplayConfiguration() };
    let settings_class = class_required("CGVirtualDisplaySettings")?;
    let mode_class = class_required("CGVirtualDisplayMode")?;
    let nsarray_class = class_required("NSArray")?;

    unsafe {
        let mode: *mut AnyObject = msg_send![mode_class, alloc];
        let mode: *mut AnyObject = msg_send![
            mode,
            initWithWidth: width,
            height: height,
            refreshRate: f64::from(refresh_hz),
        ];
        if mode.is_null() {
            return Err(anyhow!("[CGVirtualDisplayMode init...] returned nil"));
        }
        let modes: *mut AnyObject = msg_send![nsarray_class, arrayWithObject: mode];

        let settings: *mut AnyObject = msg_send![settings_class, alloc];
        let settings: *mut AnyObject = msg_send![settings, init];
        if settings.is_null() {
            return Err(anyhow!("[CGVirtualDisplaySettings init] returned nil"));
        }
        // hiDPI=1 makes macOS offer a Retina twin (W×H pt @ 2W×2H px) of every
        // registered mode; the 1× mode stays selectable. Because the display then
        // comes up in its largest 1× mode rather than the one registered, every
        // caller follows this with `select_mode` (docs/research/2026-09-29-hidpi-virtual-display.md).
        let _: () = msg_send![settings, setHiDPI: u32::from(hidpi)];
        let _: () = msg_send![settings, setModes: modes];

        let ok: bool = msg_send![display, applySettings: settings];
        if !ok {
            return Err(anyhow!(
                "[CGVirtualDisplay applySettings:] returned false — mode {width}x{height}@{refresh_hz} \
                 is likely unsupported"
            ));
        }
    }
    Ok(())
}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe {
                let _: () = msg_send![self.raw, release];
            }
            self.raw = std::ptr::null_mut();
        }
    }
}

/// Allocate a virtual display at the requested pixel size, apply a
/// single-mode settings object, and return the owning handle.
///
/// Temporary Obj-C objects (descriptor, mode, mode-array, settings)
/// are intentionally leaked — they're tiny, only allocated once per
/// process, and avoiding manual `release` calls keeps the code
/// obviously free of double-free hazards.
pub(super) fn create(width: u32, height: u32, refresh_hz: u32, name: &str) -> Result<Handle> {
    let desc_class = class_required("CGVirtualDisplayDescriptor")?;
    let display_class = class_required("CGVirtualDisplay")?;

    let ns_name = NSString::from_str(name);

    unsafe {
        // 1. Descriptor: alloc/init + property setters. Every property
        //    is cosmetic except the pixel dimensions; the rest just
        //    populates the metadata Displays.app shows for the monitor.
        let desc: *mut AnyObject = msg_send![desc_class, alloc];
        let desc: *mut AnyObject = msg_send![desc, init];
        if desc.is_null() {
            return Err(anyhow!("CGVirtualDisplayDescriptor init returned nil"));
        }
        let _: () = msg_send![desc, setName: &*ns_name];
        // Deliver display updates on a global dispatch queue. Without a queue they
        // target the main run loop, which this tokio process never pumps — a live
        // hiDPI re-mode then never publishes its new mode list (Phase 1 finding).
        extern "C" {
            fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut AnyObject;
        }
        let queue = dispatch_get_global_queue(0, 0);
        let _: () = msg_send![desc, setQueue: queue];
        // Max dimensions are a CAP fixed for the display's lifetime (the
        // descriptor can't be re-applied), not the active resolution — the
        // mode below is what sizes the framebuffer. Advertise the RDP
        // protocol maximum (8192, MS-RDPBCGR) instead of the initial mode
        // size so a live resize (`apply_mode`) can later switch to any
        // protocol-legal mode, including ones larger than the first.
        let _: () = msg_send![desc, setMaxPixelsWide: 8192u32];
        let _: () = msg_send![desc, setMaxPixelsHigh: 8192u32];
        // 600x338 mm ≈ a 27-inch 16:9 panel. Just a label.
        let _: () = msg_send![desc, setSizeInMillimeters: CGSize { width: 600.0, height: 338.0 }];
        let _: () = msg_send![desc, setProductID: 0x6D616372u32]; // "macr"
        let _: () = msg_send![desc, setVendorID: 0x6D616372u32];
        // Was hardcoded to 1: a second macrdp process (e.g. two concurrent
        // installs, or dev testing alongside a running instance) registering a
        // display with an identical vendor/product/serial triple gets its
        // descriptor rejected by CGVirtualDisplay initWithDescriptor: outright
        // (returns nil). Per-process id keeps concurrent instances from colliding.
        // Unique per display, not just per process: a second display in the same
        // process with the same vendor/product/serial inherits the first one's
        // stale mode list (found by Phase 1's display tests).
        static NEXT_SERIAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let serial = std::process::id()
            .wrapping_mul(64)
            .wrapping_add(NEXT_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let _: () = msg_send![desc, setSerialNum: serial];
        // sRGB primaries + D65 white (spec §8.1): macOS colour-manages every app
        // into sRGB for this display, which is what RDP clients assume.
        let _: () = msg_send![desc, setRedPrimary: CGPoint { x: 0.64, y: 0.33 }];
        let _: () = msg_send![desc, setGreenPrimary: CGPoint { x: 0.30, y: 0.60 }];
        let _: () = msg_send![desc, setBluePrimary: CGPoint { x: 0.15, y: 0.06 }];
        let _: () = msg_send![desc, setWhitePoint: CGPoint { x: 0.3127, y: 0.3290 }];

        // 2. CGVirtualDisplay instance from the descriptor.
        let display: *mut AnyObject = msg_send![display_class, alloc];
        let display: *mut AnyObject = msg_send![display, initWithDescriptor: desc];
        if display.is_null() {
            return Err(anyhow!(
                "[CGVirtualDisplay initWithDescriptor:] returned nil — the \
                 descriptor was rejected (try different dimensions)"
            ));
        }

        // 3+4. One mode at the requested resolution, applied via settings —
        //      the applySettings: call is what actually registers the display
        //      with the WindowServer so it appears in Displays. Shared with
        //      the live-resize path (`Handle::apply_mode`).
        if let Err(e) = apply_single_mode(display, width, height, refresh_hz) {
            let _: () = msg_send![display, release];
            return Err(e);
        }

        // 5. Read back the assigned displayID.
        let display_id: u32 = msg_send![display, displayID];
        if display_id == 0 {
            let _: () = msg_send![display, release];
            return Err(anyhow!(
                "[CGVirtualDisplay displayID] returned 0 — display registered \
                 but the system hasn't assigned a directDisplayID"
            ));
        }

        let handle = Handle {
            raw: display,
            display_id,
        };
        // With hiDPI=1 the display comes up in its largest 1× mode; pin the one asked for.
        select_mode(display_id, width, height, false)?;
        Ok(handle)
    }
}

mod cg_modes {
    //! Raw display-mode FFI. `core_graphics` 0.24's `CGDisplayMode::all_display_modes`
    //! wraps each array element with `from_ptr` (taking ownership of a reference the
    //! CFArray still owns), so modes are released twice — reproduced here as a SIGSEGV
    //! on the first enumeration. Everything below balances its own retains.
    use std::ffi::c_void;

    use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
    use core_foundation::base::{CFRelease, CFTypeRef};
    use core_foundation::dictionary::CFDictionaryRef;

    pub type ModeRef = *mut c_void;

    extern "C" {
        fn CGDisplayCopyAllDisplayModes(display: u32, options: CFDictionaryRef) -> CFArrayRef;
        fn CGDisplayCopyDisplayMode(display: u32) -> ModeRef;
        fn CGDisplayModeRelease(mode: ModeRef);
        fn CGDisplayModeGetWidth(mode: ModeRef) -> usize;
        fn CGDisplayModeGetHeight(mode: ModeRef) -> usize;
        fn CGDisplayModeGetPixelWidth(mode: ModeRef) -> usize;
        fn CGDisplayModeGetPixelHeight(mode: ModeRef) -> usize;
        fn CGBeginDisplayConfiguration(config: *mut *mut c_void) -> i32;
        fn CGConfigureDisplayWithDisplayMode(
            config: *mut c_void,
            display: u32,
            mode: ModeRef,
            options: CFDictionaryRef,
        ) -> i32;
        fn CGCompleteDisplayConfiguration(config: *mut c_void, option: u32) -> i32;
    }

    /// `(points_w, points_h, pixels_w, pixels_h)` of a mode.
    pub type Dims = (u32, u32, u32, u32);

    unsafe fn dims(m: ModeRef) -> Dims {
        (
            CGDisplayModeGetWidth(m) as u32,
            CGDisplayModeGetHeight(m) as u32,
            CGDisplayModeGetPixelWidth(m) as u32,
            CGDisplayModeGetPixelHeight(m) as u32,
        )
    }

    pub fn current(display: u32) -> Option<Dims> {
        unsafe {
            let m = CGDisplayCopyDisplayMode(display);
            if m.is_null() {
                return None;
            }
            let d = dims(m);
            CGDisplayModeRelease(m);
            Some(d)
        }
    }

    pub fn list(display: u32, options: CFDictionaryRef) -> Vec<Dims> {
        unsafe {
            let arr = CGDisplayCopyAllDisplayModes(display, options);
            if arr.is_null() {
                return Vec::new();
            }
            let v = (0..CFArrayGetCount(arr))
                .map(|i| dims(CFArrayGetValueAtIndex(arr, i) as ModeRef))
                .collect();
            CFRelease(arr as CFTypeRef);
            v
        }
    }

    /// Find a mode with exactly `want` dims and switch to it (session-scoped).
    /// `Ok(false)` = no such mode offered; `Err(code)` = a CG call failed.
    pub fn switch_to(display: u32, options: CFDictionaryRef, want: Dims) -> Result<bool, i32> {
        unsafe {
            let arr = CGDisplayCopyAllDisplayModes(display, options);
            if arr.is_null() {
                return Ok(false);
            }
            let mut mode: ModeRef = std::ptr::null_mut();
            for i in 0..CFArrayGetCount(arr) {
                let m = CFArrayGetValueAtIndex(arr, i) as ModeRef;
                if dims(m) == want {
                    mode = m; // borrowed: valid while `arr` is alive
                    break;
                }
            }
            let result = if mode.is_null() {
                Ok(false)
            } else {
                let mut cfg: *mut c_void = std::ptr::null_mut();
                let e = CGBeginDisplayConfiguration(&mut cfg);
                if e != 0 {
                    Err(e)
                } else {
                    let e = CGConfigureDisplayWithDisplayMode(cfg, display, mode, std::ptr::null());
                    // 0 = kCGConfigureForAppOnly: ForSession pins the mode list so later re-modes never publish new modes (spikes/hidpi/remode.swift).
                    let c = CGCompleteDisplayConfiguration(cfg, 0);
                    if e != 0 {
                        Err(e)
                    } else if c != 0 {
                        Err(c)
                    } else {
                        Ok(true)
                    }
                }
            };
            CFRelease(arr as CFTypeRef);
            result
        }
    }
}

/// Select the `points_w × points_h` mode of `display_id`, 1× or Retina (2×
/// backing), and wait until the WindowServer reports it active. Retina variants
/// are only listed with `kCGDisplayShowDuplicateLowResolutionModes`. A switch can
/// report success yet not take effect (observed in the HiDPI spike), so the result
/// is verified and the configuration retried once before giving up.
/// Active mode of `display_id` as `(points_w, points_h, pixels_w, pixels_h)`.
pub(super) fn current_mode(display_id: u32) -> Option<(u32, u32, u32, u32)> {
    cg_modes::current(display_id)
}

pub(super) fn select_mode(
    display_id: u32,
    points_w: u32,
    points_h: u32,
    hidpi: bool,
) -> Result<(u32, u32, u32, u32)> {
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_graphics::display::kCGDisplayShowDuplicateLowResolutionModes;

    let scale = if hidpi { 2 } else { 1 };
    let want = (points_w, points_h, scale * points_w, scale * points_h);
    if cg_modes::current(display_id) == Some(want) {
        return Ok(want);
    }
    let opts = CFDictionary::from_CFType_pairs(&[(
        unsafe { CFString::wrap_under_get_rule(kCGDisplayShowDuplicateLowResolutionModes) },
        CFBoolean::true_value(),
    )]);
    for attempt in 0..2 {
        // A freshly registered (or re-moded) display publishes its mode list
        // asynchronously; poll up to ~2 s for the wanted mode to appear.
        let mut offered = false;
        for _ in 0..40 {
            match cg_modes::switch_to(display_id, opts.as_concrete_TypeRef(), want) {
                Ok(true) => {
                    offered = true;
                    break;
                }
                Ok(false) => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(e) => return Err(anyhow!("display mode switch failed: CGError {e}")),
            }
        }
        if !offered {
            let offered = cg_modes::list(display_id, opts.as_concrete_TypeRef());
            return Err(anyhow!(
                "display {display_id} offers no {points_w}×{points_h} mode at {scale}× backing \
                 (current {:?}; offered {:?})",
                cg_modes::current(display_id),
                offered
            ));
        }
        for _ in 0..60 {
            if cg_modes::current(display_id) == Some(want) {
                return Ok(want);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        tracing::warn!(
            display_id,
            points_w,
            points_h,
            hidpi,
            attempt,
            "display mode switch reported success but did not take effect — retrying"
        );
    }
    Err(anyhow!(
        "display {display_id}: {points_w}×{points_h} at {scale}× never became active"
    ))
}
