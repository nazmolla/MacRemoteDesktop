//! Declare a no-reordering stream in the H.264 sequence parameter set.
//!
//! VideoToolbox never reorders frames (`AllowFrameReordering` is off) but its
//! SPS leaves `bitstream_restriction_flag` at 0. Without that restriction a
//! conforming decoder must assume reordering up to the level's DPB size and may
//! hold up to 16 frames before outputting the first one. Windows 11's RDP client
//! does exactly that: frames decode but are not presented (or acknowledged)
//! until enough later frames arrive, so an idle screen stays black (2026-10-01).
//! Windows' own RDP hosts set the restriction. This rewrites the SPS to add it
//! with `max_num_reorder_frames = 0`, so every decoder can output each frame at
//! once.
//!
//! The SPS is parsed only as far as the VUI's `bitstream_restriction_flag`, its
//! last syntax element. Anything unexpected (no VUI, scaling matrices, a
//! truncated header) returns `None` and the caller keeps the original.

/// `sps` is one SPS NAL unit (header byte first, no start code). Returns the
/// rewritten NAL, or `None` to keep the original.
pub(crate) fn declare_no_reordering(sps: &[u8]) -> Option<Vec<u8>> {
    if sps.first()? & 0x1f != 7 {
        return None;
    }
    let rbsp = unescape(&sps[1..]);
    let (flag_pos, max_num_ref_frames) = locate_restriction_flag(&rbsp)?;
    let mut at_flag = BitReader {
        data: &rbsp,
        pos: flag_pos,
    };
    if at_flag.bits(1)? == 1 {
        return None; // bitstream_restriction already present
    }

    let mut w = BitWriter::default();
    let mut c = BitReader::new(&rbsp);
    for _ in 0..flag_pos {
        w.bit(c.bits(1)? as u8);
    }
    w.bit(1); // bitstream_restriction_flag
    w.bit(1); // motion_vectors_over_pic_boundaries_flag
    w.ue(2); // max_bytes_per_pic_denom (spec default)
    w.ue(1); // max_bits_per_mb_denom (spec default)
    w.ue(16); // log2_max_mv_length_horizontal
    w.ue(16); // log2_max_mv_length_vertical
    w.ue(0); // max_num_reorder_frames
    w.ue(max_num_ref_frames.max(1)); // max_dec_frame_buffering
    w.bit(1); // rbsp_stop_one_bit
    let mut out = vec![sps[0]];
    out.extend(escape(&w.finish()));
    Some(out)
}

/// Bit position of `bitstream_restriction_flag` in an SPS RBSP, and the SPS's
/// `max_num_ref_frames`.
fn locate_restriction_flag(rbsp: &[u8]) -> Option<(usize, u32)> {
    let mut r = BitReader::new(rbsp);
    let profile_idc = r.bits(8)?;
    r.bits(16)?; // constraint flags + level_idc
    r.ue()?; // seq_parameter_set_id
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        if r.ue()? == 3 {
            r.bits(1)?; // separate_colour_plane_flag
        }
        r.ue()?; // bit_depth_luma_minus8
        r.ue()?; // bit_depth_chroma_minus8
        r.bits(1)?; // qpprime_y_zero_transform_bypass_flag
        if r.bits(1)? == 1 {
            return None; // seq_scaling_matrix_present_flag: not parsed
        }
    }
    r.ue()?; // log2_max_frame_num_minus4
    match r.ue()? {
        0 => {
            r.ue()?; // log2_max_pic_order_cnt_lsb_minus4
        }
        1 => {
            r.bits(1)?; // delta_pic_order_always_zero_flag
            r.se()?; // offset_for_non_ref_pic
            r.se()?; // offset_for_top_to_bottom_field
            for _ in 0..r.ue()? {
                r.se()?; // offset_for_ref_frame[i]
            }
        }
        _ => {}
    }
    let max_num_ref_frames = r.ue()?;
    r.bits(1)?; // gaps_in_frame_num_value_allowed_flag
    r.ue()?; // pic_width_in_mbs_minus1
    r.ue()?; // pic_height_in_map_units_minus1
    if r.bits(1)? == 0 {
        r.bits(1)?; // mb_adaptive_frame_field_flag
    }
    r.bits(1)?; // direct_8x8_inference_flag
    if r.bits(1)? == 1 {
        for _ in 0..4 {
            r.ue()?; // frame_crop_*_offset
        }
    }
    if r.bits(1)? == 0 {
        return None; // vui_parameters_present_flag
    }
    if r.bits(1)? == 1 && r.bits(8)? == 255 {
        r.bits(32)?; // sar_width, sar_height
    }
    if r.bits(1)? == 1 {
        r.bits(1)?; // overscan_appropriate_flag
    }
    if r.bits(1)? == 1 {
        r.bits(4)?; // video_format, video_full_range_flag
        if r.bits(1)? == 1 {
            r.bits(24)?; // colour_primaries, transfer, matrix
        }
    }
    if r.bits(1)? == 1 {
        r.ue()?; // chroma_sample_loc_type_top_field
        r.ue()?; // chroma_sample_loc_type_bottom_field
    }
    if r.bits(1)? == 1 {
        r.bits(32)?; // num_units_in_tick
        r.bits(32)?; // time_scale
        r.bits(1)?; // fixed_frame_rate_flag
    }
    let nal_hrd = r.bits(1)? == 1;
    if nal_hrd {
        skip_hrd(&mut r)?;
    }
    let vcl_hrd = r.bits(1)? == 1;
    if vcl_hrd {
        skip_hrd(&mut r)?;
    }
    if nal_hrd || vcl_hrd {
        r.bits(1)?; // low_delay_hrd_flag
    }
    r.bits(1)?; // pic_struct_present_flag
    Some((r.pos, max_num_ref_frames))
}

