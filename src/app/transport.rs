//! The auxiliary transports offered on top of the TCP connection: the
//! auto-reconnect cookie and the UDP multitransport listener.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64};

/// Provision the Server Auto-Reconnect Cookie unless `MACRDP_AUTO_RECONNECT=0`.
pub(super) fn provision_auto_reconnect(server: &mut RdpServer) {
    // Server Auto-Reconnect Cookie (MS-RDPBCGR ARC): provision it so a client
    // (mstsc) auto-reconnects on an ungraceful drop instead of showing
    // "disconnected". This is what makes the EGFX blank-recovery connection drop
    // (src/h264.rs, when a reconnect lands on mstsc's stale surface and never
    // presents) heal seamlessly — the client re-establishes on its own. Default
    // on (harmless + standard RDP server behavior); MACRDP_AUTO_RECONNECT=0
    // disables. The returning ARC_CS cookie is not validated (single console
    // session, NLA re-auths every connection), so a fixed per-process value is
    // fine — it only enables the client's auto-reconnect loop.
    let auto_reconnect = !matches!(
        crate::tunables::var("MACRDP_AUTO_RECONNECT").as_deref(),
        Ok("0") | Ok("false") | Ok("FALSE")
    );
    if auto_reconnect {
        let mut random_bits = [0u8; 16];
        match getrandom::getrandom(&mut random_bits) {
            Ok(()) => {
                // logon_id is informational for us; a stable per-process id.
                let logon_id = std::process::id();
                server.set_auto_reconnect_cookie(logon_id, random_bits);
                info!("server auto-reconnect cookie provisioned (clients auto-reconnect on an ungraceful drop)");
            }
            Err(e) => {
                warn!(error = %e, "could not generate auto-reconnect cookie random bits — skipping")
            }
        }
    }
}

/// The TLS material the UDP flows reuse from the TCP listener.
pub(super) struct UdpTls<'a> {
    pub(super) config: &'a Arc<ServerConfig>,
    pub(super) cert_der: &'a [u8],
    pub(super) key_der: &'a [u8],
}

/// Flags shared between the server's transport switching and the H.264
/// pipeline (see where they are created in `run`).
pub(super) struct EgfxTransportFlags<'a> {
    pub(super) on_lossy: &'a Arc<AtomicBool>,
    pub(super) on_udp: &'a Arc<AtomicBool>,
    pub(super) demigrate: &'a Arc<AtomicBool>,
    pub(super) congestion_retransmits: &'a Arc<AtomicU64>,
}

