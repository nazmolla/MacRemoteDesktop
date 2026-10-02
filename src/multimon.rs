//! The multi-monitor layout of the current connection (Phase 4a), shared by
//! capture, the H.264 pipeline and input. `None` for single-monitor sessions,
//! which keep the existing single-display paths unchanged.

use std::sync::{Arc, Mutex};

use crate::sync_ext::LockExt;

/// One monitor: where the client shows it, its size, and the Mac display
/// that serves it. Index 0 is the primary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Monitor {
    /// Position in the client's output, relative to the top-left of all monitors.
    pub origin: (u32, u32),
    pub size: (u16, u16),
    pub display_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// Size of the client's whole desktop (the bounding box of all monitors).
    pub union: (u16, u16),
    pub monitors: Vec<Monitor>,
}

impl Layout {
    /// Build from client monitor rectangles (left, top, width, height), primary
    /// first, and the Mac display id for each.
    pub fn from_rects(rects: &[(i32, i32, u16, u16)], display_ids: &[u32]) -> Option<Self> {
        if rects.len() < 2 || rects.len() != display_ids.len() {
            return None;
        }
        let min_x = rects.iter().map(|r| r.0).min()?;
        let min_y = rects.iter().map(|r| r.1).min()?;
        let max_x = rects.iter().map(|r| r.0 + i32::from(r.2)).max()?;
        let max_y = rects.iter().map(|r| r.1 + i32::from(r.3)).max()?;
        let monitors = rects
            .iter()
            .zip(display_ids)
            .map(|(r, &id)| Monitor {
                origin: ((r.0 - min_x) as u32, (r.1 - min_y) as u32),
                size: (r.2, r.3),
                display_id: id,
            })
            .collect();
        Some(Self {
            union: (u16::try_from(max_x - min_x).ok()?, u16::try_from(max_y - min_y).ok()?),
            monitors,
        })
    }

    /// The monitor containing client point `(x, y)` and the point relative to
    /// that monitor's top-left; the nearest monitor's edge when outside all.
    pub fn locate(&self, x: f64, y: f64) -> (&Monitor, f64, f64) {
        let inside = |m: &Monitor| {
            let (ox, oy) = (f64::from(m.origin.0), f64::from(m.origin.1));
            x >= ox && y >= oy && x < ox + f64::from(m.size.0) && y < oy + f64::from(m.size.1)
        };
        let m = self.monitors.iter().find(|m| inside(m)).unwrap_or(&self.monitors[0]);
        let (ox, oy) = (f64::from(m.origin.0), f64::from(m.origin.1));
        let lx = (x - ox).clamp(0.0, f64::from(m.size.0) - 1.0);
        let ly = (y - oy).clamp(0.0, f64::from(m.size.1) - 1.0);
        (m, lx, ly)
    }
}

/// Shared cell; set at each connection's first display sync.
#[derive(Debug, Clone, Default)]
pub struct SharedLayout(Arc<Mutex<Option<Layout>>>);

impl SharedLayout {
    pub fn get(&self) -> Option<Layout> {
        self.0.lock_or_recover().clone()
    }

    pub fn set(&self, layout: Option<Layout>) {
        *self.0.lock_or_recover() = layout;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_from_side_by_side_monitors() {
        // Primary 2560x1440 at 0,0; second 1920x1080 to its left.
        let l = Layout::from_rects(&[(0, 0, 2560, 1440), (-1920, 0, 1920, 1080)], &[10, 11]).unwrap();
        assert_eq!(l.union, (4480, 1440));
        assert_eq!(l.monitors[0].origin, (1920, 0));
        assert_eq!(l.monitors[1].origin, (0, 0));
        let (m, x, y) = l.locate(100.0, 50.0);
        assert_eq!((m.display_id, x, y), (11, 100.0, 50.0));
        let (m, x, _) = l.locate(2000.0, 10.0);
        assert_eq!((m.display_id, x), (10, 80.0));
    }

    #[test]
    fn single_monitor_is_not_a_layout() {
        assert!(Layout::from_rects(&[(0, 0, 1920, 1080)], &[1]).is_none());
    }
}
