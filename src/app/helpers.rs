//! Helper processes (the app-switcher HUD and the shield windows), sleep
//! prevention, and thread QoS.

use super::*;

/// Prevent macOS from going to sleep, dimming/sleeping the display, idle-
/// locking, or spinning down disks while macrdp is running. `caffeinate
/// -w PID` exits automatically when the supplied PID exits, so there's
/// nothing to clean up on shutdown.
#[cfg(target_os = "macos")]
pub(super) fn prevent_sleep() {
    let pid = std::process::id().to_string();
    let res = std::process::Command::new("/usr/bin/caffeinate")
        .args(["-dimsu", "-w", &pid])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match res {
        Ok(child) => info!(caffeinate_pid = child.id(), "preventing sleep / auto-lock"),
        Err(e) => warn!("could not spawn caffeinate to prevent sleep: {e}"),
    }
}

/// Locate the `macrdphud` app-switcher overlay helper: `MACRDP_HUD_HELPER` env
/// override, else the bundled copy next to us (`../Resources/macrdphud` from
/// `Contents/MacOS/macrdp`), else the dev build (`gui/.build/release/macrdphud`
/// up the repo tree from `target/<profile>/macrdp`).
#[cfg(target_os = "macos")]
pub(super) fn locate_hud_helper() -> Option<std::path::PathBuf> {
    if let Some(p) = crate::tunables::var_os("MACRDP_HUD_HELPER") {
        let p = std::path::PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    // Bundled: .../macrdp.app/Contents/MacOS/macrdp -> .../Contents/Resources/macrdphud
    if let Some(macos_dir) = exe.parent() {
        let bundled = macos_dir.join("../Resources/macrdphud");
        if bundled.is_file() {
            return Some(bundled);
        }
    }
    // Dev: .../<repo>/target/<profile>/macrdp -> .../<repo>/gui/.build/release/macrdphud
    let dev = exe
        .ancestors()
        .nth(3)
        .map(|root| root.join("gui/.build/release/macrdphud"));
    dev.filter(|p| p.is_file())
}

/// Locate the `macrdpshield` blanking helper. Same search order as
/// [`locate_hud_helper`]: `MACRDP_SHIELD_HELPER` env override, else the bundled
/// copy (`../Resources/macrdpshield`), else the dev build.
#[cfg(target_os = "macos")]
pub(super) fn locate_shield_helper() -> Option<std::path::PathBuf> {
    if let Some(p) = crate::tunables::var_os("MACRDP_SHIELD_HELPER") {
        let p = std::path::PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    if let Some(macos_dir) = exe.parent() {
        let bundled = macos_dir.join("../Resources/macrdpshield");
        if bundled.is_file() {
            return Some(bundled);
        }
    }
    let dev = exe
        .ancestors()
        .nth(3)
        .map(|root| root.join("gui/.build/release/macrdpshield"));
    dev.filter(|p| p.is_file())
}

/// Spawn the shield helper. Unlike the HUD helper (cosmetic, so a miss is a
/// warning), this one is **required** for `--shield-primary`: without it there is
/// nothing to blank the panel with, so a miss is a hard error and the server
/// refuses to start rather than running a "headless" session over a fully visible
/// desktop.
#[cfg(target_os = "macos")]
pub(super) fn spawn_shield_helper() -> Result<std::process::Child> {
    let path = locate_shield_helper().ok_or_else(|| {
        anyhow!(
            "--shield-primary needs the macrdpshield helper, which was not found. \
             Set MACRDP_SHIELD_HELPER, or build it with gui/make-shield-helper.sh. \
             Refusing to start: without the helper the physical display would stay \
             fully visible while the session claims to be headless."
        )
    })?;
    let mut cmd = std::process::Command::new(&path);
    cmd.env("MACRDP_SHIELD_PARENT", std::process::id().to_string());
    if let Some(port) = crate::tunables::var_os("MACRDP_SHIELD_PORT") {
        cmd.env("MACRDP_SHIELD_PORT", port);
    }
    // stderr is INHERITED, not nulled (the HUD helper nulls all three). Every
    // helper-side diagnostic — bind failure, a display it could not shield, the
    // achieved count — goes to stderr, and nulling it made macrdp structurally
    // blind to exactly the failures that leave the desktop visible. Inheriting
    // lands them in macrdp.err.log under the LaunchAgent.
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit());
    let child = cmd
        .spawn()
        .with_context(|| format!("spawning the shield helper {}", path.display()))?;
    info!(
        shield_pid = child.id(),
        helper = %path.display(),
        "shield helper spawned"
    );
    Ok(child)
}

/// Spawn the app-switcher HUD helper. Passes the loopback port and our pid (so it
/// self-exits if we die). Returns the child handle to hold for our lifetime.
#[cfg(target_os = "macos")]
pub(super) fn spawn_hud_helper() -> Option<std::process::Child> {
    let Some(path) = locate_hud_helper() else {
        warn!(
            "--app-switcher-hud set but the macrdphud helper was not found \
             (set MACRDP_HUD_HELPER, or build it: gui/make-hud-helper.sh); \
             the switcher works, just without the on-screen HUD"
        );
        return None;
    };
    let mut cmd = std::process::Command::new(&path);
    cmd.env("MACRDP_HUD_PARENT", std::process::id().to_string());
    if let Some(port) = crate::tunables::var_os("MACRDP_HUD_PORT") {
        cmd.env("MACRDP_HUD_PORT", port);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(child) => {
            info!(hud_pid = child.id(), helper = %path.display(), "app-switcher HUD helper spawned");
            Some(child)
        }
        Err(e) => {
            warn!(
                "could not spawn app-switcher HUD helper {}: {e}",
                path.display()
            );
            None
        }
    }
}

/// Boost the calling pthread to `QOS_CLASS_USER_INITIATED` so the macOS
/// scheduler keeps macrdp's worker threads ahead of background work
/// (rustc, Spotlight indexer, Time Machine, etc.) under CPU contention.
/// Without this, running a heavy compile on the same host produces
/// audible audio glitches on the RDP session because tokio's
/// default-QoS workers lose CPU time to nice-0 background processes.
///
/// USER_INITIATED is the right level here, not USER_INTERACTIVE —
/// macrdp is an interactive server the user is actively engaged with,
/// but USER_INTERACTIVE is reserved for UI animation / hard real-time
/// (e.g. WindowServer) and would starve the OS itself under contention.
///
/// **Important:** this is applied to tokio worker threads ONLY, not to
/// the main thread. An earlier attempt boosted the main thread too;
/// `CGVirtualDisplay initWithDescriptor:` runs on that thread, and
/// boosting it broke SCK's display enumeration — SCK couldn't see the
/// freshly-registered virtual display for up to a minute on mstsc
/// connection attempts. Workers-only works because the actual async
/// work happens on workers; main thread just sits in
/// `Runtime::block_on`.
///
/// `pthread_set_qos_class_self_np` has been stable since macOS 10.10
/// (2014). Best-effort: errors are silently ignored — losing the boost
/// is degraded behavior, not a failure.
#[cfg(target_os = "macos")]
pub(crate) fn boost_thread_qos() {
    use std::os::raw::{c_int, c_uint};
    // From <sys/qos.h>:
    //   QOS_CLASS_USER_INTERACTIVE = 0x21
    //   QOS_CLASS_USER_INITIATED   = 0x19
    //   QOS_CLASS_DEFAULT          = 0x15
    //   QOS_CLASS_UTILITY          = 0x11
    //   QOS_CLASS_BACKGROUND       = 0x09
    const QOS_CLASS_USER_INITIATED: c_uint = 0x19;
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: c_uint, relative_priority: c_int) -> c_int;
    }
    // SAFETY: pthread_set_qos_class_self_np only changes the calling thread's QoS class and takes
    // plain integers.
    unsafe {
        let _ = pthread_set_qos_class_self_np(QOS_CLASS_USER_INITIATED, 0);
    }
}
