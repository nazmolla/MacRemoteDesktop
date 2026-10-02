//! Input forwarding: RDP keyboard/mouse PDUs → macOS CGEvents.
//!
//! `ironrdp_server` hands us scancodes (PS/2 Set 1) and absolute mouse coords
//! in desktop-pixel space. We translate to macOS virtual keycodes and post via
//! `CGEventPost(kCGHIDEventTap)`. Non-macOS targets get a logging stub.

use ironrdp_server::{KeyboardEvent, MouseEvent, RdpServerInputHandler};
#[cfg(not(target_os = "macos"))]
use tracing::trace;

/// Shared cell the vendored server fills with the connected client's Windows
/// keyboard-layout identifier (KLID); 0 = unknown / not announced. Read by the
/// input handler to auto-select a non-US layout when `--keyboard-layout` isn't
/// given. Mirrors the `display_suppressed` shared-flag pattern.
pub type SharedKeyboardLayout = std::sync::Arc<std::sync::atomic::AtomicU32>;

pub struct MacInputHandler {
    /// Live session desktop size, shared with `CaptureDisplay`. Read per
    /// mouse event (not copied at construction) so coordinate scaling stays
    /// correct when the client-resolution auto-adopt resizes the session.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    desktop_size: crate::capture::SharedDesktopSize,
    /// Records mouse-button-down timestamps so the H.264 path can lower its
    /// keyframe threshold briefly after a click. `None` unless `--enable-h264`.
    click_signal: Option<crate::capture::ClickSignal>,
    activity: InputActivity,
    #[cfg(target_os = "macos")]
    inner: macos::Inner,
}

/// Logs when input arrives after a long silence, so a session whose input
/// stopped can be told apart from one whose client stopped sending.
#[derive(Default)]
struct InputActivity {
    last: Option<std::time::Instant>,
    events: u64,
}

impl InputActivity {
    const SILENCE: std::time::Duration = std::time::Duration::from_secs(300);

    fn note(&mut self, kind: &'static str) {
        let now = std::time::Instant::now();
        match self.last {
            None => tracing::info!(kind, "first input event received"),
            Some(t) if now.duration_since(t) >= Self::SILENCE => tracing::info!(
                kind,
                silent_secs = now.duration_since(t).as_secs(),
                events_before = self.events,
                "input received after a long silence"
            ),
            _ => {}
        }
        self.last = Some(now);
        self.events += 1;
    }
}

impl MacInputHandler {
    /// `target_display_id` identifies the macOS display we're posting
    /// events into: `None` means the current primary panel; `Some(id)`
    /// is the `CGDirectDisplayID` we should re-query bounds for on
    /// every event. Re-querying matters because the target's global
    /// origin can move *after* this handler is constructed — e.g.
    /// `--detach-primary` disables the built-in panel mid-session,
    /// which shifts the virtual display to `(0, 0)`.
    pub fn new(
        desktop_size: crate::capture::SharedDesktopSize,
        target_display_id: crate::virtual_display::DisplayIdCell,
        click_signal: Option<crate::capture::ClickSignal>,
        keyboard_layout: Option<String>,
        keyboard_layout_klid: Option<SharedKeyboardLayout>,
        resync: crate::resync::ResyncSignal,
    ) -> anyhow::Result<Self> {
        #[cfg(target_os = "macos")]
        let inner = macos::Inner::new(
            target_display_id,
            keyboard_layout.as_deref(),
            keyboard_layout_klid,
            resync,
        )?;
        #[cfg(not(target_os = "macos"))]
        {
            let _ = target_display_id;
            let _ = keyboard_layout;
            let _ = keyboard_layout_klid;
            let _ = resync;
        }
        Ok(Self {
            desktop_size,
            click_signal,
            activity: InputActivity::default(),
            #[cfg(target_os = "macos")]
            inner,
        })
    }
}

impl RdpServerInputHandler for MacInputHandler {
    fn keyboard(&mut self, event: KeyboardEvent) {
        self.activity.note("keyboard");
        #[cfg(target_os = "macos")]
        self.inner.keyboard(event);
        #[cfg(not(target_os = "macos"))]
        trace!(?event, "keyboard event (stub)");
    }

    fn mouse(&mut self, event: MouseEvent) {
        self.activity.note("mouse");
        // A button-down is the "user intent" signal the H.264 path uses to lower
        // its keyframe threshold for a moment (a click usually precedes a UI
        // change). Record before delegating, since `inner.mouse` takes `event`.
        if let Some(sig) = &self.click_signal {
            if matches!(
                event,
                MouseEvent::LeftPressed | MouseEvent::RightPressed | MouseEvent::MiddlePressed
            ) {
                sig.record_click();
            }
        }
        #[cfg(target_os = "macos")]
        {
            let (width, height) = self.desktop_size.get();
            let letterbox = self.desktop_size.letterbox();
            self.inner.mouse(event, width, height, letterbox);
        }
        #[cfg(not(target_os = "macos"))]
        trace!(?event, "mouse event (stub)");
    }
}

/// Map an RDP client coordinate (`x`,`y` in a `dw`×`dh` client frame) to a point
/// *offset* within the target Mac display (`sw`×`sh` points; caller adds the
/// display origin).
///
/// - `letterbox = false` (stretch/fill): the whole client frame maps linearly
///   onto the whole Mac screen.
/// - `letterbox = true`: the Mac occupies a centered, aspect-preserved sub-rect
///   of the client frame (matching capture's `preserves_aspect_ratio`), so we map
///   into that sub-rect; coords landing in the black bars clamp to the nearest
///   screen edge rather than off-screen.
///
/// Pure (no platform deps) so it's unit-tested on every target.
fn map_client_to_display(
    x: u16,
    y: u16,
    dw: u16,
    dh: u16,
    sw: f64,
    sh: f64,
    letterbox: bool,
) -> (f64, f64) {
    let dwf = f64::from(dw.max(1));
    let dhf = f64::from(dh.max(1));
    let (mx, my) = if letterbox {
        let mac_aspect = sw / sh.max(1.0);
        let out_aspect = dwf / dhf;
        let (cw, ch) = if out_aspect > mac_aspect {
            (dhf * mac_aspect, dhf) // pillarbox: bars left/right
        } else {
            (dwf, dwf / mac_aspect) // letterbox: bars top/bottom
        };
        let off_x = (dwf - cw) / 2.0;
        let off_y = (dhf - ch) / 2.0;
        let fx = ((f64::from(x) - off_x) / cw).clamp(0.0, 1.0);
        let fy = ((f64::from(y) - off_y) / ch).clamp(0.0, 1.0);
        (fx * sw, fy * sh)
    } else {
        (f64::from(x) * sw / dwf, f64::from(y) * sh / dhf)
    };
    // Clamp the result INTO the display, on both paths.
    //
    // Why the direct-scale path needs it at all: some clients (observed: iOS
    // Windows App in "Mouse pointer" mode, whose touch-to-cursor math
    // rubber-bands past the desktop bounds when the user keeps dragging beyond
    // where the on-screen cursor visually stops) report x/y well outside
    // [0,dw)x[0,dh) — e.g. y=549 against a negotiated desktop_h of 505.
    //
    // Why the bound is `s - 1` and not `s`: a display of height `sh` covers
    // rows 0..=sh-1, so `sh` itself is the first row PAST it. That one pixel is
    // load-bearing on a headless-vd layout, where the vd sits at (0,0) and the
    // physical panel is parked beside it at (sw, 0): a point at y == sh is dead
    // space owned by NO display (so macOS's edge-triggered UI — Dock and
    // menu-bar auto-reveal — never fires), and x == sw is the physical panel's
    // first column (so the cursor teleports off the display the client is
    // watching). The letterbox path reached the same two values via its
    // clamp-to-1.0 fraction, so it gets the same treatment.
    (
        mx.clamp(0.0, (sw - 1.0).max(0.0)),
        my.clamp(0.0, (sh - 1.0).max(0.0)),
    )
}

