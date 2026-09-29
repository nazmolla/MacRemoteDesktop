# Phase 1 Verification

2026-09-29

FAIL 1920x1080@100%: expected '1× at 1920×1080', got: <no plan line>
FAIL 3840x2160@200%: expected 'Retina 1920×1080 pt', got: <no plan line>
FAIL 3000x2000@150%: expected 'Retina 2000×1333 pt', got: <no plan line>
FAIL 1714x1288@100%: expected '1× at 1714×1288', got: <no plan line>

PASS 0 (manual stub)

## Task rulings summary

- Task 3: Renumber server divergence to avoid merge collision; risk: renumbering on sync.
- Task 4: CoreGraphics double-release SIGSEGV mitigated with raw FFI; no cost.
- Task 4: Use global dispatch queue for descriptor; side effect: detach‑primary reset on resize; cost: detach‑override users see physical panel re‑enable.
- Task 4: Second virtual display never online; multi‑monitor must create displays up front; risk: redesign needed.
- Task 4: Use per‑display serial (pid*64+counter) instead of pid; no cost.
- Task 5: Drop capture_output_size YAGNI; risk: 1px letterbox at fractional scales.
- Task 6: Local agent wiring fixed; scale carried in ClientAdvert; no cost.
- Task 7: Flags bool‑only; risk: cannot disable via CLI except env.
- Task 7: Virtual display placeholder 1920x1080; no cost.
- Task 7: UDP multitransport ON; EGFX-over‑UDP stays off; no cost.
- Task 7: Probe function renamed; no cost.

## Manual checks pending (unlocked Mac + real mstsc)

- Windowed resize on an ultrawide monitor.
- Dragging mstsc window between a 150% and a 100% monitor.
- Clipboard copy from Windows→Mac (text, image, file).
- Clipboard copy from Mac→Windows (text, image, file).
- Visual sharpness at 150% scaling.
- Lock screen with the privacy shield active.

## Loopback run (2026-09-29, zero server flags)
```
PASS 1920x1080@100%
PASS 3840x2160@200%
PASS 3000x2000@150%
PASS 1714x1288@100%
```
