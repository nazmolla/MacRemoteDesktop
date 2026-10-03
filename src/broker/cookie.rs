//! Parsing the routing identity out of an RDP client's first bytes — the
//! X.224 Connection Request (MS-RDPBCGR 2.2.1.1), which travels in **cleartext
//! before TLS**. This is exactly the hook real RD Connection Brokers / RDP load
//! balancers use: the client advertises `Cookie: mstshash=<user>\r\n` (mstsc
//! when a username is pre-filled, FreeRDP with `/d:`/`/u:` depending on build),
//! and the broker routes on it without terminating TLS or NLA. Everything here
//! is pure and unit-tested; the socket plumbing lives in the parent module.
//!
//! Wire shape of the first PDU:
//! ```text
//! TPKT header:  03 00  LEN_HI LEN_LO            (version 3, reserved 0, total length)
//! X.224 CR:     LI  E0  00 00  00 00  00  ...   (CR TPDU, dst/src ref 0, class 0)
//!               [ "Cookie: mstshash=<id>\r\n" ]  (optional routing token / cookie)
//!               [ RDP_NEG_REQ (type 0x01, 8 bytes) ]
//! ```
//! We only read the cookie; the bytes are forwarded verbatim to the agent, so a
//! missing or malformed cookie is never fatal — it just means "no route hint".

/// How many leading bytes the broker should read before deciding a route. The
/// X.224 CR is small; TPKT caps a CR well under this. We read up to the TPKT
/// length and never more.
pub(super) const PEEK_MAX: usize = 1024;

/// The routing hint extracted from a connection's first PDU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Route {
    /// `Cookie: mstshash=<user>` was present — route to this user.
    User(String),
    /// A well-formed X.224 CR with no username cookie — route to the default
    /// (primary / first) agent.
    NoCookie,
    /// Not (yet) a complete/valid X.224 CR in the bytes seen.
    Incomplete,
}

/// Total length declared by the TPKT header, if `buf` holds a full 4-byte
/// header. `None` if fewer than 4 bytes or not a TPKT v3 packet.
pub(super) fn tpkt_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 || buf[0] != 0x03 {
        return None;
    }
    Some(((buf[2] as usize) << 8) | buf[3] as usize)
}

/// Parse the routing hint from the first PDU bytes. `buf` should contain at
/// least the full TPKT (see [`tpkt_len`]); if it doesn't, returns
/// [`Route::Incomplete`] so the caller reads more.
pub(super) fn parse(buf: &[u8]) -> Route {
    let Some(total) = tpkt_len(buf) else {
        return Route::Incomplete;
    };
    // Need the whole declared PDU, and a plausible X.224 CR (LI at [4], CR code
    // 0xE0 at [5]). total of 0 or an absurd length is junk, not RDP.
    if !(7..=PEEK_MAX).contains(&total) || buf.len() < total {
        return Route::Incomplete;
    }
    if buf[5] != 0xE0 {
        // Not a Connection Request — let the agent reject it; no route.
        return Route::NoCookie;
    }
    // The cookie, if any, is ASCII "Cookie: mstshash=<id>\r\n" somewhere in the
    // variable part (after the 7-byte fixed CR header). Scan the declared PDU.
    const TAG: &[u8] = b"Cookie: mstshash=";
    let body = &buf[..total];
    if let Some(pos) = find(body, TAG) {
        let rest = &body[pos + TAG.len()..];
        let end = rest
            .iter()
            .position(|&b| b == b'\r' || b == b'\n')
            .unwrap_or(rest.len());
        let id = &rest[..end];
        // mstshash values are plain account names; reject empty / non-ascii so a
        // garbage cookie can't become a bogus username lookup.
        if !id.is_empty() && id.iter().all(|&b| b.is_ascii_graphic() || b == b' ') {
            if let Ok(s) = std::str::from_utf8(id) {
                return Route::User(s.trim().to_owned());
            }
        }
    }
    Route::NoCookie
}

/// Substring search (no regex dep). Returns the index of `needle` in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a TPKT+X.224 CR with an optional cookie string.
    fn cr(cookie: Option<&str>) -> Vec<u8> {
        let mut body = vec![0xE0u8, 0, 0, 0, 0, 0]; // CR, dst/src ref, class
        if let Some(c) = cookie {
            body.extend_from_slice(format!("Cookie: mstshash={c}\r\n").as_bytes());
        }
        let li = body.len() as u8; // X.224 length indicator = bytes after LI
        let mut x224 = vec![li];
        x224.extend_from_slice(&body);
        let total = 4 + x224.len();
        let mut pkt = vec![0x03, 0x00, (total >> 8) as u8, (total & 0xff) as u8];
        pkt.extend_from_slice(&x224);
        pkt
    }

    #[test]
    fn extracts_mstshash_username() {
        assert_eq!(parse(&cr(Some("vmspike"))), Route::User("vmspike".into()));
    }

    #[test]
    fn no_cookie_is_routable_as_default() {
        assert_eq!(parse(&cr(None)), Route::NoCookie);
    }

    #[test]
    fn partial_tpkt_is_incomplete() {
        let full = cr(Some("alice"));
        assert_eq!(parse(&full[..3]), Route::Incomplete); // header not even complete
        assert_eq!(parse(&full[..full.len() - 2]), Route::Incomplete); // body short
    }

    #[test]
    fn trims_and_rejects_empty_cookie() {
        assert_eq!(parse(&cr(Some("  bob  "))), Route::User("bob".into()));
        assert_eq!(parse(&cr(Some(""))), Route::NoCookie);
    }

    #[test]
    fn non_tpkt_is_incomplete_not_a_panic() {
        assert_eq!(parse(&[0xff, 0xff, 0x00, 0x10, 0, 0, 0]), Route::Incomplete);
        assert_eq!(parse(&[]), Route::Incomplete);
    }

    #[test]
    fn cookie_with_domain_style_value() {
        // Some clients send mstshash=DOMAIN\user; we keep it verbatim (the
        // route layer maps it to a local account, stripping any domain).
        assert_eq!(
            parse(&cr(Some("CORP\\carol"))),
            Route::User("CORP\\carol".into())
        );
    }
}
