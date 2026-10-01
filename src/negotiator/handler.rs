//! ConnectionHandler decorator that records what the client advertised during
//! the handshake (scale factor, platform) for the Session Negotiator and applies
//! per-connection input choices. Forwards every call to the wrapped handler.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ironrdp_server::{ConnectionHandler, PostConnectionAction};

use crate::negotiator::session::{classify_platform, ClientPlatform};

/// Latest client-advertised values, shared with the display path.
#[derive(Debug, Default)]
pub struct ClientAdvert {
    /// Desktop scale factor in percent; 0 = not sent.
    pub scale_pct: AtomicU32,
    /// 0 = unknown/Other, 1 = Windows, 2 = Apple.
    platform: AtomicU8,
    /// Set once per connection (never on a reactivation); taken by the first
    /// display sync, the only point where the display may still be replaced.
    new_connection: AtomicBool,
}

impl ClientAdvert {
    /// True once per connection, for the first caller.
    pub fn take_new_connection(&self) -> bool {
        self.new_connection.swap(false, Ordering::AcqRel)
    }

    #[allow(
        dead_code,
        reason = "phase 1 of the negotiated-session plan; wired in by a later phase"
    )]
    pub fn platform(&self) -> ClientPlatform {
        match self.platform.load(Ordering::Relaxed) {
            1 => ClientPlatform::Windows,
            2 => ClientPlatform::Apple,
            _ => ClientPlatform::Other,
        }
    }
    fn set_platform(&self, p: ClientPlatform) {
        self.platform.store(
            match p {
                ClientPlatform::Windows => 1,
                ClientPlatform::Apple => 2,
                ClientPlatform::Other => 0,
            },
            Ordering::Relaxed,
        );
    }
}

pub struct NegotiationHandler {
    inner: Option<Box<dyn ConnectionHandler>>,
    advert: Arc<ClientAdvert>,
    /// Developer override for Ctrl→Cmd (`--map-ctrl-to-cmd` given explicitly).
    ctrl_to_cmd_override: Option<bool>,
    /// Called with the negotiated Ctrl→Cmd choice each connection.
    apply_ctrl_to_cmd: fn(bool),
}

impl NegotiationHandler {
    pub fn wrap(
        inner: Option<Box<dyn ConnectionHandler>>,
        advert: Arc<ClientAdvert>,
        ctrl_to_cmd_override: Option<bool>,
        apply_ctrl_to_cmd: fn(bool),
    ) -> Box<dyn ConnectionHandler> {
        Box::new(Self {
            inner,
            advert,
            ctrl_to_cmd_override,
            apply_ctrl_to_cmd,
        })
    }
}

impl ConnectionHandler for NegotiationHandler {
    fn on_accept(&mut self, peer: SocketAddr) -> bool {
        // Default true when inner is None
        if let Some(h) = self.inner.as_mut() {
            h.on_accept(peer)
        } else {
            true
        }
    }

    fn on_disconnected(
        &mut self,
        peer: SocketAddr,
        duration: Duration,
        error: Option<&anyhow::Error>,
    ) -> PostConnectionAction {
        // Default PostConnectionAction::Continue when inner is None
        if let Some(h) = self.inner.as_mut() {
            h.on_disconnected(peer, duration, error)
        } else {
            PostConnectionAction::Continue
        }
    }

    fn on_handshake_failed(&mut self, peer: SocketAddr, failure: ironrdp_server::HandshakeFailure) {
        if let Some(h) = self.inner.as_mut() {
            h.on_handshake_failed(peer, failure);
        }
    }

    fn on_authenticated(&mut self, peer: SocketAddr, success: bool, reason: Option<&str>) {
        // Forward as lock_activity.rs does
        if let Some(h) = self.inner.as_mut() {
            h.on_authenticated(peer, success, reason);
        }
    }

