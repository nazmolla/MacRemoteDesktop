//! The session's display: creating the virtual display, resolving the
//! desktop size and geometry, and the bitmap codecs advertised to clients.

use super::*;

pub(super) const FALLBACK_WIDTH: u16 = 1280;
pub(super) const FALLBACK_HEIGHT: u16 = 720;

/// Shared handle to the virtual display (see where it is created in `run`).
pub(super) type SharedVirtualDisplay = Arc<std::sync::Mutex<virtual_display::VirtualDisplay>>;

/// Create the virtual display when `--virtual-display` is set.
pub(super) fn create_virtual_display(args: &Args) -> Result<Option<SharedVirtualDisplay>> {
    if !args.virtual_display {
        return Ok(None);
    }
    let w = args
        .width
        .ok_or_else(|| anyhow!("--virtual-display requires --width"))?;
    let h = args
        .height
        .ok_or_else(|| anyhow!("--virtual-display requires --height"))?;
    // 60 Hz: real displays bottom out around 24 Hz. Refresh rate is
    // metadata here (capture cadence is governed by --fps); pass a
    // safe value so CGVirtualDisplay doesn't reject the mode.
    let vd = virtual_display::VirtualDisplay::new(u32::from(w), u32::from(h), 60)
        .context("attaching virtual display")?;
    info!(
        display_id = vd.display_id(),
        origin = ?vd.origin_pts(),
        size = ?vd.size_pts(),
        "virtual display attached — the RDP session uses this surface; \
         your primary panel is untouched"
    );
    Ok(Some(Arc::new(std::sync::Mutex::new(vd))))
}

/// Resolve desktop dimensions + geometry. Three paths:
///   - virtual display: width/height are required by
///     [`create_virtual_display`]; geometry comes from the vdisplay's CGDisplayBounds.
///   - primary panel, no --width/--height override: query SCK for
///     native size and use CGDisplay::main() for the point-space bounds.
///   - primary panel with override: use the override + main geometry.
pub(super) async fn resolve_desktop(
    args: &Args,
    virtual_display: Option<&SharedVirtualDisplay>,
) -> Result<(u16, u16, Option<u32>, (f64, f64))> {
    let geometry = if let Some(vd) = virtual_display {
        let vd = vd.lock_or_recover();
        // Both required earlier, so the unwraps can't fire.
        let w = args
            .width
            .expect("checked above when --virtual-display set");
        let h = args
            .height
            .expect("checked above when --virtual-display set");
        // Re-query CGDisplayBounds rather than trusting the cached
        // values from VirtualDisplay creation: if --make-primary
        // moved the display to (0, 0), the cached size is fine but
        // we want a fresh read for parity with the input handler.
        #[cfg(target_os = "macos")]
        let size = {
            let b = core_graphics::display::CGDisplay::new(vd.display_id()).bounds();
            (b.size.width, b.size.height)
        };
        #[cfg(not(target_os = "macos"))]
        let size = vd.size_pts();
        info!(
            width = w,
            height = h,
            display_id = vd.display_id(),
            "desktop size (virtual display)"
        );
        (w, h, Some(vd.display_id()), size)
    } else {
        let detected = primary_display_size().await?;
        let mut w = args
            .width
            .or(detected.map(|(w, _)| w))
            .unwrap_or(FALLBACK_WIDTH);
        let mut h = args
            .height
            .or(detected.map(|(_, h)| h))
            .unwrap_or(FALLBACK_HEIGHT);
        // --hidpi: capture at the display's backing (Retina) pixel resolution
        // instead of logical points, unless the user pinned an explicit
        // --width/--height (in which case they've chosen the size themselves).
        #[cfg(target_os = "macos")]
        if args.hidpi && args.width.is_none() && args.height.is_none() {
            if let Some((bw, bh)) = primary_backing_size() {
                info!(
                    points_w = w,
                    points_h = h,
                    backing_w = bw,
                    backing_h = bh,
                    "--hidpi: capturing at backing pixel resolution"
                );
                w = bw;
                h = bh;
            } else {
                warn!("--hidpi: could not read backing pixel size; staying at logical points");
            }
        }
        if let Some((dw, dh)) = detected {
            info!(
                width = w,
                height = h,
                detected_w = dw,
                detected_h = dh,
                "desktop size"
            );
        } else {
            info!(width = w, height = h, "desktop size (no display detected)");
        }
        let (_origin, size) = primary_screen_geometry();
        (w, h, None, size)
    };
    Ok(geometry)
}

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
        "Screen Recording permission NOT granted. Portico will appear in \
         System Settings → Privacy & Security → Screen Recording. Enable it, \
         then RESTART Portico (TCC grants only take effect on next launch)."
    );
    // request() registers the binary with TCC and opens the prompt; the
    // returned bool reflects current state, which is still false on first run.
    let _ = tcc.request();
}
