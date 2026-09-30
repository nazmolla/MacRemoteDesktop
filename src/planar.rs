//! Lossless RDP6 Planar encoding of one surface region, for EGFX
//! `send_planar_frame` refinement (spec §8.3, Phase 2a Task 2).

use anyhow::{bail, Result};
use ironrdp_graphics::rdp6::{BgrAChannels, BitmapStreamEncoder};

use crate::refine::Rect;

/// Encode `rect` of a tightly strided BGRA framebuffer (`stride` bytes/row,
/// `height` rows) as an RLE-compressed Planar bitmap stream.
pub fn encode_region(bgra: &[u8], stride: usize, height: u32, rect: Rect) -> Result<Vec<u8>> {
    let width_px = (stride / 4) as u32;
    if rect.w == 0 || rect.h == 0 || rect.x + rect.w > width_px || rect.y + rect.h > height {
        bail!("planar region {rect:?} outside {width_px}×{height} surface");
    }
    let (w, h) = (rect.w as usize, rect.h as usize);
    let mut pixels = Vec::with_capacity(w * h * 4);
    for row in rect.y as usize..rect.y as usize + h {
        let start = row * stride + rect.x as usize * 4;
        pixels.extend_from_slice(&bgra[start..start + w * 4]);
    }
    // Worst case (RLE off, no gain): header + 3 planes of w*h, plus slack.
    let mut out = vec![0u8; w * h * 4 + 64];
    let n = BitmapStreamEncoder::new(w, h)
        .encode_bitmap::<BgrAChannels>(&pixels, &mut out, true)
        .map_err(|e| anyhow::anyhow!("planar encode failed: {e:?}"))?;
    out.truncate(n);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp_graphics::rdp6::BitmapStreamDecoder;

    fn decode(data: &[u8], w: usize, h: usize) -> Vec<u8> {
        let mut rgb = Vec::new();
        BitmapStreamDecoder::default()
            .decode_bitmap_stream_to_rgb24(data, &mut rgb, w, h)
            .expect("decode");
        rgb
    }

    fn assert_exact(bgra: &[u8], stride: usize, r: Rect, rgb: &[u8]) {
        for y in 0..r.h as usize {
            for x in 0..r.w as usize {
                let s = (r.y as usize + y) * stride + (r.x as usize + x) * 4;
                let d = (y * r.w as usize + x) * 3;
                assert_eq!(
                    [bgra[s + 2], bgra[s + 1], bgra[s]],
                    [rgb[d], rgb[d + 1], rgb[d + 2]],
                    "pixel {x},{y}"
                );
            }
        }
    }

    #[test]
    fn roundtrip_is_bit_exact_on_colorchecker_pattern() {
        let p = crate::color_pattern::generate(1714, 1288);
        let r = Rect {
            x: 10,
            y: 10,
            w: 200,
            h: 150,
        };
        let enc = encode_region(&p.bgra, p.width * 4, p.height as u32, r).unwrap();
        assert_exact(&p.bgra, p.width * 4, r, &decode(&enc, 200, 150));
        let e = p.edge;
        let r = Rect {
            x: e.x as u32,
            y: e.y as u32,
            w: 64,
            h: 16,
        };
        let enc = encode_region(&p.bgra, p.width * 4, p.height as u32, r).unwrap();
        assert_exact(&p.bgra, p.width * 4, r, &decode(&enc, 64, 16));
    }

    #[test]
    fn roundtrip_odd_sized_rect() {
        let (w, h) = (37usize, 21usize);
        let bgra: Vec<u8> = (0..w * h * 4).map(|i| (i * 31 % 251) as u8).collect();
        let r = Rect {
            x: 3,
            y: 5,
            w: 13,
            h: 7,
        };
        let enc = encode_region(&bgra, w * 4, h as u32, r).unwrap();
        assert_exact(&bgra, w * 4, r, &decode(&enc, 13, 7));
    }

    #[test]
    fn rect_outside_surface_is_an_error() {
        let bgra = vec![0u8; 16 * 16 * 4];
        assert!(encode_region(
            &bgra,
            64,
            16,
            Rect {
                x: 10,
                y: 0,
                w: 10,
                h: 4
            }
        )
        .is_err());
        assert!(encode_region(
            &bgra,
            64,
            16,
            Rect {
                x: 0,
                y: 14,
                w: 4,
                h: 4
            }
        )
        .is_err());
    }
}
