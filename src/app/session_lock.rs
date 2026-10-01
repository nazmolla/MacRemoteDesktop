//! Locking the Mac when the last client leaves, and unlocking it again when
//! one reconnects (`--lock-on-disconnect`, `--auto-unlock`).

use super::*;

/// Spawn the session-transition watcher used by `--detach-primary`
/// and `--capture-primary`. On the 0→≥1 client transition (after a
/// short debounce that absorbs mstsc's cert-trust flap) it calls
/// `install(vd_id)` and stows the returned guard in `slot`; on
/// ≥1→0 it takes the guard back out and drops it (which is what
/// runs the actual re-enable / release). Edge-triggered: changes
/// between two non-zero counts are no-ops.
/// cfg-safe wrapper around the macOS-only window-gather so the cross-platform
/// overlay watcher compiles on Linux CI. Returns the number of windows moved.
#[cfg(target_os = "macos")]
pub(super) fn restore_gather_windows(display_id: u32) -> usize {
    input::gather_windows_onto_display(display_id)
}
#[cfg(not(target_os = "macos"))]
pub(super) fn restore_gather_windows(_display_id: u32) -> usize {
    0
}

/// Extra safety buffer on top of the overlay watcher's `REACTIVATION_GRACE`
/// before `--lock-on-disconnect` actually locks the screen. Default chosen to
/// clear the documented blank-recovery worst case (~12-15s drop-then-ARC-
/// reconnect cycle, see docs/known-quirks.md) with real margin — a false-
/// positive lock mid a legitimate self-heal is a worse outcome than staying
/// unlocked a bit longer than strictly necessary after a genuine disconnect.
/// Tunable via MACRDP_LOCK_ON_DISCONNECT_DELAY_MS / config
/// LOCK_ON_DISCONNECT_DELAY_MS.
pub(super) const LOCK_ON_DISCONNECT_DELAY_DEFAULT_MS: u64 = 22_500; // + 2.5s grace ≈ 25s total

pub(super) fn parse_lock_on_disconnect_delay_ms(env_override: Option<&str>) -> u64 {
    env_override
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(LOCK_ON_DISCONNECT_DELAY_DEFAULT_MS)
}

pub(super) fn lock_on_disconnect_delay_ms() -> u64 {
    parse_lock_on_disconnect_delay_ms(
        crate::tunables::var("MACRDP_LOCK_ON_DISCONNECT_DELAY_MS")
            .ok()
            .as_deref(),
    )
}

/// Lock the local macOS session by launching `ScreenSaverEngine.app`
/// (no special TCC/entitlement needed beyond what `open` itself needs).
/// **Note the older `CGSession -suspend` menu-extra trick documented widely
/// online does NOT work here** — as of macOS 26 the `Menu Extras/User.menu`
/// bundle that historically shipped `CGSession` no longer exists (confirmed
/// live: only `AirPort.menu`/`VPN.menu`/etc. remain under `Menu Extras/`).
/// `open -a ScreenSaverEngine` engages the screensaver, which macOS then
/// treats as an actual password-required lock **only if the account has
/// "require password" set to Immediately** (`sysadminctl -screenLock
/// status`) — macrdp does not check or change that setting (changing
/// security settings is out of scope), so this is a best-effort action: on
/// an account configured with a delayed password requirement, this merely
/// starts the screensaver rather than truly locking. Returns whether the
/// launch command itself succeeded (best-effort — we can't independently
/// confirm the screen actually locked, only that `open` didn't fail).
#[cfg(target_os = "macos")]
pub(super) fn lock_session() -> bool {
    const SCREEN_SAVER_ENGINE: &str = "/System/Library/CoreServices/ScreenSaverEngine.app";
    if !std::path::Path::new(SCREEN_SAVER_ENGINE).exists() {
        tracing::warn!(
            path = SCREEN_SAVER_ENGINE,
            "lock-on-disconnect: ScreenSaverEngine not found — cannot lock the screen"
        );
        return false;
    }
    match std::process::Command::new("/usr/bin/open")
        .arg(SCREEN_SAVER_ENGINE)
        .status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            tracing::warn!(
                ?status,
                "lock-on-disconnect: open ScreenSaverEngine.app exited non-zero"
            );
            false
        }
        Err(e) => {
            tracing::warn!("lock-on-disconnect: failed to launch ScreenSaverEngine.app: {e}");
            false
        }
    }
}
#[cfg(not(target_os = "macos"))]
pub(super) fn lock_session() -> bool {
    false
}

