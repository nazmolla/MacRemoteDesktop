# Phase 1 — Session Negotiator + Headless Single User Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Connecting with a stock RDP client to a headless Mac yields a correctly sized, correctly scaled (Retina where the client is HiDPI), sRGB-correct desktop on a virtual display, with no server flags — every choice made by a pure, tested Negotiator from the client handshake and the host's state, with a logged reason per decision.

**Architecture:** New pure module `src/negotiator/` (`display.rs` scale rules, `session.rs` whole-session plan, `host.rs` display classification) with no macOS calls. Two small vendored IronRDP divergences carry the client's scale factors/physical size from the GCC Core Data to a new `ConnectionHandler::on_client_display` hook. The virtual display gains Retina modes (per `docs/research/2026-09-29-hidpi-virtual-display.md`) and sRGB primaries. `main.rs` builds its defaults from the Negotiator; the old CLI flags become hidden overrides.

**Tech Stack:** Rust (stable 1.98), vendored `ironrdp-acceptor` / `ironrdp-server`, `ironrdp-displaycontrol` (git rev a5d1c682), objc2 (`CGVirtualDisplay*`), CoreGraphics display-mode API, ScreenCaptureKit (`screencapturekit` 2.1), FreeRDP 3 (`sdl-freerdp`) for loopback verification.

**Spec:** `docs/superpowers/specs/2026-09-29-negotiated-mac-rdp-design.md` — §2 principle, §6 Negotiator, §7 displays, §8.1 color, §14 Phase 1 row.

## Global Constraints

