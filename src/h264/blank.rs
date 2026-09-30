//! Blank-presentation detection and recovery (the mstsc reconnect blank; see
//! docs/known-quirks.md). The detector state and decisions are pure; the
//! pipeline in `mod.rs` feeds them QoE reports and runs the chosen action.

use super::*;

/// Tunables for the blank-presentation detector + recovery (the mstsc
/// reconnect-blank). See [`should_blank_recover`] and the H.264 reconnect
/// quirk note in `docs/known-quirks.md`.
#[derive(Clone, Copy, Debug)]
pub(super) struct BlankRecoveryParams {
    /// QoE reports that must accumulate (all with `time_diff_dr == 0`) before
    /// the session is declared blank. Upstream delivers ~8 `on_qoe_metrics`
    /// callbacks/s during active decoding (~1 per DVC batch, NOT per wire PDU —
    /// measured live 2026-07-02: ~850 wire QoE PDUs produced exactly 120
    /// callbacks), so the default 24 ≈ 3 s of active decoding. Also a floor on
    /// real decode activity — a static screen accrues slowly and simply defers
    /// detection. A *rendering* session is disarmed by its very first
    /// callbacks: the initial full-screen IDR present always costs >1 ms
    /// (observed 7–14 ms within ~230 ms of connect on every healthy session),
    /// so a whole all-zero window is unambiguous well before 24. (The original
    /// default was 40 ≈ 5 s; tightened once the drop became self-healing via
    /// the auto-reconnect cookie — a hypothetical false positive now costs one
    /// client-driven reconnect, not a dead session.)
    pub(super) min_qoe_reports: u64,
    /// Consecutive nonzero-EDR reports that count as "the client is presenting"
    /// and disarm the detector. **Sustained, not a single report** — see
    /// [`QoeEvidence`] for the live case that forced this: a client can resume
    /// reporting nonzero decode+render times after a recovery reactivation
    /// while its picture stays black, and a one-report disarm then suppressed
    /// the fallback drop forever. `MACRDP_BLANK_RECOVERY_MIN_RENDER_REPORTS`,
    /// default 3 — at the ~8 callbacks/s upstream delivers during active
    /// decoding that is ~0.4 s, so a genuinely healthy session still disarms
    /// long before the 3 s `arm_delay` lets the detector evaluate anything.
    /// Deliberately NOT scaled by the RTT gate ([`blank_params_scaled`]):
    /// raising it on a slow link would make the disarm *harder*, and slow links
    /// are exactly where a false positive is most costly.
    pub(super) min_render_reports: u64,
    /// Consecutive nonzero-EDR reports that mark a session as ESTABLISHED
    /// (presented for a meaningful stretch, presumed healthy). At the ~8
    /// callbacks/s active cadence the default 40 is ~5 s of continuous
    /// presentation. Above this bar a relapse to zero EDR is treated as
    /// probably-transient and held to `established_min_qoe` instead of the
    /// aggressive `min_qoe_reports`; below it (a never/barely-presented
    /// connection, incl. a post-reactivation few-frame flicker) the aggressive
    /// connect-blank path applies. Deliberately HIGHER than `min_render_reports`
    /// (the few-frame disarm) — the two gate opposite things. NOT RTT-scaled.
    /// `MACRDP_BLANK_RECOVERY_ESTABLISHED_REPORTS`.
    pub(super) established_render_reports: u64,
    /// All-zero QoE window required to recover an ESTABLISHED session (see
    /// `established_render_reports`). Much larger than `min_qoe_reports`: this
    /// client has ~3 s windows where it stops reporting nonzero EDR while
    /// displaying fine, and the aggressive count dropped a healthy 12-minute
    /// session live (2026-07-22). At ~8/s the default 160 is ~20 s of sustained
    /// zeros — long enough that a transient clears first, while a genuine (rare)
    /// mid-session blackout still eventually recovers. RTT-scaled like
    /// `min_qoe_reports`. `MACRDP_BLANK_RECOVERY_ESTABLISHED_MIN_QOE`.
    pub(super) established_min_qoe: u64,
    /// Wall-clock companion to `established_min_qoe`: an established session is
    /// also declared blank once no nonzero-EDR report has arrived for this long
    /// AND `established_wall_reports` consecutive zeros are in evidence. Exists
    /// because the count path assumes the active ~8/s QoE cadence — on a STATIC
    /// blank the cadence collapses to ~0.3/s and 160 reports is ~9 minutes, so
    /// without this bound a genuine mid-session blackout on an idle screen
    /// would practically never recover. Default 30 s; RTT-scaled like
    /// `blank_max_wait`. `MACRDP_BLANK_RECOVERY_ESTABLISHED_MAX_WAIT_MS`.
    pub(super) established_max_wait: Duration,
    /// Consecutive-zero floor for the established wall-clock branch (above).
    /// Proves the client is still decoding/acking while nothing presents;
    /// without it an IDLE healthy session (frames stop ⇒ QoE stops ⇒ the
    /// since-nonzero clock grows unboundedly) would trip the branch after any
    /// quiet half-minute. Default 16 (~2 s of active decode). Not RTT-scaled —
    /// it is paired with the wall clock, which is.
    /// `MACRDP_BLANK_RECOVERY_ESTABLISHED_WALL_REPORTS`.
    pub(super) established_wall_reports: u64,
    /// Don't evaluate before this much of the connection has elapsed — the
    /// connect-time surface/caps churn shouldn't race the detector.
    pub(super) arm_delay: Duration,
    /// Minimum spacing between recovery attempts (the QoE-report counter also
    /// resets per attempt, so a re-fire needs a full fresh all-zero window).
    pub(super) retry_interval: Duration,
    /// Post-attempt heal-confirmation deadline (2026-07-23, Windows App for
    /// macOS build 68576): once a recovery attempt has run, the session must
    /// PROVE it healed — a sustained nonzero-EDR run (`min_render_reports`) at
    /// some point since the attempt — within this much wall-clock, or the next
    /// attempt (normally the fallback drop) fires. Exists because this client
    /// starved the consecutive-zero escalation paths after a reactivation two
    /// distinct ways — interleaved phantom nonzero reports (runs of 2–18 while
    /// visibly black) that kept resetting `zero_streak`, or total QoE silence —
    /// and the user stared at black for 12 s then reconnected by hand while
    /// the drop that recovers this client sat unreachable. Guarded so an
    /// idle-but-healed session can't trip it: it requires the client to have
    /// ACKED frames since the attempt (the post-attempt IDR + flush frames
    /// give a live client something to ack; no acks ⇒ nothing shipped ⇒
    /// nothing to conclude) and either total QoE silence while acking (the
    /// blank tell — this client emitted QoE fine before the attempt) or
    /// `blank_min_reports` CUMULATIVE zeros since the attempt (immune to the
    /// interleaved blips, which reset the streak but not the tally). A healed
    /// static-desktop mstsc emits a few honest nonzero reports and no zeros
    /// post-heal, so it matches neither arm. RTT-scaled like `blank_max_wait`.
    /// `MACRDP_BLANK_RECOVERY_HEAL_CONFIRM_MS`, default 8000; 0 disables.
    pub(super) heal_confirm_deadline: Duration,
    /// Total attempts per connection. All attempts but the last REMAP the
    /// output to a fresh surface (non-destructive); the LAST attempt drops the
    /// connection so the client auto-reconnects (a fresh attempt renders with
    /// high probability, and the detector re-checks the new session). The
    /// default is 1 — i.e. go STRAIGHT to the drop: the remap was live-verified
    /// (2026-07-02) to never heal mstsc (its layer-2 re-composite bug is
    /// client-fatal for in-session surface swaps), so remap-first only added
    /// ~10 s of black (a wasted attempt + a second full detection window)
    /// before the drop that actually heals. Set ≥2 via
    /// `MACRDP_BLANK_RECOVERY_MAX_ATTEMPTS` to re-enable remap-first
    /// experimentation (e.g. against a non-mstsc QoE-reporting client).
    pub(super) max_attempts: u32,
    /// ON by default (`MACRDP_BLANK_RECOVERY_REACTIVATE=0` reverts to the
    /// remap/drop path): make the FIRST recovery attempt a bare core
    /// Deactivation–Reactivation ([`BlankAction::Reactivate`]); if it doesn't
    /// heal, the second attempt drops. Forces `max_attempts` to ≥2 so the
    /// fallback drop can fire.
    pub(super) reactivate: bool,
    /// Wall-clock fast-path for detection on a STATIC blank. A blank desktop
    /// changes little, so QoE reports trickle in slowly (~0.3/s vs ~8/s on an
    /// active screen) and the `min_qoe_reports` count alone can take ~70 s to
    /// accumulate. Once this much wall-clock has elapsed with acks flowing and a
    /// small handful of all-zero reports (enough to rule out a client that sends
    /// no QoE at all), the session is conclusively blank — fire without waiting
    /// for the full count. Safe to be prompt because the reactivation heal is
    /// non-destructive: an occasional early fire costs a brief re-handshake, not
    /// a dropped session. `MACRDP_BLANK_RECOVERY_MAX_WAIT_MS`, default 4000.
    pub(super) blank_max_wait: Duration,
    /// Minimum all-zero QoE reports for the wall-clock fast-path (above). Its
    /// only job is to rule out a client that sends NO QoE (e.g. FreeRDP, which
    /// would otherwise satisfy `!qoe_render_seen` forever) — so the default is a
    /// low **1**: a single all-zero report after `arm_delay` (by which a
    /// rendering session has already presented and disarmed via
    /// `qoe_render_seen`, ~1-2 s on LAN) is conclusive on a trustworthy-RTT
    /// link. Raise it (`MACRDP_BLANK_RECOVERY_MIN_WALL_REPORTS`) if a
    /// slow-to-first-present client trips a spurious (cheap) reactivation.
    pub(super) blank_min_reports: u64,
    /// Reconnect-storm guard: if this many CONSECUTIVE connections all ended in
    /// a blank-recovery drop (no connection in between ever presented a frame),
    /// stop dropping — the client is truly stuck (mstsc retains surfaces for
    /// its whole process lifetime, and on rare clients every in-process
    /// reconnect lands blank), and an endless drop → auto-reconnect → blank →
    /// drop loop flashing "reconnecting…" every few seconds is worse than a
    /// stable session plus clear log guidance (close + reopen the client — the
    /// known-reliable recovery). The counter resets the moment any connection
    /// reports a nonzero decode+render time (i.e. actually presents). 0 = no
    /// cap.
    pub(super) max_consecutive_drops: u32,
    /// RTT gate (link-aware detection, 2026-07-05): the kernel-measured TCP RTT
    /// (ms) at or above which the DROP lever is withheld entirely for the
    /// connection. The blank signature (`timeDiffEDR == 0` while acks flow) is
    /// only trustworthy on fast links — live-verified over ZeroTier (~200 ms):
    /// a session that IS visibly rendering reports zero EDR on every frame, so
    /// the detector force-dropped a working session every ~5 s and the repeated
    /// drops poisoned mstsc's surface into a REAL permanent black (the recovery
    /// *caused* the blank). Below the gate the evidence window scales with RTT
    /// (see [`blank_rtt_gate`]); at/above it the detector is disarmed for the
    /// connection (log-only). 0 = no RTT gating (pre-2026-07-05 behavior).
    pub(super) max_rtt_ms: u32,
}

