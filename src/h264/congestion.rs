//! The adaptive bitrate and frame-rate controller: RTT and queue-delay
//! estimation, AIMD, IDR back-off and the EGFX-over-UDP throttle floor. Pure
//! functions, called from the pipeline in `mod.rs`.

use super::*;

/// Minimum spacing between "trickle" frames let through the EGFX-on-UDP
/// backpressure gate while the client's frame-ack lag is over the threshold.
/// ~10 fps: enough trailing frames for mstsc to keep presenting + acking (so
/// the window reopens and lag recovers) while still throttling well below the
/// full 60 fps so the client's decode queue drains net. See the gate in
/// `submit_bgra` and `ConnectionContext::last_throttle_ship`.
pub(super) const UDP_THROTTLE_FLOOR: Duration = Duration::from_millis(100);

/// P3 cold-start guard: how long after the first frame-ack the adaptive controller
/// ignores the ack-lag congestion signal. At connect the encoder ships an initial
/// burst (keyframe + first frames) before the client starts acking, so `shipped −
/// acked` spikes (~25) for ~1.5 s — that's startup backlog, not congestion, and
/// honoring it dips the bitrate right as the session opens. The retransmit signal
/// stays active during warmup (it's acks-independent).
pub(super) const ADAPTIVE_WARMUP: Duration = Duration::from_secs(2);

/// Slots in the per-connection ship-time ring used to sample each frame's
/// ship→ack round trip (indexed `frame_id % RTT_RING`). 128 comfortably covers
/// any realistic frames-in-flight window (even 500 ms RTT at 60 fps is ~30).
pub(super) const RTT_RING: usize = 128;

/// Bucket width of the two-bucket windowed-minimum RTT filter. The min over
/// the current + previous bucket is the base-RTT estimate the queue-delay
/// signal subtracts; a route change (VPN reconnect) re-baselines within ~2
/// buckets. Long on purpose: a SHORT window lets a slowly-growing standing
/// queue launder itself into the baseline (the min "chases" the queued
/// samples and the measured delay reads as growth-per-window instead of the
/// absolute queue). 30 s buckets keep the base honest for 30–60 s while
/// still re-baselining after a genuine route change within a minute.
pub(super) const RTT_MIN_WINDOW: Duration = Duration::from_secs(30);

/// Fold one ack-RTT sample into the current windowed-min bucket and return
/// `(new_bucket_min, standing_queue_delay_ms)` — the delay is the sample's
/// excess over the two-bucket minimum (never negative). Pure, unit-tested;
/// bucket rotation happens at the call site (it needs the clock).
pub(super) fn queue_delay_fold(
    sample_ms: f64,
    bucket_min_ms: f64,
    prev_bucket_min_ms: f64,
) -> (f64, f64) {
    let cur = bucket_min_ms.min(sample_ms);
    let base = cur.min(prev_bucket_min_ms);
    (cur, (sample_ms - base).max(0.0))
}

/// The controller's effective congestion sample for one interval: the latest
/// ack-derived queue delay, floored by the **no-ack fallback** — when frames
/// are outstanding and we're actively shipping but acks have gone quiet, the
/// time since the last ack is itself a lower bound on the standing delay.
/// Without this, TOTAL saturation goes dark: a fully choked pipe delivers so
/// few frames that acks stop entirely → no RTT samples → `queue_delay_ms`
/// freezes at its last (healthy) value and the controller reads a drowning
/// link as clean (observed live on a shaped 500 Kbit pipe 2026-07-04: ack lag
/// grew 275→4746 while the sampled delay stayed 0.0). On a healthy link acks
/// arrive continuously, so `since_ack_ms` is just the tiny inter-ack gap and
/// the max() is a no-op; with nothing outstanding (static screen) or shipping
/// stopped (suppressed), the fallback is skipped so idle never reads as
/// congestion. Pure, unit-tested.
pub(super) fn effective_queue_delay(
    queue_delay_ms: f64,
    since_ack_ms: f64,
    outstanding: bool,
    actively_shipping: bool,
) -> f64 {
    if outstanding && actively_shipping {
        queue_delay_ms.max(since_ack_ms)
    } else {
        queue_delay_ms
    }
}
/// RTT-seeded initial bitrate (2026-07-05): the encoder's starting target for a
/// new connection. On a slow link (`link_rtt_ms >= seed_rtt_ms`), start at
/// **ceiling / 3** (clamped to `[floor, ceiling]`) instead of slamming the full
/// ceiling into a pipe we already know is distant — the adaptive controller
/// then climbs toward the ceiling if the link has headroom (long-but-fat links
/// recover full quality within seconds) or backs off if it strains. Fast /
/// unknown links start at the ceiling exactly as before. `seed_rtt_ms == 0`
/// disables seeding. Pure, unit-tested.
pub(super) fn seeded_initial_bitrate(
    ceiling: u32,
    floor: u32,
    link_rtt_ms: u32,
    seed_rtt_ms: u32,
) -> u32 {
    if seed_rtt_ms == 0 || link_rtt_ms == 0 || link_rtt_ms < seed_rtt_ms {
        return ceiling;
    }
    (ceiling / 3).clamp(floor.min(ceiling), ceiling.max(1))
}