/// macOS virtual keycodes whose `Ctrl+<key>` combo is remapped to `Cmd+<key>`
/// under --map-ctrl-to-cmd: C V X A Z S F N T W O P R G (copy / paste / cut /
/// select-all / undo / save / find / new / new-tab / close / open / print /
/// reload / find-next). Deliberately EXCLUDES Q (`Cmd+Q` quits — a nasty
/// surprise from `Ctrl+Q`) and all nav keys (Mac word-nav is Option+arrow, not
/// Cmd+arrow). Windows redo (`Ctrl+Y`) is intentionally not here — Mac redo is
/// `Cmd+Shift+Z`, reachable via `Ctrl+Shift+Z` through the Z mapping.
///
/// Pure (no platform deps) so it lives at file scope and is unit-tested on
/// every target; `mod macos` reaches it via `use super::is_remappable_shortcut`.
fn is_remappable_shortcut(vk: u16) -> bool {
    matches!(
        vk,
        0x08 // C
            | 0x09 // V
            | 0x07 // X
            | 0x00 // A
            | 0x06 // Z
            | 0x01 // S
            | 0x03 // F
            | 0x2D // N
            | 0x11 // T
            | 0x0D // W
            | 0x1F // O
            | 0x23 // P
            | 0x0F // R
            | 0x05 // G
    )
}

