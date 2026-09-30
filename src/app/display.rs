//! The main display's geometry and the bitmap codecs advertised to clients.

use super::*;

pub(super) const FALLBACK_WIDTH: u16 = 1280;
pub(super) const FALLBACK_HEIGHT: u16 = 720;

/// Codecs advertised to the client. The actual encoder picks the one the
/// client also speaks; on conflict, the negotiation prefers (in order)
/// QOIZ > QOI > RemoteFx > NSCodec.
///
/// NSCodec is here for Microsoft Remote Desktop on macOS, which speaks only
/// NSCodec. The encoder is a Phase-1 stub right now — see CLAUDE.md.
pub(crate) fn bitmap_codecs() -> BitmapCodecs {
    BitmapCodecs(vec![
        // NSCodec — for Microsoft Remote Desktop on macOS.
        Codec {
            id: 0,
            property: CodecProperty::NsCodec(NsCodec {
                is_dynamic_fidelity_allowed: false,
                is_subsampling_allowed: false, // Phase 3 will flip this on.
                color_loss_level: 3,
            }),
        },
        // RemoteFx — what mstsc actually uses.
        Codec {
            id: 0,
            property: CodecProperty::RemoteFx(RemoteFxContainer::ServerContainer(1)),
        },
        Codec {
            id: 0,
            property: CodecProperty::ImageRemoteFx(RemoteFxContainer::ServerContainer(1)),
        },
        // QOI/QOIZ — for FreeRDP and any other client that picks them up.
        Codec {
            id: 0,
            property: CodecProperty::Qoi,
        },
        Codec {
            id: 0,
            property: CodecProperty::QoiZ,
        },
    ])
}

/// Bounds of the user's primary display in macOS's global point-coord
/// space — origin is `(0, 0)` by convention. Used to feed input.rs and
/// cursor.rs when we're capturing the primary panel; the virtual-display
/// path queries `VirtualDisplay::{origin_pts, size_pts}` instead.
#[cfg(target_os = "macos")]
pub(super) fn primary_screen_geometry() -> ((f64, f64), (f64, f64)) {
    use core_graphics::display::CGDisplay;
    // Size in *logical points* (e.g. 1512×982), matching the virtual-display
    // path and the `CGDisplay::bounds()` scaling in input.rs. The cursor's
    // HiDPI scale factor is derived as framebuffer_pixels / these points, so
    // this MUST be points — NOT `pixels_wide/high` (backing pixels), which
    // would make the ratio 1.0 even at Retina and leave the cursor half-size.
    let b = CGDisplay::main().bounds();
    ((b.origin.x, b.origin.y), (b.size.width, b.size.height))
}

/// Backing (Retina) pixel resolution of the main display, for the HiDPI
/// capture path. `CGDisplayMode::pixel_width/height` is the true backing size
/// (e.g. 3024×1964 on a 14" MBP), distinct from the logical point size
/// (`mode.width/height`, ~1512×982) and from `CGDisplay::pixels_wide/high`
/// (the canonical mode, which equals points on many configs). Returns `None`
/// if the mode can't be read or the size doesn't fit u16.
#[cfg(target_os = "macos")]
pub(super) fn primary_backing_size() -> Option<(u16, u16)> {
    use core_graphics::display::CGDisplay;
    let mode = CGDisplay::main().display_mode()?;
    let w = u16::try_from(mode.pixel_width()).ok()?;
    let h = u16::try_from(mode.pixel_height()).ok()?;
    Some((w, h))
}

#[cfg(not(target_os = "macos"))]
pub(super) fn primary_screen_geometry() -> ((f64, f64), (f64, f64)) {
    ((0.0, 0.0), (0.0, 0.0))
}

#[cfg(target_os = "macos")]
pub(super) fn ensure_screen_recording_access() {
    use core_graphics::access::ScreenCaptureAccess;
    let tcc = ScreenCaptureAccess;
    if tcc.preflight() {
        info!("Screen Recording permission already granted");
        return;
    }
    warn!(
        "Screen Recording permission NOT granted. macrdp will appear in \
         System Settings → Privacy & Security → Screen Recording. Enable it, \
         then RESTART macrdp (TCC grants only take effect on next launch)."
    );
    // request() registers the binary with TCC and opens the prompt; the
    // returned bool reflects current state, which is still false on first run.
    let _ = tcc.request();
}
