//! Mapping a policy [`Verdict`](super::policy::Verdict) to a concrete per-user
//! session-agent address, and the small system lookups that needs: account
//! name → uid, the current console user, and whether a user's agent is actually
//! listening. Kept apart from the pure policy so the account/OS calls stay in
//! one place.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use super::policy::{normalize_user, Verdict};

/// Base loopback port for session agents. Each agent binds
/// `127.0.0.1:(AGENT_PORT_BASE + uid % 1000)`, derived purely from its own uid,
/// so one shared Aqua LaunchAgent plist serves every user and the broker can
/// find a user's agent from the uid alone — no registration handshake.
pub(super) const AGENT_PORT_BASE: u16 = 39000;

/// The loopback address a user's session agent binds, from the uid.
pub(super) fn agent_addr(uid: u32, base: u16) -> SocketAddr {
    let port = base.wrapping_add((uid % 1000) as u16);
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
}

/// Resolve a local account name to its uid via `getpwnam_r`. `None` if the
/// account doesn't exist. Accepts `DOMAIN\user` / `user@realm` (normalized).
pub(super) fn uid_for_user(name: &str) -> Option<u32> {
    let short = normalize_user(name);
    let c = std::ffi::CString::new(short).ok()?;
    // getpwnam_r into a stack+heap buffer. Retry with a larger buffer on ERANGE.
    let mut bufsize = 1024usize;
    loop {
        // SAFETY: `libc::passwd` is a C struct of plain integers/pointers; an
        // all-zero value is a valid (empty) initial state, overwritten by
        // getpwnam_r before any field is read.
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut buf = vec![0i8; bufsize];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: all pointers are valid for the call's duration; `buf` is
        // `bufsize` bytes; `result` is set to `&pwd` on success, null if absent.
        let rc = unsafe {
            libc::getpwnam_r(
                c.as_ptr(),
                &mut pwd,
                buf.as_mut_ptr(),
                bufsize,
                &mut result,
            )
        };
        if rc == libc::ERANGE && bufsize < 1 << 20 {
            bufsize *= 2;
            continue;
        }
        if rc != 0 || result.is_null() {
            return None;
        }
        return Some(pwd.pw_uid);
    }
}

/// The uid of the user currently on the physical console, from the owner of
/// `/dev/console`. Used to resolve [`Verdict::RouteDefault`] to a session when
/// no cookie / no primary pins one. `None` if nobody is at the console
/// (login window) or the stat fails.
pub(super) fn console_uid() -> Option<u32> {
    let path = std::ffi::CString::new("/dev/console").ok()?;
    // SAFETY: `libc::stat` is a plain C struct; all-zero is a valid initial
    // value that `stat(2)` fully overwrites on success (rc == 0).
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a valid NUL-terminated string; `st` is a valid out-ptr.
    let rc = unsafe { libc::stat(path.as_ptr(), &mut st) };
    if rc != 0 {
        return None;
    }
    // root (uid 0) owns /dev/console at the login window — not a real user.
    if st.st_uid == 0 {
        None
    } else {
        Some(st.st_uid)
    }
}

/// Where a connection should go, resolved from the policy verdict. `Ok(addr)`
/// is a listening agent; `Err(reason)` is a human-readable refusal.
pub(super) async fn resolve(
    verdict: Verdict,
    base: u16,
    primary_user: Option<&str>,
) -> Result<SocketAddr, String> {
    let uid = match verdict {
        Verdict::Deny(reason) => return Err(reason),
        Verdict::Route(user) => uid_for_user(&user)
            .ok_or_else(|| format!("no local account '{user}'"))?,
        Verdict::RouteDefault => {
            // Prefer the configured primary user if it has a live agent; else the
            // console user. This is the "default session" the spec's scenarios 1
            // and the no-cookie case route to.
            let primary_uid = primary_user.and_then(uid_for_user);
            match primary_uid {
                Some(u) if agent_listening(agent_addr(u, base)).await => u,
                _ => console_uid().ok_or_else(|| {
                    "no console session and no primary-user agent to route to".to_owned()
                })?,
            }
        }
    };
    let addr = agent_addr(uid, base);
    if agent_listening(addr).await {
        Ok(addr)
    } else {
        Err(format!(
            "no running Viga session agent for uid {uid} at {addr} \
             (user not logged in, or their session agent hasn't started)"
        ))
    }
}

/// Is a session agent listening on `addr`? A short loopback connect probe.
pub(super) async fn agent_listening(addr: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_millis(300), tokio::net::TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_port_is_deterministic_from_uid() {
        assert_eq!(agent_addr(502, 39000).port(), 39502);
        assert_eq!(agent_addr(0, 39000).port(), 39000);
        assert_eq!(agent_addr(1502, 39000).port(), 39502); // wraps mod 1000
        assert!(agent_addr(501, 39000).ip().is_loopback());
    }

    #[test]
    fn root_and_known_users_resolve() {
        // root always exists with uid 0 on macOS/Linux CI.
        assert_eq!(uid_for_user("root"), Some(0));
        assert_eq!(uid_for_user("DOMAIN\\root"), Some(0)); // domain stripped
        assert_eq!(uid_for_user("definitely-no-such-user-xyz"), None);
    }
}