/// Best-effort check of whether this account's "require password after
/// sleep or screen saver begins" delay is set to Immediately —
/// `--lock-on-disconnect`'s `open ScreenSaverEngine.app` is only a true,
/// password-required lock under that setting; otherwise it just starts the
/// screen saver, so the flag's name would silently overpromise. Returns
/// `None` when the check itself couldn't be run (parse the output loosely
/// rather than fail the whole startup over a diagnostic). macrdp
/// deliberately does not check or change this setting on its own — see
/// docs/known-quirks.md.
#[cfg(target_os = "macos")]
pub(super) fn screen_lock_delay_is_immediate() -> Option<bool> {
    let output = std::process::Command::new("/usr/sbin/sysadminctl")
        .args(["-screenLock", "status"])
        .output()
        .ok()?;
    // sysadminctl logs its answer to stderr via NSLog, e.g. "screenLock
    // delay is immediate" or "screenLock delay is N seconds" — match
    // loosely rather than parse a brittle exact format.
    let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase()
        + &String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if !output.status.success() || text.trim().is_empty() {
        return None;
    }
    Some(text.contains("immediate"))
}
#[cfg(not(target_os = "macos"))]
pub(super) fn screen_lock_delay_is_immediate() -> Option<bool> {
    None
}

/// Bumped on every disconnect edge that arms a `--lock-on-disconnect` lock, so
/// a pending lock from an earlier disconnect stands down in favour of the
/// latest one (which is timed from its own edge).
pub(super) static LOCK_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Total Return-keypress SUBMISSIONS sent while trying to auto-unlock the
/// CURRENT lock cycle — reset to 0 the moment the screen is observed
/// unlocked (a fresh lock cycle starts with a fresh budget) and on a
/// successful unlock. This tracks actual submissions, NOT calls to
/// [`attempt_auto_unlock`]: a single call's internal Return-retry loop
/// draws from this same shared pool as every other call for this lock and
/// stops the instant the pool is empty, rather than each call getting its
/// own private allowance of retries. That distinction is load-bearing — see
/// [`AUTO_UNLOCK_MAX_SUBMISSIONS`].
pub(super) static AUTO_UNLOCK_SUBMISSIONS: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);
/// Whether the "gave up" alert has already fired for the current lock cycle
/// (reset alongside [`AUTO_UNLOCK_SUBMISSIONS`]) — so a client that keeps
/// reconnecting after the budget is spent gets one loud alert, not one per
/// reconnect.
pub(super) static AUTO_UNLOCK_GAVE_UP: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Every Return pressed while the correct password sits in a secure field is
/// potentially scored by macOS's PAM/OpenDirectory throttle as a real
/// authentication attempt — indistinguishable, from the outside, from "the
/// field wasn't focused yet and ignored it" (both leave the screen locked).
/// Capped at 2: a failure here is far more likely a wake/timing mechanical
/// issue than a wrong password (the password is the same one PAM already
/// validated at startup and every connecting RDP client has proven
/// knowledge of via CredSSP), so one retry is worth allowing — but macOS's
/// PAM throttle gives only 3 free attempts before an escalating delay, so 2
/// stays inside that margin rather than risking real lockout escalation
/// from a stuck bug. **This must cap actual submissions, not calls** — an
/// earlier version capped calls to [`attempt_auto_unlock`] while each call
/// internally retried Return up to 3 times, so two calls could submit up to
/// 6 times against a comment promising fewer than 3.
pub(super) const AUTO_UNLOCK_MAX_SUBMISSIONS: u32 = 2;
// Compile-time guarantee that the cap above never regresses past macOS's
// 3-free-attempts PAM/OpenDirectory throttle margin.
const _: () = assert!(AUTO_UNLOCK_MAX_SUBMISSIONS < 3);