/// Which recovery lever to pull for a given (1-based) attempt number: every
/// attempt before the last remaps to a fresh surface; the last one drops the
/// connection. Pure, unit-tested.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(super) enum BlankAction {
    /// Create a fresh surface, map it over the output, force an IDR — the
    /// non-destructive in-session heal. Deliberately sends NO DeleteSurface and
    /// NO RESET_GRAPHICS: the resize-dance variant (upstream `resize_with_
    /// monitors`, whose first PDU deletes the mapped surface) was tried live
    /// 2026-07-02 and KILLED mstsc's GFX channel outright (zero acks/QoE for
    /// the rest of the session) — the third independent confirmation that any
    /// DeleteSurface aimed at a blank mstsc is fatal. The old surface is left
    /// alive; the client leaks at most (max_attempts − 1) surfaces.
    Remap,
    /// Drop the connection (`ServerEvent::Quit` → per-connection
    /// `RunState::Disconnect`). mstsc treats the unexpected loss as an outage
    /// and auto-reconnects with its reconnect cookie; a fresh connection
    /// renders with high probability and the detector re-checks it.
    Drop,
    /// Gated by `MACRDP_BLANK_RECOVERY_REACTIVATE` (default on): trigger a bare
    /// core RDP **Deactivation–Reactivation** (Server Deactivate All → new
    /// Demand Active) WITHOUT touching the EGFX pipeline. Injected as a no-op
    /// `DisplayUpdate::Resize(current_size)` (see `capture.rs`), which the
    /// vendored server turns into `deactivate_all` +
    /// `Acceptor::new_deactivation_reactivation` — and that call PRESERVES the
    /// static channels, so the EGFX DVC (and our `ConnectionContext`/surface)
    /// survive: `build_server_with_handle` is not re-run, `ensure_surface` skips
    /// (`surface_id` already `Some`), so NO `resize_with_monitors` / NO
    /// DeleteSurface fires. A forced IDR follows. **LIVE-VERIFIED 2026-07-07 to
    /// HEAL the mstsc reconnect-blank** — 5/5 blanks on real mstsc/WiFi went
    /// EDR=0 → presenting in ~1-2 s with zero drops (frame ids mid-stream, so
    /// the same connection healed in place, no reconnect). This is the DEFAULT
    /// first recovery action and overturns the long-held "layer-2 is
    /// client-fatal / not server-fixable" conclusion: prior attempts all
    /// bundled a surface delete or DVC close (which ARE client-fatal); a bare
    /// core reactivation, uniquely, is not. If it ever fails to heal, the
    /// detector re-fires and attempt 2 falls through to [`BlankAction::Drop`].
    Reactivate,
}

