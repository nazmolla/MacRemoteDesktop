//! The Cmd+Tab / Option+Tab app switcher, reimplemented in user space
//! (WindowServer's own switcher only reacts to kernel HID events).

use super::*;

/// Hard upper bound on cycle-session lifetime. Generation matching
/// is the primary "is the session still alive" signal — this
/// timeout is only a safety net for the rare case where the
/// Cmd-release event was eaten before reaching us (RDP client
/// focus loss taking the keyup with it). 5 min comfortably
/// accommodates "user paused to read something" cases while still
/// eventually recovering from a stuck session.
pub(super) const CYCLE_RESUME_GRACE: Duration = Duration::from_secs(300);

/// Commit the active cycle session — promote cursor's bundle to
/// MRU front and clear the session. Called when Cmd is fully
/// released, and lazily from cycle_apps when the grace timeout
/// has elapsed.
pub(super) fn commit_cycle_session() {
    // Take the session out under the CYCLE_SESSION lock, then DROP the lock
    // (it's confined to this block) before any HUD / AX / mru /
    // LAST_FOCUS_BUNDLE work below — those acquire OTHER mutexes, and holding
    // CYCLE_SESSION across them would nest locks (the same lock-order hazard
    // removed from h264 `ship_frames`: LAST_FOCUS_BUNDLE is written cross-
    // thread by the focus observer / click hit-test). Nothing below needs the
    // session map locked, only the taken session's data. The session is
    // already `take()`n, so a racing cycle_apps just starts a fresh one — the
    // correct outcome after a commit.
    let session = {
        let mut guard = CYCLE_SESSION.lock_or_recover();
        match guard.take() {
            Some(s) => s,
            None => return,
        }
    };
    // A session existed → tear down the on-screen HUD (best-effort).
    if super::super::APP_SWITCHER_HUD.load(std::sync::atomic::Ordering::Relaxed) {
        crate::switcher_hud::hide();
    }
    if let Some((bundle, _, _)) = session
        .snapshot
        .iter()
        .find(|(_, _, p)| *p == session.cursor_pid)
    {
        promote_bundle_to_front(bundle);
        // Record the landed app as the current focus for the Ctrl→Cmd
        // remap. Its `frontmost_is_excluded` reads LAST_FOCUS_BUNDLE
        // first, and our own Cmd+Tab/Option+Tab AX activation doesn't
        // reliably post an NSWorkspace activation — so without this,
        // cycling from an excluded app (terminal/VSCode) to a normal one
        // left the remap suppressed until the user clicked the new app.
        if let Ok(mut g) = LAST_FOCUS_BUNDLE.lock() {
            *g = Some(bundle.clone());
        }
    }
    let cursor_pid = session.cursor_pid;
    // On commit (Cmd release), re-assert the landed app and, if it has no
    // window (e.g. Notes/Calendar/Mail with their window closed), reopen it so
    // it surfaces — both gated to commit so apps merely cycled *through* aren't
    // un-minimized or popped open. Un-minimize only when --unminimize-on-switch.
    ax_make_frontmost(
        cursor_pid,
        super::super::UNMINIMIZE_ON_SWITCH.load(std::sync::atomic::Ordering::Relaxed),
        true,
    );
}

