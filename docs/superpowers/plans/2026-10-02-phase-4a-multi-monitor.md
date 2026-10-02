# Phase 4a: multi-monitor spanning

Goal: a client with N monitors (mstsc "Use all my monitors", RDM "span"/"multimon") gets N Mac displays at its own sizes and positions, one per monitor, with full-quality H.264 on each.

## Current state (2026-10-02)
- `vendor/ironrdp-acceptor` parses the GCC Client Core Data (size, scale) into `ClientDisplayInfo`, but not **Client Monitor Data** (`TS_UD_CS_MONITOR`) or **Client Monitor Extended Data** (`TS_UD_CS_MONITOR_EX`).
- One virtual display per session, owned by one `macrdpdisplay` helper (`src/virtual_display/host.rs`), id shared through `DisplayIdCell`.
- One EGFX surface (id 0) mapped at (0,0), one VideoToolbox encoder (`src/h264/mod.rs`).
- Input maps client coords onto one display (`src/input/mod.rs::move_to`).

## Steps (each verified on FreeRDP `/multimon` or `/monitors:` first, then on the user's Windows client)
1. **Acceptor:** parse `TS_UD_CS_MONITOR`/`_EX` into `ClientDisplayInfo.monitors: Vec<{left, top, right, bottom, primary, scale, physical mm}>` (vendored divergence; ironrdp-pdu already decodes `gcc::ClientMonitorData`). Unit test with a captured FreeRDP `/multimon` Connect Initial.
2. **Negotiator:** `DisplayPlan` per monitor (`src/negotiator/display.rs`), Retina per monitor scale. Keep single-monitor behaviour byte-identical when one monitor is announced.
3. **Displays:** one `VirtualDisplay` (one helper process) per monitor; arrange them with a `CGConfigureDisplayOrigin` transaction to mirror the client layout (primary at (0,0)). Replace `DisplayIdCell` with a `DisplaySet` (ordered ids + client rects). Respect `MIN_CHANGE_GAP` and create displays sequentially (WindowServer churn: see memory).
4. **Capture:** one ScreenCaptureKit stream per display, frames tagged with the monitor index.
5. **EGFX:** `ResetGraphics` with the full monitor array (already called with one monitor in `ensure_surface`), one surface per monitor created and `MapSurfaceToOutput` at the monitor's (left, top), one encoder per surface; ack pacing and region tracking per surface.
6. **Input:** map client desktop coordinates to (display, local point) using the client rects; cursor shape unchanged.
7. **Live layout changes:** MS-RDPEDISP `request_layout` with several monitors → reconnect-style re-plan (the display helper replaces displays between connections; do not re-mode mid-session).
8. **Audio and shield:** audio binds to the primary display; the shield excludes every virtual display.

## Risks
- WindowServer load: N displays created at connect. Create one at a time and wait for each to come online.
- Memory and encode cost scale with N; measure with 2 x 1440p.
- mstsc requires the monitor array in `ResetGraphics` to match the GCC monitor data exactly (left/top/right/bottom inclusive).
