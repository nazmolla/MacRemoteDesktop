//! Product identity: the name people see, and the identifiers derived from it.
//!
//! Change the product's name here. Internal names (the crate, `MACRDP_*`
//! tunables, the bundled helper executables) deliberately keep the upstream
//! `macrdp` prefix; they are never shown to users. The bundle identifier and
//! LaunchAgent label stay `ca.nazmi.portico` (see `packaging/make-app.sh`):
//! macOS keys the Screen Recording and Accessibility grants to it.

/// Keychain services read before [`ID`], newest first, so an entry stored
/// under an earlier product name keeps working.
pub const LEGACY_KEYCHAIN_SERVICES: &[&str] = &["portico", "macrdp"];

/// Display name: app bundle, virtual display, prompts.
pub const NAME: &str = "Viga";

/// Lower-case identifier: executable, Keychain service, log file stem.
pub const ID: &str = "viga";
