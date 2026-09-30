//! How H.264 NAL units are framed in the AVC420 wire payload.

use super::*;

/// How the H.264 NAL units are framed inside the AVC420 wire payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WireFormat {
    /// 4-byte big-endian length prefix per NAL (VideoToolbox's native AVCC).
    /// ironrdp's decoder documents this as the expected format.
    LengthPrefixed,
    /// `00 00 00 01` start codes (historical Windows/FreeRDP convention).
    AnnexB,
}

impl WireFormat {
    /// Annex-B is the verified-correct framing for Microsoft's decoder
    /// (mstsc renders the desktop with it; length-prefixed AVCC gets ZERO
    /// frame-acks and a blank surface — confirmed empirically 2026-05-20).
    /// Default to Annex-B; keep length-prefixed one env var away
    /// (`MACRDP_H264_LENGTH_PREFIXED=1`) for ironrdp-decoder interop testing.
    /// The legacy `MACRDP_H264_ANNEXB=1` is still accepted (now a no-op since
    /// Annex-B is the default).
    pub(super) fn from_env() -> Self {
        match crate::tunables::var("MACRDP_H264_LENGTH_PREFIXED") {
            Ok(v) if v == "1" || v.eq_ignore_ascii_case("true") => Self::LengthPrefixed,
            _ => Self::AnnexB,
        }
    }
}
/// Rewrite AVCC (4-byte length-prefixed NALs) to Annex-B (`00 00 00 01` start
/// codes), prepending SPS/PPS on keyframes. Only used when `MACRDP_H264_ANNEXB`
/// selects Annex-B framing.
pub(crate) fn avcc_to_annex_b(
    avcc: &[u8],
    parameter_sets: &[Vec<u8>],
    is_keyframe: bool,
) -> Vec<u8> {
    const START_CODE: [u8; 4] = [0, 0, 0, 1];
    let mut out = Vec::with_capacity(avcc.len() + 64);

    if is_keyframe {
        for ps in parameter_sets {
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(ps);
        }
    }

    let mut i = 0;
    while i + 4 <= avcc.len() {
        let nal_len = u32::from_be_bytes([avcc[i], avcc[i + 1], avcc[i + 2], avcc[i + 3]]) as usize;
        i += 4;
        if i + nal_len > avcc.len() {
            warn!(
                avcc_len = avcc.len(),
                offset = i,
                nal_len,
                "AVCC NAL length overflows buffer; truncating"
            );
            break;
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&avcc[i..i + nal_len]);
        i += nal_len;
    }
    out
}
