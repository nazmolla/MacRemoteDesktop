//! Video codec ladder (spec §8, Phase 2a Task 6): pick the best codec the
//! client's EGFX CapabilitiesAdvertise allows — AVC444, then AVC420, then
//! legacy bitmaps — and whether to follow it with lossless refinement of
//! static regions. Pure: the caller logs `reason` once per connection.
// Not wired into the EGFX path yet (h264.rs integration is separate work).
#![allow(dead_code)]

use ironrdp_egfx::pdu::{
    CapabilitiesV103Flags, CapabilitiesV104Flags, CapabilitiesV107Flags, CapabilitiesV10Flags,
    CapabilitiesV81Flags, CapabilitySet,
};

/// What the client's EGFX CapabilitiesAdvertise implies about video decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VideoCaps {
    /// The client opened the graphics pipeline and advertised capsets.
    pub egfx: bool,
    /// The client can decode AVC420 (H.264 4:2:0) surfaces.
    pub avc420: bool,
    /// The client can decode AVC444 (H.264 4:4:4 as main + aux streams).
    pub avc444: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264 4:4:4 over EGFX (full chroma).
    Avc444,
    /// H.264 4:2:0 over EGFX.
    Avc420,
    /// Legacy BitmapUpdate — no EGFX pipeline in use.
    Bitmaps,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPlan {
    pub codec: VideoCodec,
    /// Send lossless tiles over regions that stop changing (EGFX only).
    pub lossless_refinement: bool,
    pub reason: String,
}

/// Derive [`VideoCaps`] from the capsets in a client's CapabilitiesAdvertise.
///
/// Follows `h264.rs::caps_indicate_avc`: AVC needs a POSITIVE signal — V8.1
/// with `AVC420_ENABLED`, or a flagged V10+ capset without `AVC_DISABLED`.
/// Bare `V8` / `V10_1` carry no AVC flag and count as no signal (upstream
/// treats `V10_1` as AVC-capable; we stay conservative, as h264.rs does).
/// AVC444 requires a flagged V10+ capset without `AVC_DISABLED`; V8.1 is
/// AVC420-only. An empty advertise is treated as no EGFX.
pub fn caps_from_egfx(caps: &[CapabilitySet]) -> VideoCaps {
    let mut out = VideoCaps {
        egfx: !caps.is_empty(),
        ..VideoCaps::default()
    };
    for c in caps {
        let (avc420, avc444) = match c {
            CapabilitySet::V8_1 { flags } => {
                (flags.contains(CapabilitiesV81Flags::AVC420_ENABLED), false)
            }
            CapabilitySet::V10 { flags } | CapabilitySet::V10_2 { flags } => {
                let on = !flags.contains(CapabilitiesV10Flags::AVC_DISABLED);
                (on, on)
            }
            CapabilitySet::V10_3 { flags } => {
                let on = !flags.contains(CapabilitiesV103Flags::AVC_DISABLED);
                (on, on)
            }
            CapabilitySet::V10_4 { flags }
            | CapabilitySet::V10_5 { flags }
            | CapabilitySet::V10_6 { flags }
            | CapabilitySet::V10_6Err { flags } => {
                let on = !flags.contains(CapabilitiesV104Flags::AVC_DISABLED);
                (on, on)
            }
            CapabilitySet::V10_7 { flags } => {
                let on = !flags.contains(CapabilitiesV107Flags::AVC_DISABLED);
                (on, on)
            }
            CapabilitySet::V8 { .. } | CapabilitySet::V10_1 => (false, false),
        };
        out.avc420 |= avc420;
        out.avc444 |= avc444;
    }
    out
}