/// Cycle to the next (or previous) regular-policy running app and
/// activate it. Replaces Cmd+Tab / Cmd+Shift+Tab, which WindowServer
/// won't fire for CGEvent-posted keystrokes.
pub(super) fn cycle_apps(reverse: bool) {
    // SAFETY: NSWorkspace / NSRunningApplication are documented thread-
    // safe for these read-only queries.
    unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        // `NSWorkspace.frontmostApplication` is the live-queryable
        // replacement for `isActive` checks on cached snapshots: it
        // walks the active app fresh each call. We compare by PID
        // since NSRunningApplication instances don't share identity
        // across queries.
        let workspace_front_pid = workspace
            .frontmostApplication()
            .map(|a| a.processIdentifier())
            .unwrap_or(0);
        // Pass 1: gather every running app's metadata.
        //
        // We deliberately AVOID `NSWorkspace.runningApplications()`
        // here: its result is a cache refreshed via Cocoa
        // notifications that require a main-thread runloop, and
        // tokio owns our main thread. Apps launched after macrdp
        // starts never appear in that cache. Instead we read the
        // kernel proc table directly via `proc_listallpids` and
        // ask NSRunningApplication for each PID — that path does
        // a fresh per-call lookup and sees newly-launched apps
        // immediately.
        struct AppMeta {
            bundle: String,
            name: String,
            pid: libc::pid_t,
            policy: NSApplicationActivationPolicy,
            terminated: bool,
        }
        let pids = list_all_pids();
        let mut metas: Vec<AppMeta> = Vec::with_capacity(pids.len());
        for pid in pids {
            let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            else {
                continue;
            };
            // `isTerminated` lags the kernel by a few hundred ms
            // when an app quits — long enough for the first Cmd+Tab
            // after a close to still see the dead instance in the
            // candidate list (and, via MRU, potentially target it).
            // `kill(pid, 0)` is authoritative: returns 0 if the
            // process is alive, -1/ESRCH if it isn't. EPERM means
            // it exists but is not ours to signal — still alive.
            let kernel_dead =
                pid <= 0 || (libc::kill(pid, 0) == -1 && *libc::__error() == libc::ESRCH);
            metas.push(AppMeta {
                bundle: app
                    .bundleIdentifier()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "<no-bundle-id>".to_string()),
                name: app
                    .localizedName()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                pid,
                policy: app.activationPolicy(),
                terminated: app.isTerminated() || kernel_dead,
            });
        }
        let all_count = metas.len();

        // Reconcile workspace's notion of "front" against what we
        // last AX-activated. Workspace lies (returns the pre-AX
        // app) after we've activated something, so trusting it
        // blindly would re-promote the stale value to MRU front on
        // every session start and undo the previous commit. Using
        // the reconciled `front_pid` from here on means MRU
        // tracking actually survives a Cmd+Tab → release → Cmd+Tab
        // cycle.
        let front_pid = effective_front_pid(workspace_front_pid, |p| {
            metas.iter().any(|m| m.pid == p && !m.terminated)
        });

        // Refresh MRU. Whichever bundle is currently frontmost is the
        // one the user just left (or was working in if this is their
        // first Cmd+Tab) — that's the instance Cmd+Tab back should
        // land on later.
        {
            let mru = mru_map();
            let mut guard = mru.lock_or_recover();
            if let Some(m) = metas.iter().find(|m| m.pid == front_pid) {
                if m.bundle != "<no-bundle-id>" {
                    guard.insert(m.bundle.clone(), m.pid);
                }
            }
            // Drop stale entries pointing at quit / terminated PIDs.
            guard.retain(|_, pid| metas.iter().any(|m| m.pid == *pid && !m.terminated));
        }
        let mru_snapshot: std::collections::HashMap<String, libc::pid_t> = {
            let mru = mru_map();
            mru.lock_or_recover().clone()
        };

        // Pass 2: dedup by bundle, preferring the MRU instance. Apps
        // without a bundle ID pass through individually (rare for
        // regular-policy apps).
        let mut regular: Vec<(String, String, libc::pid_t)> = Vec::new();
        let mut by_bundle: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut dup_pids: HashSet<libc::pid_t> = HashSet::new();
        for m in &metas {
            if m.policy != NSApplicationActivationPolicy::Regular || m.terminated {
                continue;
            }
            if m.bundle == "<no-bundle-id>" {
                regular.push((m.bundle.clone(), m.name.clone(), m.pid));
                continue;
            }
            match by_bundle.get(&m.bundle).copied() {
                None => {
                    by_bundle.insert(m.bundle.clone(), regular.len());
                    regular.push((m.bundle.clone(), m.name.clone(), m.pid));
                }
                Some(existing_idx) => {
                    // Two instances of the same bundle. Replace the
                    // kept entry only if the new candidate is the
                    // bundle's MRU pid; otherwise keep what we have.
                    if mru_snapshot.get(&m.bundle) == Some(&m.pid) {
                        dup_pids.insert(regular[existing_idx].2);
                        regular[existing_idx] = (m.bundle.clone(), m.name.clone(), m.pid);
                    } else {
                        dup_pids.insert(m.pid);
                    }
                }
            }
        }

        // Build the human-readable dump after dedup so DUP tags reflect
        // actual decisions.
        let all_summary: Vec<String> = metas
            .iter()
            .map(|m| {
                let mut tags = String::new();
                if m.pid == front_pid {
                    tags.push_str(", FRONT");
                }
                if m.terminated {
                    tags.push_str(", TERMINATED");
                }
                if dup_pids.contains(&m.pid) {
                    tags.push_str(", DUP");
                }
                if mru_snapshot.get(&m.bundle) == Some(&m.pid) {
                    tags.push_str(", MRU");
                }
                format!(
                    "{bundle} (name={name:?}, policy={policy:?}, pid={pid}{tags})",
                    bundle = m.bundle,
                    name = m.name,
                    policy = m.policy.0,
                    pid = m.pid,
                )
            })
            .collect();
        debug!(
            count = all_count,
            regular_count = regular.len(),
            front_pid,
            workspace_front_pid,
            "cycle_apps: full app list:\n  {}",
            all_summary.join("\n  ")
        );
        if regular.is_empty() {
            warn!("cycle_apps: no regular apps running");
            return;
        }

        // Decide: continue an existing held-Cmd session, or start
        // a fresh one. A fresh session rebuilds the snapshot in
        // MRU order (most recently used first) so press #1 lands
        // on the previous-MRU app, press #2 on the one before
        // that, etc. — matching native macOS Cmd+Tab. A continued
        // session reuses the frozen snapshot so successive
        // presses walk further back without the list reshuffling
        // under us (each AX activation would otherwise promote
        // the new target and ping-pong the cursor).
        let now = Instant::now();
        let current_release_gen = CMD_RELEASE_GENERATION.load(std::sync::atomic::Ordering::SeqCst);
        let mut session_guard = CYCLE_SESSION.lock_or_recover();
        // Continue if the session's stored release-generation
        // matches *and* the session is younger than the safety
        // bound. Generation matching is the real authority —
        // it's true iff Cmd has been held continuously since
        // session start. The grace check only kicks in to
        // recover from a missed Cmd-release event (RDP focus
        // loss eating the keyup).
        let continuing = session_guard.as_ref().is_some_and(|s| {
            s.cmd_release_gen == current_release_gen
                && now.duration_since(s.last_press_at) < CYCLE_RESUME_GRACE
        });
        if !continuing && session_guard.is_some() {
            // Either Cmd was released between presses (generation
            // bumped) or the safety bound elapsed. Treat
            // any pending cursor as the user's last actual
            // selection and commit it to MRU before starting over.
            if let Some(stale) = session_guard.take() {
                if let Some((bundle, _, _)) = stale
                    .snapshot
                    .iter()
                    .find(|(_, _, p)| *p == stale.cursor_pid)
                {
                    promote_bundle_to_front(bundle);
                }
            }
        }

        // Build (or reuse) the snapshot. The session origin (the
        // front-at-start app) IS included so the cycle has the
        // full set of regular apps. Native macOS Cmd+Tab cycles
        // through all apps including back to the starting one —
        // mirroring that gives users a familiar "I can reach all
        // N of my apps in one Cmd hold" experience. Each press
        // visibly switches to a different app (since we re-issue
        // AX activation every step), so even the wrap-back-to-
        // origin shows on screen.
        let (snapshot, cursor_pid_in): (Vec<(String, String, libc::pid_t)>, libc::pid_t) =
            if continuing {
                let s = session_guard.as_mut().unwrap();
                // Drop snapshot entries whose pid is no longer a
                // running regular-policy app (quit mid-cycle).
                let live: HashSet<libc::pid_t> = regular.iter().map(|(_, _, p)| *p).collect();
                s.snapshot.retain(|(_, _, p)| live.contains(p));
                let cursor = s.cursor_pid;
                (s.snapshot.clone(), cursor)
            } else {
                // Fresh session. Ingest any new bundles into
                // mru_bundles (append at the end — least-recently
                // used by default), then bump the live frontmost
                // bundle to the front so it becomes the cycle
                // origin even if NSWorkspace's frontmost tracker
                // is stale relative to our last AX activation.
                {
                    let mru = mru_bundles();
                    let mut guard = mru.lock_or_recover();
                    for (bundle, _, _) in &regular {
                        if bundle != "<no-bundle-id>" && !guard.iter().any(|b| b == bundle) {
                            guard.push(bundle.clone());
                        }
                    }
                }
                if let Some(m) = metas.iter().find(|m| m.pid == front_pid) {
                    promote_bundle_to_front(&m.bundle);
                }
                // Sort regular[] by MRU position. `<no-bundle-id>`
                // entries (rare for regular-policy apps) sort to
                // the end. Front_pid is at MRU[0] post-promote, so
                // it lands at index 0 of the snapshot — the cursor
                // starts there and the first Tab press advances
                // to index 1 = previous-MRU app.
                let mru_order: Vec<String> = mru_bundles().lock_or_recover().clone();
                let mut ordered = regular.clone();
                ordered.sort_by_key(|(bundle, _, _)| {
                    mru_order
                        .iter()
                        .position(|b| b == bundle)
                        .unwrap_or(usize::MAX)
                });
                (ordered, front_pid)
            };

        if snapshot.is_empty() {
            *session_guard = None;
            if super::super::APP_SWITCHER_HUD.load(std::sync::atomic::Ordering::Relaxed) {
                crate::switcher_hud::hide();
            }
            debug!("cycle_apps: nothing to cycle to (no regular apps)");
            return;
        }

        let n = snapshot.len();
        // Locate cursor in the snapshot and advance ±1 with wrap.
        // If the cursor pid isn't in the snapshot (e.g., front_pid
        // was a non-regular app like Spotlight that doesn't appear
        // in `regular`), land on the most-recent MRU entry as the
        // best-effort starting point.
        let next_idx = match snapshot.iter().position(|(_, _, p)| *p == cursor_pid_in) {
            Some(i) => {
                if reverse {
                    (i + n - 1) % n
                } else {
                    (i + 1) % n
                }
            }
            None => {
                if reverse {
                    n - 1
                } else {
                    0
                }
            }
        };
        let (target_bundle, target_name, target_pid) = snapshot[next_idx].clone();

        // Save updated session so the next intra-hold press
        // advances from where we are now. Snapshot stays frozen —
        // do NOT promote target to MRU front here; that happens
        // at session commit (Cmd release / grace expiry).
        *session_guard = Some(CycleSession {
            snapshot: snapshot.clone(),
            cursor_pid: target_pid,
            last_press_at: now,
            cmd_release_gen: current_release_gen,
        });
        drop(session_guard);

        // Drive the optional on-screen HUD (best-effort; --app-switcher-hud).
        // SHOW on a fresh session (full app list), ADVANCE within a hold.
        if super::super::APP_SWITCHER_HUD.load(std::sync::atomic::Ordering::Relaxed) {
            if continuing {
                crate::switcher_hud::advance(next_idx);
            } else {
                let apps: Vec<(i32, String)> = snapshot
                    .iter()
                    .map(|(_, name, pid)| (*pid, name.clone()))
                    .collect();
                crate::switcher_hud::show(apps, next_idx);
            }
        }

        // Pin the per-bundle MRU pid to the instance we're
        // activating, so if the user cycles away and back the
        // dedup keeps the same instance.
        if target_bundle != "<no-bundle-id>" {
            mru_map()
                .lock_or_recover()
                .insert(target_bundle.clone(), target_pid);
        }
        // Dump the snapshot order (the actual cycle order, with
        // session-origin excluded) and the underlying global MRU
        // list, so a confusing cycle decision is debuggable from
        // the log alone without re-deriving state.
        let snapshot_order: Vec<String> = snapshot
            .iter()
            .map(|(b, _, p)| format!("{b}#{p}"))
            .collect();
        let mru_order: Vec<String> = mru_bundles().lock_or_recover().clone();
        debug!(
            reverse,
            continuing,
            cursor_pid_in,
            next_idx,
            snapshot_len = n,
            effective_front_pid = front_pid,
            workspace_front_pid,
            target = %target_bundle,
            name = %target_name,
            pid = target_pid,
            snapshot_order = ?snapshot_order,
            mru_bundles = ?mru_order,
            "cycle_apps activating"
        );
        // Three activation paths exist on modern macOS:
        //   - NSRunningApplication.activateWithOptions: silently no-ops
        //     when the caller isn't already the front app (macOS 14+
        //     front-app-only enforcement).
        //   - osascript `tell application "X" to activate`: Apple
        //     Event, gated by TCC Automation. Unsigned macrdp running
        //     headless never gets the first-run grant prompt and the
        //     command silently fails.
        //   - `/usr/bin/open -b <bundle-id>`: LaunchServices route.
        //     The target's self-activate hits the same front-app-only
        //     gate, so it doesn't activate — BUT it does *launch* the
        //     bundle if no process is running, which gets in our way:
        //     when the user has just quit an app, falling through here
        //     re-launches it instead of skipping. Removed.
        //
        // Accessibility API is the one that works. macrdp already
        // holds the Accessibility TCC grant (required for CGEventPost
        // to take effect at all); AX permission lets us set
        // kAXFrontmostAttribute on a target app's AXUIElement, which
        // is the same gesture the Dock uses internally. No
        // front-app-only block, and — critically — doesn't relaunch
        // dead processes.
        // Capture workspace's current value the first time we're
        // about to make it go stale. After this AX activation,
        // `frontmostApplication()` will keep returning whatever
        // it was returning *right now*, and that's the value
        // `effective_front_pid` needs to recognize as lying.
        // Only set if currently None — within a held-Cmd cycle the
        // first activation already captured it; subsequent intra-
        // cycle activations don't change what workspace is lying
        // about.
        {
            let mut lie = WORKSPACE_LIE_FRONT.lock_or_recover();
            if lie.is_none() {
                *lie = Some(workspace_front_pid);
            }
        }
        // Force-show: un-minimize the target on each cycle step too (when
        // enabled), so a minimized app pops open as you Cmd+Tab to it rather
        // than only on release. A brief de-miniaturize flicker is expected.
        // Per cycle step: do NOT reopen windowless apps (that's commit-only,
        // so apps cycled *through* don't pop windows).
        let ax_err = ax_make_frontmost(
            target_pid,
            super::super::UNMINIMIZE_ON_SWITCH.load(std::sync::atomic::Ordering::Relaxed),
            false,
        );
        if ax_err == AX_ERROR_SUCCESS {
            *LAST_AX_ACTIVATED_PID.lock_or_recover() = Some(target_pid);
            // Promote on every successful activation, not just on
            // Cmd-release commit. Our cycle has no switcher UI, so
            // each Tab press visibly switches apps and the user
            // experiences every step as a real visit — MRU should
            // reflect that or the next session's cycle order will
            // disagree with what they just saw on screen. Snapshot
            // is frozen so this doesn't disrupt the active cycle;
            // it only affects what subsequent sessions consult.
            if target_bundle != "<no-bundle-id>" {
                promote_bundle_to_front(&target_bundle);
            }
            debug!(target = %target_bundle, pid = target_pid, "cycle_apps: AX activation ok");
        } else {
            warn!(
                target = %target_bundle,
                pid = target_pid,
                ax_err,
                "cycle_apps: AX activation failed — skipping (no relaunch fallback)"
            );
        }
    }
}