pub(super) fn blank_action(attempt: u32, max_attempts: u32) -> BlankAction {
    if attempt < max_attempts {
        BlankAction::Remap
    } else {
        BlankAction::Drop
    }
}

/// Reconnect-storm guard (see [`BlankRecoveryParams::max_consecutive_drops`]):
/// true when the blank-recovery DROP lever must be withheld because the last
/// `cap` consecutive connections all ended in a blank drop without any
/// connection presenting in between. Pure, unit-tested.
pub(super) fn blank_drop_capped(consecutive_drops: u32, cap: u32) -> bool {
    cap > 0 && consecutive_drops >= cap
}

/// Whether an in-flight connection should clear the reconnect-storm drop counter
/// (see the call site + [`blank_drop_capped`]). Only a genuinely-ESTABLISHED
/// connection resets it — a brief blip (a few post-reactivation frames that then
/// relapse to black) does NOT, so a brief-present-then-drop still counts toward
/// the cap. Reset only matters when the counter is non-zero. Pure, unit-tested.
pub(super) fn storm_guard_should_reset(
    qoe: QoeEvidence,
    established_render_reports: u64,
    current_drops: u32,
) -> bool {
    current_drops != 0 && qoe.established(established_render_reports)
}

/// Raw QoE decode+render-time counters for the blank detector — pure tallies,
/// no policy (the thresholds live in [`BlankRecoveryParams`]).
///
/// **Why streaks rather than a "has ever rendered" latch.** The original design
/// latched `qoe_render_seen` on the *first* report with `time_diff_dr > 0` and
/// never cleared it, on the reasoning that one nonzero EDR proves the client
/// composited a frame. Live evidence (2026-07-22, Windows App for macOS)
/// refuted that as a disarm condition: after a recovery *reactivation* the
/// client resumed reporting nonzero decode+render times while the picture on
/// screen stayed black. The latch made that permanent — the detector considered
/// the session healed, so the fallback drop (the lever that actually recovers
/// this client) could never fire, and the user had to reconnect by hand.
///
/// Two independent counters fix both halves of that:
/// - `nonzero_streak` — consecutive nonzero-EDR reports. Requiring several
///   ("sustained") means a brief post-reactivation blip no longer disarms.
/// - `zero_streak` — consecutive all-zero reports, reset by ANY nonzero one.
///   This is what makes the disarm revocable: a client that lapses back to
///   zero rebuilds a full fresh evidence window and the detector re-fires.
///
/// `max_nonzero_streak` is the high-water mark of `nonzero_streak`, i.e. "did
/// this connection ever genuinely present" — kept only to gate the wall-clock
/// fast path (see [`should_blank_recover`]).
///
/// The three `*_since_reset` fields are CUMULATIVE tallies over the current
/// evidence window (since connect, or since the last recovery attempt's
/// [`reset_streaks`]) — unlike the streaks, a report of the opposite kind does
/// NOT clear them. They exist for the post-attempt heal-confirmation deadline
/// (2026-07-23, Windows App for macOS build 68576): after a reactivation this
/// client can emit interleaved phantom nonzero reports (runs of 2–18 observed
/// while visibly black) that reset `zero_streak` forever, or go QoE-silent
/// entirely — either way the consecutive-zero paths starve and the fallback
/// drop never fires. Cumulative counters are immune to the interleaving, and
/// `reports_since_reset == 0` is the silence tell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct QoeEvidence {
    pub(super) zero_streak: u64,
    pub(super) nonzero_streak: u64,
    pub(super) max_nonzero_streak: u64,
    /// Total QoE reports folded in since the last [`reset_streaks`].
    pub(super) reports_since_reset: u64,
    /// Cumulative ZERO reports since the last [`reset_streaks`] — NOT cleared
    /// by a nonzero report (that is the whole point; see the type doc).
    pub(super) zeros_since_reset: u64,
    /// High-water `nonzero_streak` since the last [`reset_streaks`] — "did the
    /// client sustain presentation at any point in THIS window", as opposed to
    /// `max_nonzero_streak` which spans the whole connection.
    pub(super) nonzero_max_since_reset: u64,
    /// Durable "this connection genuinely presented at connect" latch (v0.9.2).
    /// Set by the caller (`on_qoe_metrics`) the moment a sustained nonzero-EDR
    /// run appears **while no recovery attempt has yet fired**, and never
    /// cleared for the connection. When set, [`should_blank_recover`] returns
    /// false unconditionally — the v0.9.0 one-shot-disarm behavior, restored.
    ///
    /// Why gate on "before any recovery attempt": QoE decode+render-time is
    /// bidirectionally unreliable — one client class reports nonzero-while-black
    /// (the #172 reconnect-blank client that flickers nonzero *after* a
    /// reactivation), another reports zero-while-presenting (the client whose
    /// working 50 s sessions #172 began false-dropping). The one signal that
    /// separates them is *when* the nonzero run occurs: a sustained run seen
    /// **before** any recovery proves the client painted the desktop on its own
    /// (it is not the connect-time reconnect-blank, which is black from frame
    /// one); a run seen **after** a reactivation may be the blank client's
    /// post-reactivation flicker and must not disarm. So the latch requires the
    /// former and the caller withholds it for the latter (`attempts == 0`).
    pub(super) presented_clean: bool,
}

