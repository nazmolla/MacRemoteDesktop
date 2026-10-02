//! Capture for a multi-monitor session's extra displays (Phase 4a): one
//! ScreenCaptureKit stream per extra display, each frame handed to that
//! monitor's H.264 lane. The primary display keeps the main capture path.

#![cfg(target_os = "macos")]

use anyhow::{anyhow, Context, Result};
use screencapturekit::async_api::{AsyncSCShareableContent, AsyncSCStream};
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::{
    PixelFormat as SckPixelFormat, SCContentFilter, SCStreamConfiguration, SCStreamOutputType,
};

use crate::h264::Gfx;
use crate::multimon::Monitor;

/// Start capturing extra monitor `index` (0-based among the extras). Abort the
/// returned task when the connection ends.
pub fn spawn(index: usize, monitor: Monitor, fps: u32, gfx: Gfx) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run(index, monitor, fps, gfx).await {
            tracing::warn!(error = ?e, index, "extra monitor capture stopped");
        }
    })
}

async fn run(index: usize, m: Monitor, fps: u32, gfx: Gfx) -> Result<()> {
    let content = AsyncSCShareableContent::get()
        .await
        .map_err(|e| anyhow!("SCShareableContent: {e:?}"))?;
    let displays = content.displays();
    let display = displays
        .iter()
        .find(|d| d.display_id() == m.display_id)
        .with_context(|| format!("display {} not available for capture", m.display_id))?;
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build();
    let config = SCStreamConfiguration::new()
        .with_width(u32::from(m.size.0))
        .with_height(u32::from(m.size.1))
        .with_pixel_format(SckPixelFormat::BGRA)
        .with_fps(fps)
        .with_shows_cursor(false);
    let stream = AsyncSCStream::new(&filter, &config, 4, SCStreamOutputType::Screen);
    stream
        .start_capture()
        .map_err(|e| anyhow!("SCStream::start_capture: {e:?}"))?;
    tracing::info!(index, display_id = m.display_id, size = ?m.size, "extra monitor capture started");
    while let Some(sample) = stream.next().await {
        if sample.frame_status().is_some_and(|s| !s.has_content()) {
            continue;
        }
        let Some(pixels) = sample.image_buffer() else {
            continue;
        };
        let guard = pixels
            .lock(CVPixelBufferLockFlags::READ_ONLY)
            .map_err(|e| anyhow!("CVPixelBuffer::lock OSStatus {e}"))?;
        gfx.submit_monitor(index, m.origin, guard.as_slice(), guard.bytes_per_row(), m.size)?;
    }
    Ok(())
}