/// Pure AIMD step for congestion-responsive bitrate (P1). Given the current target,
/// the reliable-tunnel loss delta observed this control interval, and the bounds/
/// params, return the new target bitrate. **Multiplicative-decrease** on any loss
/// (back off fast, clamp to `floor_bps`); **additive-increase** when clean (climb
/// slowly, clamp to `ceiling_bps`). Pure (no clock/state) so it's unit-testable.
/// See [`Gfx::adaptive_bitrate_step`].
pub(super) fn aimd_bitrate(
    current: u32,
    loss_delta: u64,
    floor_bps: u32,
    ceiling_bps: u32,
    increase_bps: u32,
    decrease: f32,
) -> u32 {
    if loss_delta > 0 {
        (((current as f32) * decrease) as u32).max(floor_bps)
    } else {
        current.saturating_add(increase_bps).min(ceiling_bps)
    }
}

/// What the IDR-backoff sub-controller (P2a) should do this interval. A periodic
/// keyframe is a big intra frame — the worst thing to inject into a congested,
/// backed-up tunnel — so under congestion we **stretch** the keyframe interval
/// (effectively suppressing the periodic IDR) and **restore** it (plus force one
/// clean recovery IDR) once the link has fully recovered. Safe on the RELIABLE
/// tunnel: reliable delivery means there's no loss-corruption to heal, so the
/// periodic IDR is only a decode-glitch safety net we can defer until clear.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum IdrBackoff {
    /// Loss started and we're not yet backed off → suppress the periodic IDR.
    Stretch,
    /// Fully recovered (clean + back at the bitrate ceiling) while backed off →
    /// restore the normal interval and force one recovery IDR.
    Restore,
    /// No change this interval.
    Hold,
}

/// Pure IDR-backoff decision (P2a). `loss` = any reliable retransmit this interval;
/// `new_target`/`ceiling` are the post-AIMD bitrate and its ceiling; `backed_off`
/// is the current state. Unit-tested. See [`Gfx::adaptive_bitrate_step`].
pub(super) fn idr_backoff_decision(
    loss: bool,
    new_target: u32,
    ceiling: u32,
    backed_off: bool,
) -> IdrBackoff {
    if loss && !backed_off {
        IdrBackoff::Stretch
    } else if !loss && new_target >= ceiling && backed_off {
        IdrBackoff::Restore
    } else {
        IdrBackoff::Hold
    }
}

/// Whether the reliable-tunnel retransmits observed this control interval count as
/// loss, given the per-interval `tolerance`. The tunnel retransmits on *any* packet
/// loss and a wireless link (WiFi) has near-continuous low-level loss, so a single
/// retransmit must NOT read as congestion — only `retransmit_delta > tolerance` does.
/// `tolerance == 0` restores the old "any retransmit = loss" behaviour. Pure +
/// unit-tested. See [`Gfx::adaptive_bitrate_step`] and the `adaptive_retx_tolerance` field.
pub(super) fn retransmit_is_lossy(retransmit_delta: u64, tolerance: u64) -> bool {
    retransmit_delta > tolerance
}