/// Bind the UDP multitransport listener and wire it into the server when
/// `--enable-udp-multitransport` or `--enable-lossy-audio` asks for it.
/// Returns `None` when neither is set or the bind fails (TCP-only then).
pub(super) async fn start_udp_multitransport(
    server: &mut RdpServer,
    args: &Args,
    tls: UdpTls<'_>,
    flags: EgfxTransportFlags<'_>,
) -> Option<ironrdp_server::UdpMultitransportListener> {
    // EXPERIMENTAL UDP multitransport (MS-RDPEMT). When enabled, install the
    // provider so the server offers reliable UDP to clients that advertise it,
    // and (M3) bind a real UDP listener on the same address/port as TCP — the
    // client reuses the server address for the auxiliary UDP flow. The handle is
    // held in `_udp_listener` for the process lifetime (Drop aborts its task).
    //
    // M3 scope: the listener answers the RDPEUDP SYN→SYN+ACK handshake (V3/EUDP2
    // negotiation); cookie validation is soft and there's no TLS/EMT tunnel or
    // channel migration yet, so a client that completes the handshake still runs
    // the session over TCP.
    //
    // `--enable-lossy-audio` is the one-switch promotion of the verified lossy-audio
    // path: it switches on the lossy UdpFecL offer, lossy deliver-on-arrival delivery
    // and 1+1 duplicate sends (passed to the provider and the listener config below),
    // and implies the UDP listener. The matching MACRDP_UDP_* tunables still work
    // alone for experiments.
    if args.enable_lossy_audio && (!args.enable_aac || !args.enable_h264) {
        warn!(
            enable_aac = args.enable_aac,
            enable_h264 = args.enable_h264,
            "--enable-lossy-audio needs --enable-aac (MS-RDPEA requires AAC for the lossy DVC) \
             AND --enable-h264 (the lossy-audio Soft-Sync rides the EGFX dispatch path); without \
             both, audio stays on TCP"
        );
    }
    if args.enable_udp_multitransport || args.enable_lossy_audio {
        // The server ISN isn't client-validated; seed it from the clock to avoid a
        // new RNG dependency (the security-relevant value is the cookie, not this).
        let isn_seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let listener_defaults = ironrdp_server::ListenerConfig::default();
        let cfg = ironrdp_server::ListenerConfig {
            server_isn_seed: isn_seed,
            // --enable-lossy-audio implies lossy delivery and 1+1 duplication;
            // each can also be switched on alone for experiments.
            lossy_delivery: args.enable_lossy_audio
                || tunables::truthy("MACRDP_UDP_LOSSY_DELIVERY"),
            lossy_duplicate: args.enable_lossy_audio
                || tunables::truthy("MACRDP_UDP_LOSSY_AUDIO_DUP"),
            tunnel_dead_secs: tunables::parsed(
                "MACRDP_UDP_TUNNEL_DEAD_SECS",
                listener_defaults.tunnel_dead_secs,
            ),
            offer_cooldown_secs: tunables::parsed(
                "MACRDP_UDP_MT_COOLDOWN_SECS",
                listener_defaults.offer_cooldown_secs,
            ),
            ..listener_defaults
        };
        // M5a: a shared cookie registry binds an inbound UDP tunnel to a real TCP
        // session. The server registers each issued cookie here; the listener
        // accepts a tunnel CREATEREQUEST only if its echoed cookie matches.
        let cookie_registry = ironrdp_server::CookieRegistry::new();
        // M5c: the server→listener handoff. The server pushes EGFX frames (when
        // migrated onto the tunnel) through the sender; the listener owns the rx
        // and ships them as RDP_TUNNEL_DATA over the bound peer's UDP tunnel.
        let (tunnel_sender, tunnel_rx) = ironrdp_server::tunnel_channel();
        // Phase 2 (P2.1a): a DTLS 1.2 server context for the LOSSY (UdpFecL) flow,
        // built from the same cert as the TCP/reliable path. Non-fatal if it fails
        // to build (the lossy flow then falls back to observe-only); the reliable
        // flow is unaffected.
        let udp_dtls_config = match ironrdp_server::DtlsServerContext::from_der(
            tls.cert_der,
            tls.key_der,
        ) {
            Ok(ctx) => Some(ctx),
            Err(e) => {
                warn!(error = %e, "could not build DTLS server context for the lossy UDP flow; lossy stays observe-only");
                None
            }
        };
        match ironrdp_server::UdpMultitransportListener::bind(
            args.bind,
            cfg,
            Some(Arc::clone(tls.config)),
            Some(cookie_registry.clone()),
            Some(tunnel_rx),
            udp_dtls_config,
            Some(Arc::clone(flags.congestion_retransmits)),
        )
        .await
        {
            Ok(listener) => {
                // EGFX migration is controlled by --udp-migrate-egfx, or the
                // MACRDP_UDP_MIGRATE_EGFX tunable.
                let migrate_egfx =
                    args.udp_migrate_egfx || tunables::truthy("MACRDP_UDP_MIGRATE_EGFX");
                info!(
                    addr = %args.bind,
                    migrate_egfx,
                    "UDP multitransport listener bound — EGFX migrates to the reliable UDP tunnel \
                     when --udp-migrate-egfx is set (otherwise EGFX stays on TCP); \
                     input/audio/clipboard always ride TCP"
                );
                server.set_multitransport_provider(Some(Box::new(
                    multitransport::MacMultitransport {
                        offer_lossy: args.enable_lossy_audio
                            || tunables::truthy("MACRDP_UDP_OFFER_FECL"),
                    },
                )));
                server.set_migrate_egfx(migrate_egfx);
                server.set_migrate_egfx_lossy(tunables::truthy("MACRDP_UDP_MIGRATE_EGFX_LOSSY"));
                server.set_multitransport_offer_max_rtt_ms(tunables::parsed(
                    "MACRDP_UDP_OFFER_MAX_RTT_MS",
                    80,
                ));
                server.set_multitransport_cookie_registry(Some(cookie_registry));
                server.set_multitransport_tunnel_sender(Some(tunnel_sender));
                // Ack-driven IDR recovery (EGFX-on-lossy): hand the server the same
                // flag the H.264 pipeline reads. The server flips it true only when
                // it migrates EGFX onto the LOSSY tunnel.
                server.set_egfx_on_lossy_handle(Some(Arc::clone(flags.on_lossy)));
                // Frame-ack-lag backpressure: the server flips this true whenever it
                // migrates EGFX onto a UDP tunnel (reliable or lossy), and the H.264
                // pipeline drops captures when the client's decode backlog runs away.
                server.set_egfx_on_udp_handle(Some(Arc::clone(flags.on_udp)));
                // EGFX-over-UDP → TCP watchdog: the H.264 pipeline sets this true when
                // the reliable UDP tunnel wedges; the server reads it to de-migrate
                // EGFX back to TCP (mstsc renders it post-Soft-Sync). Reset on reconnect.
                server.set_demigrate_request_handle(Some(Arc::clone(flags.demigrate)));
                // (P2.4b) Register the lossy-UDP audio DVC (AUDIO_PLAYBACK_LOSSY_DVC),
                // driven by `--enable-lossy-audio` (or the legacy `MACRDP_UDP_LOSSY_AUDIO`
                // env fallback). Requires AAC (the lossy DVC needs version >= 8 + AAC,
                // MS-RDPEA Appendix A note <2>). Advertises the SAME format list the
                // static RDPSND path encodes; the lossy 1+1 transport is enabled by the
                // env bridge above. Verified on mstsc smooth at 5/10/15% loss.
                if (args.enable_lossy_audio || tunables::truthy("MACRDP_UDP_LOSSY_AUDIO"))
                    && args.enable_aac
                {
                    let formats = audio::server_audio_formats(args.enable_aac, args.aac_bitrate);
                    info!(
                        formats = formats.len(),
                        "P2.4b lossy-UDP audio (--enable-lossy-audio): offering AUDIO_PLAYBACK_LOSSY_DVC \
                         with 1+1 redundancy — AAC negotiation over TCP"
                    );
                    server.set_multitransport_lossy_audio_formats(Some(formats));
                }
                Some(listener)
            }
            Err(e) => {
                // Non-fatal: fall back to TCP-only. Most common cause is the UDP
                // port already being in use.
                warn!(error = %e, addr = %args.bind, "could not bind UDP multitransport listener; continuing TCP-only");
                None
            }
        }
    } else {
        None
    }
}