    fn on_client_fingerprint(
        &mut self,
        peer: SocketAddr,
        client_name: &str,
        rdp_version: u32,
        client_build: u32,
        platform: &str,
    ) {
        // Forward to inner
        if let Some(h) = self.inner.as_mut() {
            h.on_client_fingerprint(peer, client_name, rdp_version, client_build, platform);
        }

        // Record platform
        let p = classify_platform(platform);
        self.advert.set_platform(p);

        // Decide Ctrl→Cmd
        let on = self
            .ctrl_to_cmd_override
            .unwrap_or(p != ClientPlatform::Apple);

        // Call the closure
        (self.apply_ctrl_to_cmd)(on);

        // Log decision
        tracing::info!(target: "macrdp::negotiator", platform = ?p, ctrl_to_cmd = on, override = self.ctrl_to_cmd_override.is_some(), "input: Ctrl→Cmd decided from client platform");
    }

    fn on_client_display(&mut self, peer: SocketAddr, info: &ironrdp_acceptor::ClientDisplayInfo) {
        // Forward to inner
        if let Some(h) = self.inner.as_mut() {
            h.on_client_display(peer, info);
        }

        // Record scale factor
        self.advert
            .scale_pct
            .store(info.desktop_scale_factor.unwrap_or(0), Ordering::Relaxed);
        self.advert.new_connection.store(true, Ordering::Release);

        // Log display info
        tracing::info!(target: "macrdp::negotiator", width = info.desktop_width, height = info.desktop_height, scale = info.desktop_scale_factor.unwrap_or(0), "display info recorded");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Arc;

    const PEER: SocketAddr =
        SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 3389);

    // Test 1: apple_client_turns_ctrl_to_cmd_off
    #[test]
    fn apple_client_turns_ctrl_to_cmd_off() {
        static LAST: AtomicU8 = AtomicU8::new(9);

        fn rec(on: bool) {
            LAST.store(on as u8, Ordering::SeqCst);
        }

        let advert = Arc::new(ClientAdvert::default());
        let mut handler = NegotiationHandler::wrap(None, advert.clone(), None, rec);
        handler.on_client_fingerprint(PEER, "x", 0, 0, "OsX/Unspecified");

        assert_eq!(LAST.load(Ordering::SeqCst), 0);
        assert_eq!(advert.platform(), ClientPlatform::Apple);
    }

    // Test 2: windows_client_turns_it_on_and_override_wins
    #[test]
    fn windows_client_turns_it_on_and_override_wins() {
        static LAST: AtomicU8 = AtomicU8::new(9);

        fn rec(on: bool) {
            LAST.store(on as u8, Ordering::SeqCst);
        }

        // Windows client with no override → 1
        let advert = Arc::new(ClientAdvert::default());
        let mut handler = NegotiationHandler::wrap(None, advert.clone(), None, rec);
        handler.on_client_fingerprint(PEER, "y", 0, 0, "Windows/WindowsNt");

        assert_eq!(LAST.load(Ordering::SeqCst), 1);

        // Now create a new handler with override Some(false)
        let mut handler2 =
            NegotiationHandler::wrap(None, Arc::new(ClientAdvert::default()), Some(false), rec);
        handler2.on_client_fingerprint(PEER, "y", 0, 0, "Windows/WindowsNt");

        assert_eq!(LAST.load(Ordering::SeqCst), 0);
    }

    // Test 3: scale_factor_is_recorded
    #[test]
    fn scale_factor_is_recorded() {
        use ironrdp_acceptor::ClientDisplayInfo;

        let advert = Arc::new(ClientAdvert::default());
        let info_with_scale = ClientDisplayInfo {
            desktop_width: 3840,
            desktop_height: 2160,
            desktop_scale_factor: Some(150),
            ..Default::default()
        };

        let mut handler = NegotiationHandler::wrap(None, advert.clone(), None, |_| {});
        handler.on_client_display(PEER, &info_with_scale);

        assert_eq!(advert.scale_pct.load(Ordering::Relaxed), 150);

        // Reset scale
        let info_no_scale = ClientDisplayInfo {
            desktop_width: 3840,
            desktop_height: 2160,
            desktop_scale_factor: None,
            ..Default::default()
        };
        handler.on_client_display(PEER, &info_no_scale);

        assert_eq!(advert.scale_pct.load(Ordering::Relaxed), 0);
    }
}