/// When true, Cmd+Tab un-minimizes the target app's window (sets `AXMinimized`
/// to false) instead of leaving it minimized in the Dock. Off by default, which
/// matches native macOS Cmd+Tab (it activates the app but doesn't restore a
/// minimized window). Set once at startup from `--unminimize-on-switch`.
static UNMINIMIZE_ON_SWITCH: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_unminimize_on_switch(on: bool) {
    UNMINIMIZE_ON_SWITCH.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// When true, Option+Tab (i.e. Alt+Tab forwarded from the client) is accepted as
/// an *additional* trigger for the same app-cycle as Cmd+Tab — for clients/configs
/// that pass Alt+Tab through to the session but gate Win+Tab (mstsc's "Apply
/// Windows key combinations"). Opt-in via `--alt-tab-switch`; off by default so
/// Option+Tab otherwise reaches remote apps as a normal key. Cmd+Tab is
/// unaffected and always works. Reuses the same MRU session, committing on
/// Option release instead of Cmd release.
static ALT_TAB_SWITCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_alt_tab_switch(on: bool) {
    ALT_TAB_SWITCH.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// When true, Option+` (i.e. Alt+backtick forwarded from the client) is
/// accepted as an *additional* trigger for the same within-app window-cycle
/// that Cmd+` drives — mirrors `ALT_TAB_SWITCH`'s relationship to Cmd+Tab,
/// but for the window cycle instead of the app cycle. Opt-in via
/// `--alt-backtick-switch`; off by default so Option+` otherwise reaches
/// remote apps as a normal key. Cmd+` is unaffected and always works. Unlike
/// Opt+Tab there is no release-commit bookkeeping —
/// `ax_cycle_windows_of_front` acts immediately on each press.
static ALT_BACKTICK_SWITCH: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_alt_backtick_switch(on: bool) {
    ALT_BACKTICK_SWITCH.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// When true, drive the `macrdphud` overlay helper so the remote client sees a
/// visual app switcher during Cmd+Tab / Option+Tab. Opt-in via
/// `--app-switcher-hud`; off by default. The switch itself works the same either
/// way — this only adds the (best-effort) on-screen HUD.
static APP_SWITCHER_HUD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_app_switcher_hud(on: bool) {
    APP_SWITCHER_HUD.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// When true, rewrite a curated set of `Ctrl+<key>` editing shortcuts to
/// `Cmd+<key>` so Windows muscle memory (Ctrl+C/V/X/…) drives macOS copy/paste.
/// Opt-in via `--map-ctrl-to-cmd`; off by default (the default keeps Ctrl as Ctrl,
/// so the US majority + Ctrl-based shortcuts are unaffected). Suppressed when a
/// terminal app is frontmost (see `NO_REMAP_APPS`).
static MAP_CTRL_TO_CMD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_map_ctrl_to_cmd(on: bool) {
    MAP_CTRL_TO_CMD.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Bundle ids whose frontmost focus suppresses the Ctrl→Cmd remap, in ADDITION to
/// the built-in standalone-terminal list. The lever for embedded terminals that the
/// front-app check can't auto-detect (e.g. add `com.microsoft.VSCode`). Set once at
/// startup from `--no-remap-apps`.
static NO_REMAP_APPS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

pub fn set_no_remap_apps(bundles: Vec<String>) {
    let _ = NO_REMAP_APPS.set(bundles);
}

/// Start the event-driven frontmost-app observer used by the Ctrl→Cmd remap's
/// terminal/app suppression. Call once at startup when `--map-ctrl-to-cmd` is on.
/// See `macos::init_focus_observer`.
#[cfg(target_os = "macos")]
pub fn init_focus_observer() {
    macos::init_focus_observer();
}

#[cfg(not(target_os = "macos"))]
pub fn init_focus_observer() {}

#[cfg(target_os = "macos")]
mod scancodes;

// Re-export the gather-windows sweep so the capture path can trigger it
// automatically after a live virtual-display re-mode (see
// `capture.rs::sync_virtual_display`), the same routine the Ctrl+Alt+G hotkey
// runs on demand.
#[cfg(target_os = "macos")]
pub(crate) use macos::gather_windows_onto_display;

#[cfg(target_os = "macos")]
mod macos {
    use crate::sync_ext::LockExt;
    use std::collections::HashSet;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::is_remappable_shortcut;
    use super::scancodes::{is_numeric_pad_vk, scancode_to_cgkeycode};

    use anyhow::{anyhow, Result};
    use core_graphics::display::CGDisplay;
    use core_graphics::event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
        ScrollEventUnit,
    };
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;
    use ironrdp_pdu::input::fast_path::SynchronizeFlags;
    use ironrdp_server::{KeyboardEvent, MouseEvent};
    use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};
    use tracing::{debug, trace, warn};

    mod ax;
    mod focus;
    mod hotkeys;
    mod switcher;
    mod windows;

    use ax::*;
    pub(super) use focus::init_focus_observer;
    use focus::*;
    use hotkeys::*;
    use switcher::*;
    pub(crate) use windows::gather_windows_onto_display;
    use windows::*;

    // kVK_* constants we match on for symbolic-hotkey interception.
    const VK_TAB: u16 = 0x30;
    const VK_SPACE: u16 = 0x31;
    const VK_3: u16 = 0x14;
    const VK_4: u16 = 0x15;
    const VK_5: u16 = 0x17;
    const VK_GRAVE: u16 = 0x32;
    const VK_CAPS_LOCK: u16 = 0x39;
    const VK_G: u16 = 0x05; // kVK_ANSI_G — the on-demand "gather windows" chord
    const VK_R: u16 = 0x0F; // kVK_ANSI_R — the on-demand "A/V resync" chord
    const VK_LEFT: u16 = 0x7B; // kVK_LeftArrow — step the app switcher backward while it's up
    const VK_RIGHT: u16 = 0x7C; // kVK_RightArrow — step the app switcher forward while it's up

    // macOS virtual keycodes for the left/right halves of each modifier.
    // Used by ModifierState to track which physical key is held so we can
    // accurately re-derive both the masked CGEventFlag bits (Shift, Ctrl,
    // …) and the device-dependent NX_DEVICE{L,R}*KEYMASK bits a real
    // keyboard would put on a flagsChanged event.
    const VK_LSHIFT: u16 = 0x38;
    const VK_RSHIFT: u16 = 0x3C;
    const VK_LCTRL: u16 = 0x3B;
    const VK_RCTRL: u16 = 0x3E;
    const VK_LALT: u16 = 0x3A;
    const VK_RALT: u16 = 0x3D;
    const VK_LCMD: u16 = 0x37;
    const VK_RCMD: u16 = 0x36;

    // Device-dependent modifier bits — lifted from <IOKit/hidsystem/IOLLEvent.h>
    // (NX_DEVICE{L,R}{SHIFT,CTL,ALT,CMD}KEYMASK). They sit below the public
    // CGEventFlag bits (which start at 0x0100), so `from_bits_retain` keeps them
    // through CGEventFlags. Apps that distinguish left vs. right modifiers
    // (Karabiner, some games, accessibility tools) read these — a real
    // keyboard's flagsChanged event has them set; ours wouldn't without this.
    const NX_DEVICE_L_CTRL: u64 = 0x0001;
    const NX_DEVICE_L_SHIFT: u64 = 0x0002;
    const NX_DEVICE_R_SHIFT: u64 = 0x0004;
    const NX_DEVICE_L_CMD: u64 = 0x0008;
    const NX_DEVICE_R_CMD: u64 = 0x0010;
    const NX_DEVICE_L_ALT: u64 = 0x0020;
    const NX_DEVICE_R_ALT: u64 = 0x0040;
    const NX_DEVICE_R_CTRL: u64 = 0x2000;

    /// macOS's default `NSEvent.doubleClickInterval` is 0.5 s. We don't read
    /// the per-user setting (would need a CFPreferences call) — the default
    /// matches what most users have, and using less than the real threshold
    /// just means an aggressive double-click occasionally counts as two
    /// single clicks (the worse alternative is missing all double-clicks).
    const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);
    /// Pixels of cursor movement allowed between consecutive clicks for them
    /// to still count as a multi-click. macOS's slop is a few px; 5 is safe.
    const DOUBLE_CLICK_SLOP_PX: f64 = 5.0;

    #[derive(Clone, Copy)]
    struct ClickState {
        time: Instant,
        x: f64,
        y: f64,
        count: i64,
    }

    // CGEventSource wraps a thread-safe Core Foundation object; Apple documents
    // CF types as safe to send between threads. The crate doesn't impl Send
    // because the raw NonNull pointer isn't, but our usage is single-threaded
    // anyway (RdpServer serializes input callbacks).
    // SAFETY: the wrapped CGEventSource is a CF object, usable from any thread, and input callbacks
    // are serialised by the server (see the note above).
    unsafe impl Send for Inner {}

    /// Per-side modifier state. We track left/right halves of each modifier
    /// separately for two reasons:
    ///   1) When the user holds *both* left and right Shift and releases
    ///      one, the masked `CGEventFlagShift` bit must stay set. A single
    ///      `flags |= / -= CGEventFlagShift` toggle can't represent that —
    ///      releasing one half wrongly drops the bit while the other half
    ///      is still down.
    ///   2) Real keyboards include device-dependent left/right bits on
    ///      `flagsChanged` events (`NX_DEVICE{L,R}*KEYMASK`). Apps that
    ///      key off left-only vs. right-only modifiers (Karabiner,
    ///      remappers, some games) read those. Without per-side tracking
    ///      we can't produce them.
    ///
    /// `caps_lock` is a *toggle*, not a held-down bool: pressing the key
    /// flips it; the release is a no-op. That matches how real Mac
    /// keyboards report Caps Lock and what Cocoa apps assume when they
    /// check `CGEventFlagAlphaShift`.
    #[derive(Default, Clone, Copy)]
    struct ModifierState {
        l_shift: bool,
        r_shift: bool,
        l_ctrl: bool,
        r_ctrl: bool,
        l_alt: bool,
        r_alt: bool,
        l_cmd: bool,
        r_cmd: bool,
        caps_lock: bool,
    }

    impl ModifierState {
        /// Bitfield to put on every CGEvent. Combines the public masked
        /// bits (the ones apps query via `[NSEvent modifierFlags] &
        /// NSEventModifierFlagShift` and friends) with the
        /// device-dependent NX_DEVICE* left/right bits below 0x100.
        fn cg_flags(&self) -> CGEventFlags {
            let mut bits = 0u64;
            if self.l_shift || self.r_shift {
                bits |= CGEventFlags::CGEventFlagShift.bits();
            }
            if self.l_ctrl || self.r_ctrl {
                bits |= CGEventFlags::CGEventFlagControl.bits();
            }
            if self.l_alt || self.r_alt {
                bits |= CGEventFlags::CGEventFlagAlternate.bits();
            }
            if self.l_cmd || self.r_cmd {
                bits |= CGEventFlags::CGEventFlagCommand.bits();
            }
            if self.caps_lock {
                bits |= CGEventFlags::CGEventFlagAlphaShift.bits();
            }
            if self.l_shift {
                bits |= NX_DEVICE_L_SHIFT;
            }
            if self.r_shift {
                bits |= NX_DEVICE_R_SHIFT;
            }
            if self.l_ctrl {
                bits |= NX_DEVICE_L_CTRL;
            }
            if self.r_ctrl {
                bits |= NX_DEVICE_R_CTRL;
            }
            if self.l_alt {
                bits |= NX_DEVICE_L_ALT;
            }
            if self.r_alt {
                bits |= NX_DEVICE_R_ALT;
            }
            if self.l_cmd {
                bits |= NX_DEVICE_L_CMD;
            }
            if self.r_cmd {
                bits |= NX_DEVICE_R_CMD;
            }
            CGEventFlags::from_bits_retain(bits)
        }

        fn has_shift(&self) -> bool {
            self.l_shift || self.r_shift
        }
        fn has_ctrl(&self) -> bool {
            self.l_ctrl || self.r_ctrl
        }
        fn has_alt(&self) -> bool {
            self.l_alt || self.r_alt
        }
        fn has_cmd(&self) -> bool {
            self.l_cmd || self.r_cmd
        }

        /// True if `vk` is a known modifier (incl. Caps Lock). The caller
        /// uses this to branch into the FlagsChanged path.
        fn is_modifier_vk(vk: u16) -> bool {
            matches!(
                vk,
                VK_LSHIFT
                    | VK_RSHIFT
                    | VK_LCTRL
                    | VK_RCTRL
                    | VK_LALT
                    | VK_RALT
                    | VK_LCMD
                    | VK_RCMD
                    | VK_CAPS_LOCK
            )
        }

        /// Update state for a press/release of a modifier or Caps Lock.
        /// Returns true if anything changed (callers skip emitting a
        /// FlagsChanged event when nothing did — e.g. a Caps Lock release).
        fn apply(&mut self, vk: u16, down: bool) -> bool {
            // Caps Lock is a toggle: flip on press, ignore release. This is
            // what real Mac keyboards do — the up event carries no new
            // information, and emitting one would unset AlphaShift mid-press.
            if vk == VK_CAPS_LOCK {
                if down {
                    self.caps_lock = !self.caps_lock;
                    return true;
                }
                return false;
            }
            let slot = match vk {
                VK_LSHIFT => &mut self.l_shift,
                VK_RSHIFT => &mut self.r_shift,
                VK_LCTRL => &mut self.l_ctrl,
                VK_RCTRL => &mut self.r_ctrl,
                VK_LALT => &mut self.l_alt,
                VK_RALT => &mut self.r_alt,
                VK_LCMD => &mut self.l_cmd,
                VK_RCMD => &mut self.r_cmd,
                _ => return false,
            };
            if *slot == down {
                return false; // idempotent — auto-repeat of a held modifier
            }
            *slot = down;
            true
        }
    }

    pub struct Inner {
        source: CGEventSource,
        // Secondary source used *only* to mirror modifier FlagsChanged
        // events. See `Inner::new` for why one source isn't enough.
        source_hid: CGEventSource,
        last_x: f64,
        last_y: f64,
        left_down: bool,
        right_down: bool,
        middle_down: bool,
        mods: ModifierState,
        // `None` → use CGDisplay::main() bounds; `Some(id)` → look up
        // that specific display. Re-queried (through a short TTL cache —
        // see `target_bounds`) so a mid-session bounds change (e.g.
        // --detach-primary disabling the built-in mid-flight) doesn't
        // strand events on stale coords.
        target_display_id: crate::virtual_display::DisplayIdCell,
        // TTL cache for `target_bounds`: the CoreGraphics bounds query sits
        // on the busiest input path (mouse-move fires hundreds of times/s
        // during a drag) while display geometry changes only on a rare
        // reconfiguration. The cache sheds ~99% of the queries; worst case a
        // reconfig mis-maps moves for < the TTL before self-correcting.
        cached_bounds: Option<(f64, f64, f64, f64)>,
        bounds_cached_at: Instant,
        // Per-button click history so a quick second press at (roughly) the
        // same spot becomes click_count=2 and Finder recognises a double-
        // click. Without this every press has click_count=1 implicitly and
        // double-click actions never fire.
        click_left: Option<ClickState>,
        click_right: Option<ClickState>,
        click_middle: Option<ClickState>,
        // Running remainder of raw wheel units not yet converted to a whole
        // line, carried across calls. See `scroll()` — without this, a
        // client that slices a gesture into many small per-event deltas
        // (observed: magnitude ~2-40, not one 120-per-notch value) has
        // nearly every event truncated to zero and the scroll never moves.
        scroll_v_accum: i32,
        scroll_h_accum: i32,
        // Virtual keycodes whose key-down we intercepted as a symbolic
        // hotkey (Cmd+Tab, Cmd+Space, screencapture combos). The matching
        // key-up gets swallowed too so the focused app doesn't see a bare
        // release with no preceding press.
        consumed_keys: HashSet<u16>,
        // Virtual keycodes currently held that we remapped Ctrl→Cmd on the way
        // down (--map-ctrl-to-cmd). The matching key-up is posted with the same
        // Cmd swap + restores the real (Ctrl) modifier state.
        remapped_keys: HashSet<u16>,
        // Optional non-US keyboard layout. When set, ordinary typing keys are
        // translated to characters against this layout and posted as Unicode
        // strings instead of positional keycodes. `None` → keycode path only.
        layout: Option<crate::keyboard_layout::KeyboardLayout>,
        // Auto-detect plumbing: when no explicit --keyboard-layout was given,
        // `auto_layout` is true and we (re)resolve `layout` from the client's
        // announced KLID published in `klid_handle`, re-checking when it changes
        // (e.g. a reconnect from a differently-configured client).
        klid_handle: Option<super::SharedKeyboardLayout>,
        last_klid: u32,
        auto_layout: bool,
        // The Ctrl+Alt+Shift+R resync request, consumed by capture and audio.
        resync: crate::resync::ResyncSignal,
    }

    impl Inner {
        pub fn new(
            target_display_id: crate::virtual_display::DisplayIdCell,
            keyboard_layout: Option<&str>,
            klid_handle: Option<super::SharedKeyboardLayout>,
            resync: crate::resync::ResyncSignal,
        ) -> Result<Self> {
            // Two sources because macOS has two independent modifier-state
            // machines and different consumers read from different ones:
            //
            //   CombinedSessionState backs `[NSEvent modifierFlags]` — the
            //   query Cocoa apps use to decide "is Cmd held right now?" for
            //   app-level shortcuts (Cmd+C, Cmd+Q, Cmd+~).
            //
            //   HIDSystemState backs the HID-level modifier view that the
            //   WindowServer's *symbolic hotkey* dispatcher (Dock, Spotlight,
            //   screenshot, Mission Control, Show Desktop) checks before
            //   firing. Cmd+Tab is the canonical case.
            //
            // A flagsChanged event posted from one source does NOT update the
            // other. So we maintain both: the session source is the canonical
            // one used for every event, and `source_hid` exists solely to
            // mirror modifier flagsChanged so symbolic hotkeys also see Cmd
            // as held. Without the mirror, Cmd+Tab is a no-op; with it, both
            // classes of shortcut work.
            let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
                .map_err(|_| anyhow!("CGEventSource::new(CombinedSessionState) failed"))?;
            let source_hid = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
                .map_err(|_| anyhow!("CGEventSource::new(HIDSystemState) failed"))?;
            // Start the workspace-frontmost poller so the global MRU
            // tracks ALL focus changes (Dock clicks, app launches, etc.),
            // not just our Cmd+Tab commits. Idempotent — only one thread
            // is ever spawned across handler reconstructions.
            start_workspace_mru_poller();
            // Layout selection precedence:
            //   --keyboard-layout <spec>  → force that layout (no auto-detect)
            //   --keyboard-layout none/off → force positional keycodes
            //   (unset) + a KLID handle    → auto-detect from the client
            let explicit = keyboard_layout.map(str::trim);
            let disabled = matches!(explicit, Some("none") | Some("off") | Some(""));
            let (layout, auto_layout) = if disabled {
                (None, false)
            } else if let Some(spec) = explicit {
                (crate::keyboard_layout::KeyboardLayout::resolve(spec), false)
            } else {
                (None, klid_handle.is_some())
            };
            if let Some(l) = &layout {
                tracing::info!(layout = %l.label(), "non-US keyboard layout translation active");
            } else if auto_layout {
                tracing::info!("keyboard layout will auto-detect from the connecting client");
            }
            Ok(Self {
                source,
                source_hid,
                last_x: 0.0,
                last_y: 0.0,
                left_down: false,
                right_down: false,
                middle_down: false,
                mods: ModifierState::default(),
                target_display_id,
                cached_bounds: None,
                bounds_cached_at: Instant::now(),
                click_left: None,
                click_right: None,
                click_middle: None,
                scroll_v_accum: 0,
                scroll_h_accum: 0,
                consumed_keys: HashSet::new(),
                remapped_keys: HashSet::new(),
                layout,
                klid_handle,
                last_klid: 0,
                auto_layout,
                resync,
            })
        }

        /// How long a `target_bounds` result may be served from cache. Long
        /// enough to shed the per-mouse-move CoreGraphics query (hundreds/s
        /// during a drag), short enough that a display reconfiguration (rare;
        /// e.g. --detach-primary engaging at connect) mis-maps coords for at
        /// most a quarter second before self-correcting.
        const BOUNDS_CACHE_TTL: Duration = Duration::from_millis(250);

        /// Current global-coord bounds of the target display. Re-queried
        /// through a short TTL cache so a layout change since this handler
        /// was built (vd moving to (0,0) when --detach-primary disables the
        /// built-in panel) doesn't leave us posting to a stale rectangle,
        /// without paying a CG display lookup on every mouse move.
        fn target_bounds(&mut self) -> (f64, f64, f64, f64) {
            if let Some(b) = self.cached_bounds {
                if self.bounds_cached_at.elapsed() < Self::BOUNDS_CACHE_TTL {
                    return b;
                }
            }
            let d = match self.target_display_id.get() {
                Some(id) => CGDisplay::new(id),
                None => CGDisplay::main(),
            };
            let b = d.bounds();
            let bounds = (b.origin.x, b.origin.y, b.size.width, b.size.height);
            self.cached_bounds = Some(bounds);
            self.bounds_cached_at = Instant::now();
            bounds
        }

        pub fn keyboard(&mut self, event: KeyboardEvent) {
            mark_input_activity();
            match event {
                KeyboardEvent::Pressed { code, extended } => self.key(code, extended, true),
                KeyboardEvent::Released { code, extended } => self.key(code, extended, false),
                KeyboardEvent::UnicodePressed(c) => self.unicode(c, true),
                KeyboardEvent::UnicodeReleased(c) => self.unicode(c, false),
                KeyboardEvent::Synchronize(sync) => self.synchronize(sync),
            }
        }

        /// MS-RDPBCGR Synchronize event — the client tells us its current
        /// lock-key state (Caps Lock, Num Lock, etc.). Only Caps Lock has a
        /// CGEventFlag equivalent on macOS, so we reconcile that one: if our
        /// internal toggle disagrees with the client, flip ours and emit a
        /// FlagsChanged so any focused app's AlphaShift query is correct
        /// from the first keystroke. (Num/Scroll/Kana Lock are kernel-side
        /// concepts on Windows that macOS doesn't model.)
        fn synchronize(&mut self, sync: SynchronizeFlags) {
            let want_caps = sync.contains(SynchronizeFlags::CAPS_LOCK);
            if want_caps != self.mods.caps_lock {
                debug!(
                    have = self.mods.caps_lock,
                    want = want_caps,
                    "syncing CapsLock to client state"
                );
                self.mods.caps_lock = want_caps;
                self.post_flags_changed(VK_CAPS_LOCK);
            }
        }

        /// When auto-detecting (no explicit `--keyboard-layout`), (re)resolve
        /// the translation layout from the client's announced KLID. Cheap when
        /// unchanged (one atomic load); only re-resolves on a new KLID, so a
        /// reconnect from a differently-configured client is picked up. US
        /// English (0x0409) and unknown (0) keep the positional keycode path.
        fn refresh_auto_layout(&mut self) {
            if !self.auto_layout {
                return;
            }
            let Some(handle) = &self.klid_handle else {
                return;
            };
            let klid = handle.load(std::sync::atomic::Ordering::Relaxed);
            if klid == self.last_klid {
                return;
            }
            self.last_klid = klid;
            self.layout = if klid != 0 && (klid & 0xFFFF) != 0x0409 {
                let spec = format!("0x{:04x}", klid & 0xFFFF);
                let resolved = crate::keyboard_layout::KeyboardLayout::resolve(&spec);
                match &resolved {
                    Some(l) => tracing::info!(
                        klid = format!("0x{klid:04X}"),
                        layout = %l.label(),
                        "auto-selected the client's keyboard layout"
                    ),
                    None => tracing::warn!(
                        klid = format!("0x{klid:04X}"),
                        "client announced a keyboard layout with no installed macOS match; using positional keycodes"
                    ),
                }
                resolved
            } else {
                None
            };
        }

        fn key(&mut self, scancode: u8, extended: bool, down: bool) {
            let Some(vk) = scancode_to_cgkeycode(scancode, extended) else {
                tracing::debug!(scancode, extended, down, "unmapped scancode");
                return;
            };
            self.refresh_auto_layout();

            // Modifier path: update per-side state and emit FlagsChanged
            // mirrored to both sources. We do this BEFORE any of the
            // symbolic-hotkey logic so the held-modifier check in
            // try_symbolic_hotkey sees the just-pressed modifier.
            if ModifierState::is_modifier_vk(vk) {
                let changed = self.mods.apply(vk, down);
                tracing::debug!(
                    scancode = format!("0x{scancode:02X}"),
                    extended,
                    down,
                    vk = format!("0x{vk:02X}"),
                    is_modifier = true,
                    flags = format!("0x{:08X}", self.mods.cg_flags().bits()),
                    changed,
                    "key post (modifier)"
                );
                if changed {
                    self.post_flags_changed(vk);
                }
                // Commit any active Cmd+Tab session the moment both
                // Cmd halves are released. Matches native macOS: the
                // app the cursor landed on becomes MRU front only on
                // release, not on each intermediate Tab press.
                //
                // Also bump CMD_RELEASE_GENERATION so the next Cmd+Tab
                // can detect this release happened — a session whose
                // stored generation differs from the current value
                // started in a *previous* Cmd hold and shouldn't be
                // resumed. This replaces the old time-based grace,
                // which falsely split sessions whenever the user
                // paused mid-hold longer than 2 s.
                let cmd_released = matches!(vk, VK_LCMD | VK_RCMD)
                    && !down
                    && !self.mods.l_cmd
                    && !self.mods.r_cmd;
                // With --alt-tab-switch, an Opt+Tab session commits on Option
                // release, mirroring the Cmd path (same release-generation
                // counter — you can't hold both switchers at once).
                let opt_released = super::ALT_TAB_SWITCH.load(std::sync::atomic::Ordering::Relaxed)
                    && matches!(vk, VK_LALT | VK_RALT)
                    && !down
                    && !self.mods.l_alt
                    && !self.mods.r_alt;
                if cmd_released || opt_released {
                    CMD_RELEASE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    commit_cycle_session();
                }
                return;
            }

            tracing::debug!(
                scancode = format!("0x{scancode:02X}"),
                extended,
                down,
                vk = format!("0x{vk:02X}"),
                is_modifier = false,
                flags = format!("0x{:08X}", self.mods.cg_flags().bits()),
                "key post"
            );

            // Symbolic-hotkey interception: WindowServer's internal hotkey
            // dispatcher (which fires Cmd+Tab, Cmd+Space, Cmd+Shift+3/4/5,
            // Mission Control, etc.) only triggers on kernel-injected HID
            // events — user-space CGEventPost cannot wake it, regardless of
            // source state or tap location. For the common combos we
            // re-implement the action in user space and swallow the
            // keystroke. Triggers on key-down; the matching key-up is
            // tracked in `consumed_keys` so the bare key-up doesn't reach
            // the focused app either.
            if down && self.try_symbolic_hotkey(vk) {
                self.consumed_keys.insert(vk);
                return;
            }
            if !down && self.consumed_keys.remove(&vk) {
                return;
            }

            // Ctrl→Cmd remap (--map-ctrl-to-cmd): rewrite a curated set of editing
            // shortcuts so Windows muscle memory (Ctrl+C/V/X/…) drives macOS
            // copy/paste. The key-down swaps Ctrl→Cmd (with matching session
            // FlagsChanged) so the app sees a clean Cmd+key; the matching key-up
            // is posted the same way and restores the held-Ctrl state. Suppressed
            // when a terminal / excluded app is frontmost so Ctrl+C stays SIGINT.
            // Checked before the layout branch so a remapped key-up isn't swallowed
            // there.
            if !down && self.remapped_keys.remove(&vk) {
                self.post_ctrl_as_cmd(vk, false);
                return;
            }
            if down
                && super::MAP_CTRL_TO_CMD.load(std::sync::atomic::Ordering::Relaxed)
                && self.mods.has_ctrl()
                && !self.mods.has_cmd()
                && !self.mods.has_alt()
                && is_remappable_shortcut(vk)
                && !frontmost_is_excluded()
            {
                self.post_ctrl_as_cmd(vk, true);
                self.remapped_keys.insert(vk);
                return;
            }

            // Non-US layout translation: for ordinary typing keys with no
            // Cmd/Ctrl held, produce the character this key yields in the
            // configured layout and post it as a Unicode string, leaving the
            // Mac's own input source untouched. Cmd/Ctrl combos fall through to
            // the keycode path below so shortcuts still work. Once a layout is
            // active these keys are fully owned here — the matching key-up is
            // swallowed too (Unicode string events have no release side, like
            // the RDP Unicode-keyboard path).
            if self.layout.is_some()
                && !self.mods.has_cmd()
                && !self.mods.has_ctrl()
                && crate::keyboard_layout::is_translatable_keycode(vk)
            {
                if down {
                    let shift = self.mods.has_shift();
                    let option = self.mods.has_alt();
                    let caps = self.mods.caps_lock;
                    if let Some(text) = self
                        .layout
                        .as_mut()
                        .and_then(|l| l.translate(vk, shift, option, caps))
                    {
                        // Empty string = a pending dead key; swallow it and let
                        // the composed character arrive on the next keystroke.
                        if !text.is_empty() {
                            let utf16: Vec<u16> = text.encode_utf16().collect();
                            if let Ok(ev) =
                                CGEvent::new_keyboard_event(self.source.clone(), 0, true)
                            {
                                ev.set_string_from_utf16_unchecked(&utf16);
                                ev.post(CGEventTapLocation::HID);
                            }
                        }
                    }
                }
                return;
            }

            let Ok(ev) = CGEvent::new_keyboard_event(self.source.clone(), vk, down) else {
                warn!(vk, down, "CGEvent::new_keyboard_event failed");
                return;
            };
            let mut flags = self.mods.cg_flags();
            // macOS adds CGEventFlagNumericPad on events from the numeric
            // keypad. Some apps (Finder for arrow navigation, games) key off
            // it. The keypad vk range is in the `scancodes` module.
            if is_numeric_pad_vk(vk) {
                flags |= CGEventFlags::CGEventFlagNumericPad;
            }
            ev.set_flags(flags);
            ev.post(CGEventTapLocation::HID);
        }

        /// Emit a FlagsChanged event for `vk` from both sources. We send
        /// the same event twice so two different parts of macOS see the
        /// modifier as held:
        ///   - CombinedSessionState backs `[NSEvent modifierFlags]` —
        ///     what Cocoa apps query for Cmd+C / Cmd+Q / Cmd+~.
        ///   - HIDSystemState backs the modifier view that WindowServer's
        ///     symbolic-hotkey dispatcher (Dock, Spotlight, screencapture,
        ///     Mission Control, Show Desktop) checks before firing.
        ///
        /// A FlagsChanged posted from one source does NOT update the other,
        /// so without the mirror you get either app shortcuts or symbolic
        /// hotkeys but not both.
        fn post_flags_changed(&self, vk: u16) {
            let flags = self.mods.cg_flags();
            for source in [&self.source, &self.source_hid] {
                // `down` parameter is ignored for FlagsChanged: macOS
                // derives press-vs-release purely from the diff between
                // the prior flags state and the new flags carried on the
                // event. We pass `true` for shape only.
                let Ok(ev) = CGEvent::new_keyboard_event(source.clone(), vk, true) else {
                    warn!(vk, "CGEvent::new_keyboard_event failed (modifier)");
                    continue;
                };
                ev.set_flags(flags);
                ev.set_type(CGEventType::FlagsChanged);
                ev.post(CGEventTapLocation::HID);
            }
        }

        /// Post a curated `Ctrl+<vk>` shortcut as `Cmd+<vk>` (--map-ctrl-to-cmd).
        /// On **down**: present Cmd-held (not Ctrl) to both modifier views via a
        /// FlagsChanged carrying the swapped flags, then post the key-down with
        /// those flags — so the focused app sees a clean Cmd+key. On **up**: post
        /// the key-up with the swapped flags, then restore the real (Ctrl) state
        /// via `post_flags_changed` (the user is still physically holding Ctrl, so
        /// `self.mods` already reflects it). Shift/Caps in `self.mods` carry
        /// through unchanged, so `Ctrl+Shift+Z` → `Cmd+Shift+Z` (redo) works.
        fn post_ctrl_as_cmd(&self, vk: u16, down: bool) {
            // Swap Control→Command in both the public CGEventFlag bits and the
            // device-dependent NX bits, leaving every other modifier intact.
            let mut bits = self.mods.cg_flags().bits();
            bits &= !CGEventFlags::CGEventFlagControl.bits();
            bits &= !(NX_DEVICE_L_CTRL | NX_DEVICE_R_CTRL);
            bits |= CGEventFlags::CGEventFlagCommand.bits();
            bits |= NX_DEVICE_L_CMD;
            let swapped = CGEventFlags::from_bits_retain(bits);

            if down {
                for source in [&self.source, &self.source_hid] {
                    let Ok(ev) = CGEvent::new_keyboard_event(source.clone(), VK_LCMD, true) else {
                        warn!("CGEvent::new_keyboard_event failed (ctrl→cmd flags)");
                        continue;
                    };
                    ev.set_flags(swapped);
                    ev.set_type(CGEventType::FlagsChanged);
                    ev.post(CGEventTapLocation::HID);
                }
            }
            if let Ok(ev) = CGEvent::new_keyboard_event(self.source.clone(), vk, down) {
                ev.set_flags(swapped);
                ev.post(CGEventTapLocation::HID);
            } else {
                warn!(vk, "CGEvent::new_keyboard_event failed (ctrl→cmd key)");
            }
            if !down {
                // Restore the real modifier state (Ctrl still held).
                self.post_flags_changed(VK_LCMD);
            }
        }

        /// Match the current modifier state + non-modifier vk against the
        /// set of symbolic hotkeys we re-implement. Returns true if we
        /// handled it (and the caller should suppress the keystroke).
        fn try_symbolic_hotkey(&self, vk: u16) -> bool {
            let f = self.mods.cg_flags();
            let cmd = f.contains(CGEventFlags::CGEventFlagCommand);
            let shift = f.contains(CGEventFlags::CGEventFlagShift);
            let ctrl = f.contains(CGEventFlags::CGEventFlagControl);
            let opt = f.contains(CGEventFlags::CGEventFlagAlternate);

            // Ctrl+Option+G: on-demand "gather windows" — sweep any window stranded
            // off the session display (e.g. an app opened on the now-blanked
            // physical panel under --capture-primary) back onto the display the RDP
            // client sees. From a Windows client this is Ctrl+Alt+G — deliberately
            // Win-key-free so mstsc forwards it, and it's not a system shortcut on
            // either OS. Only acts when there's a virtual display to gather onto;
            // runs off-thread so input never stalls.
            if ctrl && opt && !cmd && vk == VK_G {
                if let Some(id) = self.target_display_id.get() {
                    std::thread::spawn(move || {
                        let moved = gather_windows_onto_display(id);
                        tracing::info!(
                            moved,
                            display_id = id,
                            "on-demand gather-windows hotkey (Ctrl+Option+G)"
                        );
                    });
                    return true;
                }
                // No virtual display — nothing to gather onto; let the key through.
                return false;
            }

            // Ctrl+Option+Shift+R: on-demand A/V resync. Recovers a session gone
            // stale after a long idle — a blanked mstsc surface and/or drifted
            // audio (Windows' audiodg buffers playback downstream where the
            // server can't see it, so this is a manual lever, not auto-detected).
            // Raises the shared resync request: the video path (`capture.rs`) forces a
            // clean IDR keyframe to repaint a stale/idle-blanked mstsc
            // presentation (live-verified to un-blank with no reconnect flicker —
            // the heavier core reactivation cascades into a session re-cycle on
            // the headless virtual-display path, so it's kept only as an
            // escalation), and the audio path (`audio.rs`) rebuilds its SCK stream
            // (a brief gap lets the client's audio backlog drain, the same
            // principle as a minimize→unminimize resync). From a Windows client
            // this is Ctrl+Alt+Shift+R — Win-key-free so mstsc forwards it, and
            // not a system shortcut on either OS. Cheap atomic stores, so no
            // off-thread hop needed; the consumers act on their next poll.
            if ctrl && opt && shift && !cmd && vk == VK_R {
                self.resync.request();
                tracing::info!(
                    "on-demand A/V resync hotkey (Ctrl+Option+Shift+R) — refreshing video (IDR) + rebuilding audio"
                );
                return true;
            }

            // Opt+Tab / Opt+Shift+Tab as an alternative app-cycle trigger
            // (opt-in via --alt-tab-switch). Only plain Option (no Cmd/Ctrl) so
            // it doesn't shadow Option-as-AltGr typing or Cmd shortcuts; the
            // session commits on Option release (see the modifier-release block).
            if super::ALT_TAB_SWITCH.load(std::sync::atomic::Ordering::Relaxed)
                && opt
                && !cmd
                && !ctrl
                && vk == VK_TAB
            {
                cycle_apps(shift);
                return true;
            }

            // Opt+` / Opt+Shift+` as an alternative within-app window-cycle
            // trigger (opt-in via --alt-backtick-switch), mirroring Cmd+`/Cmd+Shift+`.
            // No release-session bookkeeping needed here (unlike Opt+Tab above) —
            // ax_cycle_windows_of_front acts immediately on each press.
            if super::ALT_BACKTICK_SWITCH.load(std::sync::atomic::Ordering::Relaxed)
                && opt
                && !cmd
                && !ctrl
                && vk == VK_GRAVE
            {
                ax_cycle_windows_of_front(shift);
                return true;
            }

            // Left/Right arrows step an ACTIVE app-switcher cycle, matching
            // native Cmd+Tab (hold the switcher, arrow to move the selection).
            // Gated on a live cycle session so bare arrows reach the focused app
            // the rest of the time; that session only exists while the switcher
            // modifier is held (Cmd, or Option with --alt-tab-switch), and the
            // extra cmd||opt guard keeps a stray arrow from a lingering
            // grace-window session from being swallowed. Right = forward (next in
            // the row), Left = backward (previous). Reuses cycle_apps, so it
            // continues the frozen snapshot and drives the HUD's ADVANCE exactly
            // like a Tab tap. Placed before the `!cmd` gate below because an
            // Option+Tab session holds Option, not Cmd.
            if (cmd || opt) && !ctrl && (vk == VK_LEFT || vk == VK_RIGHT) && cycle_session_active()
            {
                cycle_apps(vk == VK_LEFT);
                return true;
            }

            if !cmd {
                return false;
            }
            // Only Cmd / Cmd+Shift are interesting here — any other extra
            // modifier means the combo isn't one of the symbolic hotkeys
            // WindowServer would have caught.
            if ctrl || opt {
                return false;
            }

            match (shift, vk) {
                (false, VK_TAB) => {
                    cycle_apps(false);
                    true
                }
                (true, VK_TAB) => {
                    cycle_apps(true);
                    true
                }
                (false, VK_SPACE) => {
                    invoke_spotlight();
                    true
                }
                (true, VK_3) => {
                    screencapture(&["-x"]);
                    true
                }
                (true, VK_4) => {
                    screencapture(&["-i"]);
                    true
                }
                (true, VK_5) => {
                    open_screenshot_app();
                    true
                }
                // Cmd+` / Cmd+Shift+` — cycle windows of the currently
                // frontmost app, native macOS semantics. Lets the user
                // explicitly step through windows within (e.g.) VSCode
                // without affecting the inter-app cycle. Whether the
                // RDP client forwards the backtick scancode at all
                // depends on its key-passthrough setting; mstsc treats
                // Win+` as a local "switch input source" combo by
                // default in some Windows versions, so it may need to
                // be re-bound on the client side to reach us.
                (false, VK_GRAVE) => {
                    ax_cycle_windows_of_front(false);
                    true
                }
                (true, VK_GRAVE) => {
                    ax_cycle_windows_of_front(true);
                    true
                }
                _ => false,
            }
        }

        fn unicode(&self, c: u16, down: bool) {
            // For unicode, send a "null" keycode and set the string. Only fire
            // on key-down — Mac doesn't have a release-side for typed text.
            if !down {
                return;
            }
            let Ok(ev) = CGEvent::new_keyboard_event(self.source.clone(), 0, true) else {
                warn!("unicode CGEvent create failed");
                return;
            };
            ev.set_string_from_utf16_unchecked(&[c]);
            ev.post(CGEventTapLocation::HID);
        }

        pub fn mouse(
            &mut self,
            event: MouseEvent,
            desktop_w: u16,
            desktop_h: u16,
            letterbox: bool,
        ) {
            mark_input_activity();
            match event {
                MouseEvent::Move { x, y } => self.move_to(x, y, desktop_w, desktop_h, letterbox),
                MouseEvent::LeftPressed => self.button(CGMouseButton::Left, true),
                MouseEvent::LeftReleased => self.button(CGMouseButton::Left, false),
                MouseEvent::RightPressed => self.button(CGMouseButton::Right, true),
                MouseEvent::RightReleased => self.button(CGMouseButton::Right, false),
                MouseEvent::MiddlePressed => self.button(CGMouseButton::Center, true),
                MouseEvent::MiddleReleased => self.button(CGMouseButton::Center, false),
                MouseEvent::VerticalScroll { value } => self.scroll(i32::from(value), 0),
                MouseEvent::Scroll { x, y } => self.scroll(y, x),
                MouseEvent::Button4Pressed
                | MouseEvent::Button4Released
                | MouseEvent::Button5Pressed
                | MouseEvent::Button5Released => {
                    trace!(?event, "extra mouse buttons not implemented");
                }
                MouseEvent::RelMove { x, y } => self.move_rel(x, y),
            }
        }

        fn move_to(&mut self, x: u16, y: u16, desktop_w: u16, desktop_h: u16, letterbox: bool) {
            // Scale desktop coords → screen points, then translate into the
            // target display's slot in the global coord space. The origin
            // offset is what makes CGEventPost route events to a non-primary
            // display (virtual or external) — the WindowServer dispatches by
            // which display contains the global coord.
            let (ox, oy, sw, sh) = self.target_bounds();
            let (mx, my) =
                super::map_client_to_display(x, y, desktop_w, desktop_h, sw, sh, letterbox);
            self.post_move(ox + mx, oy + my);
        }

        /// Relative mouse motion (MS-RDPBCGR `TS_POINTERREL_EVENT` / the
        /// MS-RDPEI "Advanced Input" channel's REL flag), sent by clients that
        /// emulate a trackpad rather than mapping touches 1:1 (e.g. Windows
        /// App's "Mouse pointer" input mode on iOS). Unlike `Move`, there is
        /// no absolute client coordinate to map — the delta is applied
        /// directly to the server-side cursor position, clamped to the
        /// target display's bounds so a client that keeps pushing past an
        /// edge (exactly the gesture macOS's Dock/menu-bar auto-reveal
        /// watches for) leaves the cursor pinned at the true edge pixel
        /// instead of silently doing nothing. Previously this variant was
        /// dropped entirely (`trace!` no-op), which — on a client that relies
        /// on it for some or all pointer motion — left the cursor unable to
        /// reach the screen edge at all.
        fn move_rel(&mut self, dx: i32, dy: i32) {
            let (ox, oy, sw, sh) = self.target_bounds();
            let sx = (self.last_x + f64::from(dx)).clamp(ox, ox + sw - 1.0);
            let sy = (self.last_y + f64::from(dy)).clamp(oy, oy + sh - 1.0);
            self.post_move(sx, sy);
        }

        fn post_move(&mut self, sx: f64, sy: f64) {
            self.last_x = sx;
            self.last_y = sy;

            let etype = if self.left_down {
                CGEventType::LeftMouseDragged
            } else if self.right_down {
                CGEventType::RightMouseDragged
            } else if self.middle_down {
                CGEventType::OtherMouseDragged
            } else {
                CGEventType::MouseMoved
            };
            let button = if self.middle_down {
                CGMouseButton::Center
            } else if self.right_down {
                CGMouseButton::Right
            } else {
                CGMouseButton::Left
            };
            let Ok(ev) =
                CGEvent::new_mouse_event(self.source.clone(), etype, CGPoint::new(sx, sy), button)
            else {
                return;
            };
            ev.post(CGEventTapLocation::HID);
        }

        fn button(&mut self, button: CGMouseButton, down: bool) {
            let etype = match (button, down) {
                (CGMouseButton::Left, true) => CGEventType::LeftMouseDown,
                (CGMouseButton::Left, false) => CGEventType::LeftMouseUp,
                (CGMouseButton::Right, true) => CGEventType::RightMouseDown,
                (CGMouseButton::Right, false) => CGEventType::RightMouseUp,
                (CGMouseButton::Center, true) => CGEventType::OtherMouseDown,
                (CGMouseButton::Center, false) => CGEventType::OtherMouseUp,
            };
            match button {
                CGMouseButton::Left => self.left_down = down,
                CGMouseButton::Right => self.right_down = down,
                CGMouseButton::Center => self.middle_down = down,
            }

            // Compute the click count: increment on a down event close in
            // time + space to the previous click; reset to 1 otherwise. The
            // matching up event reuses the count from the most recent down
            // so Finder sees a paired {down, up} with identical click_state.
            let state_slot = match button {
                CGMouseButton::Left => &mut self.click_left,
                CGMouseButton::Right => &mut self.click_right,
                CGMouseButton::Center => &mut self.click_middle,
            };
            let click_count = if down {
                let now = Instant::now();
                let count = match *state_slot {
                    Some(prev)
                        if now.duration_since(prev.time) <= DOUBLE_CLICK_INTERVAL
                            && (self.last_x - prev.x).abs() <= DOUBLE_CLICK_SLOP_PX
                            && (self.last_y - prev.y).abs() <= DOUBLE_CLICK_SLOP_PX =>
                    {
                        prev.count + 1
                    }
                    _ => 1,
                };
                *state_slot = Some(ClickState {
                    time: now,
                    x: self.last_x,
                    y: self.last_y,
                    count,
                });
                count
            } else {
                state_slot.map(|s| s.count).unwrap_or(1)
            };

            let Ok(ev) = CGEvent::new_mouse_event(
                self.source.clone(),
                etype,
                CGPoint::new(self.last_x, self.last_y),
                button,
            ) else {
                warn!("CGEvent mouse button create failed");
                return;
            };
            ev.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, click_count);
            ev.post(CGEventTapLocation::HID);

            // On a button-down, record which app's window is under the cursor so
            // the Ctrl→Cmd suppression knows the real focus target — the only way
            // to catch an Electron app (VSCode) clicked into in a headless session,
            // which takes key focus without activating. Gated to when the remap is
            // on (the only consumer); the hit-test runs off-thread, best-effort.
            if down && super::MAP_CTRL_TO_CMD.load(std::sync::atomic::Ordering::Relaxed) {
                update_focus_from_click(self.last_x, self.last_y);
            }
        }

        fn scroll(&mut self, vertical: i32, horizontal: i32) {
            // RDP wheel rotation units are 120 per "tick", but this client
            // slices a single gesture into many small legacy MOUSE PDU
            // events (observed raw magnitude ~2-40 per event, not one
            // 120-per-notch value). A flat `delta / SCALE` per event
            // truncates almost every one of those to zero — scroll never
            // moves at all. Accumulate the raw delta across calls instead,
            // and only emit whole lines once the running total crosses
            // SCALE, carrying the remainder forward so no motion is lost.
            const SCALE: i32 = 60;
            self.scroll_v_accum += vertical;
            self.scroll_h_accum += horizontal;
            let v = (self.scroll_v_accum / SCALE).clamp(-5, 5);
            let h = (self.scroll_h_accum / SCALE).clamp(-5, 5);
            self.scroll_v_accum -= v * SCALE;
            self.scroll_h_accum -= h * SCALE;
            if v == 0 && h == 0 {
                return;
            }
            let Ok(ev) =
                CGEvent::new_scroll_event(self.source.clone(), ScrollEventUnit::LINE, 2, v, h, 0)
            else {
                return;
            };
            ev.post(CGEventTapLocation::HID);
        }
    }
}

/// Probe whether this process has Accessibility (AX) permission, prompting if
/// not. Without it, posted CGEvents are silently dropped by the WindowServer.
#[cfg(target_os = "macos")]
pub fn ensure_accessibility_access() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    use std::os::raw::c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    }

    // kAXTrustedCheckOptionPrompt — passing true makes macOS surface the
    // "Allow X to control this computer" prompt the first time we ask.
    let key = CFString::from_static_string("AXTrustedCheckOptionPrompt");
    let value = CFBoolean::true_value();
    let opts = CFDictionary::from_CFType_pairs(&[(key, value)]);
    // SAFETY: `opts` is a CFDictionary that outlives the call.
    unsafe { AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef().cast()) }
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_accessibility_access() -> bool {
    true
}

