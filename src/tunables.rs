//! Every `MACRDP_*` environment variable macrdp reads, in one place.
//!
//! Before this module the variables were read with `std::env::var` inside the
//! modules that used them, so they were invisible: not in `--help`, not in
//! `config.env`, and not in any list a reader could check. Now:
//!
//! - [`ALL`] is the one table of them, with type, default and meaning.
//! - [`load`] snapshots them once at startup (after `config.env` has been
//!   bridged into the environment) and logs every one that is set, so the
//!   effective non-default configuration is in the log of every run.
//! - Modules read them with [`var`] / [`var_os`], never `std::env::var`. Tests
//!   (below) fail if a `MACRDP_*` name appears in the source without an entry
//!   here, or if one is read directly from the environment.
//!
//! Most of these are experiment and tuning knobs behind the negotiated defaults
//! (the fork's rule is that sessions are configured by negotiation, not flags);
//! they exist for field diagnosis, not everyday use. Three are read by the
//! Objective-C USB shim with `getenv`; they are listed so they are documented
//! and logged, and `config.env` bridges them like the rest.

use std::collections::HashMap;
use std::env::VarError;
use std::ffi::OsString;
use std::sync::OnceLock;

/// The value type, for documentation and the startup log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    Float,
    Path,
}

/// One registered variable.
#[derive(Debug)]
pub struct Tunable {
    pub name: &'static str,
    pub kind: Kind,
    /// The value used when the variable is unset, as text.
    pub default: &'static str,
    /// Where it is read.
    pub read_by: &'static str,
    pub doc: &'static str,
}

const fn t(
    name: &'static str,
    kind: Kind,
    default: &'static str,
    read_by: &'static str,
    doc: &'static str,
) -> Tunable {
    Tunable {
        name,
        kind,
        default,
        read_by,
        doc,
    }
}

use Kind::{Bool, Float, Int, Path};

