//! Product identity: the name people see, and the identifiers derived from it.
//!
//! Change the product's name here. Internal names (the crate, `MACRDP_*`
//! tunables, the bundled helper executables) deliberately keep the upstream
//! `macrdp` prefix; they are never shown to users.

/// Display name: app bundle, virtual display, prompts.
pub const NAME: &str = "Portico";

/// Lower-case identifier: executable, Keychain service, log file stem.
pub const ID: &str = "portico";
