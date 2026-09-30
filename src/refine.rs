//! Static-region tracker for lossless refinement (spec §8.3, Phase 2a Task 1).
//!
//! The surface is split into 64×64 tiles. A tile dirtied by capture and then
//! left unchanged for `idle` becomes ready to be re-sent losslessly exactly
//! once. Pure bookkeeping — no pixels, no I/O; state is one entry per tile.

use std::time::{Duration, Instant};

const TILE: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Copy)]
enum TileState {
    /// Unchanged since the last refinement (or never touched).
    Clean,
    /// Changed at this instant; not yet refined.
    Dirty(Instant),
}

pub struct Tracker {
    width: u32,
    height: u32,
    cols: u32,
    rows: u32,
    tiles: Vec<TileState>,
}

impl Tracker {
    pub fn new(width: u32, height: u32) -> Self {
        let mut t = Self {
            width: 0,
            height: 0,
            cols: 0,
            rows: 0,
            tiles: Vec::new(),
        };
        t.resize(width, height);
        t
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Whether any tile is waiting to be refined (ready now or later).
    pub fn has_pending(&self) -> bool {
        self.tiles.iter().any(|t| matches!(t, TileState::Dirty(_)))
    }

    /// Change the surface size; forgets all pending tiles.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.cols = width.div_ceil(TILE);
        self.rows = height.div_ceil(TILE);
        self.tiles.clear();
        self.tiles
            .resize((self.cols * self.rows) as usize, TileState::Clean);
    }

    /// Record changed regions; every overlapped tile restarts its idle timer.
    pub fn mark_dirty(&mut self, rects: &[Rect], now: Instant) {
        for r in rects {
            if r.w == 0 || r.h == 0 || r.x >= self.width || r.y >= self.height {
                continue;
            }
            let x1 = (r.x + r.w).min(self.width);
            let y1 = (r.y + r.h).min(self.height);
            for row in r.y / TILE..=(y1 - 1) / TILE {
                for col in r.x / TILE..=(x1 - 1) / TILE {
                    self.tiles[(row * self.cols + col) as usize] = TileState::Dirty(now);
                }
            }
        }
    }

    /// Tiles idle for at least `idle`, up to `budget_tiles`, row-major, with
    /// horizontally adjacent tiles of one row merged. Returned tiles are
    /// marked refined; the rest stay pending for a later call.
    pub fn take_ready(&mut self, now: Instant, idle: Duration, budget_tiles: usize) -> Vec<Rect> {
        let mut out: Vec<Rect> = Vec::new();
        let mut taken = 0usize;
        for row in 0..self.rows {
            let mut run: Option<Rect> = None;
            for col in 0..self.cols {
                let i = (row * self.cols + col) as usize;
                let ready = matches!(self.tiles[i], TileState::Dirty(t) if now.saturating_duration_since(t) >= idle);
                if ready && taken < budget_tiles {
                    self.tiles[i] = TileState::Clean;
                    taken += 1;
                    let tile = self.tile_rect(col, row);
                    match run.as_mut() {
                        Some(r) if r.x + r.w == tile.x => r.w += tile.w,
                        _ => {
                            out.extend(run.take());
                            run = Some(tile);
                        }
                    }
                } else {
                    out.extend(run.take());
                }
            }
            out.extend(run);
            if taken >= budget_tiles {
                break;
            }
        }
        out
    }

    fn tile_rect(&self, col: u32, row: u32) -> Rect {
        let x = col * TILE;
        let y = row * TILE;
        Rect {
            x,
            y,
            w: TILE.min(self.width - x),
            h: TILE.min(self.height - y),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: Duration = Duration::from_millis(200);
    fn r(x: u32, y: u32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn not_ready_before_idle() {
        let t0 = Instant::now();
        let mut t = Tracker::new(256, 256);
        t.mark_dirty(&[r(0, 0, 10, 10)], t0);
        assert!(t
            .take_ready(t0 + Duration::from_millis(100), IDLE, 100)
            .is_empty());
    }

    #[test]
    fn ready_after_idle_then_not_again() {
        let t0 = Instant::now();
        let mut t = Tracker::new(256, 256);
        t.mark_dirty(&[r(0, 0, 10, 10)], t0);
        assert_eq!(t.take_ready(t0 + IDLE, IDLE, 100), vec![r(0, 0, 64, 64)]);
        assert!(t.take_ready(t0 + IDLE * 5, IDLE, 100).is_empty());
    }

    #[test]
    fn redirty_resets_the_idle_timer() {
        let t0 = Instant::now();
        let mut t = Tracker::new(256, 256);
        t.mark_dirty(&[r(0, 0, 10, 10)], t0);
        t.mark_dirty(&[r(5, 5, 1, 1)], t0 + Duration::from_millis(150));
        assert!(t.take_ready(t0 + IDLE, IDLE, 100).is_empty());
        assert_eq!(
            t.take_ready(t0 + Duration::from_millis(350), IDLE, 100)
                .len(),
            1
        );
    }

    #[test]
    fn budget_caps_and_leftovers_come_later() {
        let t0 = Instant::now();
        let mut t = Tracker::new(256, 64);
        t.mark_dirty(&[r(0, 0, 256, 64)], t0);
        assert_eq!(t.take_ready(t0 + IDLE, IDLE, 3), vec![r(0, 0, 192, 64)]);
        assert_eq!(t.take_ready(t0 + IDLE, IDLE, 3), vec![r(192, 0, 64, 64)]);
    }

    #[test]
    fn adjacent_tiles_merge_into_one_rect() {
        let t0 = Instant::now();
        let mut t = Tracker::new(320, 128);
        t.mark_dirty(&[r(0, 0, 128, 1), r(256, 70, 1, 1)], t0);
        assert_eq!(
            t.take_ready(t0 + IDLE, IDLE, 100),
            vec![r(0, 0, 128, 64), r(256, 64, 64, 64)]
        );
    }

    #[test]
    fn edge_tiles_are_clipped_at_odd_sizes() {
        let t0 = Instant::now();
        let mut t = Tracker::new(1714, 1287);
        t.mark_dirty(&[r(1700, 1280, 100, 100)], t0);
        assert_eq!(
            t.take_ready(t0 + IDLE, IDLE, 100),
            vec![r(1664, 1280, 50, 7)]
        );
    }

    #[test]
    fn resize_resets_state() {
        let t0 = Instant::now();
        let mut t = Tracker::new(256, 256);
        t.mark_dirty(&[r(0, 0, 256, 256)], t0);
        t.resize(128, 128);
        assert!(t.take_ready(t0 + IDLE, IDLE, 100).is_empty());
    }
}
