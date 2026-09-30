# Phase 2a review: `git diff main...phase-2`

Scope: `src/h264.rs`, `src/capture.rs`, `src/refine.rs`, `src/lossless.rs`, `src/negotiator/*`, `src/main.rs`, `src/videotoolbox.rs` at `phase-2` (commit f17d4f2, 2026-09-30). Line numbers are for that head. Only defects are listed.

Pinned upstream facts used below (IronRDP rev a5d1c682): `GfxHandler::max_frames_in_flight` returns `u32::MAX`, so `send_*_frame` never returns `None` on backpressure; `send_mixed_frame` still returns `None` when the server is not ready or the surface is unknown. `ClearCodecEncoder` keeps a glyph cache (tiles of 1024 px or less) and a sequence number that must stay in step with the client decoder.

## Critical

### C1. `refine_tick` takes `server_handle` while holding `ctx`: ABBA deadlock with inbound frame acks
`src/h264.rs:2581` (ctx locked) → `src/h264.rs:2636` (`ctx.server_handle.lock()`).

The file's own lock-order invariant (comment in `ship_frames`, and `GfxHandler` doc: "MUST NOT lock `server_handle` from these") is `server_handle` → `ctx`: `GfxDvcBridge::process` holds the server mutex while it calls `on_frame_ack` (`:3186`) and `on_qoe_metrics` (`:3276`), both of which lock `ctx`. `refine_tick` does the reverse.

Scenario: user stops typing, the flush burst ships, 200 ms later `refine_tick` locks `ctx` and blocks on `server_handle`, while the client loop is inside `process()` for the FrameAcknowledge of the last flush frame and blocks on `ctx`. Both threads hang for good. Because the blocked side is the server's client loop, input, audio and clipboard for the session freeze too, not only video. The timing is not rare: refinement fires exactly when the last acks of a burst are arriving.

Fix: collect `surface_id`, `server_handle` clone and the ready tiles under `ctx`, drop `ctx`, then lock `server_handle`. The "no AVC frame in between" guarantee does not need `ctx` held: `submit_bgra_regions` and `refine_tick` both run on the capture task, so no new frame can be submitted while `refine_tick` runs, and `submitted == shipped` cannot become false.

### C2. Full-surface repaints are not fed to the refinement tracker, so refined content is overwritten with lossy pixels and never re-refined
`src/h264.rs:2862-2866` (keyframe ⇒ whole-surface region), `:2846` (queue miss ⇒ whole surface), `:4955` (`Rects([])` ⇒ whole surface) versus `:2080-2081` (tracker marked only with the submitted rects).

The tracker is marked at submit time with the dirty rects, but the region actually sent is decided at ship time. Whenever ship sends the whole surface while submit marked only rects, every previously refined tile is repainted lossily and the tracker still thinks it is clean. Cases:
- every periodic IDR from VideoToolbox (`is_keyframe` without `force_keyframe`; default keyframe interval 2 s),
- a region-queue miss (`_ => None`), e.g. after the PTS reset in I3,
- SCK returning an empty dirty-rect list (`Rects(vec![])` → `avc_regions` returns full, `mark_dirty(&[])` marks nothing).

Scenario: static document, a blinking caret keeps frames flowing; 2 s later the periodic IDR repaints the whole screen at QP 22 and only the caret tile is re-refined. The static text stays lossy for the rest of the session, which is the exact outcome §8.3 exists to prevent.

Fix: mark the tracker in `ship_frames` Phase 1 (already under `ctx`) from the effective region list, full surface when `f.is_keyframe` or the list resolves to full. Remove the submit-time marking, or keep it only as an early "not idle yet" hint.

### C3. `phase-2` does not compile off macOS
`src/main.rs:2610` calls `locate_shield_helper()`, which exists only under `#[cfg(target_os = "macos")]` (`src/main.rs:845-846`). `cargo test --locked` fails on Linux with E0425, so upstream-style Linux CI is red.

Fix: `#[cfg(not(target_os = "macos"))] let shield_helper_available = false;` beside the macOS call. (Applied on `phase-2-avc444-math` as its first commit.)

