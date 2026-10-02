//! Surface regions: which part of the client surface a frame updates, and
//! the regions still owed to the client when a capture is dropped.

use super::*;

/// Which part of the client surface a submitted frame updates (MS-RDPEGFX
/// AVC420 region rects). Areas outside the regions keep what the client has —
/// which is what lets lossless refinement survive later frames (spec §8.3).
#[derive(Debug, Clone)]
pub(crate) enum FrameRegions {
    /// Whole surface.
    Full,
    /// Only these rectangles changed.
    Rects(Vec<crate::refine::Rect>),
    /// Same pixels as the previous submit (flush frames): reuse its regions.
    SameAsLast,
}
/// Regions submitted but not yet carried by an encoded frame. A dropped
/// capture's change must ride on the next encoded frame (review I1).
#[derive(Debug, Clone, Default)]
pub(super) enum RegionDebt {
    #[default]
    None,
    Full,
    Rects(Vec<crate::refine::Rect>),
}

impl RegionDebt {
    /// Add a frame's regions; `None` = whole surface.
    pub(super) fn add(&mut self, rects: Option<&[crate::refine::Rect]>) {
        *self = match (std::mem::take(self), rects) {
            (Self::Full, _) | (_, None) => Self::Full,
            (Self::None, Some(r)) => Self::Rects(r.to_vec()),
            (Self::Rects(mut v), Some(r)) => {
                v.extend_from_slice(r);
                if v.len() > 4 * MAX_AVC_REGIONS {
                    Self::Full
                } else {
                    Self::Rects(v)
                }
            }
        };
    }

    /// Regions for the frame being encoded now (`None` = whole surface).
    pub(super) fn take(&mut self) -> Option<Vec<crate::refine::Rect>> {
        match std::mem::take(self) {
            Self::Full => None,
            Self::None => Some(Vec::new()),
            Self::Rects(v) => Some(v),
        }
    }
}

/// Cap before collapsing a frame's regions into their bounding box.
pub(super) const MAX_AVC_REGIONS: usize = 16;
/// AVC420 regions for one shipped frame: the queued dirty rects (clipped,
/// inclusive edges), the bounding box beyond [`MAX_AVC_REGIONS`], or the whole
/// surface when `rects` is `None`/empty.
pub(super) fn avc_regions(
    rects: Option<&[crate::refine::Rect]>,
    width: u16,
    height: u16,
) -> Vec<Avc420Region> {
    let region = |l: u32, t: u32, r: u32, b: u32| Avc420Region {
        left: l as u16,
        top: t as u16,
        right: r as u16,
        bottom: b as u16,
        quantization_parameter: 22,
        quality: 100,
    };
    let (w, h) = (u32::from(width), u32::from(height));
    let full = || vec![region(0, 0, w.saturating_sub(1), h.saturating_sub(1))];
    let Some(rects) = rects.filter(|r| !r.is_empty()) else {
        return full();
    };
    let mut clipped: Vec<(u32, u32, u32, u32)> = rects
        .iter()
        .filter(|r| r.w > 0 && r.h > 0 && r.x < w && r.y < h)
        .map(|r| (r.x, r.y, (r.x + r.w).min(w) - 1, (r.y + r.h).min(h) - 1))
        .collect();
    if clipped.is_empty() {
        return full();
    }
    // Clients reject a frame whose regions overlap (FreeRDP aborts the decode),
    // and ScreenCaptureKit's dirty rects can overlap: merge overlaps into their union.
    merge_overlaps(&mut clipped);
    if clipped.len() > MAX_AVC_REGIONS {
        let l = clipped.iter().map(|c| c.0).min().unwrap_or(0);
        let t = clipped.iter().map(|c| c.1).min().unwrap_or(0);
        let r = clipped.iter().map(|c| c.2).max().unwrap_or(0);
        let b = clipped.iter().map(|c| c.3).max().unwrap_or(0);
        return vec![region(l, t, r, b)];
    }
    clipped
        .into_iter()
        .map(|(l, t, r, b)| region(l, t, r, b))
        .collect()
}

/// Replace overlapping inclusive rects `(l, t, r, b)` by their union until none overlap.
fn merge_overlaps(rects: &mut Vec<(u32, u32, u32, u32)>) {
    let overlap = |a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)| {
        a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
    };
    'again: loop {
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                if overlap(rects[i], rects[j]) {
                    let b = rects.swap_remove(j);
                    let a = &mut rects[i];
                    *a = (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3));
                    continue 'again;
                }
            }
        }
        return;
    }
}

#[cfg(test)]
mod avc_region_tests {
    use super::avc_regions;
    use crate::refine::Rect;

    #[test]
    fn none_or_empty_means_whole_surface() {
        for rs in [None, Some(&[][..])] {
            let v = avc_regions(rs, 1714, 1287);
            assert_eq!((v.len(), v[0].right, v[0].bottom), (1, 1713, 1286));
        }
    }

    #[test]
    fn rects_are_clipped_with_inclusive_edges() {
        let v = avc_regions(
            Some(&[Rect {
                x: 1700,
                y: 10,
                w: 100,
                h: 5,
            }]),
            1714,
            1287,
        );
        assert_eq!(
            (v[0].left, v[0].top, v[0].right, v[0].bottom),
            (1700, 10, 1713, 14)
        );
    }

    #[test]
    fn many_rects_collapse_to_bounding_box() {
        let rs: Vec<Rect> = (0..20)
            .map(|i| Rect {
                x: i * 10,
                y: i,
                w: 5,
                h: 5,
            })
            .collect();
        let v = avc_regions(Some(&rs), 1920, 1080);
        assert_eq!(
            (v.len(), v[0].left, v[0].top, v[0].right, v[0].bottom),
            (1, 0, 0, 194, 23)
        );
    }

    #[test]
    fn overlapping_rects_are_merged() {
        let r = |x, y, w, h| Rect { x, y, w, h };
        let v = avc_regions(
            Some(&[r(0, 0, 100, 100), r(50, 50, 100, 100), r(500, 500, 10, 10)]),
            1920,
            1080,
        );
        assert_eq!(v.len(), 2);
        assert!(v.iter().any(|g| (g.left, g.top, g.right, g.bottom) == (0, 0, 149, 149)));
        let touching = avc_regions(Some(&[r(0, 0, 10, 10), r(10, 0, 10, 10)]), 1920, 1080);
        assert_eq!(touching.len(), 2);
    }
}