- New code adds **no user-facing flags**; existing flags stay as `hide = true` developer overrides that win over the Negotiator (spec §2, §6.3).
- `negotiate()` / `plan_display()` are pure: no macOS calls, no I/O (spec §6.2).
- Every decision carries a human-readable reason, logged at INFO once per connection (spec §6.2).
- Scale rules (spec §7.2 + HiDPI research): 100% → 1× at client pixels; ≥ 200% → Retina, points = pixels ÷ 2; 125–175% → Retina at points = pixels ÷ scale, captured down to client pixels.
- Retina backing must stay ≤ 8192 px per axis (virtual display descriptor max); otherwise fall back to 1×.
- Widths from clients are even (MS-RDPEDISP); heights may be odd.
- Virtual displays use sRGB primaries: R(0.64, 0.33) G(0.30, 0.60) B(0.15, 0.06) W(0.3127, 0.3290) (spec §8.1).
- Every vendored change gets a numbered entry in that crate's `CLAUDE.md` divergence log **and** a row in `FORK.md`.
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Client sends no scale factor** (FreeRDP default, older mstsc) → must behave exactly like today at 100%, never divide by zero or pick Retina. Pinned by `plan_display_treats_missing_scale_as_100` (Task 1).
2. **Huge client monitor at a fractional scale** (8K at 125%) → Retina backing would exceed 8192 px; must fall back to 1× with a reason instead of failing the mode switch. Pinned by `plan_display_falls_back_when_backing_exceeds_max` (Task 1).
3. **Retina mode switch silently not taking effect** (observed in the HiDPI spike: `err = 0`, mode unchanged) → must verify and retry, and if it still fails, fall back to 1× at client pixels rather than serving a mis-sized session. Pinned by `select_mode_verifies_and_falls_back` (Task 4, macOS integration test).
4. **Live resize to a different scale** (window dragged from a 150% monitor to a 100% laptop screen) → the new scale must be applied, not only the new size. Pinned by `resize_request_carries_scale` (Task 6).
5. **The client identifies as a Mac** (Microsoft's macOS client) → Ctrl→Cmd remapping must stay off. Pinned by `ctrl_to_cmd_off_for_apple_clients` (Task 2).

---

### Task 1: Display scale rules (pure)

**Files:**
- Create: `src/negotiator/mod.rs`, `src/negotiator/display.rs`
- Modify: `src/main.rs` (add `mod negotiator;` after `mod multitransport;`)

**Interfaces:**
- Produces:
  - `negotiator::display::ClientMonitor { width_px: u32, height_px: u32, desktop_scale_pct: u32 }` (0 = not sent)
  - `negotiator::display::ScaleMode { OneX, Retina, RetinaDownscaled }`
  - `negotiator::display::DisplayPlan { points_w: u32, points_h: u32, hidpi: bool, capture_w: u32, capture_h: u32, mode: ScaleMode, reason: String }`
  - `negotiator::display::plan_display(m: ClientMonitor) -> DisplayPlan`
  - `negotiator::display::MAX_BACKING_PX: u32 = 8192`

- [ ] **Step 1: Module skeleton + failing tests**

`src/negotiator/mod.rs`:
```rust
//! Session Negotiator (spec §6): pure functions that turn what the client
//! advertised and what the host is into a session plan, with a reason for
//! every decision. No macOS calls, no I/O — everything here is unit-tested.

pub mod display;
```

`src/negotiator/display.rs` (tests only for now):
```rust
//! Display sizing and scale rules (spec §7.2, docs/research/2026-09-29-hidpi-virtual-display.md).

#[cfg(test)]
mod tests {
    use super::*;

    fn m(w: u32, h: u32, s: u32) -> ClientMonitor {
        ClientMonitor { width_px: w, height_px: h, desktop_scale_pct: s }
    }

    #[test]
    fn plan_display_100_percent_is_one_to_one() {
        let p = plan_display(m(1920, 1080, 100));
        assert_eq!((p.points_w, p.points_h, p.hidpi, p.capture_w, p.capture_h), (1920, 1080, false, 1920, 1080));
        assert_eq!(p.mode, ScaleMode::OneX);
    }

    #[test]
    fn plan_display_treats_missing_scale_as_100() {
        let p = plan_display(m(1714, 1287, 0));
        assert_eq!((p.points_w, p.points_h, p.hidpi), (1714, 1287, false));
        assert!(p.reason.contains("no scale"), "{}", p.reason);
    }

    #[test]
    fn plan_display_200_percent_is_pixel_exact_retina() {
        let p = plan_display(m(3840, 2160, 200));
        assert_eq!((p.points_w, p.points_h, p.hidpi, p.capture_w, p.capture_h), (1920, 1080, true, 3840, 2160));
        assert_eq!(p.mode, ScaleMode::Retina);
    }

    #[test]
    fn plan_display_150_percent_renders_retina_then_downscales() {
        let p = plan_display(m(3840, 2160, 150));
        assert_eq!((p.points_w, p.points_h, p.hidpi, p.capture_w, p.capture_h), (2560, 1440, true, 3840, 2160));
        assert_eq!(p.mode, ScaleMode::RetinaDownscaled);
    }

    #[test]
    fn plan_display_odd_height_at_150_rounds_points() {
        let p = plan_display(m(1714, 1287, 150));
        assert_eq!((p.points_w, p.points_h, p.capture_w, p.capture_h), (1143, 858, 1714, 1287));
    }

    #[test]
    fn plan_display_odd_height_at_200_is_downscaled_not_exact() {
        let p = plan_display(m(1714, 1287, 200));
        assert_eq!((p.points_w, p.points_h), (857, 644));
        assert_eq!(p.mode, ScaleMode::RetinaDownscaled);
    }

    #[test]
    fn plan_display_falls_back_when_backing_exceeds_max() {
        let p = plan_display(m(7680, 4320, 125));
        assert_eq!((p.points_w, p.points_h, p.hidpi), (7680, 4320, false));
        assert_eq!(p.mode, ScaleMode::OneX);
        assert!(p.reason.contains("8192"), "{}", p.reason);
    }

    #[test]
    fn plan_display_8k_at_200_fits() {
        let p = plan_display(m(7680, 4320, 200));
        assert_eq!((p.points_w, p.points_h, p.hidpi), (3840, 2160, true));
    }

    #[test]
    fn plan_display_scales_above_200_use_retina_at_half() {
        let p = plan_display(m(3840, 2160, 300));
        assert_eq!((p.points_w, p.points_h, p.hidpi), (1920, 1080, true));
        assert!(p.reason.contains("300%"), "{}", p.reason);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked negotiator::display 2>&1 | grep -E "^error\[" | sort -u | head`
Expected: `cannot find function plan_display` / `cannot find struct ClientMonitor`.

- [ ] **Step 3: Implement**

Insert above the tests in `src/negotiator/display.rs`:
```rust
/// Virtual displays are registered with an 8192×8192 maximum (MS-RDPBCGR limit).
pub const MAX_BACKING_PX: u32 = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientMonitor {
    pub width_px: u32,
    pub height_px: u32,
    /// Desktop scale factor in percent as sent by the client; 0 = not sent.
    pub desktop_scale_pct: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleMode {
    /// 1× mode at the client's pixel size.
    OneX,
    /// Retina mode whose backing equals the client's pixel size exactly.
    Retina,
    /// Retina mode captured down to the client's pixel size (GPU scaling).
    RetinaDownscaled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayPlan {
    /// Virtual display mode size in points.
    pub points_w: u32,
    pub points_h: u32,
    /// Select the Retina (2× backing) twin of the mode.
    pub hidpi: bool,
    /// Pixel size sent to the client (= the client monitor size).
    pub capture_w: u32,
    pub capture_h: u32,
    pub mode: ScaleMode,
    pub reason: String,
}

fn div_round(n: u32, d: u32) -> u32 {
    (n * 2 + d) / (2 * d)
}

pub fn plan_display(m: ClientMonitor) -> DisplayPlan {
    let (w, h) = (m.width_px, m.height_px);
    let one_x = |reason: String| DisplayPlan {
        points_w: w, points_h: h, hidpi: false, capture_w: w, capture_h: h, mode: ScaleMode::OneX, reason,
    };
    let scale = m.desktop_scale_pct;
    if scale == 0 {
        return one_x(format!("1× at {w}×{h}: client sent no scale factor"));
    }
    if scale < 113 {
        return one_x(format!("1× at {w}×{h}: client scale {scale}%"));
    }
    let (pw, ph) = if scale >= 188 {
        (w.div_ceil(2), h.div_ceil(2))
    } else {
        (div_round(w * 100, scale), div_round(h * 100, scale))
    };
    if 2 * pw > MAX_BACKING_PX || 2 * ph > MAX_BACKING_PX {
        return one_x(format!(
            "1× at {w}×{h}: Retina backing {}×{} for {scale}% would exceed {MAX_BACKING_PX} px",
            2 * pw,
            2 * ph
        ));
    }
    let exact = 2 * pw == w && 2 * ph == h;
    let mode = if exact { ScaleMode::Retina } else { ScaleMode::RetinaDownscaled };
    let note = if scale > 200 { " (macOS renders at most 2×; UI sized as 200%)" } else { "" };
    DisplayPlan {
        points_w: pw,
        points_h: ph,
        hidpi: true,
        capture_w: w,
        capture_h: h,
        mode,
        reason: format!(
            "Retina {pw}×{ph} pt ({}×{} px){} for client {w}×{h} at {scale}%{note}",
            2 * pw,
            2 * ph,
            if exact { "" } else { ", downscaled on capture" }
        ),
    }
}
```

Add to `src/main.rs` after `mod multitransport;`:
```rust
mod negotiator;
```

- [ ] **Step 4: Run tests**

Run: `cargo test --locked negotiator::display 2>&1 | grep -E "^test result"`
Expected: `test result: ok. 9 passed; 0 failed`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add src/negotiator src/main.rs
git commit -m "feat(negotiator): pure display scale rules (1×, Retina, Retina-downscaled)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Session plan + host display classification (pure)

**Files:**
- Create: `src/negotiator/session.rs`, `src/negotiator/host.rs`
- Modify: `src/negotiator/mod.rs`

**Interfaces:**
- Consumes: Task 1 `ClientMonitor`, `DisplayPlan`, `plan_display`.
- Produces:
  - `negotiator::host::OnlineDisplay { id: u32, builtin: bool, vendor: u32 }`
  - `negotiator::host::OUR_VENDOR: u32 = 0x6D61_6372`, `SCREEN_SHARING_VENDOR: u32 = 0x896`
  - `negotiator::host::physical_displays(online: &[OnlineDisplay]) -> Vec<u32>`
  - `negotiator::session::ClientPlatform { Windows, Apple, Other }`, `classify_platform(platform: &str) -> ClientPlatform`
  - `negotiator::session::ClientCaps { monitor: ClientMonitor, platform: ClientPlatform }`
  - `negotiator::session::HostCaps { physical_displays: usize, virtual_display_available: bool }`
  - `negotiator::session::Overrides { map_ctrl_to_cmd: Option<bool> }`
  - `negotiator::session::SessionPlan { display: Option<DisplayPlan>, blank_physical: bool, map_ctrl_to_cmd: bool, reasons: Vec<String> }`
  - `negotiator::session::negotiate(client: &ClientCaps, host: &HostCaps, overrides: &Overrides) -> SessionPlan`
  - `negotiator::session::connect_defaults(host: &HostCaps) -> StartupDefaults` with `StartupDefaults { virtual_display: bool, enable_h264: bool, adaptive_bitrate: bool, udp_multitransport: bool, client_resolution: bool, reasons: Vec<String> }`

- [ ] **Step 1: Failing tests**

`src/negotiator/host.rs`:
```rust
//! Classify the Mac's online displays (spec §7.5).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_sharing_and_our_displays_are_not_physical() {
        let online = [
            OnlineDisplay { id: 5, builtin: false, vendor: SCREEN_SHARING_VENDOR },
            OnlineDisplay { id: 12, builtin: false, vendor: OUR_VENDOR },
        ];
        assert!(physical_displays(&online).is_empty());
    }

    #[test]
    fn builtin_and_external_monitors_are_physical() {
        let online = [
            OnlineDisplay { id: 1, builtin: true, vendor: 0x610 },
            OnlineDisplay { id: 2, builtin: false, vendor: 0x1e6d },
            OnlineDisplay { id: 12, builtin: false, vendor: OUR_VENDOR },
        ];
        assert_eq!(physical_displays(&online), vec![1, 2]);
    }
}
```

`src/negotiator/session.rs`:
```rust
//! Whole-session plan (spec §6).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::negotiator::display::{ClientMonitor, ScaleMode};

    fn client(platform: ClientPlatform, scale: u32) -> ClientCaps {
        ClientCaps { monitor: ClientMonitor { width_px: 3840, height_px: 2160, desktop_scale_pct: scale }, platform }
    }
    fn host(physical: usize, vd: bool) -> HostCaps {
        HostCaps { physical_displays: physical, virtual_display_available: vd }
    }

    #[test]
    fn classify_platform_from_general_capability() {
        assert_eq!(classify_platform("Windows/WindowsNt"), ClientPlatform::Windows);
        assert_eq!(classify_platform("OsX/Unspecified"), ClientPlatform::Apple);
        assert_eq!(classify_platform("Macintosh/Unspecified"), ClientPlatform::Apple);
        assert_eq!(classify_platform("IOs/Unspecified"), ClientPlatform::Apple);
        assert_eq!(classify_platform("Unix/NativeXServer"), ClientPlatform::Other);
        assert_eq!(classify_platform("unknown"), ClientPlatform::Other);
    }

    #[test]
    fn ctrl_to_cmd_on_for_windows_clients() {
        let p = negotiate(&client(ClientPlatform::Windows, 150), &host(0, true), &Overrides::default());
        assert!(p.map_ctrl_to_cmd);
    }

    #[test]
    fn ctrl_to_cmd_off_for_apple_clients() {
        let p = negotiate(&client(ClientPlatform::Apple, 200), &host(0, true), &Overrides::default());
        assert!(!p.map_ctrl_to_cmd);
        assert!(p.reasons.iter().any(|r| r.contains("Ctrl→Cmd off")), "{:?}", p.reasons);
    }

    #[test]
    fn override_wins_over_heuristic() {
        let o = Overrides { map_ctrl_to_cmd: Some(false) };
        assert!(!negotiate(&client(ClientPlatform::Windows, 100), &host(0, true), &o).map_ctrl_to_cmd);
    }

    #[test]
    fn virtual_display_plan_when_available() {
        let p = negotiate(&client(ClientPlatform::Windows, 150), &host(0, true), &Overrides::default());
        let d = p.display.expect("display plan");
        assert_eq!(d.mode, ScaleMode::RetinaDownscaled);
        assert!(!p.blank_physical);
    }

    #[test]
    fn no_display_plan_without_virtual_display() {
        let p = negotiate(&client(ClientPlatform::Windows, 150), &host(1, false), &Overrides::default());
        assert!(p.display.is_none());
        assert!(p.reasons.iter().any(|r| r.contains("virtual displays unavailable")), "{:?}", p.reasons);
    }

    #[test]
    fn blank_physical_when_a_monitor_is_attached() {
        let p = negotiate(&client(ClientPlatform::Windows, 100), &host(1, true), &Overrides::default());
        assert!(p.blank_physical);
    }

    #[test]
    fn connect_defaults_enable_the_negotiated_features() {
        let d = connect_defaults(&host(0, true));
        assert!(d.virtual_display && d.enable_h264 && d.adaptive_bitrate && d.udp_multitransport && d.client_resolution);
        let d = connect_defaults(&host(1, false));
        assert!(!d.virtual_display);
        assert!(d.reasons.iter().any(|r| r.contains("physical display")), "{:?}", d.reasons);
    }
}
```

In `src/negotiator/mod.rs` add `pub mod host;` and `pub mod session;`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked negotiator:: 2>&1 | grep -E "^error\[" | sort -u | head`
Expected: unresolved `physical_displays`, `negotiate`, `classify_platform`, etc.

- [ ] **Step 3: Implement**

Above the tests in `src/negotiator/host.rs`:
```rust
/// Vendor id macrdp gives its own virtual displays ("macr").
pub const OUR_VENDOR: u32 = 0x6D61_6372;
/// Vendor id of the virtual display Apple Screen Sharing creates ("ARD screen"), observed on macOS 27.
pub const SCREEN_SHARING_VENDOR: u32 = 0x896;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnlineDisplay {
    pub id: u32,
    pub builtin: bool,
    pub vendor: u32,
}

/// Displays a person could be looking at in the room: built-in panels and real
/// monitors, excluding our own and Screen Sharing's virtual displays.
pub fn physical_displays(online: &[OnlineDisplay]) -> Vec<u32> {
    online
        .iter()
        .filter(|d| d.builtin || (d.vendor != OUR_VENDOR && d.vendor != SCREEN_SHARING_VENDOR))
        .map(|d| d.id)
        .collect()
}
```

Above the tests in `src/negotiator/session.rs`:
```rust
use crate::negotiator::display::{plan_display, ClientMonitor, DisplayPlan};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientPlatform {
    Windows,
    Apple,
    Other,
}

/// `platform` is ironrdp-server's fingerprint string: `"{major:?}/{minor:?}"`
/// from the client's General capability set.
pub fn classify_platform(platform: &str) -> ClientPlatform {
    let major = platform.split('/').next().unwrap_or("");
    match major {
        "Windows" => ClientPlatform::Windows,
        "OsX" | "Macintosh" | "IOs" => ClientPlatform::Apple,
        _ => ClientPlatform::Other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientCaps {
    pub monitor: ClientMonitor,
    pub platform: ClientPlatform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostCaps {
    pub physical_displays: usize,
    pub virtual_display_available: bool,
}

/// Developer overrides from hidden CLI flags / config; `None` = negotiate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Overrides {
    pub map_ctrl_to_cmd: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPlan {
    /// `None` when virtual displays are unavailable (physical capture fallback).
    pub display: Option<DisplayPlan>,
    pub blank_physical: bool,
    pub map_ctrl_to_cmd: bool,
    pub reasons: Vec<String>,
}

pub fn negotiate(client: &ClientCaps, host: &HostCaps, overrides: &Overrides) -> SessionPlan {
    let mut reasons = Vec::new();
    let display = if host.virtual_display_available {
        let d = plan_display(client.monitor);
        reasons.push(format!("display: {}", d.reason));
        Some(d)
    } else {
        reasons.push("display: virtual displays unavailable — capturing a physical display".to_owned());
        None
    };
    let blank_physical = display.is_some() && host.physical_displays > 0;
    if blank_physical {
        reasons.push(format!("privacy: blanking {} physical display(s) while remote", host.physical_displays));
    }
    let map_ctrl_to_cmd = match overrides.map_ctrl_to_cmd {
        Some(v) => {
            reasons.push(format!("input: Ctrl→Cmd {} (override)", if v { "on" } else { "off" }));
            v
        }
        None if client.platform == ClientPlatform::Apple => {
            reasons.push("input: Ctrl→Cmd off — client is an Apple device".to_owned());
            false
        }
        None => {
            reasons.push("input: Ctrl→Cmd on — client is not an Apple device".to_owned());
            true
        }
    };
    SessionPlan { display, blank_physical, map_ctrl_to_cmd, reasons }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupDefaults {
    pub virtual_display: bool,
    pub enable_h264: bool,
    pub adaptive_bitrate: bool,
    pub udp_multitransport: bool,
    pub client_resolution: bool,
    pub reasons: Vec<String>,
}

/// Server-wide defaults chosen before any client connects. Per-connection
/// choices (scale, Ctrl→Cmd) come from [`negotiate`].
pub fn connect_defaults(host: &HostCaps) -> StartupDefaults {
    let mut reasons = vec![
        "video: H.264 over EGFX on (clients without AVC fall back to bitmaps)".to_owned(),
        "network: adaptive bitrate on".to_owned(),
        "network: UDP offered; used only if the client accepts".to_owned(),
    ];
    let virtual_display = host.virtual_display_available;
    reasons.push(if virtual_display {
        "display: session runs on its own virtual display".to_owned()
    } else {
        "display: virtual displays unavailable — capturing the physical display".to_owned()
    });
    StartupDefaults {
        virtual_display,
        enable_h264: true,
        adaptive_bitrate: true,
        udp_multitransport: true,
        client_resolution: true,
        reasons,
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --locked negotiator:: 2>&1 | grep -E "^test result"`
Expected: `test result: ok. 19 passed; 0 failed` (9 display + 2 host + 8 session).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add src/negotiator
git commit -m "feat(negotiator): session plan, Ctrl→Cmd platform heuristic, physical-display classification

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Carry client display info through the handshake (vendored divergences)

**Files:**
- Modify: `vendor/ironrdp-acceptor/src/connection.rs` (field on `Acceptor` + `AcceptorResult`, capture in `BasicSettingsWaitInitial`), `vendor/ironrdp-acceptor/CLAUDE.md` (divergence entry)
- Modify: `vendor/ironrdp-server/src/server.rs` (`ConnectionHandler::on_client_display` default no-op; call it next to `on_client_fingerprint`), `vendor/ironrdp-server/CLAUDE.md`
- Modify: `FORK.md` (rows F3, F4)
- Test: `src/conn_test.rs` (new test)

**Interfaces:**
- Produces:
  - `ironrdp_acceptor::ClientDisplayInfo { desktop_width: u16, desktop_height: u16, desktop_scale_factor: Option<u32>, device_scale_factor: Option<u32>, physical_width_mm: Option<u32>, physical_height_mm: Option<u32> }` (re-exported from the crate root), field `client_display: ClientDisplayInfo` on `AcceptorResult`.
  - `ironrdp_server::ConnectionHandler::on_client_display(&mut self, info: &ironrdp_acceptor::ClientDisplayInfo) {}` (default no-op), called once per non-reactivation connection immediately after `on_client_fingerprint`.

- [ ] **Step 1: Failing integration test**

In `src/conn_test.rs`, add a test that drives the existing in-memory handshake helper with `desktop_scale_factor: 150` in the client `Config` and a `ConnectionHandler` test double that records `on_client_display`. Model it on the file's existing honor-client-size test: copy that test's setup, set the client config's `desktop_scale_factor: 150` (the field already exists in the config literal at `conn_test.rs:241`), install the recording handler with `.with_connection_handler(...)`, run the connect, and assert:
```rust
let seen = recorded.lock().unwrap().clone().expect("on_client_display called");
assert_eq!(seen.desktop_scale_factor, Some(150));
assert_eq!((seen.desktop_width, seen.desktop_height), (1920, 1080));
```
Name it `client_display_info_reaches_the_connection_handler`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked client_display_info_reaches 2>&1 | grep -E "^error\[" | head -3`
Expected: `no method named on_client_display` / `ClientDisplayInfo` not found.

- [ ] **Step 3: Implement the acceptor divergence**

In `vendor/ironrdp-acceptor/src/connection.rs`: define `ClientDisplayInfo` (fields above, `#[derive(Debug, Clone, Default, PartialEq, Eq)]`), add `client_display: ClientDisplayInfo` to `Acceptor` (default) and `AcceptorResult` (copied in the same places `client_build` is), and in `BasicSettingsWaitInitial`, next to the existing `self.client_build = …` line:
```rust
// (vendored) divergence (5): client display info for server-side negotiation.
let od = &gcc_blocks.core.optional_data;
self.client_display = ClientDisplayInfo {
    desktop_width: gcc_blocks.core.desktop_width,
    desktop_height: gcc_blocks.core.desktop_height,
    desktop_scale_factor: od.desktop_scale_factor,
    device_scale_factor: od.device_scale_factor,
    physical_width_mm: od.desktop_physical_width,
    physical_height_mm: od.desktop_physical_height,
};
```
Carry it across `new_deactivation_reactivation` the same way `client_build` is. Re-export `ClientDisplayInfo` from `vendor/ironrdp-acceptor/src/lib.rs`. Append divergence (5) to `vendor/ironrdp-acceptor/CLAUDE.md` describing exactly this.

- [ ] **Step 4: Implement the server divergence**

In `vendor/ironrdp-server/src/server.rs`, add to `trait ConnectionHandler` (next to `on_client_fingerprint`):
```rust
/// (vendored) Client display info from the GCC Core Data (scale factors,
/// physical size, requested desktop size), once per connection, right after
/// `on_client_fingerprint`. Default: ignore.
fn on_client_display(&mut self, info: &ironrdp_acceptor::ClientDisplayInfo) {
    let _ = info;
}
```
and in the `if !result.reactivation {` block, directly after the `on_client_fingerprint(...)` call inside `if let Some(handler)`:
```rust
handler.borrow_mut().on_client_display(&result.client_display);
```
Append the next free divergence number to `vendor/ironrdp-server/CLAUDE.md`. Add FORK.md rows:
```markdown
| F3 | `vendor/ironrdp-acceptor` | divergence (5): `ClientDisplayInfo` on `AcceptorResult` | Negotiator needs client scale/physical size | Yes |
| F4 | `vendor/ironrdp-server` | `ConnectionHandler::on_client_display` hook | Deliver `ClientDisplayInfo` to the app | Yes |
```

- [ ] **Step 5: Run tests**

Run: `cargo test --locked 2>&1 | grep -E "^test result"`
Expected: all `ok`; total = previous total + 1 (the new conn test) + 19 negotiator tests.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add vendor/ironrdp-acceptor vendor/ironrdp-server src/conn_test.rs FORK.md
git commit -m "feat(handshake): deliver client scale factors + physical size to the app (vendored divergences)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Retina modes + sRGB primaries on the virtual display

**Files:**
- Modify: `src/virtual_display/private_api.rs` (`create` sets primaries + `hiDPI = 1`; new `select_mode`), `src/virtual_display/mod.rs` (`VirtualDisplay::new_planned`, `VirtualDisplay::apply_plan`, `VirtualDisplay::backing_scale`)
- Modify: `docs/known-quirks.md` (supersede the HiDPI dead-end note with a pointer to the research doc), `FORK.md` (row F5)
- Test: `src/virtual_display/mod.rs` (`#[cfg(all(test, target_os = "macos"))]` ignored integration tests)

**Interfaces:**
- Consumes: `negotiator::display::DisplayPlan` (Task 1).
- Produces:
  - `VirtualDisplay::new_planned(plan: &DisplayPlan) -> Result<VirtualDisplay>`
  - `VirtualDisplay::apply_plan(&mut self, plan: &DisplayPlan) -> Result<AppliedMode>` where `AppliedMode { points_w: u32, points_h: u32, pixels_w: u32, pixels_h: u32, fell_back_to_one_x: bool }`
  - `VirtualDisplay::backing_pixels(&self) -> (u32, u32)`

- [ ] **Step 1: Failing integration tests (Review Focus #3)**

Add to `src/virtual_display/mod.rs`:
```rust
#[cfg(all(test, target_os = "macos"))]
mod planned_tests {
    use super::VirtualDisplay;
    use crate::negotiator::display::{plan_display, ClientMonitor};

    fn plan(w: u32, h: u32, s: u32) -> crate::negotiator::display::DisplayPlan {
        plan_display(ClientMonitor { width_px: w, height_px: h, desktop_scale_pct: s })
    }

    #[test]
    #[ignore = "needs WindowServer; run: cargo test -- --ignored planned_tests --test-threads=1"]
    fn retina_plan_gets_2x_backing() {
        let vd = VirtualDisplay::new_planned(&plan(2560, 1440, 200)).expect("create");
        assert_eq!(vd.size_pts(), (1280.0, 720.0));
        assert_eq!(vd.backing_pixels(), (2560, 1440));
    }

    #[test]
    #[ignore = "needs WindowServer"]
    fn one_x_plan_is_one_to_one() {
        let vd = VirtualDisplay::new_planned(&plan(1714, 1288, 100)).expect("create");
        assert_eq!(vd.backing_pixels(), (1714, 1288));
    }

    #[test]
    #[ignore = "needs WindowServer"]
    fn select_mode_verifies_and_falls_back() {
        let mut vd = VirtualDisplay::new_planned(&plan(1920, 1080, 100)).expect("create");
        let applied = vd.apply_plan(&plan(3000, 2000, 150)).expect("apply");
        // Either the Retina twin took effect, or we fell back to 1× at client pixels —
        // never a silently mis-sized display.
        if applied.fell_back_to_one_x {
            assert_eq!((applied.pixels_w, applied.pixels_h), (3000, 2000));
        } else {
            assert_eq!((applied.points_w, applied.points_h, applied.pixels_w, applied.pixels_h), (2000, 1333, 4000, 2666));
        }
        assert_eq!(vd.backing_pixels(), (applied.pixels_w, applied.pixels_h));
    }

    #[test]
    #[ignore = "needs WindowServer"]
    fn display_colorspace_is_srgb() {
        use core_graphics::display::CGDisplay;
        let vd = VirtualDisplay::new_planned(&plan(1920, 1080, 100)).expect("create");
        let red = super::srgb_red_in_display_space(vd.display_id());
        let _ = CGDisplay::new(vd.display_id());
        assert!((red[0] - 1.0).abs() < 0.01 && red[1].abs() < 0.01 && red[2].abs() < 0.01, "{red:?}");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked planned_tests -- --ignored --test-threads=1 2>&1 | grep -E "^error\[" | sort -u | head`
Expected: `no function new_planned`, `no method apply_plan`, `backing_pixels`, `srgb_red_in_display_space` not found.

- [ ] **Step 3: Implement in `private_api.rs`**

In `create`, after `setSerialNum`, set sRGB primaries (CGPoint chromaticities):
```rust
let _: () = msg_send![desc, setRedPrimary: CGPoint { x: 0.64, y: 0.33 }];
let _: () = msg_send![desc, setGreenPrimary: CGPoint { x: 0.30, y: 0.60 }];
let _: () = msg_send![desc, setBluePrimary: CGPoint { x: 0.15, y: 0.06 }];
let _: () = msg_send![desc, setWhitePoint: CGPoint { x: 0.3127, y: 0.3290 }];
```
In `apply_single_mode`, change `setHiDPI: 0u32` to `setHiDPI: 1u32` (every registered mode then gets a Retina twin; the 1× mode is still selectable).

Add `pub(super) fn select_mode(display_id: u32, points_w: u32, points_h: u32, hidpi: bool) -> Result<(u32, u32, u32, u32)>`:
enumerate `CGDisplayCopyAllDisplayModes(display_id, {kCGDisplayShowDuplicateLowResolutionModes: true})`, pick the mode with `width == points_w && height == points_h && pixelWidth == (if hidpi {2} else {1}) * points_w`, apply it with `CGBeginDisplayConfiguration` / `CGConfigureDisplayWithDisplayMode` / `CGCompleteDisplayConfiguration(kCGConfigureForSession)`, then poll `CGDisplayCopyDisplayMode(display_id)` for up to 3 s (50 ms steps) until `pixelWidth`/`width` match; on timeout retry the configuration once; return `(width, height, pixelWidth, pixelHeight)` of the current mode, or `Err` if no matching mode was offered or it never took effect. Use the `core-graphics` crate's `CGDisplayMode` (`CGDisplayMode::all_display_modes` accepts an options dictionary) and `CGDisplay::configure_display_with_display_mode`.

- [ ] **Step 4: Implement in `mod.rs`**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppliedMode {
    pub points_w: u32,
    pub points_h: u32,
    pub pixels_w: u32,
    pub pixels_h: u32,
    pub fell_back_to_one_x: bool,
}
```
`VirtualDisplay::new_planned(plan)`: `private_api::create(plan.points_w, plan.points_h, 60, "macrdp")`, then `apply_plan`-equivalent selection, then read bounds as `new` does. `apply_plan(plan)`: `apply_mode(plan.points_w, plan.points_h, 60)`, then `select_mode(id, points_w, points_h, plan.hidpi)`; if that errors and `plan.hidpi`, log a warning with the error, re-apply a 1× mode at `(plan.capture_w, plan.capture_h)`, `select_mode(..., false)`, and return `fell_back_to_one_x: true`. Refresh `origin_pts`/`size_pts` from `CGDisplayBounds` as `resize` does. `backing_pixels()` returns `(CGDisplayPixelsWide, CGDisplayPixelsHigh)`.

Add the test helper `fn srgb_red_in_display_space(id: u32) -> [f64; 3]`: create a `CGColor` with components (1,0,0,1) in the display's colorspace (`CGDisplayCopyColorSpace`), convert it to `kCGColorSpaceSRGB` with `CGColorCreateCopyByMatchingToColorSpace` (relative colorimetric), return the components.

Replace the HiDPI bullet in `docs/known-quirks.md` (line 57) with: "**Superseded 2026-09-29:** Retina virtual displays work on macOS 27 — see `docs/research/2026-09-29-hidpi-virtual-display.md`." Add FORK.md row:
```markdown
| F5 | `src/virtual_display/*` | hiDPI=1 + Retina mode selection; sRGB primaries | Spec §7.2, §8.1 | Yes |
```

- [ ] **Step 5: Run tests**

Run: `cargo test --locked planned_tests -- --ignored --test-threads=1 2>&1 | grep -E "^test |^test result"`
Expected: 4 passed. Then `cargo test --locked 2>&1 | grep "^test result"` — all ok.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add src/virtual_display docs/known-quirks.md FORK.md
git commit -m "feat(display): Retina modes and sRGB primaries on the virtual display, verified switch with 1× fallback

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Capture at client pixels from a Retina display

**Files:**
- Modify: `src/capture.rs` (`CaptureDisplay` gains `display_plan: Arc<Mutex<Option<DisplayPlan>>>`; `sync_virtual_display` applies the plan; capture output size = `capture_w × capture_h` with stretch (no letterbox) when a plan is active)
- Test: `src/capture.rs` tests

**Interfaces:**
- Consumes: Task 1 `DisplayPlan`, Task 4 `VirtualDisplay::apply_plan`.
- Produces: `capture::capture_output_size(plan: Option<&DisplayPlan>, desktop: (u16, u16)) -> ((u16, u16), bool /*letterbox*/)`.

- [ ] **Step 1: Failing unit test**

```rust
#[test]
fn capture_output_size_follows_the_plan() {
    use crate::negotiator::display::{plan_display, ClientMonitor};
    let p = plan_display(ClientMonitor { width_px: 3840, height_px: 2160, desktop_scale_pct: 150 });
    assert_eq!(capture_output_size(Some(&p), (3840, 2160)), ((3840, 2160), false));
    assert_eq!(capture_output_size(None, (1920, 1080)), ((1920, 1080), true));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked capture_output_size 2>&1 | grep -E "^error\[" | head -2`
Expected: `cannot find function capture_output_size`.

- [ ] **Step 3: Implement**

```rust
/// Output size and letterboxing for the ScreenCaptureKit stream. With a
/// negotiated display plan the virtual display's aspect already equals the
/// client's, so capture stretches exactly to the client's pixels (the Retina
/// backing is downscaled on the GPU). Without one, keep upstream's behavior.
pub fn capture_output_size(
    plan: Option<&crate::negotiator::display::DisplayPlan>,
    desktop: (u16, u16),
) -> ((u16, u16), bool) {
    match plan {
        Some(p) => ((p.capture_w as u16, p.capture_h as u16), false),
        None => (desktop, true),
    }
}
```
Use it where `SCStreamConfiguration` is built (`with_width`/`with_height`/`with_preserves_aspect_ratio`). In `sync_virtual_display`, when `display_plan` holds a plan and the virtual display's current mode differs, call `vd.apply_plan(&plan)` instead of `vd.resize(w, h)`; if `fell_back_to_one_x`, replace the stored plan with the 1× equivalent (`points = capture`, `hidpi = false`, `mode = OneX`, reason appended "(Retina switch failed; 1× fallback)").

- [ ] **Step 4: Run tests**

Run: `cargo test --locked 2>&1 | grep "^test result"` — all ok.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add src/capture.rs
git commit -m "feat(capture): capture Retina virtual display at the client's exact pixel size

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Per-connection negotiation + scale-aware live resize

**Files:**
- Modify: `src/capture.rs` (`request_initial_size` and `request_layout` produce a `DisplayPlan`; `PendingResize` carries the scale)
- Modify: `src/main.rs` (the connection handler implements `on_client_display` + `on_client_fingerprint` → stores `ClientCaps` in a shared slot read by `CaptureDisplay`; logs `SessionPlan.reasons`; applies `map_ctrl_to_cmd` to the input handler)
- Test: `src/capture.rs` tests

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces: `PendingResize::request_scaled(width: u16, height: u16, scale_pct: u32)`, `PendingResize::take_settled_scaled(debounce) -> Option<(u16, u16, u32)>`; shared `NegotiationSlot = Arc<Mutex<Option<ClientCaps>>>`.

- [ ] **Step 1: Failing tests (Review Focus #4)**

```rust
#[test]
fn resize_request_carries_scale() {
    let p = PendingResize::new();
    p.request_scaled(3000, 2000, 150);
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(p.take_settled_scaled(std::time::Duration::from_millis(10)), Some((3000, 2000, 150)));
    p.request_scaled(1920, 1080, 100);
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(p.take_settled_scaled(std::time::Duration::from_millis(10)), Some((1920, 1080, 100)));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked resize_request_carries_scale 2>&1 | grep -E "^error\[" | head -2`
Expected: no method `request_scaled`.

- [ ] **Step 3: Implement**

- `PendingResize`: add an `AtomicU32 scale` beside the packed size; `request_scaled` stores both; `take_settled_scaled` returns both; keep `request`/`take_settled` as wrappers (scale 0) for existing callers.
- `request_layout`: read the primary monitor's `desktop_scale_factor()` (`Option<u32>`, `ironrdp-displaycontrol`) and call `request_scaled(w, h, scale.unwrap_or(0))`. Remove the `--hidpi` pin from `resizable` when a virtual display is in use.
- Where the capture loop consumes a settled resize, build `plan_display(ClientMonitor { width_px, height_px, desktop_scale_pct })`, store it in `display_plan`, and resize through Task 5's path. Log the plan's `reason` at INFO.
- `request_initial_size`: read the `NegotiationSlot`; if it holds `ClientCaps`, run `negotiate(...)` with the host caps computed at startup, store `plan.display` in `display_plan`, and return `DesktopSize { width: capture_w, height: capture_h }` (clamped by `max_client_size`); log every `reasons` line at INFO with target `macrdp::negotiator`.
- `main.rs`: in the app's `ConnectionHandler` (`AuthGuardHandler`, `src/auth_guard.rs`), implement `on_client_fingerprint` to record `classify_platform(platform)` and `on_client_display` to record the `ClientMonitor` (`desktop_scale_factor.unwrap_or(0)`), writing `ClientCaps` into the shared `NegotiationSlot` (existing fingerprint audit behavior unchanged). Apply `map_ctrl_to_cmd` to the input handler per connection via its existing remap switch.

- [ ] **Step 4: Run tests**

Run: `cargo test --locked 2>&1 | grep "^test result"` — all ok.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add src/capture.rs src/main.rs src/auth_guard.rs
git commit -m "feat(negotiator): negotiate each connection from the handshake; scale-aware live resize

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Startup defaults from the Negotiator; flags become hidden overrides

**Files:**
- Modify: `src/main.rs` (`Args`: mark every flag listed in `docs/research/macrdp-flags.md` as "derived from client" or "derived from host" with `#[arg(hide = true)]`; build `HostCaps` at startup; apply `connect_defaults` where the corresponding flag was not given)
- Modify: `docs/cli.md` (header note: the flags are developer overrides; normal use needs none)

**Interfaces:**
- Consumes: Task 2 `connect_defaults`, `HostCaps`, `physical_displays`, `OnlineDisplay`.
- Produces: `fn probe_host_caps() -> HostCaps` in `main.rs` (macOS: `CGGetOnlineDisplayList` + `CGDisplayIsBuiltin` + `CGDisplayVendorNumber` → `physical_displays`; `virtual_display_available` = the `CGVirtualDisplay` class resolves at runtime).

- [ ] **Step 1: Failing test for "flag given beats default"**

Add to `main.rs` tests:
```rust
#[test]
fn explicit_flag_beats_negotiated_default() {
    assert!(resolve_bool(None, true));
    assert!(!resolve_bool(Some(false), true));
    assert!(resolve_bool(Some(true), false));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked explicit_flag_beats 2>&1 | grep -E "^error\[" | head -2`
Expected: `cannot find function resolve_bool`.

- [ ] **Step 3: Implement**

```rust
/// A developer override (Some) wins; otherwise the negotiated default.
fn resolve_bool(flag: Option<bool>, negotiated: bool) -> bool {
    flag.unwrap_or(negotiated)
}
```
Change the affected boolean flags (`enable_h264`, `adaptive_bitrate`, `enable_udp_multitransport`, `virtual_display`, `map_ctrl_to_cmd`) from `bool` to `Option<bool>` with `#[arg(long, hide = true, num_args = 0..=1, default_missing_value = "true")]` so `--enable-h264` still works and absence means "negotiate". At startup: `let host = probe_host_caps(); let d = connect_defaults(&host);` log `d.reasons` at INFO, then resolve each setting with `resolve_bool`. `--no-client-resolution` keeps its meaning (override to false). When a virtual display is chosen and `physical_displays > 0` and no headless-mode flag was given, engage the existing `ShieldedPrimary` path (upstream's `--shield-primary`), logging "privacy: blanking N physical display(s)".

- [ ] **Step 4: Run tests + a zero-flag smoke run**

Run: `cargo test --locked 2>&1 | grep "^test result"` — all ok.
Run: `cargo build --release --locked && (target/release/macrdp --bind 127.0.0.1:3390 --skip-auth --password perf & sleep 3; grep -E "macrdp::negotiator|virtual display attached|H.264|adaptive" ~/Library/Logs/macrdp.log | tail -8; pkill -f "target/release/macrdp --bind 127.0.0.1:3390")`
Expected: log lines show the negotiated defaults (virtual display, H.264, adaptive bitrate) with **no** feature flags passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add src/main.rs docs/cli.md
git commit -m "feat(negotiator): zero-flag startup — defaults negotiated from host caps; flags are hidden overrides

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Loopback end-to-end verification

**Files:**
- Create: `scripts/verify/phase1-loopback.sh`
- Modify: `docs/research/phase0-baseline.md` → no; create `docs/research/phase1-verification.md`

**Interfaces:**
- Consumes: everything above; `sdl-freerdp` scale options `/scale-desktop:<pct>` and `/size:WxH`.

- [ ] **Step 1: Write the verification script**

```bash
#!/usr/bin/env bash
# Phase 1 loopback verification: zero server flags; FreeRDP clients at several
# sizes/scales; assert the negotiated plan and the resulting virtual display mode.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
log="$HOME/Library/Logs/macrdp.log"
frdp=$(command -v sdl-freerdp || command -v sdl3-freerdp)
cases=("1920x1080 100 1×" "3840x2160 200 Retina 1920×1080" "3000x2000 150 Retina 2000×1333" "1714x1288 100 1×")
fail=0
for c in "${cases[@]}"; do
  read -r size scale expect rest <<<"$c"
  : > "$log"
  "$root/target/release/macrdp" --bind 127.0.0.1:3390 --skip-auth --password perf >/dev/null 2>&1 &
  srv=$!; sleep 3
  "$frdp" /v:127.0.0.1:3390 /u:"$USER" /p:perf /cert:ignore /gfx:avc444 /size:"$size" /scale-desktop:"$scale" >/dev/null 2>&1 &
  cli=$!; sleep 8
  line=$(grep "macrdp::negotiator" "$log" | grep "display:" | tail -1 || true)
  kill "$cli" "$srv" 2>/dev/null || true; wait 2>/dev/null || true
  if [[ "$line" == *"$expect"*"${rest:-}"* ]]; then echo "PASS $size@$scale%: $line"; else echo "FAIL $size@$scale%: got '$line'"; fail=1; fi
done
exit $fail
```

- [ ] **Step 2: Run it**

Run: `chmod +x scripts/verify/phase1-loopback.sh && scripts/verify/phase1-loopback.sh`
Expected: four `PASS` lines. (Runs while the Mac is locked; it checks negotiation and the display mode, not pixels.)

- [ ] **Step 3: Record and commit**

Create `docs/research/phase1-verification.md` with the four PASS lines verbatim and the date, plus a "Manual checks pending an unlocked Mac + real mstsc" list: windowed resize on an ultrawide, dragging the mstsc window between a 150% and a 100% monitor, clipboard Windows→Mac and Mac→Windows (text, image, file), visual sharpness at 150%.

```bash
git add scripts/verify/phase1-loopback.sh docs/research/phase1-verification.md
git commit -m "test(phase1): loopback negotiation verification across sizes and scales

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Phase 1 exit checklist
- [ ] `cargo test --locked` green; ignored macOS integration tests (`planned_tests`, color round-trip) green locally.
- [ ] `scripts/verify/phase1-loopback.sh` → 4 PASS with zero server flags.
- [ ] Manual list in `docs/research/phase1-verification.md` completed on real mstsc (user).