## Important

### I1. Dirty rects of dropped captures are lost; the changed area stays stale on the client
`src/h264.rs:1941-1944` (frame-rate floor), `:1966` (drop-to-latest), the UDP lag trickle drop, and the blank-recovery early return all return `Ok(true)` without recording `regions`. `src/capture.rs:1743` treats `Ok(true)` as shipped (stashes the frame, arms the flush burst).

The next encoded frame encodes the whole picture, including the dropped frame's change, but its AVC420 regions list only the next frame's own rects, so the client never copies the changed area onto the surface. The tracker was not marked either, so refinement does not repair it. Flush frames use `SameAsLast` and do not help. The area stays stale until the next IDR (up to 2 s, longer with adaptive IDR backoff, which stretches the interval exactly when drops are frequent).

Scenario: under load the throttle drops the frame in which a dialog closes; the next frame's dirty rect is the caret; the closed dialog stays visible on the client.

Fix: keep a `pending_regions` accumulator in `ConnectionContext`; every early return after `regions` is known merges into it, and the next submitted frame ships `pending ∪ own` (or simply `Full` after any drop).

### I2. A single VideoToolbox drop (or a live resize) disables refinement for the rest of the connection and spins the capture loop
`src/videotoolbox.rs:1179` returns without sending anything when VT drops a frame (`sample_buffer` null) or reports an error, so `shipped` never catches up with `submitted`. `src/h264.rs:2438` resets both counters to 0 on encoder rebuild while the old ship thread still holds the same `shipped` `Arc` and bumps it for its in-flight frames, so after a live resize `shipped` can exceed `submitted` by a constant.

Either way `refine_tick`'s `sub != shp` gate never passes again. Consequences: no refinement for the rest of the session, `refine_pending()` stays true so the capture loop polls with a timeout every frame interval forever, and `info!("refine: waiting for in-flight frames")` (`:2598`) logs 60 times per second at the default level. The pre-existing drop-to-latest throttle is skewed by the same offset. The PTS-keyed pop at `:2840-2846` also discards the dropped frame's regions (feeds I1).

Fix: count completions, not deliveries: have the output callback send a `Dropped { pts }` item (or bump a per-encoder counter) on every callback; give each encoder its own fresh `Arc<AtomicU64>` pair instead of `store(0)` on shared ones; downgrade the log to `trace!`.

### I3. Region state is not reset when the encoder is rebuilt; stale PTS entries match new frames
`src/h264.rs:2664` (`reset_for_live_resize`) and `setup_locked` clear the encoder but not `region_queue`, `last_regions` or the tracker. The new encoder restarts PTS at 0. Entries still queued from the old encoder (up to `max_in_flight`, e.g. PTS 812..814) sit at the queue front; new frames get `None` (full) until new PTS 812 arrives, which then pops the old encoder's rects for that frame, in old-resolution coordinates.

Scenario: resize while three frames are in flight; about 13 s later at 60 fps, frames 812-814 of the new encoder ship with the old encoder's dirty rects, and their real changes are not painted (same stale-area effect as I1).

Fix: clear `region_queue` and `last_regions` and mark the tracker full wherever `ctx.encoder` is set to `None` or rebuilt.

### I4. Refinement runs only when SCK is idle, so static regions are never refined while anything animates at frame rate
`src/capture.rs:1528-1565`: `refine_tick` is called only from the `Err(_)` (timeout) arm of the frame wait. When any part of the screen updates at or above the capture rate (a video, a progress animation, a game in a corner), the timeout never fires and nothing else on screen is refined, although it has been static for seconds. Spec §8.3 targets exactly this mixed case.

Fix: also call `refine_tick` after a successful submit when tiles are due (cheap check, rate-limited to e.g. once per `REFINE_IDLE / 2`).

