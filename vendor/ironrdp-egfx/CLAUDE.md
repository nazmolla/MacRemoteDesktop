# vendor/ironrdp-egfx — divergence log

Local copy of ironrdp-egfx 0.3.0 from Devolutions/IronRDP@a5d1c682 (the same rev as
the other git pins). The manifest's `workspace = true` keys and sibling path deps
were replaced with concrete values; the root `[patch.crates-io]` and
`[patch."https://github.com/Devolutions/IronRDP.git"]` both point here so every
crate resolves to this one copy.

## Divergences

1. **AVC420/AVC444 region rectangles are written exclusive** (`Avc420Region::to_rectangle`,
   `src/pdu/avc.rs`). The metablock's regionRects are `RDPGFX_RECT16`, whose
   right/bottom are exclusive (MS-RDPEGFX 2.2.1.4.1); `Avc420Region` is inclusive and
   upstream wrote its values unchanged. A full-frame region therefore lost one column
   and row (unnoticed upstream, which only sent full frames), and once the server sent
   per-region updates a one-pixel region became `left == right`. FreeRDP rejects that
   frame ("rdpgfx_read_rect16 failed with error 13") and Windows clients stopped
   presenting entirely (black window, no frame acknowledgements). `compute_dest_rect`
   already converts inclusive to exclusive for the WireToSurface1 destination, so only
   the metablock changed. Test: `region_rects_are_exclusive_on_the_wire`. Upstreamable.
