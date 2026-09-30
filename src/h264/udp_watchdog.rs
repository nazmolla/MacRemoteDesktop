//! EGFX-over-UDP recovery: the ack-stall IDR on the lossy tunnel and the
//! wedge watchdog that de-migrates EGFX to TCP. Pure decisions plus their
//! configuration.

use super::*;

/// Tunables for ack-driven IDR recovery (EGFX-on-lossy). See
/// [`should_force_recovery_idr`] and `docs/rdp-udp-multitransport-feasibility.md`
/// ("Ack-driven IDR recovery").
#[derive(Clone, Copy, Debug)]
pub(super) struct RecoveryParams {
    /// We only treat silent acks as loss while we're *actively* shipping — if the
    /// last ship is older than this, the screen is static and the periodic IDR
    /// backstops. Sized to cover the flush-burst window so a loss just before the
    /// screen goes static still heals.
    pub(super) active_window: Duration,
    /// How long acks must stay silent (while shipping) before we infer a lost
    /// frame. Above normal ack jitter + RTT, below the periodic keyframe interval.
    pub(super) ack_stall: Duration,
    /// Minimum spacing between forced recovery IDRs — the IDR is large and itself
    /// loss-vulnerable, so don't storm them if it keeps getting lost.
    pub(super) min_recovery_interval: Duration,
}

/// Decide whether to force a recovery IDR from ack-staleness. Pure (takes
/// `Duration`s, not a clock) so it's unit-testable without timing. See the spec
/// in `docs/rdp-udp-multitransport-feasibility.md` — each clause guards a distinct
/// failure mode:
/// - `egfx_on_lossy`: only on the lossy tunnel; on TCP/reliable a missing ack is
///   congestion and an IDR would *worsen* it.
/// - `!acks_suspended`: with acks off (`queueDepth==0xFFFFFFFF`) loss is uninferable.
/// - `since_ship <= active_window`: only while actively shipping (else: static screen).
/// - `since_ack >= ack_stall`: the loss signal — acks went silent.
/// - `since_recovery >= min_recovery_interval`: rate-limit IDR storms.
pub(super) fn should_force_recovery_idr(
    since_ship: Duration,
    since_ack: Duration,
    since_recovery: Duration,
    acks_suspended: bool,
    egfx_on_lossy: bool,
    p: &RecoveryParams,
) -> bool {
    egfx_on_lossy
        && !acks_suspended
        && since_ship <= p.active_window
        && since_ack >= p.ack_stall
        && since_recovery >= p.min_recovery_interval
}

/// Decide whether to de-migrate EGFX from the RELIABLE UDP tunnel back onto TCP.
/// Pure (Durations, not a clock) so it's unit-testable without timing.
///
/// The reliable (UdpFecR) tunnel is ordered, so it head-of-line-blocks under loss
/// exactly like TCP (feasibility finding #4): once the client stops acking while
/// we're *actively* shipping (the #89 trickle floor guarantees we keep shipping
/// even when the ack-lag is high), the tunnel is wedged and queued frames will
/// never arrive — the video freezes with no recovery until reconnect. The fix is
/// to route EGFX back over TCP, which mstsc accepts post-Soft-Sync (Spike A,
/// verified live 2026-06-29) — the caller pairs this with a forced IDR, since the
/// last UDP frames never arrived so the client's decode reference is stale.
///
/// Each clause guards a distinct failure mode:
/// - `egfx_on_udp && !egfx_on_lossy`: only on the RELIABLE UDP tunnel. The lossy
///   tunnel uses ack-driven IDR recovery instead ([`should_force_recovery_idr`]);
///   TCP needs nothing (socket backpressure paces it).
/// - `!already_demigrated`: fire once per connection (one-way latch, no flapping).
/// - `!acks_suspended`: with acks off (`queueDepth==0xFFFFFFFF`) a wedge can't be
///   inferred from ack-staleness.
/// - `since_ship <= active_window`: only while actively shipping (else: static
///   screen, where silent acks are normal and the periodic IDR backstops).
/// - `since_ack >= wedge_timeout`: the wedge signal — acks have gone fully silent
///   long enough to rule out a transient congestion blip.
#[allow(clippy::too_many_arguments)]
pub(super) fn should_demigrate_to_tcp(
    since_ship: Duration,
    since_ack: Duration,
    acks_suspended: bool,
    egfx_on_udp: bool,
    egfx_on_lossy: bool,
    already_demigrated: bool,
    active_window: Duration,
    wedge_timeout: Duration,
) -> bool {
    egfx_on_udp
        && !egfx_on_lossy
        && !already_demigrated
        && !acks_suspended
        && since_ship <= active_window
        && since_ack >= wedge_timeout
}
/// Read the ack-recovery config from the environment once. Returns
/// `(enabled, params)`; disabled (default) keeps the feature off and the path
/// byte-identical. Tunables: `MACRDP_UDP_EGFX_ACK_STALL_MS` (200),
/// `MACRDP_UDP_EGFX_ACK_ACTIVE_MS` (500), `MACRDP_UDP_EGFX_ACK_RECOVERY_MS` (1000).
pub(super) fn recovery_config_from_env() -> (bool, RecoveryParams) {
    let ms = |name: &str, default: u64| -> Duration {
        let v = crate::tunables::var(name)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(default);
        Duration::from_millis(v)
    };
    let enabled = crate::tunables::truthy("MACRDP_UDP_EGFX_ACK_RECOVERY");
    let params = RecoveryParams {
        active_window: ms("MACRDP_UDP_EGFX_ACK_ACTIVE_MS", 500),
        ack_stall: ms("MACRDP_UDP_EGFX_ACK_STALL_MS", 200),
        min_recovery_interval: ms("MACRDP_UDP_EGFX_ACK_RECOVERY_MS", 1000),
    };
    (enabled, params)
}
