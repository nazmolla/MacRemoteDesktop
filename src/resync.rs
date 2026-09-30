//! The on-demand A/V resync request (the Ctrl+Alt+Shift+R hotkey).
//!
//! The input handler raises it; the capture loop (forced IDR) and the audio
//! capture loop (stream rebuild) each consume their half on their next poll.
//! It is created once in `main` and handed to all three at construction, so the
//! link between them is visible in their constructors rather than hidden in
//! crate globals.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct ResyncSignal {
    video: Arc<AtomicBool>,
    audio: Arc<AtomicBool>,
}

impl ResyncSignal {
    /// Ask both the video and the audio path to resync.
    pub fn request(&self) {
        self.video.store(true, Ordering::Relaxed);
        self.audio.store(true, Ordering::Relaxed);
    }

    /// Consume a pending video resync, if any.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn take_video(&self) -> bool {
        self.video.swap(false, Ordering::Relaxed)
    }

    /// Consume a pending audio resync, if any.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn take_audio(&self) -> bool {
        self.audio.swap(false, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::ResyncSignal;

    #[test]
    fn each_half_is_consumed_once_and_independently() {
        let s = ResyncSignal::default();
        let consumer = s.clone();
        assert!(!consumer.take_video());
        s.request();
        assert!(consumer.take_video());
        assert!(!consumer.take_video(), "consumed");
        assert!(consumer.take_audio(), "audio half still pending");
        assert!(!consumer.take_audio());
    }
}
