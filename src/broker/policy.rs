//! Owner policy for the multi-user broker — the two knobs the menu-bar app
//! exposes (user-requested 2026-10-03), plus an optional allow-list. Parsed
//! from a root-owned `key=value` file (same format as `config.env`), so the UI
//! edits one file after admin auth (spec §12). Pure: parsing and the routing
//! decision are unit-tested; the broker does the I/O and the PAM/NLA auth (that
//! still happens per-connection in the agent, unchanged).

use std::collections::BTreeMap;

/// Resolved broker policy. The derived default is deliberately the conservative
/// one — no primary, multi-user OFF, no allow-list — so a fresh install (or a
/// missing/unreadable policy file) behaves exactly like the single-user path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Policy {
    /// The account that always gets the full console treatment (its own virtual
    /// display, dynamic resolution, HiDPI, audio). Everyone else is served on
    /// the native-framebuffer + resize path of their background session.
    /// `None` = no designated primary (first/any user gets whatever session
    /// they land in).
    pub(super) primary_user: Option<String>,
    /// Master switch. `false` = single-user: only the primary (or, if unset,
    /// one user at a time) is served and additional users are turned away with a
    /// reason. `true` = additional users get their own background sessions.
    pub(super) multi_user: bool,
    /// Optional allow-list of local accounts permitted to connect at all. Empty
    /// = any account that passes PAM in the agent. Names are compared after
    /// normalization (see [`normalize_user`]).
    pub(super) allowed_users: Vec<String>,
}

/// Parse a `config.env`-style `key=value` body into a [`Policy`]. Unknown keys
/// are ignored (forward-compat with the shared config file). Case-insensitive
/// keys; values trimmed; `#` comment lines skipped.
pub(super) fn parse(body: &str) -> Policy {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_ascii_uppercase(), v.trim().to_owned());
        }
    }
    let truthy = |s: &str| matches!(s.trim(), "1" | "true" | "TRUE" | "yes" | "on");
    Policy {
        primary_user: map
            .get("PRIMARY_USER")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(normalize_user),
        multi_user: map.get("MULTI_USER").map(|s| truthy(s)).unwrap_or(false),
        allowed_users: map
            .get("ALLOWED_USERS")
            .map(|s| {
                s.split([',', ' ', ';'])
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(normalize_user)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Normalize an account name for comparison: strip a `DOMAIN\` or `user@realm`
/// prefix/suffix and l-case. RDP clients often send `DOMAIN\user`; local macOS
/// accounts are bare short names.
pub(super) fn normalize_user(raw: &str) -> String {
    let s = raw.trim();
    let s = s.rsplit('\\').next().unwrap_or(s); // DOMAIN\user -> user
    let s = s.split('@').next().unwrap_or(s); // user@realm -> user
    s.trim().to_ascii_lowercase()
}

/// The broker's routing verdict for an incoming connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Verdict {
    /// Route to this (normalized) user's session agent.
    Route(String),
    /// Route to the default agent — the primary user if set, else the current
    /// console user (the broker resolves "default" to a concrete session).
    RouteDefault,
    /// Refuse, with a human reason surfaced to logs / status app.
    Deny(String),
}

/// Decide what to do with a connection whose cookie resolved to `requested`
/// (`None` = no cookie present). Pure — no account lookups here; the broker maps
/// the chosen name to a uid/session afterwards.
pub(super) fn decide(policy: &Policy, requested: Option<&str>) -> Verdict {
    let requested = requested.map(normalize_user);

    // Allow-list gate (applies in both modes when set).
    if let Some(ref u) = requested {
        if !policy.allowed_users.is_empty() && !policy.allowed_users.contains(u) {
            return Verdict::Deny(format!("user '{u}' is not in the allow-list"));
        }
    }

    if policy.multi_user {
        return match requested {
            Some(u) => Verdict::Route(u),
            None => Verdict::RouteDefault,
        };
    }

    // Single-user mode: only the primary (or, if unset, the default session).
    match (&policy.primary_user, requested) {
        (Some(p), Some(u)) if &u != p => Verdict::Deny(format!(
            "multi-user is disabled; only '{p}' may connect (requested '{u}')"
        )),
        (Some(p), Some(_)) => Verdict::Route(p.clone()),
        // No cookie, or no primary configured → the single default session.
        _ => Verdict::RouteDefault,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_knobs() {
        let p = parse("PRIMARY_USER = Alice\nMULTI_USER=1\nALLOWED_USERS=alice, bob ; carol\n");
        assert_eq!(p.primary_user.as_deref(), Some("alice"));
        assert!(p.multi_user);
        assert_eq!(p.allowed_users, vec!["alice", "bob", "carol"]);
    }

    #[test]
    fn defaults_are_single_user_no_primary() {
        let p = parse("# empty\n");
        assert_eq!(p, Policy::default());
        assert!(!p.multi_user);
        assert!(p.primary_user.is_none());
    }

    #[test]
    fn normalize_strips_domain_and_realm() {
        assert_eq!(normalize_user("CORP\\Bob"), "bob");
        assert_eq!(normalize_user("bob@corp.com"), "bob");
        assert_eq!(normalize_user("  Bob  "), "bob");
    }

    #[test]
    fn multi_user_routes_by_cookie() {
        let p = parse("MULTI_USER=1");
        assert_eq!(decide(&p, Some("vmspike")), Verdict::Route("vmspike".into()));
        assert_eq!(decide(&p, None), Verdict::RouteDefault);
    }

    #[test]
    fn single_user_rejects_non_primary() {
        let p = parse("PRIMARY_USER=admin\nMULTI_USER=0");
        assert_eq!(decide(&p, Some("admin")), Verdict::Route("admin".into()));
        match decide(&p, Some("vmspike")) {
            Verdict::Deny(m) => assert!(m.contains("admin") && m.contains("vmspike")),
            other => panic!("expected Deny, got {other:?}"),
        }
        // No cookie in single-user mode → the one default session.
        assert_eq!(decide(&p, None), Verdict::RouteDefault);
    }

    #[test]
    fn allow_list_blocks_outsiders_even_in_multi_user() {
        let p = parse("MULTI_USER=1\nALLOWED_USERS=alice,bob");
        assert_eq!(decide(&p, Some("alice")), Verdict::Route("alice".into()));
        match decide(&p, Some("mallory")) {
            Verdict::Deny(m) => assert!(m.contains("mallory")),
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[test]
    fn single_user_no_primary_routes_default() {
        let p = parse("MULTI_USER=0");
        assert_eq!(decide(&p, Some("whoever")), Verdict::RouteDefault);
    }
}