fn skip_hrd(r: &mut BitReader) -> Option<()> {
    let cpb_cnt = r.ue()? + 1;
    r.bits(8)?; // bit_rate_scale, cpb_size_scale
    for _ in 0..cpb_cnt {
        r.ue()?; // bit_rate_value_minus1
        r.ue()?; // cpb_size_value_minus1
        r.bits(1)?; // cbr_flag
    }
    r.bits(20)?; // four 5-bit length / delay fields
    Some(())
}

/// Remove emulation-prevention bytes (`00 00 03` → `00 00`).
fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &b in data {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Insert emulation-prevention bytes so no `00 00 0x` (x ≤ 3) appears.
fn escape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 4);
    let mut zeros = 0;
    for &b in data {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v: u64 = 0;
        for _ in 0..n {
            let byte = *self.data.get(self.pos / 8)?;
            v = (v << 1) | u64::from((byte >> (7 - self.pos % 8)) & 1);
            self.pos += 1;
        }
        u32::try_from(v).ok()
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bits(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let rest = self.bits(zeros)?;
        u32::try_from((1u64 << zeros) - 1 + u64::from(rest)).ok()
    }

    fn se(&mut self) -> Option<i64> {
        let k = i64::from(self.ue()?);
        Some(if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) })
    }
}

#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    nbits: usize,
}

impl BitWriter {
    fn bit(&mut self, b: u8) {
        if self.nbits.is_multiple_of(8) {
            self.out.push(0);
        }
        if b != 0 {
            let last = self.out.len() - 1;
            self.out[last] |= 0x80 >> (self.nbits % 8);
        }
        self.nbits += 1;
    }

    fn ue(&mut self, v: u32) {
        let x = u64::from(v) + 1;
        let len = 64 - x.leading_zeros();
        for _ in 1..len {
            self.bit(0);
        }
        for i in (0..len).rev() {
            self.bit(((x >> i) & 1) as u8);
        }
    }

    fn finish(self) -> Vec<u8> {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An SPS VideoToolbox emitted for a 1714×1287 Baseline stream (2026-10-01).
    const VT_SPS: [u8; 16] = [
        0x27, 0x42, 0x00, 0x32, 0xab, 0x40, 0x36, 0x01, 0x47, 0xc4, 0x4b, 0x37, 0x01, 0x01, 0x01,
        0x02,
    ];

    /// `(max_num_reorder_frames, max_dec_frame_buffering)` of a rewritten SPS.
    fn restriction(sps: &[u8]) -> (u32, u32) {
        let rbsp = unescape(&sps[1..]);
        let (pos, _) = locate_restriction_flag(&rbsp).expect("parses");
        let mut r = BitReader { data: &rbsp, pos };
        assert_eq!(r.bits(1), Some(1), "bitstream_restriction_flag");
        assert_eq!(
            r.bits(1),
            Some(1),
            "motion_vectors_over_pic_boundaries_flag"
        );
        assert_eq!(
            (r.ue(), r.ue(), r.ue(), r.ue()),
            (Some(2), Some(1), Some(16), Some(16))
        );
        let fields = (r.ue().expect("reorder"), r.ue().expect("buffering"));
        assert_eq!(r.bits(1), Some(1), "rbsp_stop_one_bit");
        fields
    }

    #[test]
    fn rewrites_videotoolbox_sps() {
        let out = declare_no_reordering(&VT_SPS).expect("rewritten");
        assert_eq!(out[0], VT_SPS[0], "NAL header kept");
        assert!(out.len() > VT_SPS.len());
        assert_eq!(restriction(&out), (0, 1));
        assert!(
            declare_no_reordering(&out).is_none(),
            "already declared: left alone"
        );
    }

    #[test]
    fn keeps_everything_before_the_flag() {
        let out = declare_no_reordering(&VT_SPS).expect("rewritten");
        let a = unescape(&VT_SPS[1..]);
        let b = unescape(&out[1..]);
        let (pos, _) = locate_restriction_flag(&a).expect("parses");
        assert_eq!(&a[..pos / 8], &b[..pos / 8]);
    }

    #[test]
    fn ignores_non_sps_and_garbage() {
        assert!(declare_no_reordering(&[0x28, 0xce, 0x3c, 0x80]).is_none()); // PPS
        assert!(declare_no_reordering(&[0x27]).is_none());
        assert!(declare_no_reordering(&[0x27, 0x42, 0x00]).is_none());
    }

    #[test]
    fn escape_round_trips() {
        let raw = [0u8, 0, 1, 0, 0, 0, 0, 0, 3, 5];
        assert_eq!(unescape(&escape(&raw)), raw);
        assert!(!escape(&raw)
            .windows(3)
            .any(|w| w[0] == 0 && w[1] == 0 && w[2] <= 2));
    }
}