impl QoeEvidence {
    /// Fold in one QoE frame-acknowledge.
    pub(super) fn record(&mut self, time_diff_dr: u16) {
        self.reports_since_reset = self.reports_since_reset.saturating_add(1);
        if time_diff_dr > 0 {
            self.zero_streak = 0;
            self.nonzero_streak = self.nonzero_streak.saturating_add(1);
            self.max_nonzero_streak = self.max_nonzero_streak.max(self.nonzero_streak);
            self.nonzero_max_since_reset = self.nonzero_max_since_reset.max(self.nonzero_streak);
        } else {
            self.nonzero_streak = 0;
            self.zero_streak = self.zero_streak.saturating_add(1);
            self.zeros_since_reset = self.zeros_since_reset.saturating_add(1);
        }
    }

    /// Clear the live streaks + the cumulative window tallies after a recovery
    /// attempt so a re-fire needs a full fresh window. `max_nonzero_streak` and
    /// `presented_clean` survive — they are facts about the connection, not
    /// evidence for the current window. (`presented_clean` can never be set
    /// once an attempt has fired, so in practice this only ever runs with it
    /// already false.)
    pub(super) fn reset_streaks(&mut self) {
        self.zero_streak = 0;
        self.nonzero_streak = 0;
        self.reports_since_reset = 0;
        self.zeros_since_reset = 0;
        self.nonzero_max_since_reset = 0;
    }