/// Pure congestion decision with EWMA smoothing + hysteresis. `ewma_lag` is the
/// exponentially-smoothed frame-ack lag (shipped − acked); the caller smooths the raw
/// per-interval lag so a single spike doesn't trip a back-off (raw TCP ack-lag bursts
/// 0↔40 even at moderate loss, which a naive threshold turns into visible bitrate
/// pumping). **Hysteresis:** enter congestion when `ewma_lag` crosses `high`, then stay
/// congested until it falls below `low` (`low < high`) — so the bitrate doesn't
/// flip-flop while the signal straddles one threshold. `retransmit_lossy` (UDP — the
/// caller has already applied the per-interval retransmit *tolerance*, so this is
/// "loss above the wireless background", not "any retransmit") forces congested
/// immediately (a definite loss, no smoothing). With acks unusable (suspended / not yet
/// seen / cold-start warmup), the lag is uninferable so only the retransmit signal
/// counts. Unit-tested. See [`Gfx::adaptive_bitrate_step`].
pub(super) fn congested_hysteresis(
    ewma_lag: f64,
    high: f64,
    low: f64,
    retransmit_lossy: bool,
    acks_usable: bool,
    currently_congested: bool,
) -> bool {
    if retransmit_lossy {
        return true;
    }
    if !acks_usable {
        return false;
    }
    if currently_congested {
        ewma_lag > low // stay congested until the smoothed lag drops below the low mark
    } else {
        ewma_lag > high // only enter once the smoothed lag clears the high mark
    }
}

/// What the controller does to the bitrate this interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RateAction {
    /// Smoothed lag is above the high mark (or a retransmit) — multiplicative-decrease.
    Decrease,
    /// In the hysteresis band (congested, lag decaying between low and high) — hold the
    /// current bitrate. Stops a single spike from cratering the bitrate as the EWMA
    /// decays back through the band (it would otherwise decrease every interval → the
    /// "video sometimes stops" deep dips).
    Hold,
    /// Cleared (below the low mark / not in an episode) — additive-increase toward ceiling.
    Increase,
}

/// Pure 3-zone bitrate action on the smoothed signal (AIMD with a hold band).
/// `congested` is the hysteresis state from [`congested_hysteresis`] this interval.
/// Decrease while genuinely congested (lag above `high`, or retransmits above the
/// tolerance); hold while
/// the episode is still latched but the smoothed lag is decaying back through the band;
/// increase once cleared. So a single spike = one step down then a plateau, while
/// *sustained* congestion (lag stays above `high`) keeps decreasing toward the floor.
/// Unit-tested. See [`Gfx::adaptive_bitrate_step`].
pub(super) fn rate_action(
    ewma_lag: f64,
    high: f64,
    retransmit_lossy: bool,
    acks_usable: bool,
    congested: bool,
) -> RateAction {
    if retransmit_lossy || (acks_usable && ewma_lag > high) {
        RateAction::Decrease
    } else if !congested {
        RateAction::Increase
    } else {
        RateAction::Hold
    }
}

/// P2b — pure decision: should this capture be DROPPED to enforce a frame-rate floor?
///
/// Engages only once the bitrate controller has already cut the encoder to its floor
/// (`at_floor`) AND the link is still `congested` — i.e. lowering quality can no longer
/// help, so the next lever is shedding *frames* (fewer frames → fewer packets → less
/// load). It caps the effective frame rate to a floor by dropping any capture that
/// arrives within `min_interval` of the last one we let through (`since_last_pass`).
///
/// It never drops to zero: a capture is let through once `min_interval` has elapsed, so
/// the client always keeps receiving trailing frames to present/ack (the same reason the
/// EGFX-on-UDP trickle floor never zeroes — dropping to zero pins the lag and freezes the
/// picture). Works on BOTH transports; on TCP it's the only fps lever (there's no UDP
/// frame-ack backpressure gate). When the link recovers (`congested` clears or the
/// controller climbs off the floor) it stops dropping and the full capture rate resumes.
/// Unit-tested. See [`Gfx::submit_bgra`].
pub(super) fn frame_drop_at_floor(
    at_floor: bool,
    congested: bool,
    since_last_pass: Duration,
    min_interval: Duration,
) -> bool {
    at_floor && congested && since_last_pass < min_interval
}

/// Encoder adjustments the adaptive controller wants applied this frame: the P1
/// bitrate and the P2a keyframe interval. `None` fields = no change. The controller
/// also sets `ctx.need_keyframe` directly when forcing a recovery IDR.
#[derive(Default)]
pub(super) struct AdaptiveActions {
    pub(super) bitrate_bps: Option<u32>,
    pub(super) keyframe_frames: Option<u32>,
}
