//! Display sizing and scale rules (spec §7.2, docs/research/2026-09-29-hidpi-virtual-display.md).

/// Virtual displays are registered with an 8192×8192 maximum (MS-RDPBCGR limit).
pub const MAX_BACKING_PX: u32 = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientMonitor {
    pub width_px: u32,
    pub height_px: u32,
    /// Desktop scale factor in percent as sent by the client; 0 = not sent.
    pub desktop_scale_pct: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleMode {
    /// 1× mode at the client's pixel size.
    OneX,
    /// Retina mode whose backing equals the client's pixel size exactly.
    Retina,
    /// Retina mode captured down to the client's pixel size (GPU scaling).
    RetinaDownscaled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayPlan {
    /// Virtual display mode size in points.
    pub points_w: u32,
    pub points_h: u32,
    /// Select the Retina (2× backing) twin of the mode.
    pub hidpi: bool,
    /// Pixel size sent to the client (= the client monitor size).
    pub capture_w: u32,
    pub capture_h: u32,
    pub mode: ScaleMode,
    pub reason: String,
}

fn div_round(n: u32, d: u32) -> u32 {
    (n * 2 + d) / (2 * d)
}

pub fn plan_display(m: ClientMonitor) -> DisplayPlan {
    let (w, h) = (m.width_px, m.height_px);
    let one_x = |reason: String| DisplayPlan {
        points_w: w,
        points_h: h,
        hidpi: false,
        capture_w: w,
        capture_h: h,
        mode: ScaleMode::OneX,
        reason,
    };
    let scale = m.desktop_scale_pct;
    if scale == 0 {
        return one_x(format!("1× at {w}×{h}: client sent no scale factor"));
    }
    if scale < 113 {
        return one_x(format!("1× at {w}×{h}: client scale {scale}%"));
    }
    let (pw, ph) = if scale >= 188 {
        (w.div_ceil(2), h.div_ceil(2))
    } else {
        (div_round(w * 100, scale), div_round(h * 100, scale))
    };
    if 2 * pw > MAX_BACKING_PX || 2 * ph > MAX_BACKING_PX {
        return one_x(format!(
            "1× at {w}×{h}: Retina backing {}×{} for {scale}% would exceed {MAX_BACKING_PX} px",
            2 * pw,
            2 * ph
        ));
    }
    let exact = 2 * pw == w && 2 * ph == h;
    let mode = if exact {
        ScaleMode::Retina
    } else {
        ScaleMode::RetinaDownscaled
    };
    let note = if scale > 200 {
        " (macOS renders at most 2×; UI sized as 200%)"
    } else {
        ""
    };
    DisplayPlan {
        points_w: pw,
        points_h: ph,
        hidpi: true,
        capture_w: w,
        capture_h: h,
        mode,
        reason: format!(
            "Retina {pw}×{ph} pt ({}×{} px){} for client {w}×{h} at {scale}%{note}",
            2 * pw,
            2 * ph,
            if exact { "" } else { ", downscaled on capture" }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(w: u32, h: u32, s: u32) -> ClientMonitor {
        ClientMonitor {
            width_px: w,
            height_px: h,
            desktop_scale_pct: s,
        }
    }

    #[test]
    fn plan_display_100_percent_is_one_to_one() {
        let p = plan_display(m(1920, 1080, 100));
        assert_eq!(
            (p.points_w, p.points_h, p.hidpi, p.capture_w, p.capture_h),
            (1920, 1080, false, 1920, 1080)
        );
        assert_eq!(p.mode, ScaleMode::OneX);
    }

    #[test]
    fn plan_display_treats_missing_scale_as_100() {
        let p = plan_display(m(1714, 1287, 0));
        assert_eq!((p.points_w, p.points_h, p.hidpi), (1714, 1287, false));
        assert!(p.reason.contains("no scale"), "{}", p.reason);
    }

    #[test]
    fn plan_display_200_percent_is_pixel_exact_retina() {
        let p = plan_display(m(3840, 2160, 200));
        assert_eq!(
            (p.points_w, p.points_h, p.hidpi, p.capture_w, p.capture_h),
            (1920, 1080, true, 3840, 2160)
        );
        assert_eq!(p.mode, ScaleMode::Retina);
    }

    #[test]
    fn plan_display_150_percent_renders_retina_then_downscales() {
        let p = plan_display(m(3840, 2160, 150));
        assert_eq!(
            (p.points_w, p.points_h, p.hidpi, p.capture_w, p.capture_h),
            (2560, 1440, true, 3840, 2160)
        );
        assert_eq!(p.mode, ScaleMode::RetinaDownscaled);
    }

    #[test]
    fn plan_display_odd_height_at_150_rounds_points() {
        let p = plan_display(m(1714, 1287, 150));
        assert_eq!(
            (p.points_w, p.points_h, p.capture_w, p.capture_h),
            (1143, 858, 1714, 1287)
        );
    }

    #[test]
    fn plan_display_odd_height_at_200_is_downscaled_not_exact() {
        let p = plan_display(m(1714, 1287, 200));
        assert_eq!((p.points_w, p.points_h), (857, 644));
        assert_eq!(p.mode, ScaleMode::RetinaDownscaled);
    }

    #[test]
    fn plan_display_falls_back_when_backing_exceeds_max() {
        let p = plan_display(m(7680, 4320, 125));
        assert_eq!((p.points_w, p.points_h, p.hidpi), (7680, 4320, false));
        assert_eq!(p.mode, ScaleMode::OneX);
        assert!(p.reason.contains("8192"), "{}", p.reason);
    }

    #[test]
    fn plan_display_8k_at_200_fits() {
        let p = plan_display(m(7680, 4320, 200));
        assert_eq!((p.points_w, p.points_h, p.hidpi), (3840, 2160, true));
    }

    #[test]
    fn plan_display_scales_above_200_use_retina_at_half() {
        let p = plan_display(m(3840, 2160, 300));
        assert_eq!((p.points_w, p.points_h, p.hidpi), (1920, 1080, true));
        assert!(p.reason.contains("300%"), "{}", p.reason);
    }
}