/// Every `MACRDP_*` variable macrdp reads.
pub const ALL: &[Tunable] = &[
    // Connection guard and audit log.
    t("MACRDP_CONN_GUARD", Bool, "1", "auth_guard", "Per-source rate limit and failed-login lockout (0 disables both, and the audit handler)."),
    t("MACRDP_GUARD_RL_MAX", Int, "10", "auth_guard", "Connection attempts per window per source before rate limiting (0 = no rate limit)."),
    t("MACRDP_GUARD_RL_WINDOW_SECS", Int, "60", "auth_guard", "Rate-limit sliding window, seconds."),
    t("MACRDP_GUARD_FAIL_THRESHOLD", Int, "5", "auth_guard", "Consecutive failures before the first lockout (0 = no lockout)."),
    t("MACRDP_GUARD_COOLDOWN_BASE_SECS", Int, "30", "auth_guard", "First lockout length; doubles per further failure."),
    t("MACRDP_GUARD_COOLDOWN_MAX_SECS", Int, "900", "auth_guard", "Cap on the escalated lockout."),
    t("MACRDP_AUDIT_LOG", Bool, "1", "auth_guard", "Write macrdp::audit events."),
    t("MACRDP_AUDIT_JSON", Bool, "0", "main", "Also write the JSON audit stream at the default path."),
    t("MACRDP_AUDIT_LOG_MAX_BYTES", Int, "10485760", "logging", "JSON audit file size before rotation."),
    t("MACRDP_AUDIT_LOG_MAX_FILES", Int, "5", "logging", "Rotated JSON audit files kept."),
    t("MACRDP_LOG_MAX_BYTES", Int, "10485760", "logging", "Main log file size before rotation."),
    t("MACRDP_LOG_MAX_FILES", Int, "5", "logging", "Rotated main log files kept."),
    // Process health and lifecycle.
    t("MACRDP_HEALTHCHECK", Bool, "on when headless", "health", "Runtime watchdog that exits (for launchd to restart) if the async runtime stops responding."),
    t("MACRDP_HEALTHCHECK_INTERVAL_SECS", Int, "15", "health", "Watchdog probe interval."),
    t("MACRDP_HEALTHCHECK_TIMEOUT_SECS", Int, "30", "health", "How long a probe may take before it counts as failed."),
    t("MACRDP_HEALTHCHECK_FAILURES", Int, "2", "health", "Consecutive failed probes before exiting."),
    t("MACRDP_DETACH_RESTART_ON_STUCK", Bool, "on when headless", "main", "Exit (to be restarted) when --detach-primary cannot re-enable the built-in display (#168)."),
    t("MACRDP_LOCK_ON_DISCONNECT_DELAY_MS", Int, "22500", "main", "Extra wait after the disconnect grace before --lock-on-disconnect locks."),
    t("MACRDP_AUTO_RECONNECT", Bool, "1", "main", "Provision the Server Auto-Reconnect Cookie so clients reconnect on their own after a drop."),
    t("MACRDP_NEGOTIATE", Bool, "1", "main", "Log the negotiated session plan (0 skips the negotiator's startup report)."),
    t("MACRDP_RETINA_VD", Bool, "0", "virtual_display", "Try Retina (2x backing) modes on the virtual display for high-DPI clients. Off: always 1x at the client's pixel size, the reliable path (a Retina request that lands on its 1x twin can leave the display stuck)."),
    // Helper processes and loopback ports.
    t("MACRDP_HUD_HELPER", Path, "bundled macrdphud", "main", "Path of the app-switcher HUD helper."),
    t("MACRDP_HUD_PORT", Int, "40243", "switcher_hud", "Loopback port of the app-switcher HUD helper."),
    t("MACRDP_SHIELD_HELPER", Path, "bundled macrdpshield", "main", "Path of the shield-window helper."),
    t("MACRDP_SHIELD_PORT", Int, "40244", "main", "Loopback port of the shield-window helper (passed to it)."),
    t("MACRDP_SHIELD_KEEP_PHYSICAL_MAIN", Bool, "1", "virtual_display", "Keep the physical panel as main under --shield-primary so the lock screen is visible."),
    t("MACRDP_SCARD_PORT", Int, "40242", "rdpdr::smartcard", "Loopback port of the smart-card bridge."),
    t("MACRDP_STATS_PORT", Int, "40245", "stats", "Loopback port of the read-only stats endpoint."),
    // Video encoding.
    t("MACRDP_GPU_CONVERT", Bool, "1", "videotoolbox", "Convert captured frames to YUV on the GPU (same maths as the CPU path); 0 converts on the CPU."),
    t("MACRDP_H264_FULL_RANGE", Bool, "1", "videotoolbox", "Encode full-range NV12 (0 = let VideoToolbox produce video range)."),
    t("MACRDP_H264_LENGTH_PREFIXED", Bool, "0", "h264", "Emit length-prefixed (AVCC) NAL units instead of Annex-B, for ironrdp-decoder interop."),
    t("MACRDP_H264", Bool, "1", "args", "Negotiate H.264 over EGFX. 0 keeps every session on legacy bitmaps (diagnosis)."),
    t("MACRDP_AVC_REGIONS", Bool, "1", "h264", "Send each H.264 frame with its changed regions (0 = one full-surface region, as upstream)."),
    t("MACRDP_LOSSLESS_REFINE", Bool, "1", "h264", "Re-send regions that stop changing losslessly (ClearCodec over EGFX); 0 turns it off."),
    t("MACRDP_AVC444", Bool, "0", "h264", "Offer AVC444 to clients that support it. Off: AVC420 only (AVC444 output is corrupt on real decoders until fixed)."),
    t("MACRDP_ADAPTIVE_FLOOR_FPS", Int, "10", "h264", "Adaptive controller: lowest frame rate it throttles to."),
    t("MACRDP_ADAPTIVE_QUEUE_HIGH_MS", Float, "100", "h264", "Adaptive controller: client queue depth treated as congestion."),
    t("MACRDP_ADAPTIVE_EWMA_ALPHA", Float, "0.3", "h264", "Adaptive controller: smoothing factor for RTT and queue samples."),
    t("MACRDP_ADAPTIVE_SEED_RTT_MS", Int, "50", "h264", "Start a link at or above this RTT at a third of the bitrate ceiling (0 disables)."),
    t("MACRDP_UDP_ADAPTIVE_BITRATE", Bool, "0", "h264", "Enable the adaptive bitrate controller (same as --adaptive-bitrate)."),
    t("MACRDP_UDP_ADAPTIVE_INTERVAL_MS", Int, "300", "h264", "Adaptive controller: evaluation interval."),
    t("MACRDP_UDP_ADAPTIVE_FLOOR_BPS", Int, "ceiling/8, min 500000", "h264", "Adaptive controller: lowest bitrate."),
    t("MACRDP_UDP_ADAPTIVE_INCREASE_BPS", Int, "ceiling/16, min 250000", "h264", "Adaptive controller: additive increase per clean interval."),
    t("MACRDP_UDP_ADAPTIVE_DECREASE", Float, "0.7", "h264", "Adaptive controller: multiplicative decrease on congestion."),
    t("MACRDP_UDP_ADAPTIVE_RETX_TOLERANCE", Int, "2", "h264", "Adaptive controller: UDP retransmits per interval tolerated before backing off."),
    // Blank-presentation recovery (mstsc reconnect blank; docs/known-quirks.md).
    t("MACRDP_BLANK_RECOVERY", Bool, "0", "h264", "Detect a client that decodes but never presents, and recover it (reactivate, then drop). Opt-in: its signal misfires on some presenting clients."),
    t("MACRDP_BLANK_RECOVERY_REACTIVATE", Bool, "1", "h264", "Recover with a bare deactivation-reactivation first (0 = drop the connection)."),
    t("MACRDP_BLANK_RECOVERY_MIN_QOE", Int, "24", "h264", "All-zero QoE reports needed before recovering."),
    t("MACRDP_BLANK_RECOVERY_MIN_RENDER_REPORTS", Int, "3", "h264", "Consecutive nonzero render reports that count as presenting."),
    t("MACRDP_BLANK_RECOVERY_ESTABLISHED_REPORTS", Int, "40", "h264", "Nonzero run after which a session counts as established."),
    t("MACRDP_BLANK_RECOVERY_ESTABLISHED_MIN_QOE", Int, "160", "h264", "All-zero reports needed to recover an established session."),
    t("MACRDP_BLANK_RECOVERY_ESTABLISHED_MAX_WAIT_MS", Int, "30000", "h264", "Wall-clock silence that recovers an established session."),
    t("MACRDP_BLANK_RECOVERY_ESTABLISHED_WALL_REPORTS", Int, "16", "h264", "Zero reports required alongside that wall-clock window."),
    t("MACRDP_BLANK_RECOVERY_ARM_MS", Int, "3000", "h264", "Time after connect before the detector may fire."),
    t("MACRDP_BLANK_RECOVERY_RETRY_MS", Int, "4000", "h264", "Minimum time between recovery attempts."),
    t("MACRDP_BLANK_RECOVERY_HEAL_CONFIRM_MS", Int, "8000", "h264", "Time an attempt has to show real presentation before the next one (0 disables)."),
    t("MACRDP_BLANK_RECOVERY_MAX_ATTEMPTS", Int, "1 (2 with reactivate)", "h264", "Recovery attempts per connection."),
    t("MACRDP_BLANK_RECOVERY_MAX_CONSECUTIVE_DROPS", Int, "3", "h264", "Consecutive blank drops before giving up (0 = no cap)."),
    t("MACRDP_BLANK_RECOVERY_MAX_RTT_MS", Int, "80", "h264", "At or above this link RTT the drop lever is withheld (0 disables the gate)."),
    t("MACRDP_BLANK_RECOVERY_MAX_WAIT_MS", Int, "4000", "h264", "Wall-clock fast path: time with only zero reports before recovering."),
    t("MACRDP_BLANK_RECOVERY_MIN_WALL_REPORTS", Int, "1", "h264", "Zero reports required for the wall-clock fast path."),
    // UDP multitransport.
    t("MACRDP_UDP_MIGRATE_EGFX", Bool, "0", "main", "Move EGFX onto the reliable UDP tunnel (same as --udp-migrate-egfx)."),
    t("MACRDP_UDP_MIGRATE_EGFX_LOSSY", Bool, "0", "main", "Diagnostic: move EGFX onto the lossy (DTLS) tunnel instead."),
    t("MACRDP_UDP_OFFER_FECL", Bool, "0", "main", "Offer the lossy UdpFecL transport (implied by --enable-lossy-audio)."),
    t("MACRDP_UDP_OFFER_MAX_RTT_MS", Int, "80", "main", "Do not offer UDP to a connection at or above this TCP RTT (0 disables)."),
    t("MACRDP_UDP_LOSSY_DELIVERY", Bool, "0", "main", "Lossy flows use deliver-on-arrival delivery (implied by --enable-lossy-audio)."),
    t("MACRDP_UDP_LOSSY_AUDIO_DUP", Bool, "0", "main", "Send each lossy datagram twice (implied by --enable-lossy-audio)."),
    t("MACRDP_UDP_LOSSY_AUDIO", Bool, "0", "main", "Register the lossy audio DVC (same as --enable-lossy-audio's audio half)."),
    t("MACRDP_UDP_TUNNEL_DEAD_SECS", Int, "30", "main", "Inbound silence after which a bound UDP tunnel is declared dead (0 disables)."),
    t("MACRDP_UDP_MT_COOLDOWN_SECS", Int, "600", "main", "Multitransport offers suppressed after a tunnel death (0 = none)."),
    t("MACRDP_UDP_EGFX_MAX_FRAME_LAG", Int, "16", "h264", "Frames the client may lag behind before EGFX-over-UDP throttles to a trickle."),
    t("MACRDP_UDP_EGFX_WATCHDOG", Bool, "1", "h264", "De-migrate EGFX to TCP when the reliable tunnel wedges."),
    t("MACRDP_UDP_EGFX_WATCHDOG_MS", Int, "3000", "h264", "Ack silence while shipping that counts as a wedge."),
    t("MACRDP_UDP_EGFX_WATCHDOG_ACTIVE_MS", Int, "1000", "h264", "Window in which frames must have shipped for the watchdog to judge."),
    t("MACRDP_UDP_EGFX_ACK_RECOVERY", Bool, "0", "h264", "EGFX-on-lossy: force an IDR when acks stall."),
    t("MACRDP_UDP_EGFX_ACK_ACTIVE_MS", Int, "500", "h264", "Ack recovery: shipping window it requires."),
    t("MACRDP_UDP_EGFX_ACK_STALL_MS", Int, "200", "h264", "Ack recovery: silence that counts as a stall."),
    t("MACRDP_UDP_EGFX_ACK_RECOVERY_MS", Int, "1000", "h264", "Ack recovery: minimum time between forced IDRs."),
    // Camera and USB redirection.
    t("MACRDP_CAMERA_DUMP", Bool, "0", "camera", "Write the camera H.264 stream and first decoded frames to $TMPDIR for debugging."),
    t("MACRDP_USB_PREFETCH_DEPTH", Int, "4", "usb_spike.m", "Concurrent bulk-IN reads kept in flight per streaming endpoint (1 disables read-ahead)."),
    t("MACRDP_USB_STREAM_STALL_MS", Int, "3000", "usb_spike.m", "Streaming bulk-IN stall watchdog (0 disables)."),
    t("MACRDP_USB_SPIKE_LINGER_SECS", Int, "0", "usb_spike.m", "Test bed: keep the USB spike process alive this long."),
];

