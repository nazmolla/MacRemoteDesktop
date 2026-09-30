//! Pre-authentication handshake admission for the accept loop.
//!
//! Every accepted TCP connection goes through the same bounded path before it
//! may own or take over the session: X.224 negotiation, TLS, then CredSSP.
//! This module holds the pieces of that path that do not depend on
//! [`RdpServer`](crate::RdpServer):
//!
//! - [`HandshakeLimits`]: the deadlines and capacity caps.
//! - [`HandshakeFailure`]: why a handshake ended without authenticating, reported
//!   to the [`ConnectionHandler`](crate::ConnectionHandler).
//! - [`source_key`]: how peers are grouped for per-source limits (an IPv4
//!   address, or an IPv6 /64), shared with application-level rate limiting.
//! - `HandshakePool`: a set of in-flight handshakes polled on the accept loop's
//!   own task, so accepting never pauses while a handshake is pending.

use core::future::Future;
use core::net::{IpAddr, Ipv6Addr, SocketAddr};
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;
use std::collections::HashMap;

/// Deadlines and capacity limits for unauthenticated connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeLimits {
    /// Time allowed from accept until the X.224 connection request has been
    /// received and answered. A real client sends it immediately.
    pub pre_tls: Duration,
    /// Time allowed from accept until TLS and CredSSP have completed. Leaves
    /// room for a user answering a certificate prompt on first connect.
    pub total: Duration,
    /// Maximum number of handshakes in flight at once.
    pub max_pending: usize,
    /// Maximum number of handshakes in flight from one source (see
    /// [`source_key`]).
    pub max_per_source: usize,
}

impl Default for HandshakeLimits {
    fn default() -> Self {
        Self {
            pre_tls: Duration::from_secs(5),
            total: Duration::from_secs(30),
            max_pending: 32,
            // mstsc opens a second, abandoned connection on each attempt, and
            // several clients may share one NAT address. Four leaves room for
            // both while keeping one source to an eighth of the pool.
            max_per_source: 4,
        }
    }
}

/// Why a connection was dropped before it authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeFailure {
    /// A deadline in [`HandshakeLimits`] expired.
    Timeout,
    /// The TLS handshake failed.
    Tls,
    /// The X.224 negotiation failed or the peer did not speak RDP.
    Protocol,
    /// CredSSP rejected the credentials. Already reported through
    /// [`ConnectionHandler::on_authenticated`](crate::ConnectionHandler::on_authenticated),
    /// so the accept loop does not pass it to `on_handshake_failed`.
    Credentials,
    /// Refused without reading anything: too many handshakes in flight overall
    /// or from this source. Not evidence of an attack on its own.
    Capacity,
}

impl HandshakeFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Tls => "tls",
            Self::Protocol => "protocol",
            Self::Credentials => "credentials",
            Self::Capacity => "capacity",
        }
    }
}

/// The key used to group peers for per-source limits: the IPv4 address
/// (IPv4-mapped IPv6 addresses are folded to IPv4), or the /64 prefix of an
/// IPv6 address. One subscriber normally holds a whole /64, so keying IPv6 by
/// full address would give a single attacker 2^64 separate budgets.
pub fn source_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
    }
}

type BoxedHandshake<T> = Pin<Box<dyn Future<Output = Result<T, HandshakeFailure>>>>;

struct Pending<T> {
    peer: SocketAddr,
    key: IpAddr,
    fut: BoxedHandshake<T>,
}

/// In-flight handshakes, polled together from the accept loop's task.
///
/// Holds at most [`HandshakeLimits::max_pending`] entries and
/// [`HandshakeLimits::max_per_source`] per [`source_key`]. The futures are not
/// `Send`: the server owns `Rc` state and runs on one task.
pub(crate) struct HandshakePool<T> {
    limits: HandshakeLimits,
    pending: Vec<Pending<T>>,
    per_source: HashMap<IpAddr, usize>,
}

impl<T> HandshakePool<T> {
    pub(crate) fn new(limits: HandshakeLimits) -> Self {
        Self {
            limits,
            pending: Vec::new(),
            per_source: HashMap::new(),
        }
    }

    /// Whether a new handshake from `peer` fits within the limits.
    pub(crate) fn has_room_for(&self, peer: SocketAddr) -> bool {
        self.pending.len() < self.limits.max_pending
            && self.per_source.get(&source_key(peer.ip())).copied().unwrap_or(0) < self.limits.max_per_source
    }

    /// Start tracking a handshake. The caller checks [`Self::has_room_for`]
    /// first; `push` does not enforce it.
    pub(crate) fn push(&mut self, peer: SocketAddr, fut: impl Future<Output = Result<T, HandshakeFailure>> + 'static) {
        let key = source_key(peer.ip());
        *self.per_source.entry(key).or_insert(0) += 1;
        self.pending.push(Pending {
            peer,
            key,
            fut: Box::pin(fut),
        });
    }

    pub(crate) fn len(&self) -> usize {
        self.pending.len()
    }

    /// Resolve with the next finished handshake. Stays pending forever while
    /// the pool is empty, so it can sit in a `select!` without a guard.
    pub(crate) fn next(&mut self) -> impl Future<Output = (SocketAddr, Result<T, HandshakeFailure>)> + '_ {
        core::future::poll_fn(move |cx| self.poll_next(cx))
    }

    fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<(SocketAddr, Result<T, HandshakeFailure>)> {
        for i in 0..self.pending.len() {
            if let Poll::Ready(outcome) = self.pending[i].fut.as_mut().poll(cx) {
                let done = self.pending.swap_remove(i);
                if let Some(count) = self.per_source.get_mut(&done.key) {
                    *count -= 1;
                    if *count == 0 {
                        self.per_source.remove(&done.key);
                    }
                }
                return Poll::Ready((done.peer, outcome));
            }
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::net::Ipv4Addr;

    fn v4(a: u8, port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, a)), port)
    }

    #[test]
    fn source_key_groups_ipv6_by_64_and_folds_mapped_ipv4() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(source_key(a), source_key(b));
        assert_ne!(source_key(a), source_key(c));
        let mapped: IpAddr = "::ffff:192.0.2.7".parse().unwrap();
        assert_eq!(source_key(mapped), IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)));
    }

    #[tokio::test]
    async fn pool_enforces_per_source_and_total_caps_and_frees_slots() {
        let mut pool = HandshakePool::<u32>::new(HandshakeLimits {
            max_pending: 3,
            max_per_source: 2,
            ..HandshakeLimits::default()
        });
        assert!(pool.has_room_for(v4(1, 1)));
        pool.push(v4(1, 1), async { Ok(1) });
        pool.push(v4(1, 2), core::future::pending());
        assert!(!pool.has_room_for(v4(1, 3)), "per-source cap");
        assert!(pool.has_room_for(v4(2, 1)));
        pool.push(v4(2, 1), core::future::pending());
        assert!(!pool.has_room_for(v4(3, 1)), "total cap");

        let (peer, out) = pool.next().await;
        assert_eq!((peer, out), (v4(1, 1), Ok(1)));
        assert_eq!(pool.len(), 2);
        assert!(pool.has_room_for(v4(1, 3)), "slot freed for that source");
    }
}