#[cfg(test)]
mod coord_tests {
    use super::{is_remappable_shortcut, map_client_to_display};

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.5
    }

    #[test]
    fn curated_keys_remap_and_q_does_not() {
        // Curated editing keys (C, V, X, A, Z, S) remap.
        for vk in [0x08u16, 0x09, 0x07, 0x00, 0x06, 0x01] {
            assert!(is_remappable_shortcut(vk), "vk {vk:#x} should remap");
        }
        // Q (0x0C) is deliberately excluded so Ctrl+Q can't become Cmd+Q.
        assert!(!is_remappable_shortcut(0x0C));
        // Arrows / nav keys are untouched (e.g. left arrow 0x7B).
        assert!(!is_remappable_shortcut(0x7B));
    }

    #[test]
    fn stretch_maps_full_frame() {
        // 16:9 client onto a 16:10 Mac, fill mode: corners map to corners.
        // The far corner clamps to the LAST pixel (1511,981), not the size
        // (1512,982) — see the `s - 1` note in map_client_to_display.
        let (mx, my) = map_client_to_display(1920, 1080, 1920, 1080, 1512.0, 982.0, false);
        assert!(approx(mx, 1511.0) && approx(my, 981.0));
        let (mx, my) = map_client_to_display(960, 540, 1920, 1080, 1512.0, 982.0, false);
        assert!(approx(mx, 756.0) && approx(my, 491.0)); // center
    }

    #[test]
    fn stretch_clamps_a_client_coordinate_past_its_own_desktop_bounds() {
        // A 1:1 client/Mac size (the client-resolution auto-adopt path,
        // which forces letterbox=false since there's no aspect mismatch to
        // bar). Some clients (iOS Windows App's "Mouse pointer" mode) report
        // y past their own negotiated desktop_h when the user keeps dragging
        // beyond where their on-screen cursor visually stopped. Unclamped,
        // that posts the Mac cursor below the display's real bottom edge
        // instead of pinned at it.
        // Clamped to the LAST ROW (504), not the height (505) — y == sh is the
        // first row past the display, which on a headless-vd layout is dead
        // space owned by no display, so the Dock's edge trigger never fires.
        let (_, my) = map_client_to_display(356, 549, 944, 505, 944.0, 505.0, false);
        assert!(approx(my, 504.0));
        // Same one-pixel rule horizontally: x == sw would be the physical
        // panel's first column when it's parked at (sw, 0).
        let (mx, _) = map_client_to_display(1200, 300, 944, 505, 944.0, 505.0, false);
        assert!(approx(mx, 943.0));
    }

    #[test]
    fn pillarbox_when_client_wider_than_mac() {
        // Mac 1512x982 (~1.54) into 1920x1080 (~1.78, wider) → bars left/right.
        // content_w = 1080 * 1.54 = 1663.7, off_x = (1920-1663.7)/2 = 128.1.
        let sw = 1512.0;
        let sh = 982.0;
        // center stays center
        let (mx, my) = map_client_to_display(960, 540, 1920, 1080, sw, sh, true);
        assert!(approx(mx, sw / 2.0) && approx(my, sh / 2.0));
        // left bar (x=50 < off_x) clamps to the left edge
        let (mx, _) = map_client_to_display(50, 540, 1920, 1080, sw, sh, true);
        assert!(approx(mx, 0.0));
        // right bar clamps to the right edge
        let (mx, _) = map_client_to_display(1900, 540, 1920, 1080, sw, sh, true);
        assert!(approx(mx, sw - 1.0));
        // top/bottom fill the height → no vertical clamp at extremes
        let (_, my) = map_client_to_display(960, 0, 1920, 1080, sw, sh, true);
        assert!(approx(my, 0.0));
        let (_, my) = map_client_to_display(960, 1080, 1920, 1080, sw, sh, true);
        assert!(approx(my, sh - 1.0));
    }

    #[test]
    fn letterbox_when_client_taller_than_mac() {
        // Mac 1512x982 (~1.54) into 1280x1024 (1.25, taller) → bars top/bottom.
        let sw = 1512.0;
        let sh = 982.0;
        let (mx, my) = map_client_to_display(640, 512, 1280, 1024, sw, sh, true);
        assert!(approx(mx, sw / 2.0) && approx(my, sh / 2.0)); // center
        let (_, my) = map_client_to_display(640, 5, 1280, 1024, sw, sh, true);
        assert!(approx(my, 0.0)); // top bar clamps up
        let (mx, _) = map_client_to_display(0, 512, 1280, 1024, sw, sh, true);
        assert!(approx(mx, 0.0)); // full width → left edge maps to 0
    }
}