/// Try to reserve one submission from a budget of `max`, tracked by
/// `counter`. Returns `true` if the caller may proceed (and the reservation
/// is already recorded), `false` if `max` submissions are already spent.
/// Pure and independent of which atomic is passed in, so it's unit-testable
/// without touching the real shared [`AUTO_UNLOCK_SUBMISSIONS`] counter.
pub(super) fn try_reserve_submission(counter: &std::sync::atomic::AtomicU32, max: u32) -> bool {
    counter
        .fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |n| (n < max).then_some(n + 1),
        )
        .is_ok()
}

/// Outcome of one [`attempt_auto_unlock`] call.
pub(super) enum AutoUnlockOutcome {
    /// The screen wasn't locked (or, on non-macOS, locking doesn't apply
    /// here) — nothing was attempted, and the per-lock budget/alert state
    /// was reset since a lock cycle boundary was just observed.
    NotLocked,
    /// A password was submitted and the screen is now unlocked.
    Unlocked,
    /// Nothing was typed because doing so wasn't safe right now (the lock
    /// state couldn't be determined, Caps Lock is on, the active layout
    /// can't produce every character, or the layout couldn't be read) —
    /// no submission was spent, so the next reconnect simply tries again.
    SkippedUnsafe,
    /// The shared submission budget for this lock is exhausted (by this
    /// call, or an earlier one) — no further Return presses will be sent
    /// until the lock cycle resets (screen observed unlocked) or macrdp
    /// restarts.
    BudgetExhausted,
}

/// Best-effort, loud alert that auto-unlock has given up for this lock —
/// deliberately NOT just a log line, since this is exactly the moment the
/// user needs to know they may have to unlock manually. Layered the way
/// this project already handles unreliable notification delivery for the
/// self-signed build (see `file_promise.rs`'s afplay + best-effort osascript
/// combo): a system alert SOUND is the reliable channel here, a notification
/// banner is attempted best-effort on top of it.
#[cfg(target_os = "macos")]
pub(super) fn alert_auto_unlock_gave_up() {
    let _ = std::process::Command::new("/usr/bin/afplay")
        .arg("/System/Library/Sounds/Basso.aiff")
        .status();
    let _ = std::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "display notification \"auto-unlock stopped after repeated failures — the \
             screen may need to be unlocked manually\" with title \"macrdp\" sound name \
             \"Basso\"",
        ])
        .status();
}
#[cfg(not(target_os = "macos"))]
pub(super) fn alert_auto_unlock_gave_up() {}