/// Choose the codec and refinement policy for a connection.
pub fn choose(caps: VideoCaps) -> VideoPlan {
    if !caps.egfx {
        return VideoPlan {
            codec: VideoCodec::Bitmaps,
            lossless_refinement: false,
            reason: "legacy bitmaps: client did not advertise the EGFX graphics pipeline".into(),
        };
    }
    if caps.avc444 {
        return VideoPlan {
            codec: VideoCodec::Avc444,
            lossless_refinement: true,
            reason: "AVC444 + lossless refinement: client advertised EGFX with AVC444 decode"
                .into(),
        };
    }
    if caps.avc420 {
        return VideoPlan {
            codec: VideoCodec::Avc420,
            lossless_refinement: true,
            reason:
                "AVC420 + lossless refinement: client advertised EGFX with AVC420 only (no AVC444)"
                    .into(),
        };
    }
    VideoPlan {
        codec: VideoCodec::Bitmaps,
        lossless_refinement: false,
        reason: "legacy bitmaps: client advertised EGFX without AVC (e.g. AVC_DISABLED on every \
                 capset); the graphics pipeline is declined"
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(egfx: bool, avc420: bool, avc444: bool) -> VideoCaps {
        VideoCaps {
            egfx,
            avc420,
            avc444,
        }
    }

    #[test]
    fn no_egfx_is_bitmaps_without_refinement() {
        let p = choose(VideoCaps::default());
        assert_eq!(p.codec, VideoCodec::Bitmaps);
        assert!(!p.lossless_refinement);
        assert!(p.reason.contains("did not advertise"));
    }

    #[test]
    fn avc444_wins_with_refinement() {
        let p = choose(caps(true, true, true));
        assert_eq!(p.codec, VideoCodec::Avc444);
        assert!(p.lossless_refinement);
        assert!(!p.reason.is_empty());
    }

    #[test]
    fn avc420_only_gets_refinement() {
        let p = choose(caps(true, true, false));
        assert_eq!(p.codec, VideoCodec::Avc420);
        assert!(p.lossless_refinement);
        assert!(p.reason.contains("AVC420"));
    }

    #[test]
    fn egfx_without_avc_falls_back_to_bitmaps() {
        let p = choose(caps(true, false, false));
        assert_eq!(p.codec, VideoCodec::Bitmaps);
        assert!(!p.lossless_refinement);
        assert!(p.reason.contains("declined"));
    }

    #[test]
    fn avc_flags_without_egfx_still_bitmaps() {
        let p = choose(caps(false, true, true));
        assert_eq!(p.codec, VideoCodec::Bitmaps);
        assert!(!p.lossless_refinement);
    }

    #[test]
    fn derive_avc_disabled_everywhere_is_no_avc() {
        // Windows App for Android / decoder-less FreeRDP signature.
        let adv = [
            CapabilitySet::V8 {
                flags: ironrdp_egfx::pdu::CapabilitiesV8Flags::empty(),
            },
            CapabilitySet::V10 {
                flags: CapabilitiesV10Flags::AVC_DISABLED,
            },
            CapabilitySet::V10_1,
            CapabilitySet::V10_4 {
                flags: CapabilitiesV104Flags::AVC_DISABLED,
            },
            CapabilitySet::V10_7 {
                flags: CapabilitiesV107Flags::AVC_DISABLED,
            },
        ];
        let c = caps_from_egfx(&adv);
        assert_eq!(c, caps(true, false, false));
        assert_eq!(choose(c).codec, VideoCodec::Bitmaps);
    }

    #[test]
    fn derive_v81_avc420_enabled_is_avc420_only() {
        let adv = [CapabilitySet::V8_1 {
            flags: CapabilitiesV81Flags::AVC420_ENABLED,
        }];
        let c = caps_from_egfx(&adv);
        assert_eq!(c, caps(true, true, false));
        assert_eq!(choose(c).codec, VideoCodec::Avc420);
    }

    #[test]
    fn derive_v10_without_avc_disabled_is_avc444() {
        let adv = [
            CapabilitySet::V8_1 {
                flags: CapabilitiesV81Flags::empty(),
            },
            CapabilitySet::V10_7 {
                flags: CapabilitiesV107Flags::empty(),
            },
        ];
        let c = caps_from_egfx(&adv);
        assert_eq!(c, caps(true, true, true));
        assert_eq!(choose(c).codec, VideoCodec::Avc444);
    }

    #[test]
    fn derive_empty_advertise_is_no_egfx() {
        assert_eq!(caps_from_egfx(&[]), VideoCaps::default());
    }
}
