//! The multi-user **broker**: a thin RDP connection router that runs as a root
//! LaunchDaemon on the public port and transparently forwards each connection to
//! the right per-user session agent, chosen from the cleartext `mstshash` cookie
//! in the X.224 Connection Request (MS-RDPBCGR) — the same hook real RD
//! Connection Brokers use. It terminates **nothing** (no TLS, no NLA): the agent
//! (plain Viga, bound to a uid-derived loopback port) does the full RDP
//! handshake and auth exactly as in the single-user path. This keeps the agent
//! unchanged and needs no Server-Redirection support (which IronRDP lacks).
//!
//! Flow per connection:
//! 1. Peek the first PDU (cleartext, before TLS) → [`cookie::Route`].
//! 2. [`policy::decide`] against the owner policy (primary user / multi-user
//!    toggle / allow-list).
//! 3. [`route::resolve`] the verdict to a listening agent's loopback address.
//! 4. Forward the peeked bytes, then splice the two sockets byte-for-byte.
//!
//! What the broker does NOT do (by design, see the Phase-3 plan): it cannot
//! *create* a user's GUI session — logging a user in / activating an off-console
//! virtual display needs an Apple-only entitlement. It routes to sessions that
//! already exist; a user with no running agent is refused with a clear reason.

mod cookie;
mod policy;
mod route;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

use cookie::{Route, PEEK_MAX};

/// Resolved broker configuration (from env / LaunchDaemon config.env).
struct BrokerConfig {
    bind: SocketAddr,
    policy_file: PathBuf,
    agent_port_base: u16,
}

impl BrokerConfig {
    fn from_env() -> Result<Self> {
        let bind = crate::tunables::var("MACRDP_BROKER_BIND")
            .ok()
            .unwrap_or_else(|| "0.0.0.0:3389".to_owned())
            .parse()
            .context("MACRDP_BROKER_BIND must be host:port")?;
        let policy_file = crate::tunables::var("VIGA_POLICY_FILE")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from("/Library/Application Support/Viga/policy.env")
            });
        let agent_port_base = crate::tunables::var("MACRDP_AGENT_PORT_BASE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(route::AGENT_PORT_BASE);
        Ok(Self {
            bind,
            policy_file,
            agent_port_base,
        })
    }
}

/// Run the broker until the listener fails. Entry point from `main` when
/// `--broker` is passed.
pub(crate) async fn run() -> Result<()> {
    // The broker path skips app::run's init_logging, so set up our own
    // stderr/log-file subscriber (RUST_LOG, default info) — routing decisions
    // must be observable in the LaunchDaemon's log.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    crate::logging::init(filter, None, None);

    let cfg = BrokerConfig::from_env()?;
    let listener = TcpListener::bind(cfg.bind)
        .await
        .with_context(|| format!("broker could not bind {}", cfg.bind))?;
    info!(
        bind = %cfg.bind,
        policy = %cfg.policy_file.display(),
        agent_port_base = cfg.agent_port_base,
        "Viga broker listening — routing clients to per-user session agents by mstshash cookie"
    );
    let base = cfg.agent_port_base;
    let policy_file = std::sync::Arc::new(cfg.policy_file);
    loop {
        let (client, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "broker accept failed");
                continue;
            }
        };
        let policy_file = policy_file.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(client, peer, &policy_file, base).await {
                // Expected, routine outcomes (deny, no agent) are logged at info
                // inside handle; this catches only unexpected I/O errors.
                warn!(peer = %peer, error = %e, "broker connection ended with error");
            }
        });
    }
}

/// Peek, decide, resolve, splice — one client connection.
async fn handle(
    mut client: TcpStream,
    peer: SocketAddr,
    policy_file: &std::path::Path,
    base: u16,
) -> Result<()> {
    client.set_nodelay(true).ok();

    // 1. Peek the first PDU (cleartext X.224 CR) up to its TPKT length.
    let mut buf = Vec::with_capacity(256);
    let mut tmp = [0u8; 256];
    let route = loop {
        match cookie::parse(&buf) {
            Route::Incomplete if buf.len() < PEEK_MAX => {
                let n = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    client.read(&mut tmp),
                )
                .await
                .context("timed out reading the RDP connection request")??;
                if n == 0 {
                    // Client closed before a full CR; nothing to route.
                    return Ok(());
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            Route::Incomplete => break Route::NoCookie, // hit PEEK_MAX, give up on a cookie
            other => break other,
        }
    };
    let requested = match &route {
        Route::User(u) => Some(u.as_str()),
        _ => None,
    };

    // 2. Decide against the (freshly re-read) policy, so UI edits take effect
    //    without a broker restart.
    let pol = match std::fs::read_to_string(policy_file) {
        Ok(body) => policy::parse(&body),
        Err(_) => policy::Policy::default(), // no file yet = conservative default
    };
    let verdict = policy::decide(&pol, requested);

    // 3. Resolve to a live agent.
    let target = match route::resolve(verdict, base, pol.primary_user.as_deref()).await {
        Ok(addr) => addr,
        Err(reason) => {
            info!(peer = %peer, requested = ?requested, reason, "broker refused connection");
            // We can't speak RDP pre-TLS to send a tidy error; closing makes the
            // client report a connection failure. The reason is in the log /
            // status app.
            return Ok(());
        }
    };

    info!(peer = %peer, requested = ?requested, %target, "broker routing connection to session agent");

    // 4. Connect to the agent, replay the peeked bytes, then splice.
    let mut agent = TcpStream::connect(target)
        .await
        .with_context(|| format!("broker could not reach session agent at {target}"))?;
    agent.set_nodelay(true).ok();
    agent
        .write_all(&buf)
        .await
        .context("forwarding the connection request to the agent")?;

    match tokio::io::copy_bidirectional(&mut client, &mut agent).await {
        Ok((to_agent, to_client)) => {
            info!(peer = %peer, %target, to_agent, to_client, "broker connection closed");
        }
        Err(e) => {
            // A client or agent hang-up mid-session is normal; log at info.
            info!(peer = %peer, %target, error = %e, "broker splice ended");
        }
    }
    Ok(())
}