    /// Has this connection presented for a MEANINGFUL stretch — i.e. it is an
    /// established, presumed-healthy session rather than one that merely
    /// flickered a few frames? A few-frame blip must still be treated as a
    /// suspicious (probably still-blank) connection and recovered aggressively
    /// (and does NOT clear the reconnect-storm guard — see
    /// [`storm_guard_should_reset`]), whereas a session that genuinely showed
    /// the desktop for seconds must tolerate a transient zero-EDR window without
    /// being dropped. See [`should_blank_recover`].
    pub(super) fn established(&self, established_render_reports: u64) -> bool {
        self.max_nonzero_streak >= established_render_reports
    }
}

/// Decide whether to run a blank-recovery attempt. Pure (counters + Durations,
/// not a clock) so it's unit-testable without timing.
///
/// The signal (pcap-proven 2026-07-02, validated live the same day — see
/// [[h264-reconnect-blank]]): a reconnect that lands on mstsc's stale retained
/// surface DECODES every frame — FrameAcks and QoE acks flow normally — but
/// never PRESENTS, and its QoE Frame Acknowledge PDUs report `timeDiffEDR == 0`
/// on every single frame. A rendering session shows nonzero EDR on its first
/// callbacks (~100–230 ms after connect). So: QoE flowing + zero render-time
/// ever = the client is painting into a surface nobody composites (the
/// reconnect-blank). The caller then pulls the [`blank_action`] lever for the
/// attempt number: remap to a fresh surface, or drop the connection.
///
/// Each clause guards a distinct failure mode:
/// - `zero_streak >= min_qoe_reports`: enough evidence, and implies the client
///   actually sends QoE acks at all (FreeRDP-family clients that don't are
///   simply never evaluated — no false recovery on non-QoE clients).
/// - `!presenting_now`: a SUSTAINED run of nonzero EDR proves presentation and
///   disarms the detector — see [`QoeEvidence`] for why a single report is not
///   enough and why the disarm has to be revocable.
/// - `egfx_acks_seen && !acks_suspended`: regular FrameAcks flowing too — the
///   blank signature is "acking normally while EDR stays zero", not a stalled
///   or suspended client (those are congestion, handled elsewhere).
/// - `since_connect >= arm_delay`: skip the connect-time churn window.
/// - `since_last_attempt >= retry_interval` + `attempts < max_attempts`:
///   rate-limit; the final attempt is the connection drop, after which the
///   fresh connection starts a fresh detector.
/// - `acked_since_attempt`: whether the client has acknowledged frames since
///   the last recovery attempt — only consulted by the post-attempt
///   heal-confirmation deadline (see [`BlankRecoveryParams::heal_confirm_deadline`]).
#[allow(clippy::too_many_arguments)]
pub(super) fn should_blank_recover(
    qoe: QoeEvidence,
    egfx_acks_seen: bool,
    acks_suspended: bool,
    since_connect: Duration,
    since_last_nonzero: Duration,
    since_last_attempt: Duration,
    attempts: u32,
    acked_since_attempt: bool,
    p: &BlankRecoveryParams,
) -> bool {
    // Durable clean-presentation latch (v0.9.2): a connection that produced a
    // sustained nonzero-EDR run BEFORE any recovery attempt genuinely presented
    // the desktop at connect, so it is not the reconnect-blank (which is black
    // from frame one) — never recover it. This is the v0.9.0 one-shot disarm,
    // restored: it fixes a client that presents fine but reports zero EDR
    // mid-session (its short nonzero runs never reach the `established` bar, so
    // the revocable disarm was force-dropping working 50 s sessions), WITHOUT
    // re-breaking #172's blank client — that one produces its nonzero run only
    // AFTER a reactivation, and the caller withholds the latch there
    // (`attempts == 0`). See [`QoeEvidence::presented_clean`].
    if qoe.presented_clean {
        return false;
    }
    // "Presenting" = the LAST `min_render_reports` reports were all nonzero.
    // Streak, not a latch: a client that goes back to reporting zero (the
    // post-reactivation relapse) re-arms the detector automatically, because a
    // single zero report resets the streak.
    let presenting_now = qoe.nonzero_streak >= p.min_render_reports;
    // Established = the session presented for a MEANINGFUL stretch, so it is
    // presumed healthy. This is the false-positive fix (2026-07-22): this
    // client (Windows App for macOS) has brief windows — a few seconds — where
    // it stops reporting nonzero EDR while still displaying fine. On a
    // never/barely-presented connection those zeros are the reconnect-blank and
    // we act in ~3 s; on an ESTABLISHED session they are almost always one of
    // those transients, so requiring only `min_qoe_reports` (~3 s) dropped a
    // healthy 12-minute session live. An established session therefore needs a
    // much longer sustained zero window (`established_min_qoe`, ~20 s) — long
    // enough that a hiccup clears first, while a genuine (rare) mid-session
    // blackout still eventually recovers. `established_render_reports` (~5 s of
    // presentation) is deliberately a HIGHER bar than `min_render_reports` (the
    // few-frame disarm), so a post-reactivation few-frame flicker stays on the
    // aggressive path and the reconnect-blank escalation is unaffected.
    let established = qoe.established(p.established_render_reports);
    let count_threshold = if established {
        p.established_min_qoe
    } else {
        p.min_qoe_reports
    };
    // The established tier needs its own WALL-CLOCK branch, because the count
    // path alone silently assumes the active QoE cadence (~8 reports/s): on a
    // STATIC blank the cadence collapses to ~0.3/s, at which the 160-report
    // window is ~9 minutes — the drop escalation would be theoretically
    // reachable but practically never fire. So: an established session is also
    // considered blank once NOTHING nonzero has arrived for
    // `established_max_wait` wall-clock AND at least `established_wall_reports`
    // consecutive zeros prove the client is still decoding/acking (without that
    // floor, an IDLE healthy session — frames stop, QoE stops, the since-
    // nonzero clock grows unboundedly — would trip this after any quiet
    // half-minute). `since_last_nonzero` is wall time since the last nonzero
    // EDR report, saturating at `since_connect` for a session that never had
    // one (irrelevant here — this branch requires `established`).
    let established_blackout = established
        && since_last_nonzero >= p.established_max_wait
        && qoe.zero_streak >= p.established_wall_reports;
    // Post-attempt heal-confirmation deadline (2026-07-23, Windows App for
    // macOS build 68576 — see the param doc): the consecutive-zero branches
    // above all assume the client keeps emitting zeros in an unbroken run, and
    // after a recovery attempt this client starved them for 12+ s live —
    // either interleaved phantom nonzero reports (each one resetting
    // `zero_streak`) or total QoE silence — so the fallback drop, the lever
    // that actually recovers it, never fired and the user reconnected by hand.
    // Once an attempt has run, the burden of proof flips: the session must
    // show a sustained nonzero run within the deadline, or the escalation
    // fires on cumulative-zero / silence evidence the blips can't reset.
    // `acked_since_attempt` keeps an idle session out (no frames shipped ⇒
    // nothing to conclude), and `nonzero_max_since_reset < min_render_reports`
    // implies `!presenting_now` below, so the arms can't fight.
    let post_attempt_unconfirmed = attempts >= 1
        && !p.heal_confirm_deadline.is_zero()
        && since_last_attempt >= p.heal_confirm_deadline
        && acked_since_attempt
        && qoe.nonzero_max_since_reset < p.min_render_reports
        && (qoe.reports_since_reset == 0 || qoe.zeros_since_reset >= p.blank_min_reports);
    // Blank evidence: EITHER the full all-zero report count (fast on an active
    // screen), OR the established wall-clock blackout above, OR the
    // post-attempt heal-confirmation deadline above, OR — for a STATIC
    // connect-time blank whose QoE trickles in slowly — enough wall-clock
    // elapsed with at least `blank_min_reports` all-zero reports (which rules
    // out a client that sends no QoE, e.g. FreeRDP, from ever firing on the
    // wall-clock branch). That last fast path is withheld once the session is
    // established: with `blank_min_reports` as low as 1, a single stray zero
    // from a healthy long-running session would otherwise satisfy it.
    let blank_evidence = qoe.zero_streak >= count_threshold
        || established_blackout
        || post_attempt_unconfirmed
        || (since_connect >= p.blank_max_wait
            && qoe.zero_streak >= p.blank_min_reports
            && !established);
    blank_evidence
        && !presenting_now
        && egfx_acks_seen
        && !acks_suspended
        && since_connect >= p.arm_delay
        && since_last_attempt >= p.retry_interval
        && attempts < p.max_attempts
}

