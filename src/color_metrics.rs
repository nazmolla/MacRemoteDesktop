//! Color-fidelity metrics for the test harness: sRGB → CIELAB (D65),
//! CIEDE2000, and full-range BT.709 YUV ↔ RGB (the interpretation mstsc
//! applies to AVC420 luma/chroma). Test-only.

/// sRGB 8-bit → CIELAB, D65 white.
pub fn srgb8_to_lab(rgb: [u8; 3]) -> [f64; 3] {
    fn lin(c: u8) -> f64 {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    fn f(t: f64) -> f64 {
        const D: f64 = 6.0 / 29.0;
        if t > D * D * D {
            t.cbrt()
        } else {
            t / (3.0 * D * D) + 4.0 / 29.0
        }
    }
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
    let z = 0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b;
    let (fx, fy, fz) = (f(x / 0.950_47), f(y), f(z / 1.088_83));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 colour difference (kL = kC = kH = 1).
pub fn delta_e_2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let (l1, a1, b1) = (lab1[0], lab1[1], lab1[2]);
    let (l2, a2, b2) = (lab2[0], lab2[1], lab2[2]);
    let pow25_7 = 25f64.powi(7);
    let c_bar = ((a1 * a1 + b1 * b1).sqrt() + (a2 * a2 + b2 * b2).sqrt()) / 2.0;
    let g = 0.5 * (1.0 - (c_bar.powi(7) / (c_bar.powi(7) + pow25_7)).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = ((a1p * a1p + b1 * b1).sqrt(), (a2p * a2p + b2 * b2).sqrt());
    let hue = |b: f64, ap: f64| {
        if b == 0.0 && ap == 0.0 {
            0.0
        } else {
            let h = b.atan2(ap).to_degrees();
            if h < 0.0 {
                h + 360.0
            } else {
                h
            }
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let zero_chroma = c1p * c2p == 0.0;
    let dhp = if zero_chroma {
        0.0
    } else {
        let d = h2p - h1p;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let dhp_big = 2.0 * (c1p * c2p).sqrt() * (dhp.to_radians() / 2.0).sin();
    let lp_bar = (l1 + l2) / 2.0;
    let cp_bar = (c1p + c2p) / 2.0;
    let hp_bar = if zero_chroma {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hp_bar - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hp_bar).to_radians().cos()
        + 0.32 * (3.0 * hp_bar + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hp_bar - 63.0).to_radians().cos();
    let d_theta = 30.0 * (-((hp_bar - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cp_bar.powi(7) / (cp_bar.powi(7) + pow25_7)).sqrt();
    let l50 = (lp_bar - 50.0).powi(2);
    let sl = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let sc = 1.0 + 0.045 * cp_bar;
    let sh = 1.0 + 0.015 * cp_bar * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;
    let (tl, tc, th) = (dlp / sl, dcp / sc, dhp_big / sh);
    (tl * tl + tc * tc + th * th + rt * tc * th).sqrt()
}

fn clamp_u8(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// Full-range BT.709 Y'CbCr → R'G'B' (8-bit).
pub fn yuv709_full_to_rgb8(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let (y, cb, cr) = (f64::from(y), f64::from(cb) - 128.0, f64::from(cr) - 128.0);
    [
        clamp_u8(y + 1.5748 * cr),
        clamp_u8(y - 0.187_324 * cb - 0.468_124 * cr),
        clamp_u8(y + 1.8556 * cb),
    ]
}

/// Full-range BT.709 R'G'B' (8-bit) → Y'CbCr. Used only by tests to check the
/// inverse above.
pub fn rgb8_to_yuv709_full(rgb: [u8; 3]) -> [u8; 3] {
    let (r, g, b) = (f64::from(rgb[0]), f64::from(rgb[1]), f64::from(rgb[2]));
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    [
        clamp_u8(y),
        clamp_u8((b - y) / 1.8556 + 128.0),
        clamp_u8((r - y) / 1.5748 + 128.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn lab_of_white_black_red() {
        let w = srgb8_to_lab([255, 255, 255]);
        assert!(
            close(w[0], 100.0, 0.01) && close(w[1], 0.0, 0.01) && close(w[2], 0.0, 0.01),
            "{w:?}"
        );
        let k = srgb8_to_lab([0, 0, 0]);
        assert!(close(k[0], 0.0, 0.01), "{k:?}");
        let r = srgb8_to_lab([255, 0, 0]);
        assert!(
            close(r[0], 53.24, 0.05) && close(r[1], 80.09, 0.05) && close(r[2], 67.20, 0.05),
            "{r:?}"
        );
    }

    // Reference pairs from Sharma, Wu & Dalal (2005), "The CIEDE2000
    // Color-Difference Formula: Implementation Notes", Table 1.
    #[test]
    fn ciede2000_matches_sharma_reference_pairs() {
        let cases = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
            ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
            (
                [60.2574, -34.0099, 36.2677],
                [60.4626, -34.1751, 39.4387],
                1.2644,
            ),
        ];
        for (a, b, want) in cases {
            let got = delta_e_2000(a, b);
            assert!(
                close(got, want, 1e-4),
                "{a:?} vs {b:?}: got {got}, want {want}"
            );
            assert!(
                close(delta_e_2000(b, a), want, 1e-4),
                "not symmetric for {a:?}"
            );
        }
        assert_eq!(delta_e_2000([42.0, 10.0, -5.0], [42.0, 10.0, -5.0]), 0.0);
    }

    #[test]
    fn yuv709_full_range_endpoints() {
        assert_eq!(yuv709_full_to_rgb8(255, 128, 128), [255, 255, 255]);
        assert_eq!(yuv709_full_to_rgb8(0, 128, 128), [0, 0, 0]);
        assert_eq!(yuv709_full_to_rgb8(128, 128, 128), [128, 128, 128]);
    }

    #[test]
    fn yuv709_full_range_round_trip_within_two_levels() {
        for rgb in [
            [115u8, 82, 68],
            [98, 122, 157],
            [214, 126, 44],
            [8, 133, 161],
            [200, 200, 200],
        ] {
            let [y, cb, cr] = rgb8_to_yuv709_full(rgb);
            let back = yuv709_full_to_rgb8(y, cb, cr);
            for c in 0..3 {
                assert!(
                    (i16::from(back[c]) - i16::from(rgb[c])).abs() <= 2,
                    "{rgb:?} -> {back:?}"
                );
            }
        }
    }
}