### I5. `refine_tick` marks tiles clean, and advances ClearCodec state, before the send can fail
`src/h264.rs:2608-2610` flips the taken tiles to `Clean` before `encode_region` (`?` at `:2622`), the `channel_id()` check and `send_mixed_frame` (return value ignored at `:2640`). Any failure after that loses those tiles permanently. Two concrete triggers:
- After a live resize the stashed `last_frame` can be narrower than the new `dims`: the guard at `:2603` checks `bgra.len() >= stride * h` but not `stride >= w * 4`, so `encode_region` bails on the first tile past the old width and the rest of the batch is dropped.
- When `send_mixed_frame` returns `None` (surface id unknown after a remap/reset) the ClearCodec encoder has already stored glyphs and bumped its sequence number for data the client never received. The next identical small edge tile (1024 px or less, e.g. 50×7 at 1714×1287) is encoded as a glyph hit the client cannot resolve, a decode error on the graphics channel.

Fix: check `stride >= w * 4` and readiness (`channel_id`, surface present) before `take_ready`; on any failure after it, re-mark the tiles (`mark_dirty(&ready, now - REFINE_IDLE)`) and reset the ClearCodec encoder together with a fresh surface, never on its own.

## Minor

### M1. Per-frame heap allocations on the hot path (plan constraint)
`src/capture.rs:1701` collects a `Vec<Rect>` per frame; `src/h264.rs:2066-2069` clones the region vector twice per submit; `ship_frames` builds a `Vec<Option<Vec<Rect>>>` plus one `Vec<Avc420Region>` per frame (`avc_regions`); `src/lossless.rs:26` allocates per tile and `refine_tick` a tile vector per tick. The plan's global constraint is no per-frame allocation in steady state. Fix: reusable buffers in `ConnectionContext` and `ScreenCaptureUpdates`, and `SmallVec<[Rect; 16]>` for regions (the list is capped at `MAX_AVC_REGIONS` anyway).

### M2. Refinement is not repeated after a loss on the lossy UDP tunnel
Once EGFX rides the lossy UDP flow, a dropped mixed frame is not retransmitted, but the tracker has already marked its tiles clean, so they stay lossy until touched again. Fix: tie `Clean` to the mixed frame's FrameAcknowledge (keep tiles "sent, unacked" until the ack for that frame id arrives), or skip refinement while `egfx_on_lossy`.

### M3. `negotiator::video::caps_from_egfx` ORs flags across every advertised capset
`src/negotiator/video.rs:50-82`. The server confirms one capset (the highest), so a client advertising `V8_1{AVC420_ENABLED}` plus `V10_x{AVC_DISABLED}` is planned as AVC420 while the confirmed capset disables AVC; sending AVC420 then is a protocol violation. Latent (the ladder is not wired yet; `h264.rs::caps_indicate_avc` has the same shape). Fix: derive `VideoCaps` from the negotiated capset passed to `on_ready`, not from the advertise list.

## Resolution (2026-09-30)
- C1 fixed: `refine_tick` prepares tiles under `ctx`, releases it, then locks `server_handle`.
- C2 fixed: refinement tracker is marked at ship time from the regions actually painted (keyframe / full / empty → whole surface); submit-time marking removed.
- C3 fixed on `phase-2-avc444-math` (merged).
- I1 fixed: `RegionDebt` accumulates every capture's regions before any drop decision; the next encoded frame ships the union.
- I2 fixed: in-flight gate removed (tiles are refined with the newest pixels; a later repaint re-marks them), so VT drops / counter resets can't stall refinement; log is `trace!`.
- I3 fixed: region queue, last regions and debt reset wherever the encoder is dropped or rebuilt.
- I4 fixed: refinement also piggybacks on submits (rate-limited to 100 ms), not only on idle timeouts.
- I5 fixed: stride/size checked before `take_ready`; on encode/send failure tiles are re-marked due and the ClearCodec encoder restarts.
- Minor M1–M3 deferred to Phase 2b.
- Verification gap: loopback on this Mac mirrors the virtual display onto the screen that shows the FreeRDP window (feedback loop → full-screen change every frame), so idle refinement can't be observed locally; covered by the real-mstsc checklist.