/// RTT gate for the blank detector (see [`BlankRecoveryParams::max_rtt_ms`]).
/// Returns the evidence-window multiplier for this connection's link RTT, or
/// `None` when the link is slow enough that the detector must not drop at all
/// (the EDR==0 signal is unreliable there — ZeroTier live finding 2026-07-05).
/// `link_rtt_ms == 0` means "unknown" (non-macOS / sample failed) and keeps
/// today's LAN behavior; `max_rtt_ms == 0` disables gating entirely. The
/// multiplier grows linearly from 1× at ≤25 ms, capped at 4×, so a moderately
/// distant client just needs a proportionally longer all-zero window. Pure,
/// unit-tested.
pub(super) fn blank_rtt_gate(link_rtt_ms: u32, max_rtt_ms: u32) -> Option<f64> {
    if max_rtt_ms == 0 || link_rtt_ms == 0 {
        return Some(1.0);
    }
    if link_rtt_ms >= max_rtt_ms {
        return None;
    }
    Some((f64::from(link_rtt_ms) / 25.0).clamp(1.0, 4.0))
}

/// Scale a [`BlankRecoveryParams`] evidence window by the RTT multiplier from
/// [`blank_rtt_gate`]: more all-zero QoE reports required and a longer arm
/// delay before the first evaluation. Attempt spacing/caps are unchanged.
pub(super) fn blank_params_scaled(p: &BlankRecoveryParams, mult: f64) -> BlankRecoveryParams {
    BlankRecoveryParams {
        min_qoe_reports: ((p.min_qoe_reports as f64) * mult).ceil() as u64,
        established_min_qoe: ((p.established_min_qoe as f64) * mult).ceil() as u64,
        arm_delay: p.arm_delay.mul_f64(mult),
        blank_max_wait: p.blank_max_wait.mul_f64(mult),
        established_max_wait: p.established_max_wait.mul_f64(mult),
        // mul_f64 of zero stays zero, so "0 = disabled" survives scaling.
        heal_confirm_deadline: p.heal_confirm_deadline.mul_f64(mult),
        ..*p
    }
}