/// Names that appear in the source but are not read as configuration: set by
/// macrdp for a helper process, or used only inside tests.
#[cfg(test)]
const NOT_CONFIGURATION: &[&str] = &[
    "MACRDP_HUD_PARENT",
    "MACRDP_SHIELD_PARENT",
    "MACRDP_SOAK_RECONNECTS",
    "MACRDP_LOG_MAX_BYTES_NOPE",
    "MACRDP_LOG_MAX_FILES_NOPE",
    "MACRDP_TEST_ENV_COUNT_RANGE",
    "MACRDP_TEST_TRUTHY",
];

static SNAPSHOT: OnceLock<HashMap<&'static str, OsString>> = OnceLock::new();

fn registered(name: &str) -> bool {
    ALL.iter().any(|t| t.name == name)
}

/// Snapshot every registered variable and log the ones that are set. Call once,
/// after `config.env` has been bridged into the environment. Later calls do
/// nothing.
pub fn load() {
    let mut set = HashMap::new();
    for t in ALL {
        if let Some(v) = std::env::var_os(t.name) {
            tracing::info!(
                name = t.name,
                value = %v.to_string_lossy(),
                default = t.default,
                kind = ?t.kind,
                read_by = t.read_by,
                meaning = t.doc,
                "tunable set"
            );
            set.insert(t.name, v);
        }
    }
    let _ = SNAPSHOT.set(set);
}

