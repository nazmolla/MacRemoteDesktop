//! Deterministic BGRA test pattern for the color harness: the 24 ColorChecker
//! patches (sRGB) on a neutral background, plus a chroma-edge torture strip of
//! alternating 1-px red/blue columns (what 4:2:0 subsampling smears). Test-only.

/// X-Rite ColorChecker Classic, commonly published 8-bit sRGB values,
/// row-major (dark skin … black).
pub const COLORCHECKER_SRGB: [[u8; 3]; 24] = [
    [115, 82, 68],
    [194, 150, 130],
    [98, 122, 157],
    [87, 108, 67],
    [133, 128, 177],
    [103, 189, 170],
    [214, 126, 44],
    [80, 91, 166],
    [193, 90, 99],
    [94, 60, 108],
    [157, 188, 64],
    [224, 163, 46],
    [56, 61, 150],
    [70, 148, 73],
    [175, 54, 60],
    [231, 199, 31],
    [187, 86, 149],
    [8, 133, 161],
    [243, 243, 242],
    [200, 200, 200],
    [160, 160, 160],
    [122, 122, 121],
    [85, 85, 85],
    [52, 52, 52],
];

const BACKGROUND: [u8; 3] = [128, 128, 128];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

#[derive(Debug, Clone)]
pub struct Patch {
    pub rect: Rect,
    pub srgb: [u8; 3],
}

pub struct Pattern {
    pub width: usize,
    #[allow(dead_code)] // part of the pattern's public shape; not every harness reads it
    pub height: usize,
    /// Tightly packed BGRA, stride = width * 4.
    pub bgra: Vec<u8>,
    pub patches: Vec<Patch>,
    /// Region of alternating 1-px red/blue columns.
    pub edge: Rect,
}

fn fill(bgra: &mut [u8], width: usize, r: Rect, rgb: [u8; 3]) {
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let i = (y * width + x) * 4;
            bgra[i..i + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }
    }
}

/// Top two thirds: 6×4 ColorChecker grid. Bottom third: chroma-edge strip.
pub fn generate(width: usize, height: usize) -> Pattern {
    assert!(
        width >= 6 * 48 && height >= 3 * 64,
        "pattern too small: {width}x{height}"
    );
    let mut bgra = vec![0u8; width * height * 4];
    fill(
        &mut bgra,
        width,
        Rect {
            x: 0,
            y: 0,
            w: width,
            h: height,
        },
        BACKGROUND,
    );

    let grid_h = height * 2 / 3;
    let (cell_w, cell_h) = (width / 6, grid_h / 4);
    let margin = (cell_w.min(cell_h) / 8).max(4);
    let mut patches = Vec::with_capacity(24);
    for (i, &srgb) in COLORCHECKER_SRGB.iter().enumerate() {
        let (col, row) = (i % 6, i / 6);
        let rect = Rect {
            x: col * cell_w + margin,
            y: row * cell_h + margin,
            w: cell_w - 2 * margin,
            h: cell_h - 2 * margin,
        };
        fill(&mut bgra, width, rect, srgb);
        patches.push(Patch { rect, srgb });
    }

    let edge = Rect {
        x: margin,
        y: grid_h + margin,
        w: width - 2 * margin,
        h: height - grid_h - 2 * margin,
    };
    for y in edge.y..edge.y + edge.h {
        for x in edge.x..edge.x + edge.w {
            let rgb = if (x - edge.x).is_multiple_of(2) {
                [255, 0, 0]
            } else {
                [0, 0, 255]
            };
            let i = (y * width + x) * 4;
            bgra[i..i + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }
    }
    Pattern {
        width,
        height,
        bgra,
        patches,
        edge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(p: &Pattern, x: usize, y: usize) -> [u8; 3] {
        let i = (y * p.width + x) * 4;
        [p.bgra[i + 2], p.bgra[i + 1], p.bgra[i]]
    }

    #[test]
    fn patches_have_their_colorchecker_colors() {
        for (w, h) in [(1920, 1080), (1714, 1288)] {
            let p = generate(w, h);
            assert_eq!(p.bgra.len(), w * h * 4);
            assert_eq!(p.patches.len(), 24);
            for (i, patch) in p.patches.iter().enumerate() {
                assert_eq!(patch.srgb, COLORCHECKER_SRGB[i]);
                let r = &patch.rect;
                assert!(r.w >= 16 && r.h >= 16, "patch {i} too small: {r:?}");
                assert!(r.x + r.w <= w && r.y + r.h <= h);
                assert_eq!(pixel(&p, r.x, r.y), patch.srgb);
                assert_eq!(pixel(&p, r.x + r.w - 1, r.y + r.h - 1), patch.srgb);
            }
        }
    }

    #[test]
    fn edge_strip_alternates_red_and_blue_columns() {
        let p = generate(1714, 1288);
        let e = &p.edge;
        assert!(e.w >= 64 && e.h >= 16);
        assert_eq!(pixel(&p, e.x, e.y), [255, 0, 0]);
        assert_eq!(pixel(&p, e.x + 1, e.y), [0, 0, 255]);
        assert_eq!(pixel(&p, e.x + 2, e.y + e.h - 1), [255, 0, 0]);
    }

    #[test]
    #[should_panic(expected = "pattern too small")]
    fn rejects_tiny_sizes() {
        let _ = generate(100, 100);
    }
}