/// Attempt to unlock the local session by typing `password` into the
/// screensaver lock screen, if (and only if) it's currently locked
/// ([`virtual_display::screen_is_locked`]). Types it as REAL per-character
/// keycode events (see below) followed by Return. This is the same class of
/// mechanism Apple's own Screen Sharing/VNC and Apple Remote Desktop use to
/// unlock a screensaver-locked Mac remotely — see docs/known-quirks.md for
/// the SecureEventInput research behind why synthetic input is accepted here
/// at all. Returns `true` if nothing needed to be done or the unlock
/// succeeded; `false` if it was locked and is still locked afterward.
///
/// **Three failed live iterations are baked into the current shape — read
/// before changing any of it (full write-up in docs/known-quirks.md):**
/// 1. Leading with a synthetic Return as a "wake gesture" before typing
///    submitted an EMPTY password (field visibly shook) and left the real
///    password landing in a still-settling field. Never reintroduce a
///    keypress before the password.
/// 2. Filling the field with one bulk `set_string_from_utf16_unchecked`
///    event then posting Return did nothing — retried Returns and longer
///    settle delays did not help either, because…
/// 3. …the bulk fill leaves the field INERT: the text is displayed but the
///    field's own text-change bookkeeping never fires, so it ignores every
///    subsequent Return — **including a REAL Return typed on the physical
///    keyboard**, until a character was manually added and deleted. That
///    observation is what ruled out timing entirely and forced the current
///    approach: only genuine per-character keycode events (what a physical
///    keyboard sends, and what the original successful manual test used)
///    make the field accept a submit.
#[cfg(target_os = "macos")]
pub(super) fn attempt_auto_unlock(password: &str) -> AutoUnlockOutcome {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // `CGEventSourceFlagsState` isn't wrapped by the `core-graphics` crate,
    // so declare it directly — same pattern the project already uses for
    // small FFI surfaces (see input.rs's ApplicationServices AX block).
    // `CGEventSourceStateID`/`CGEventFlags` are both `#[repr(C)]`/bitflags
    // types the crate itself passes across this exact boundary (see
    // `CGEventSourceCreate`/`CGEventGetFlags`), so they're safe to reuse
    // here as the parameter/return types.
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceFlagsState(state_id: CGEventSourceStateID) -> CGEventFlags;
    }

    // macOS virtual keycode for Return (matches the constant already named
    // in the keyboard-layout translation notes in known-quirks.md).
    const VK_RETURN: u16 = 0x24;

    match virtual_display::screen_is_locked() {
        Some(true) => {}
        Some(false) => {
            // CONFIRMED not locked — a lock-cycle boundary. Clear the shared
            // submission/alert state so the NEXT lock starts with a full
            // budget rather than inheriting whatever a previous, unrelated
            // lock spent.
            use std::sync::atomic::Ordering as AtomicOrdering;
            AUTO_UNLOCK_SUBMISSIONS.store(0, AtomicOrdering::SeqCst);
            AUTO_UNLOCK_GAVE_UP.store(false, AtomicOrdering::SeqCst);
            return AutoUnlockOutcome::NotLocked;
        }
        None => {
            // The lookup itself failed — genuinely unknown, NOT "not
            // locked." Must not reset the budget here: a flaky lookup
            // resetting it on every failure would defeat the cap
            // AUTO_UNLOCK_MAX_SUBMISSIONS exists to enforce. Skip this
            // attempt (safe direction); the next reconnect tries again.
            warn!(
                "auto-unlock: could not determine whether the screen is \
                 locked — skipping this attempt"
            );
            return AutoUnlockOutcome::SkippedUnsafe;
        }
    }
    // The submission budget is shared across every call for this lock (see
    // AUTO_UNLOCK_SUBMISSIONS's docs) — if an earlier call already spent it,
    // don't even wake/type; there is nothing left to safely submit with.
    if AUTO_UNLOCK_SUBMISSIONS.load(std::sync::atomic::Ordering::SeqCst)
        >= AUTO_UNLOCK_MAX_SUBMISSIONS
    {
        return AutoUnlockOutcome::BudgetExhausted;
    }
    // Caps Lock inverts letter case; `reverse_map` below always resolves
    // assuming Caps is OFF, and whether a posted synthetic CGEvent honors or
    // ignores the Mac's hardware Caps Lock state is unconfirmed (flagged in
    // review; verifying it needs the live rig). Rather than gamble on which
    // way that resolves and possibly submit an inverted-case password, skip
    // the attempt entirely while Caps Lock is on — this check runs before
    // anything is typed, so no submission budget is spent, and the next
    // reconnect tries again for free once Caps Lock is off.
    // SAFETY: CGEventSourceFlagsState only reads the modifier state of a system event source; it
    // takes an enum by value and has no pointer arguments.
    let caps_lock_on = unsafe { CGEventSourceFlagsState(CGEventSourceStateID::HIDSystemState) }
        .contains(CGEventFlags::CGEventFlagAlphaShift);
    if caps_lock_on {
        warn!(
            "auto-unlock: Caps Lock is on — skipping the attempt rather than \
             risking an inverted-case password"
        );
        return AutoUnlockOutcome::SkippedUnsafe;
    }
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) else {
        warn!("auto-unlock: CGEventSource::new failed");
        return AutoUnlockOutcome::SkippedUnsafe;
    };
    // Modifier state has to be posted on both sources — see `post_flags` below.
    let Ok(source_hid) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
        warn!("auto-unlock: CGEventSource::new (HID) failed");
        return AutoUnlockOutcome::SkippedUnsafe;
    };

    // Resolve EVERY character to a real keystroke before typing anything —
    // if any character can't be produced on this Mac's active layout, bail
    // without typing rather than submit a partial/wrong password (which
    // would shake the field and burn one of macOS's 3 free PAM attempts).
    let Some(mut layout) = keyboard_layout::KeyboardLayout::current() else {
        warn!("auto-unlock: could not read the Mac's active keyboard layout — skipping");
        return AutoUnlockOutcome::SkippedUnsafe;
    };
    let map = layout.reverse_map();
    let mut plan: Vec<(u16, bool, bool)> = Vec::with_capacity(password.len());
    for ch in password.chars() {
        let Some(&keystroke) = map.get(&ch) else {
            // Deliberately does NOT log the character — it's password material.
            warn!(
                "auto-unlock: a password character has no keystroke on the Mac's \
                 active keyboard layout — skipping the attempt rather than \
                 submitting an incomplete password"
            );
            return AutoUnlockOutcome::SkippedUnsafe;
        };
        plan.push(keystroke);
    }

    // Wake/focus the field BEFORE typing. LIVE-TESTED 2026-08-28 (round 5):
    // with per-character typing the field finally accepted a submit, but the
    // password arrived one character SHORT (8 of 9) — the first keystroke is
    // consumed waking/focusing the lock screen instead of entering text.
    // Round 1's mistake was using RETURN as that wake (it submitted an empty
    // password); a bare SHIFT tap cannot insert text and cannot submit, so
    // it wakes the field harmlessly. Posted as FlagsChanged on BOTH event
    // sources, the pattern `input.rs::post_flags_changed` established —
    // `[NSEvent modifierFlags]` and the HID-level state are backed by
    // different sources and both need to see it.
    const VK_SHIFT: u16 = 0x38;
    let post_flags = |flags: CGEventFlags| {
        for src in [&source, &source_hid] {
            if let Ok(ev) = CGEvent::new_keyboard_event(src.clone(), VK_SHIFT, true) {
                ev.set_flags(flags);
                ev.set_type(CGEventType::FlagsChanged);
                ev.post(CGEventTapLocation::HID);
            }
        }
    };
    post_flags(CGEventFlags::CGEventFlagShift);
    post_flags(CGEventFlags::empty());
    std::thread::sleep(std::time::Duration::from_millis(250));

    // Clear anything the wake (or a previous rejected attempt) may have left
    // behind, so a stray character can't corrupt the password we're about to
    // type. Backspace on an empty field is a harmless no-op, which is why
    // this is safe to do unconditionally.
    const VK_DELETE: u16 = 0x33;
    for _ in 0..(plan.len() + 4).min(64) {
        if let Ok(down) = CGEvent::new_keyboard_event(source.clone(), VK_DELETE, true) {
            down.post(CGEventTapLocation::HID);
        }
        if let Ok(up) = CGEvent::new_keyboard_event(source.clone(), VK_DELETE, false) {
            up.post(CGEventTapLocation::HID);
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    std::thread::sleep(std::time::Duration::from_millis(150));

    // Type it as real per-character keycode events. LIVE-TESTED 2026-08-28:
    // a single bulk `set_string_from_utf16_unchecked` event fills the field
    // VISIBLY but leaves it inert — even a REAL Return on the physical
    // keyboard was then ignored, until a character was manually added and
    // deleted. So the field's text-change bookkeeping only fires for genuine
    // keystrokes; no amount of settle delay or Return-retry fixes a bulk
    // fill. Modifiers are held across runs via real FlagsChanged events (not
    // just per-event flags) so a shifted character can't come out unshifted
    // — same reasoning as `input.rs::post_flags_changed`.
    let mut shift_held = false;
    let mut option_held = false;
    let flags_for = |shift: bool, option: bool| {
        let mut f = CGEventFlags::empty();
        if shift {
            f |= CGEventFlags::CGEventFlagShift;
        }
        if option {
            f |= CGEventFlags::CGEventFlagAlternate;
        }
        f
    };
    for (vk, shift, option) in &plan {
        let (shift, option) = (*shift, *option);
        if shift != shift_held || option != option_held {
            post_flags(flags_for(shift, option));
            shift_held = shift;
            option_held = option;
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        let flags = flags_for(shift, option);
        if let Ok(down) = CGEvent::new_keyboard_event(source.clone(), *vk, true) {
            down.set_flags(flags);
            down.post(CGEventTapLocation::HID);
        }
        if let Ok(up) = CGEvent::new_keyboard_event(source.clone(), *vk, false) {
            up.set_flags(flags);
            up.post(CGEventTapLocation::HID);
        }
        // Inter-key gap so the field processes each keystroke in order and
        // two identical adjacent characters aren't coalesced into one.
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    if shift_held || option_held {
        post_flags(CGEventFlags::empty());
    }
    std::thread::sleep(std::time::Duration::from_millis(150));

    // LIVE-TESTED 2026-08-28 (round 2): a single Return here is flaky — one
    // connect unlocked cleanly, the next (same code, same settle) left the
    // password visibly typed-but-not-submitted again. So the "field not
    // ready yet" window isn't reliably cleared by any one fixed delay before
    // Return; retry the SUBMIT keypress itself instead of chasing a bigger
    // magic number. Deliberately does NOT retype the password (which is
    // already sitting correctly in the field per the observed symptom) —
    // just re-sends Return a few times with its own settle between
    // attempts, polling `screen_is_locked()` throughout so a successful
    // attempt (whichever one it is) resolves immediately rather than
    // waiting out the rest of the budget. Same backoff-retry shape already
    // used for exactly this class of "helper/field needs more time"
    // flakiness in ShieldedPrimary::install's SHOW_BACKOFF_MS loop.
    //
    // Each Return below is a genuine PAM submission from the OS's point of
    // view — the loop cannot tell "ignored" from "submitted and rejected"
    // apart, both leave the screen locked — so every iteration reserves one
    // unit from the SHARED per-lock budget before pressing anything, and
    // stops the moment that budget (spent by this call or an earlier one)
    // is empty, rather than always spending its own private allowance of
    // retries. That is what keeps the total across every call for this lock
    // under AUTO_UNLOCK_MAX_SUBMISSIONS.
    //
    // LIVE-TESTED 2026-09-25 on a Mac mini over ZeroTier: 400ms was too
    // tight a window for `screen_is_locked()` to observe a real, successful
    // unlock — 2 of 3 attempts spent their SECOND submission on a Return
    // that landed after the first had already unlocked the desktop (a
    // wasted/risky keypress into a live session), then reported a false
    // `BudgetExhausted` because neither poll caught the `Some(false)` flip
    // in time, even though the unlock had genuinely succeeded. Raised to
    // 3s per the live-verified fix suggestion so a slower link/lookup has
    // real room to register before the next Return is spent.
    const RETURN_ATTEMPT_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);
    const UNLOCK_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
    loop {
        if !try_reserve_submission(&AUTO_UNLOCK_SUBMISSIONS, AUTO_UNLOCK_MAX_SUBMISSIONS) {
            tracing::debug!(
                "auto-unlock: submission budget exhausted — stopping without a \
                 further Return"
            );
            return AutoUnlockOutcome::BudgetExhausted;
        }
        if let Ok(down) = CGEvent::new_keyboard_event(source.clone(), VK_RETURN, true) {
            down.post(CGEventTapLocation::HID);
        }
        if let Ok(up) = CGEvent::new_keyboard_event(source.clone(), VK_RETURN, false) {
            up.post(CGEventTapLocation::HID);
        }
        let deadline = std::time::Instant::now() + RETURN_ATTEMPT_BUDGET;
        loop {
            // Only a CONFIRMED `Some(false)` counts as success. `None`
            // (lookup failed) must NOT be read as "unlocked" — that would
            // both falsely report success and reset the submission budget,
            // silently defeating the cap on a flaky lookup. Treat it the
            // same as "still locked": keep polling within this Return's
            // budget, then fall through to retry (which still costs a
            // real reserved submission, so this can't loop forever).
            if virtual_display::screen_is_locked() == Some(false) {
                use std::sync::atomic::Ordering as AtomicOrdering;
                AUTO_UNLOCK_SUBMISSIONS.store(0, AtomicOrdering::SeqCst);
                AUTO_UNLOCK_GAVE_UP.store(false, AtomicOrdering::SeqCst);
                return AutoUnlockOutcome::Unlocked;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(UNLOCK_POLL_INTERVAL);
        }
        tracing::debug!("auto-unlock: Return not acknowledged yet — retrying");
    }
}
#[cfg(not(target_os = "macos"))]
pub(super) fn attempt_auto_unlock(_password: &str) -> AutoUnlockOutcome {
    AutoUnlockOutcome::NotLocked
}

/// Exit code used when a stuck `--detach-primary` disconnect bounces the process
/// so launchd restarts it (#168). **Deliberately non-zero** (`EX_UNAVAILABLE`,
/// distinct from the health watchdog's `70`): with the shipped LaunchAgent
/// (`KeepAlive = true`, unconditional) any exit restarts, but a non-zero code
/// also restarts under a `KeepAlive = { SuccessfulExit: false }` plist — so the
/// fix doesn't silently depend on KeepAlive being unconditional — and it makes
/// the intentional bounce distinguishable from a clean shutdown in telemetry.
pub(super) const DETACH_STUCK_EXIT_CODE: i32 = 69;

/// Whether a `--detach-primary` disconnect that couldn't re-enable the physical
/// panel should `process::exit` so launchd restarts a fresh process (which is
/// the only thing that reverts the CGS disable on macOS 26.x — see #168).
///
/// Mirrors [`health::should_arm`]: an explicit `MACRDP_DETACH_RESTART_ON_STUCK=
/// 1/0` wins; otherwise **on when headless** (stdout not a TTY ⇒ under launchd,
/// which will restart the bounce) and **off** interactively (a `cargo run`
/// session has nothing to restart it, so self-exiting would just kill the dev
/// server and leave the panel stuck anyway — strictly worse). Pure + unit-tested.
pub(super) fn detach_restart_on_stuck(stdout_is_tty: bool, env_override: Option<&str>) -> bool {
    match env_override
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("1") | Some("on") | Some("true") | Some("yes") => true,
        Some("0") | Some("off") | Some("false") | Some("no") => false,
        _ => !stdout_is_tty,
    }
}

#[cfg(test)]
mod detach_restart_tests {
    use super::detach_restart_on_stuck;

    #[test]
    fn explicit_override_wins_over_tty() {
        // Forced on even interactively (for testing on a TTY).
        assert!(detach_restart_on_stuck(true, Some("1")));
        assert!(detach_restart_on_stuck(true, Some("on")));
        assert!(detach_restart_on_stuck(true, Some("TRUE")));
        // Forced off even headless.
        assert!(!detach_restart_on_stuck(false, Some("0")));
        assert!(!detach_restart_on_stuck(false, Some("off")));
        assert!(!detach_restart_on_stuck(false, Some(" no ")));
    }

    #[test]
    fn defaults_on_when_headless_off_interactively() {
        // Headless (not a TTY, i.e. under launchd) ⇒ default on.
        assert!(detach_restart_on_stuck(false, None));
        // Interactive (a TTY, e.g. `cargo run`) ⇒ default off (nothing to
        // restart it, so self-exit would just kill the dev server).
        assert!(!detach_restart_on_stuck(true, None));
        // An unrecognized value falls back to the headless default.
        assert!(detach_restart_on_stuck(false, Some("maybe")));
        assert!(!detach_restart_on_stuck(true, Some("maybe")));
    }
}
#[cfg(test)]
mod lock_on_disconnect_tests {
    use super::{parse_lock_on_disconnect_delay_ms, LOCK_ON_DISCONNECT_DELAY_DEFAULT_MS};

    #[test]
    fn defaults_when_unset_or_garbage() {
        assert_eq!(
            parse_lock_on_disconnect_delay_ms(None),
            LOCK_ON_DISCONNECT_DELAY_DEFAULT_MS
        );
        assert_eq!(
            parse_lock_on_disconnect_delay_ms(Some("not-a-number")),
            LOCK_ON_DISCONNECT_DELAY_DEFAULT_MS
        );
        assert_eq!(
            parse_lock_on_disconnect_delay_ms(Some("")),
            LOCK_ON_DISCONNECT_DELAY_DEFAULT_MS
        );
    }

    #[test]
    fn honors_a_valid_override() {
        assert_eq!(parse_lock_on_disconnect_delay_ms(Some("5000")), 5000);
        // Surrounding whitespace tolerated (matches env_u64 elsewhere).
        assert_eq!(parse_lock_on_disconnect_delay_ms(Some(" 100 ")), 100);
        assert_eq!(parse_lock_on_disconnect_delay_ms(Some("0")), 0);
    }

    #[test]
    fn lock_on_disconnect_alone_parses_without_a_headless_mode() {
        // The flag is accepted by clap on its own; the no-headless-mode case
        // is a runtime warn!, not a parse error (mirrors
        // --restore-windows-on-disconnect's validation).
        use clap::Parser;
        let args = super::Args::try_parse_from(["macrdp", "--lock-on-disconnect"]);
        assert!(args.is_ok());
        assert!(args.unwrap().lock_on_disconnect);
    }
}
#[cfg(test)]
mod auto_unlock_flag_tests {
    #[test]
    fn negotiated_defaults_turn_features_on_and_respect_pins() {
        use crate::negotiator::session::HostCaps;
        use clap::Parser as _;
        let mut a = super::Args::try_parse_from(["macrdp"]).unwrap();
        let r = super::apply_negotiated_defaults(
            &mut a,
            &HostCaps {
                physical_displays: 0,
                virtual_display_available: true,
            },
            true,
        );
        assert!(
            a.enable_h264
                && a.adaptive_bitrate
                && !a.enable_udp_multitransport
                && a.virtual_display
        );
        assert!(!a.shield_primary);
        assert!(!r.is_empty());

        let mut a = super::Args::try_parse_from(["macrdp"]).unwrap();
        super::apply_negotiated_defaults(
            &mut a,
            &HostCaps {
                physical_displays: 1,
                virtual_display_available: true,
            },
            true,
        );
        assert!(a.shield_primary);

        let mut a =
            super::Args::try_parse_from(["macrdp", "--capture-primary", "--virtual-display"])
                .unwrap();
        super::apply_negotiated_defaults(
            &mut a,
            &HostCaps {
                physical_displays: 1,
                virtual_display_available: true,
            },
            true,
        );
        assert!(!a.shield_primary, "an explicit headless mode is kept");

        let mut a =
            super::Args::try_parse_from(["macrdp", "--width", "1920", "--height", "1080"]).unwrap();
        super::apply_negotiated_defaults(
            &mut a,
            &HostCaps {
                physical_displays: 0,
                virtual_display_available: true,
            },
            true,
        );
        assert!(!a.virtual_display, "a pinned size keeps mirror capture");

        let mut a = super::Args::try_parse_from(["macrdp"]).unwrap();
        super::apply_negotiated_defaults(
            &mut a,
            &HostCaps {
                physical_displays: 1,
                virtual_display_available: false,
            },
            true,
        );
        assert!(!a.virtual_display && !a.shield_primary);
    }

    #[test]
    fn parses_as_a_plain_opt_in_flag() {
        use clap::Parser;
        let args = super::Args::try_parse_from(["macrdp", "--auto-unlock"]);
        assert!(args.is_ok());
        assert!(args.unwrap().auto_unlock);

        let args = super::Args::try_parse_from(["macrdp"]);
        assert!(args.is_ok());
        assert!(!args.unwrap().auto_unlock);
    }
}
#[cfg(test)]
mod auto_unlock_submission_budget_tests {
    use super::try_reserve_submission;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn reserves_up_to_the_max_then_refuses() {
        let counter = AtomicU32::new(0);
        assert!(try_reserve_submission(&counter, 2));
        assert!(try_reserve_submission(&counter, 2));
        // The budget of 2 is now spent — a THIRD reservation must be
        // refused regardless of whether it comes from the same call's
        // internal Return-retry loop or a brand-new call for the same lock.
        // This is exactly the bug the fix guards against: capping CALLS
        // rather than actual submissions let a single call's internal
        // retries alone exceed the documented budget.
        assert!(!try_reserve_submission(&counter, 2));
        assert!(!try_reserve_submission(&counter, 2));
    }

    #[test]
    fn zero_budget_never_reserves() {
        let counter = AtomicU32::new(0);
        assert!(!try_reserve_submission(&counter, 0));
    }
}
