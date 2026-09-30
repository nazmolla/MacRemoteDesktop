//! The session watcher that engages and releases the headless-display modes
//! (detach, capture or shield the physical panel) as clients come and go.

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_primary_overlay_watcher<T: Send + 'static>(
    label: &'static str,
    vd_id: u32,
    tracker: capture::SessionTracker,
    slot: Arc<std::sync::Mutex<Option<T>>>,
    install: fn(u32) -> Result<T>,
    // (--restore-windows-on-disconnect) When true, make windows follow the
    // session: sweep them onto `physical_main_id` on real disconnect (so the
    // Mac is usable locally) and auto-gather them onto the virtual display on
    // reconnect (so the client sees them without Ctrl+Alt+G). No-op otherwise.
    restore_windows: bool,
    physical_main_id: u32,
    // (--lock-on-disconnect) When set, lock the local session on a genuine
    // last-client-disconnect (after an extra safety buffer beyond the
    // REACTIVATION_GRACE poll above), holding it while a reconnect is still
    // handshaking (see lock_activity). None = feature off.
    lock_on_disconnect: Option<Arc<lock_activity::ConnectionActivity>>,
    // (--auto-unlock) When true, try to unlock the local session on
    // reconnect using the exact same validated credential used for RDP
    // auth. A no-op if the screen isn't locked, or if the credential monitor
    // has revoked the password (the cell is then empty).
    password: credential_monitor::SecretCell,
    auto_unlock: bool,
) {
    tokio::spawn(async move {
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};
        // Minimum dwell time on the engaged state before honoring a
        // re-attach. The WindowServer commits configure transactions
        // asynchronously — a disable→enable cycle that races the
        // first commit's propagation through SkyLight can be rejected
        // with CGError 1001 by the second tx. Conservative for capture
        // too where the API is synchronous; unifies the loop shape.
        const ENGAGED_SETTLE: Duration = Duration::from_secs(3);
        // mstsc's cert-trust handshake does a 0→1→0→1 flap within a
        // few seconds. Waiting briefly before acting on a 0→≥1
        // notification lets that bounce collapse to a no-op so we
        // don't kick off a 10-second blocking install for a session
        // that's already gone.
        const CONNECT_DEBOUNCE: Duration = Duration::from_millis(750);
        // Disconnect-side flap absorber. A core deactivation–reactivation (a live
        // resize on maximize, or blank recovery) briefly drops the session count
        // to 0 and right back to 1 as the vendored server drops the old
        // `CountedUpdates` and builds a new one — a flap, not a real disconnect.
        // Without absorbing it, the headless `CapturedPrimary` drops (restore
        // gamma) and re-engages (re-blank) on every resize — a visible flicker,
        // and the session re-cycle restarts audio. The gap is VARIABLE (observed
        // ~0.5–0.9 s) because it includes the virtual-display re-mode, which
        // blocks a variable amount — so a single fixed sleep can't reliably cover
        // it. Instead POLL for the session to come back, up to REACTIVATION_GRACE,
        // and skip the teardown as soon as it does; only a count that STAYS 0 for
        // the whole window is a real disconnect (then teardown is delayed by at
        // most the grace — a couple seconds of extra gamma-blank, harmless).
        const REACTIVATION_GRACE: Duration = Duration::from_millis(2500);
        const REACTIVATION_POLL: Duration = Duration::from_millis(75);
        let mut was_zero = true;
        let mut installed_at: Option<Instant> = None;
        loop {
            tracker.notify.notified().await;
            let count = tracker.count.load(Ordering::SeqCst);
            let now_zero = count == 0;
            info!(label, was_zero, now_zero, count, "overlay watcher woke");
            match (was_zero, now_zero) {
                (true, false) => {
                    tokio::time::sleep(CONNECT_DEBOUNCE).await;
                    if tracker.count.load(Ordering::SeqCst) == 0 {
                        info!(
                            label,
                            "connection flapped during debounce — skipping install"
                        );
                        was_zero = true;
                        continue;
                    }
                    match install(vd_id) {
                        Ok(ovr) => {
                            installed_at = Some(Instant::now());
                            info!(
                                label,
                                "RDP client connected — Mac is now headless via the \
                                 virtual display"
                            );
                            *slot.lock_or_recover() = Some(ovr);
                            // (--restore-windows-on-disconnect) Auto-gather any
                            // windows stranded on the built-in panel (e.g. swept
                            // there by a previous disconnect) onto the virtual
                            // display the client now sees — the reconnect half of
                            // "follow me". Off-thread; the install's reposition has
                            // already settled, so the vd's live bounds are correct.
                            if restore_windows {
                                std::thread::spawn(move || {
                                    let moved = restore_gather_windows(vd_id);
                                    if moved > 0 {
                                        info!(
                                            label,
                                            moved,
                                            display_id = vd_id,
                                            "restore-windows: gathered windows onto the \
                                             virtual display on connect"
                                        );
                                    }
                                });
                            }
                            // Auto-unlock: if the screen happens to be locked
                            // (for any reason — a lock-on-disconnect lock, one
                            // set manually, or macOS's own idle policy), try to
                            // unlock it now that a client is connected. No-op
                            // instantly if it isn't locked. Off-thread so the
                            // watcher loop isn't blocked by the settle delays
                            // inside attempt_auto_unlock.
                            if auto_unlock {
                                let password = Arc::clone(&password);
                                std::thread::spawn(move || {
                                    use std::sync::atomic::Ordering as AtomicOrdering;
                                    let Some(password) = password.read_or_recover().clone() else {
                                        warn!(
                                            label,
                                            "auto-unlock: skipped, the password was revoked"
                                        );
                                        return;
                                    };
                                    match attempt_auto_unlock(password.as_str()) {
                                        AutoUnlockOutcome::NotLocked
                                        | AutoUnlockOutcome::SkippedUnsafe => {}
                                        AutoUnlockOutcome::Unlocked => {
                                            info!(label, "auto-unlock: succeeded");
                                        }
                                        AutoUnlockOutcome::BudgetExhausted => {
                                            // Fire the loud alert exactly once per lock
                                            // cycle — a client that keeps reconnecting
                                            // while still locked and out of budget would
                                            // otherwise re-trigger it on every attempt.
                                            if !AUTO_UNLOCK_GAVE_UP
                                                .swap(true, AtomicOrdering::SeqCst)
                                            {
                                                error!(
                                                    label,
                                                    max = AUTO_UNLOCK_MAX_SUBMISSIONS,
                                                    "auto-unlock: exhausted its submission \
                                                     budget for this lock without \
                                                     unlocking the screen — giving up \
                                                     until it's unlocked another way (or \
                                                     macrdp restarts), to avoid tripping \
                                                     macOS's password-retry lockout"
                                                );
                                                alert_auto_unlock_gave_up();
                                            } else {
                                                warn!(
                                                    label,
                                                    "auto-unlock: still locked and out of \
                                                     submission budget for this lock"
                                                );
                                            }
                                        }
                                    }
                                });
                            }
                        }
                        Err(e) => warn!(
                            label,
                            "could not engage headless overlay on client connect: {e:#}"
                        ),
                    }
                }
                (false, true) => {
                    // Absorb a transient count→0: a core deactivation–reactivation
                    // (a live resize on maximize, or blank recovery) flaps 1→0→1
                    // as the vendored server rebuilds `CountedUpdates`. Poll for
                    // the session to come back (up to REACTIVATION_GRACE); if it
                    // does, keep the headless overlay engaged so it doesn't drop +
                    // re-capture (the visible flicker) and doesn't restart audio.
                    // Only a count that STAYS 0 for the whole window is a real
                    // disconnect. `was_zero` is left false on the skip (overlay
                    // still engaged), so the paired count→1 `enter` notification
                    // lands as a no-op (false, false).
                    let deadline = Instant::now() + REACTIVATION_GRACE;
                    let mut came_back = false;
                    while Instant::now() < deadline {
                        tokio::time::sleep(REACTIVATION_POLL).await;
                        if tracker.count.load(Ordering::SeqCst) > 0 {
                            came_back = true;
                            break;
                        }
                    }
                    if came_back {
                        info!(
                            label,
                            "session flapped during grace (reactivation) — keeping \
                             headless overlay engaged"
                        );
                        continue;
                    }
                    if let Some(installed) = installed_at {
                        let elapsed = installed.elapsed();
                        if elapsed < ENGAGED_SETTLE {
                            let wait = ENGAGED_SETTLE - elapsed;
                            info!(
                                label,
                                ?wait,
                                "waiting for WindowServer to settle before disengaging"
                            );
                            tokio::time::sleep(wait).await;
                        }
                    }
                    if let Some(ovr) = slot.lock_or_recover().take() {
                        // Drop runs the actual restore + emits its own
                        // success/warn logs; don't claim a result here.
                        drop(ovr);
                        installed_at = None;
                        info!(label, "last RDP client disconnected");

                        // #168: on --detach-primary, macOS 26.x can't re-enable
                        // the physical panel in-process — only process exit
                        // reverts the CGS disable. If Drop just flagged that
                        // failure, restart (under launchd) so the panel comes
                        // back, rather than leaving it dark until the next manual
                        // kickstart. Only DetachedPrimary::drop sets the flag, so
                        // this is a guaranteed no-op for --capture-primary (and
                        // off macOS, where the stub returns false). The take_*
                        // read clears the flag.
                        if virtual_display::take_detach_reenable_failed() {
                            if detach_restart_on_stuck(
                                std::io::stdout().is_terminal(),
                                crate::tunables::var("MACRDP_DETACH_RESTART_ON_STUCK")
                                    .ok()
                                    .as_deref(),
                            ) {
                                warn!(
                                    label,
                                    "detach could not re-enable the built-in display \
                                     (macOS won't do it in-process, #168) — exiting so \
                                     launchd restarts a fresh process to restore it. \
                                     Set MACRDP_DETACH_RESTART_ON_STUCK=0 to disable."
                                );
                                // Mirror the signal handler's cleanup before a
                                // process::exit (which bypasses Drop): flush lazy-
                                // paste state + unmount RDPDR NFS volumes. On this
                                // disconnect edge the per-connection Drops have
                                // usually already run, so these are near-no-ops —
                                // but they keep the bounce as clean as the signal
                                // path, and the startup reaper backstops anything
                                // that slips through on the relaunch.
                                #[cfg(target_os = "macos")]
                                file_promise_lazy::shutdown_cleanup();
                                rdpdr::shutdown_cleanup();
                                std::process::exit(DETACH_STUCK_EXIT_CODE);
                            } else {
                                warn!(
                                    label,
                                    "detach could not re-enable the built-in display \
                                     (#168) and restart-on-stuck is off — the panel \
                                     stays dark until macrdp restarts (launchctl \
                                     kickstart -k, or quit + relaunch)."
                                );
                            }
                        }
                        // (--restore-windows-on-disconnect) Sweep windows off the
                        // virtual display back onto the built-in panel so the Mac
                        // is usable locally. MUST run AFTER `drop(ovr)` — that's
                        // what restores the physical panel to main and repositions
                        // the vd off (0,0); the gather reads each display's live
                        // bounds. Off-thread so the watcher loop isn't blocked.
                        if restore_windows {
                            std::thread::spawn(move || {
                                let moved = restore_gather_windows(physical_main_id);
                                if moved > 0 {
                                    info!(
                                        label,
                                        moved,
                                        display_id = physical_main_id,
                                        "restore-windows: swept windows back onto the \
                                         built-in display on disconnect"
                                    );
                                }
                            });
                        }
                        // (--lock-on-disconnect) Lock the local session after an
                        // EXTRA safety buffer on top of the REACTIVATION_GRACE poll
                        // above. That poll only absorbs a fast in-place flap (a core
                        // reactivation); it can't distinguish a genuine disconnect
                        // from blank-recovery's slower fallback (a full connection
                        // drop + ARC auto-reconnect, documented up to ~12-15s worst
                        // case) — so this waits longer still and re-checks
                        // `tracker.count` before actually locking, canceling if the
                        // session came back. Off-thread so the watcher loop isn't
                        // blocked; naturally abandoned (never fires) if the process
                        // exits in the meantime (e.g. the #168 restart-on-stuck
                        // path) — lock-on-disconnect only ever fires on a genuine
                        // watcher-observed last-client-disconnect, never on server
                        // shutdown/kill. Heuristic, not a guarantee — see
                        // docs/known-quirks.md.
                        if let Some(activity) = lock_on_disconnect.clone() {
                            let tracker_for_lock = tracker.clone();
                            // A later disconnect supersedes this one's pending
                            // lock (it arms its own, timed from ITS edge).
                            let generation = LOCK_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
                            let disconnect_ms = activity.now_ms();
                            std::thread::spawn(move || {
                                let delay = Duration::from_millis(lock_on_disconnect_delay_ms());
                                std::thread::sleep(delay);
                                // A client reconnecting late in the buffer only
                                // counts as live once FULLY connected (~10 s over
                                // ZeroTier), so hold the lock while a reconnect
                                // is still handshaking — capped, so a peer that
                                // merely opens connections can only delay it.
                                let due_ms = activity.now_ms();
                                let mut holding = false;
                                loop {
                                    if LOCK_GENERATION.load(Ordering::SeqCst) != generation {
                                        tracing::debug!(
                                            label,
                                            "lock-on-disconnect: superseded by a later disconnect"
                                        );
                                        return;
                                    }
                                    let live = tracker_for_lock.count.load(Ordering::SeqCst) > 0;
                                    match activity.decide(live, disconnect_ms, due_ms) {
                                        lock_activity::LockDecision::Skip => {
                                            info!(
                                                label,
                                                "lock-on-disconnect: session came back during \
                                                 the safety buffer — skipping lock (treating \
                                                 as a delayed reconnect/self-heal)"
                                            );
                                            return;
                                        }
                                        lock_activity::LockDecision::Hold => {
                                            if !holding {
                                                info!(
                                                    label,
                                                    "lock-on-disconnect: a client is reconnecting — \
                                                     holding the lock until it connects (capped)"
                                                );
                                                holding = true;
                                            }
                                            std::thread::sleep(Duration::from_millis(250));
                                        }
                                        lock_activity::LockDecision::Lock => break,
                                    }
                                }
                                // Reset the auto-unlock submission budget/alert
                                // latch HERE, not just on an observed unlock —
                                // this is a point where the screen is USUALLY
                                // unlocked (we're about to lock it), and it's
                                // the fix for a real live-reproduced lockout:
                                // if a prior auto-unlock attempt actually
                                // succeeded but a too-tight detection window
                                // (see RETURN_ATTEMPT_BUDGET above) missed the
                                // `Some(false)` flip and reported a false
                                // BudgetExhausted, nothing would otherwise ever
                                // reset the shared budget — the NEXT lock
                                // cycle would start pre-exhausted and refuse to
                                // even attempt an unlock, indefinitely, on a
                                // Mac nobody is physically at to recover.
                                //
                                // GATED on a CONFIRMED `Some(false)` read —
                                // caught in review: an unconditional reset
                                // assumed the screen is unlocked here without
                                // checking, which breaks if it's actually
                                // already locked (e.g. the account password
                                // changed while macrdp kept running: RDP still
                                // accepts the cached old password via NLA, but
                                // auto-unlock keeps typing that same stale
                                // password and genuinely fails every time).
                                // Without the gate, every disconnect/reconnect
                                // cycle would hand that genuinely-failing
                                // attempt a fresh budget and burn 2 more real
                                // PAM submissions against a lock that will
                                // never clear — exactly the macOS
                                // escalating-lockout risk the cap exists to
                                // prevent. `Some(true)` (still locked) or
                                // `None` (unknown) leaves the budget/latch
                                // alone, so a persistently-wrong password
                                // stays given-up rather than retrying forever.
                                if virtual_display::screen_is_locked() == Some(false) {
                                    use std::sync::atomic::Ordering as AtomicOrdering;
                                    AUTO_UNLOCK_SUBMISSIONS.store(0, AtomicOrdering::SeqCst);
                                    AUTO_UNLOCK_GAVE_UP.store(false, AtomicOrdering::SeqCst);
                                }
                                info!(label, "lock-on-disconnect: locking the local session");
                                if !lock_session() {
                                    warn!(
                                        label,
                                        "lock-on-disconnect: failed to lock the \
                                         session (see warning above)"
                                    );
                                }
                            });
                        }
                    }
                }
                _ => {}
            }
            was_zero = now_zero;
        }
    });
}
