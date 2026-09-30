//! End-to-end color fidelity of the H.264 path, without a client: pattern →
//! VideoToolbox (the production encoder) → Annex-B file → ffmpeg decode to raw
//! planes (no colour conversion) → full-range BT.709 → CIEDE2000 vs source.

use crate::color_metrics::{delta_e_2000, srgb8_to_lab, yuv709_full_to_rgb8};
use crate::color_pattern::{generate, Rect};

/// Parse `ffprobe -show_entries stream=width,height,pix_fmt -of default=noprint_wrappers=1`
/// output. Only 8-bit 4:2:0 planar output is measurable; anything else is refused.
pub fn parse_probe(out: &str) -> Result<(usize, usize), String> {
    let (mut w, mut h, mut fmt) = (None, None, None);
    for line in out.lines() {
        match line.split_once('=') {
            Some(("width", v)) => w = v.trim().parse::<usize>().ok(),
            Some(("height", v)) => h = v.trim().parse::<usize>().ok(),
            Some(("pix_fmt", v)) => fmt = Some(v.trim().to_string()),
            _ => {}
        }
    }
    match (w, h, fmt.as_deref()) {
        (Some(w), Some(h), Some("yuv420p" | "yuvj420p")) => Ok((w, h)),
        (_, _, Some(other)) if other != "yuv420p" && other != "yuvj420p" => {
            Err(format!("unsupported pix_fmt {other}; refusing to measure"))
        }
        _ => Err(format!("incomplete ffprobe output: {out:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_probe_accepts_8bit_420() {
        assert_eq!(
            parse_probe("width=1713\nheight=1288\npix_fmt=yuvj420p\n"),
            Ok((1713, 1288))
        );
        assert_eq!(
            parse_probe("pix_fmt=yuv420p\nwidth=1920\nheight=1080"),
            Ok((1920, 1080))
        );
    }

    #[test]
    fn parse_probe_refuses_other_formats() {
        let e = parse_probe("width=1920\nheight=1080\npix_fmt=yuv420p10le\n").unwrap_err();
        assert!(e.contains("yuv420p10le"), "{e}");
        assert!(parse_probe("width=1920\n").is_err());
    }
}

#[derive(Debug)]
pub struct RoundTripReport {
    pub width: usize,
    pub height: usize,
    pub patch_mean_de: f64,
    pub patch_max_de: f64,
    pub edge_mean_de: f64,
}

impl RoundTripReport {
    fn to_json(&self) -> String {
        format!(
            "{{\"width\":{},\"height\":{},\"patch_mean_de\":{:.3},\"patch_max_de\":{:.3},\"edge_mean_de\":{:.3}}}",
            self.width, self.height, self.patch_mean_de, self.patch_max_de, self.edge_mean_de
        )
    }
}

fn run(cmd: &mut std::process::Command) -> Result<Vec<u8>, String> {
    let out = cmd.output().map_err(|e| format!("spawn {cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{cmd:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(out.stdout)
}

fn mean_de(src: &[u8], dec: &[[u8; 3]], width: usize, r: &Rect) -> f64 {
    let mut sum = 0.0;
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let i = y * width + x;
            let s = [src[i * 4 + 2], src[i * 4 + 1], src[i * 4]];
            sum += delta_e_2000(srgb8_to_lab(s), srgb8_to_lab(dec[i]));
        }
    }
    sum / (r.w * r.h) as f64
}

pub fn run_roundtrip(width: usize, height: usize) -> Result<RoundTripReport, String> {
    let pattern = generate(width, height);
    let mut enc =
        crate::videotoolbox::Encoder::new(width as u16, height as u16, 60, 50_000_000, 2.0)
            .map_err(|e| e.to_string())?;
    // A static screen is re-sent as P-frames; measure the converged last frame.
    for i in 0..30 {
        enc.encode_bgra(&pattern.bgra, width * 4, i == 0)
            .map_err(|e| e.to_string())?;
    }
    let frames = enc.flush().map_err(|e| e.to_string())?;
    if frames.is_empty() {
        return Err("encoder produced no frames".into());
    }
    let mut annexb = Vec::new();
    for f in &frames {
        annexb.extend(crate::h264::avcc_to_annex_b(
            &f.data,
            &f.parameter_sets,
            f.is_keyframe,
        ));
    }
    let path = std::env::temp_dir().join(format!("macrdp-color-{width}x{height}.h264"));
    std::fs::write(&path, &annexb).map_err(|e| e.to_string())?;

    let probe = run(std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,pix_fmt",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(&path))?;
    let (dw, dh) = parse_probe(&String::from_utf8_lossy(&probe))?;
    if dw < width || dh < height {
        return Err(format!(
            "decoded {dw}x{dh} smaller than source {width}x{height}"
        ));
    }
    // No -pix_fmt: raw decoded planes, so ffmpeg performs no colour conversion.
    let raw = run(std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&path)
        .args(["-f", "rawvideo", "-"]))?;
    let (cw, ch) = (dw.div_ceil(2), dh.div_ceil(2));
    let frame_len = dw * dh + 2 * cw * ch;
    if raw.len() < frame_len || raw.len() % frame_len != 0 {
        return Err(format!(
            "raw output {} bytes is not a whole number of {frame_len}-byte frames",
            raw.len()
        ));
    }
    let last = &raw[raw.len() - frame_len..];
    let (yp, rest) = last.split_at(dw * dh);
    let (up, vp) = rest.split_at(cw * ch);

    // Crop to the source size (Review Focus #2).
    let mut dec = vec![[0u8; 3]; width * height];
    for y in 0..height {
        for x in 0..width {
            let ci = (y / 2) * cw + x / 2;
            dec[y * width + x] = yuv709_full_to_rgb8(yp[y * dw + x], up[ci], vp[ci]);
        }
    }

    let inset = |r: &Rect| Rect {
        x: r.x + 8,
        y: r.y + 8,
        w: r.w - 16,
        h: r.h - 16,
    };
    let des: Vec<f64> = pattern
        .patches
        .iter()
        .map(|p| mean_de(&pattern.bgra, &dec, width, &inset(&p.rect)))
        .collect();
    Ok(RoundTripReport {
        width,
        height,
        patch_mean_de: des.iter().sum::<f64>() / des.len() as f64,
        patch_max_de: des.iter().cloned().fold(0.0, f64::max),
        edge_mean_de: mean_de(&pattern.bgra, &dec, width, &pattern.edge),
    })
}

#[test]
#[ignore = "needs VideoToolbox + ffmpeg; run: cargo test --release color_roundtrip -- --ignored --nocapture"]
fn color_roundtrip_avc420_baseline() {
    // Widths are always even (MS-RDPEDISP forbids odd widths). 1714x1288 is the
    // windowed-client case: even, but not a multiple of 16.
    for (w, h) in [(1920, 1080), (1714, 1288), (1714, 1287)] {
        let report = run_roundtrip(w, h).unwrap_or_else(|e| panic!("{w}x{h}: {e}"));
        println!("COLOR_REPORT {}", report.to_json());
        // Sanity gate only: flat patches must survive AVC420 nearly intact.
        // Phase 2 tightens this to ΔE < 1 (AVC444) and bit-exact (refinement).
        assert!(report.patch_mean_de < 2.0, "{report:?}");
    }
}

/// Tightly packed (y, u, v) planes.
type Planes = (Vec<u8>, Vec<u8>, Vec<u8>);

/// Encode full-range I420 planes (`w×h`) 30× with a fresh encoder, decode the
/// last frame with ffmpeg; returns tightly packed (y, u, v) at `w×h`.
fn encode_decode_i420(
    tag: &str,
    y: &[u8],
    u: &[u8],
    v: &[u8],
    w: usize,
    h: usize,
) -> Result<Planes, String> {
    let mut enc = crate::videotoolbox::Encoder::new(w as u16, h as u16, 60, 50_000_000, 2.0)
        .map_err(|e| e.to_string())?;
    for i in 0..30 {
        enc.encode_yuv420(y, u, v, i == 0)
            .map_err(|e| e.to_string())?;
    }
    let frames = enc.flush().map_err(|e| e.to_string())?;
    let mut annexb = Vec::new();
    for f in &frames {
        annexb.extend(crate::h264::avcc_to_annex_b(
            &f.data,
            &f.parameter_sets,
            f.is_keyframe,
        ));
    }
    let path = std::env::temp_dir().join(format!("macrdp-avc444-{tag}-{w}x{h}.h264"));
    std::fs::write(&path, &annexb).map_err(|e| e.to_string())?;
    let probe = run(std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,pix_fmt",
        ])
        .args(["-of", "default=noprint_wrappers=1"])
        .arg(&path))?;
    let (dw, dh) = parse_probe(&String::from_utf8_lossy(&probe))?;
    let raw = run(std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&path)
        .args(["-f", "rawvideo", "-"]))?;
    let (cw, ch) = (dw.div_ceil(2), dh.div_ceil(2));
    let frame_len = dw * dh + 2 * cw * ch;
    if raw.len() < frame_len {
        return Err("no decoded frame".into());
    }
    let last = &raw[raw.len() - frame_len..];
    let (ydec, rest) = last.split_at(dw * dh);
    let (udec, vdec) = rest.split_at(cw * ch);
    let crop = |p: &[u8], pw: usize, ow: usize, oh: usize| -> Vec<u8> {
        (0..oh)
            .flat_map(|r| p[r * pw..r * pw + ow].to_vec())
            .collect()
    };
    Ok((
        crop(ydec, dw, w, h),
        crop(udec, cw, w / 2, h / 2),
        crop(vdec, cw, w / 2, h / 2),
    ))
}

