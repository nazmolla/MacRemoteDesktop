# HiDPI (Retina) on a CGVirtualDisplay — it works (2026-09-29, macOS 27)

Upstream macrdp concluded HiDPI was impossible on virtual displays
(`docs/known-quirks.md`, tested on macOS 26.4). A throwaway experiment
(`spikes/hidpi/main.swift`) shows it is possible; upstream missed two steps.

## What works
1. Create the display with `CGVirtualDisplaySettings.hiDPI = 1` and register the
   desired **point** size as a mode (e.g. `1280×720`).
2. macOS then offers, for every registered mode `W×H`, a Retina twin
   `W×H points @ 2W×2H pixels` — but only when modes are enumerated with
   `CGDisplayCopyAllDisplayModes(id, {kCGDisplayShowDuplicateLowResolutionModes: true})`.
   Without that option the list is degenerate (what upstream saw).
3. The display comes up in its **largest 1× mode**, not the Retina one. Switch
   explicitly with `CGBeginDisplayConfiguration` → `CGConfigureDisplayWithDisplayMode`
   → `CGCompleteDisplayConfiguration(.forSession)`.

Result (variant a): `current = 800×600 points @ 1600×1200 pixels`,
`NSScreen.backingScaleFactor = 2.0`.

## Caveats observed
- With two registered modes (variant b) a switch returned `err = 0` but the display
  stayed at its previous mode. Production code must **verify** the switch
  (`CGDisplayCopyDisplayMode(id).pixelWidth == 2 * width`) and retry or re-apply settings.
- `hiDPI = 2` (variant c) behaves like 0 — no Retina twins.
- macOS also adds a fixed list of standard sizes; we select by exact point size.

## Consequence for the design (spec §7.2)
| Client scale | Register mode (points) | Select | Capture |
|---|---|---|---|
| 100% | `P × Q` | 1× `P×Q` | as is |
| 200% | `P/2 × Q/2` | Retina twin (`P×Q` pixels) | as is — pixel-exact |
| 125–175% | `P/s × Q/s` | Retina twin (`2P/s × 2Q/s` pixels) | ScreenCaptureKit output size `P×Q` (GPU downscale) |
