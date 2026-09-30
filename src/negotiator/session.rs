//! Whole-session plan (spec §6).

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

#[allow(
    dead_code,
    reason = "phase 1 of the negotiated-session plan; wired in by a later phase"
)]
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
#[allow(
    dead_code,
    reason = "phase 1 of the negotiated-session plan; wired in by a later phase"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Overrides {
    pub map_ctrl_to_cmd: Option<bool>,
}

#[allow(
    dead_code,
    reason = "phase 1 of the negotiated-session plan; wired in by a later phase"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPlan {
    /// `None` when virtual displays are unavailable (physical capture fallback).
    pub display: Option<DisplayPlan>,
    pub blank_physical: bool,
    pub map_ctrl_to_cmd: bool,
    pub reasons: Vec<String>,
}

#[allow(
    dead_code,
    reason = "phase 1 of the negotiated-session plan; wired in by a later phase"
)]
pub fn negotiate(client: &ClientCaps, host: &HostCaps, overrides: &Overrides) -> SessionPlan {
    let mut reasons = Vec::new();
    let display = if host.virtual_display_available {
        let d = plan_display(client.monitor);
        reasons.push(format!("display: {}", d.reason));
        Some(d)
    } else {
        reasons.push(
            "display: virtual displays unavailable — capturing a physical display".to_owned(),
        );
        None
    };
    let blank_physical = display.is_some() && host.physical_displays > 0;
    if blank_physical {
        reasons.push(format!(
            "privacy: blanking {} physical display(s) while remote",
            host.physical_displays
        ));
    }
    let map_ctrl_to_cmd = match overrides.map_ctrl_to_cmd {
        Some(v) => {
            reasons.push(format!(
                "input: Ctrl→Cmd {} (override)",
                if v { "on" } else { "off" }
            ));
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
    SessionPlan {
        display,
        blank_physical,
        map_ctrl_to_cmd,
        reasons,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::negotiator::display::{ClientMonitor, ScaleMode};

    fn client(platform: ClientPlatform, scale: u32) -> ClientCaps {
        ClientCaps {
            monitor: ClientMonitor {
                width_px: 3840,
                height_px: 2160,
                desktop_scale_pct: scale,
            },
            platform,
        }
    }
    fn host(physical: usize, vd: bool) -> HostCaps {
        HostCaps {
            physical_displays: physical,
            virtual_display_available: vd,
        }
    }

    #[test]
    fn classify_platform_from_general_capability() {
        assert_eq!(
            classify_platform("Windows/WindowsNt"),
            ClientPlatform::Windows
        );
        assert_eq!(classify_platform("OsX/Unspecified"), ClientPlatform::Apple);
        assert_eq!(
            classify_platform("Macintosh/Unspecified"),
            ClientPlatform::Apple
        );
        assert_eq!(classify_platform("IOs/Unspecified"), ClientPlatform::Apple);
        assert_eq!(
            classify_platform("Unix/NativeXServer"),
            ClientPlatform::Other
        );
        assert_eq!(classify_platform("unknown"), ClientPlatform::Other);
    }

    #[test]
    fn ctrl_to_cmd_on_for_windows_clients() {
        let p = negotiate(
            &client(ClientPlatform::Windows, 150),
            &host(0, true),
            &Overrides::default(),
        );
        assert!(p.map_ctrl_to_cmd);
    }

    #[test]
    fn ctrl_to_cmd_off_for_apple_clients() {
        let p = negotiate(
            &client(ClientPlatform::Apple, 200),
            &host(0, true),
            &Overrides::default(),
        );
        assert!(!p.map_ctrl_to_cmd);
        assert!(
            p.reasons.iter().any(|r| r.contains("Ctrl→Cmd off")),
            "{:?}",
            p.reasons
        );
    }

    #[test]
    fn override_wins_over_heuristic() {
        let o = Overrides {
            map_ctrl_to_cmd: Some(false),
        };
        assert!(
            !negotiate(&client(ClientPlatform::Windows, 100), &host(0, true), &o).map_ctrl_to_cmd
        );
    }

    #[test]
    fn virtual_display_plan_when_available() {
        let p = negotiate(
            &client(ClientPlatform::Windows, 150),
            &host(0, true),
            &Overrides::default(),
        );
        let d = p.display.expect("display plan");
        assert_eq!(d.mode, ScaleMode::RetinaDownscaled);
        assert!(!p.blank_physical);
    }

    #[test]
    fn no_display_plan_without_virtual_display() {
        let p = negotiate(
            &client(ClientPlatform::Windows, 150),
            &host(1, false),
            &Overrides::default(),
        );
        assert!(p.display.is_none());
        assert!(
            p.reasons
                .iter()
                .any(|r| r.contains("virtual displays unavailable")),
            "{:?}",
            p.reasons
        );
    }

    #[test]
    fn blank_physical_when_a_monitor_is_attached() {
        let p = negotiate(
            &client(ClientPlatform::Windows, 100),
            &host(1, true),
            &Overrides::default(),
        );
        assert!(p.blank_physical);
    }

    #[test]
    fn connect_defaults_enable_the_negotiated_features() {
        let d = connect_defaults(&host(0, true));
        assert!(
            d.virtual_display
                && d.enable_h264
                && d.adaptive_bitrate
                && d.udp_multitransport
                && d.client_resolution
        );
        let d = connect_defaults(&host(1, false));
        assert!(!d.virtual_display);
        assert!(
            d.reasons.iter().any(|r| r.contains("physical display")),
            "{:?}",
            d.reasons
        );
    }
}