/// AVC444 v1 through the production encoder: split → two H.264 streams →
/// decode → combine → CIEDE2000 vs source (spec §8.2).
pub fn run_roundtrip_avc444(width: usize, height: usize) -> Result<RoundTripReport, String> {
    use crate::avc444::*;
    let pattern = generate(width, height);
    let (y, u, v) = bgra_to_yuv444_full_bt709(&pattern.bgra, width * 4, width, height);
    let (cw, ch) = (width / 2, height / 2);
    let ah = padded_aux_height(height);
    let (mut my, mut mu, mut mv) = (
        vec![0u8; width * height],
        vec![0u8; cw * ch],
        vec![0u8; cw * ch],
    );
    let (mut ay, mut au, mut av) = (
        vec![0u8; width * ah],
        vec![0u8; cw * ah / 2],
        vec![0u8; cw * ah / 2],
    );
    split_yuv444_to_yuv420_v1(
        &y, &u, &v, width, width, width, &mut my, &mut mu, &mut mv, width, cw, cw, &mut ay,
        &mut au, &mut av, width, cw, cw, width, height,
    );
    let (dmy, dmu, dmv) = encode_decode_i420("main", &my, &mu, &mv, width, height)?;
    let (day, dau, dav) = encode_decode_i420("aux", &ay, &au, &av, width, ah)?;
    let (mut oy, mut ou, mut ov) = (
        vec![0u8; width * height],
        vec![0u8; width * height],
        vec![0u8; width * height],
    );
    combine_luma_to_yuv444(
        &dmy, &dmu, &dmv, width, cw, cw, &mut oy, &mut ou, &mut ov, width, width, width, width,
        height,
    );
    combine_chroma_v1_to_yuv444(
        &day, &dau, &dav, width, cw, cw, &mut ou, &mut ov, width, width, width, height,
    );
    let dec: Vec<[u8; 3]> = (0..width * height)
        .map(|i| yuv709_full_to_rgb8(oy[i], ou[i], ov[i]))
        .collect();
    let inset = |r: &Rect| Rect {
        x: r.x + 8,
        y: r.y + 8,
        w: r.w - 16,
        h: r.h - 16,
    };
    let des: Vec<f64> = pattern
        .patches
        .iter()
        .map(|p| mean_de(&pattern.bgra, &dec, width, &inset(&p.rect)))
        .collect();
    Ok(RoundTripReport {
        width,
        height,
        patch_mean_de: des.iter().sum::<f64>() / des.len() as f64,
        patch_max_de: des.iter().cloned().fold(0.0, f64::max),
        edge_mean_de: mean_de(&pattern.bgra, &dec, width, &pattern.edge),
    })
}

#[test]
#[ignore = "needs VideoToolbox + ffmpeg; run: cargo test --release color_roundtrip -- --ignored --nocapture"]
fn color_roundtrip_avc444() {
    let report = run_roundtrip_avc444(1920, 1080).unwrap_or_else(|e| panic!("{e}"));
    println!("COLOR_REPORT_AVC444 {}", report.to_json());
    assert!(report.patch_mean_de < 2.0, "{report:?}");
    // AVC420 baseline smears 1-px chroma edges to ΔE≈32; AVC444 must recover them.
    assert!(report.edge_mean_de < 1.0, "{report:?}"); // spec §1.3: ΔE < 1 in motion with AVC444
}