/// Read a registered variable. After [`load`] this is the startup snapshot;
/// before it (unit tests, early startup) it reads the live environment.
pub fn var_os(name: &str) -> Option<OsString> {
    debug_assert!(
        registered(name) || cfg!(test),
        "{name} is not in tunables::ALL"
    );
    match SNAPSHOT.get() {
        Some(snapshot) => snapshot.get(name).cloned(),
        None => std::env::var_os(name),
    }
}

/// [`var_os`] as a string, with `std::env::var`'s error type.
pub fn var(name: &str) -> Result<String, VarError> {
    match var_os(name) {
        Some(v) => v.into_string().map_err(VarError::NotUnicode),
        None => Err(VarError::NotPresent),
    }
}

/// An on/off switch read by value: unset, empty, `0`, `false`, `no` and `off`
/// (any case) are off; anything else is on.
pub fn truthy(name: &str) -> bool {
    match var(name) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
        Err(_) => false,
    }
}

/// A number, or `default` when unset or unparsable.
pub fn parsed<T: std::str::FromStr>(name: &str, default: T) -> T {
    var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                source_files(&path, out);
            } else if path
                .extension()
                .is_some_and(|e| e == "rs" || e == "m" || e == "swift")
            {
                out.push(path);
            }
        }
    }

    fn names_in(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(at) = rest.find("MACRDP_") {
            let tail = &rest[at..];
            let len = tail
                .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
                .unwrap_or(tail.len());
            out.push(tail[..len].trim_end_matches('_').to_owned());
            rest = &tail[len..];
        }
        out
    }

    /// Every `MACRDP_*` name quoted in the code is registered (or explicitly
    /// listed as not configuration), so none can be added invisibly.
    #[test]
    fn every_quoted_name_is_registered() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        source_files(&root.join("src"), &mut files);
        source_files(&root.join("vendor"), &mut files);
        let mut missing = std::collections::BTreeSet::new();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            for line in text.lines() {
                let code = line.trim_start();
                if code.starts_with("//") || code.starts_with("///") || code.starts_with("//!") {
                    continue;
                }
                for name in names_in(line) {
                    let quoted = line.contains(&format!("\"{name}\""));
                    if quoted && !registered(&name) && !NOT_CONFIGURATION.contains(&name.as_str()) {
                        missing.insert(format!("{name} ({})", f.display()));
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "unregistered MACRDP_ names: {missing:?}"
        );
    }

    /// Nothing outside this module reads a `MACRDP_*` variable from the
    /// environment directly.
    #[test]
    fn no_direct_environment_reads() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        source_files(&root.join("src"), &mut files);
        source_files(&root.join("vendor"), &mut files);
        let mut direct = Vec::new();
        for f in files {
            if f.ends_with("tunables.rs") || f.extension().is_some_and(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&f).unwrap();
            for (i, line) in text.lines().enumerate() {
                for call in ["env::var(\"MACRDP_", "env::var_os(\"MACRDP_"] {
                    if line.contains(call) {
                        direct.push(format!("{}:{}", f.display(), i + 1));
                    }
                }
            }
        }
        assert!(
            direct.is_empty(),
            "read through crate::tunables instead: {direct:?}"
        );
    }

    #[test]
    fn names_are_unique_and_documented() {
        let mut seen = std::collections::HashSet::new();
        for t in ALL {
            assert!(seen.insert(t.name), "{} listed twice", t.name);
            assert!(t.name.starts_with("MACRDP_"), "{}", t.name);
            assert!(!t.doc.is_empty() && !t.default.is_empty(), "{}", t.name);
        }
    }

    #[test]
    fn truthy_reads_by_value() {
        for (value, on) in [
            ("1", true),
            ("yes", true),
            ("0", false),
            ("off", false),
            ("", false),
        ] {
            std::env::set_var("MACRDP_TEST_TRUTHY", value);
            assert_eq!(truthy("MACRDP_TEST_TRUTHY"), on, "{value:?}");
        }
        std::env::remove_var("MACRDP_TEST_TRUTHY");
        assert!(!truthy("MACRDP_TEST_TRUTHY"));
    }
}
