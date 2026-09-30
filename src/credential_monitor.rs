//! Keeps the RDP credentials in step with the macOS account.
//!
//! macrdp authenticates RDP clients against one static credential: the
//! account password, checked with PAM once at startup. Without this module
//! that credential never changes while the process runs, so a disabled
//! account, or a password changed at the Mac, keeps working over RDP until
//! the next restart.
//!
//! What it does:
//! - Every [`RECHECK_INTERVAL`] it checks the current password with PAM. If
//!   PAM rejects it (wrong password, disabled or expired account), RDP logins
//!   are revoked at once: the server's credentials are replaced with a random
//!   password nobody knows, and auto-unlock stops typing the old one. That
//!   password is never checked again, since every PAM failure counts toward
//!   the account's own lockout.
//! - With `--keychain`, it also reads the Keychain entry every
//!   [`KEYCHAIN_POLL_INTERVAL`]. A new value is checked with PAM once and, if
//!   accepted, becomes the RDP password. So a password change reaches RDP by
//!   updating the Keychain entry (the menu-bar controller does this), without
//!   a restart.
//!
//! A PAM error that says nothing about the password ([`Verdict::Unavailable`])
//! changes nothing; the next scheduled check tries again.
//!
//! Sessions that are already connected are not ended by a revocation. They
//! authenticated with a password that was valid at the time.
//!
//! [`Monitor`] is the pure decision logic, driven by `now` so it unit-tests
//! without a clock or PAM. [`spawn`] runs it against the real system.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use crate::auth::Verdict;
use crate::sync_ext::RwLockExt;

/// How often the current password is re-checked with PAM.
pub const RECHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// How often the Keychain entry is read, with `--keychain`.
pub const KEYCHAIN_POLL_INTERVAL: Duration = Duration::from_secs(60);
/// How many rejected passwords are remembered so they are never re-checked.
const REMEMBERED_REJECTIONS: usize = 8;

/// The password RDP currently accepts, shared with auto-unlock. `None` after a
/// revocation.
pub type SecretCell = Arc<RwLock<Option<Zeroizing<String>>>>;

/// Whether RDP logins are currently possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Active,
    Revoked,
}

/// Work that is due now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    RecheckCurrent,
    PollKeychain,
}

/// What the driver must do after feeding a result in.
#[derive(PartialEq, Eq)]
pub enum Action {
    Nothing,
    /// Stop accepting the current password.
    Revoke,
    /// Check this Keychain value with PAM, once, and report back through
    /// [`Monitor::on_validated`].
    Validate(Zeroizing<String>),
    /// Accept this password from now on.
    Install(Zeroizing<String>),
}

// Never print a password, even in a test failure.
impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nothing => f.write_str("Nothing"),
            Self::Revoke => f.write_str("Revoke"),
            Self::Validate(_) => f.write_str("Validate(..)"),
            Self::Install(_) => f.write_str("Install(..)"),
        }
    }
}

/// The decision logic. See the module docs.
pub struct Monitor {
    current: Zeroizing<String>,
    status: Status,
    rejected: Vec<Zeroizing<String>>,
    next_recheck: Instant,
    next_poll: Option<Instant>,
}

impl Monitor {
    /// `current` must be the password that just passed the startup PAM check.
    pub fn new(now: Instant, current: Zeroizing<String>, keychain: bool) -> Self {
        Self {
            current,
            status: Status::Active,
            rejected: Vec::new(),
            next_recheck: now + RECHECK_INTERVAL,
            next_poll: keychain.then(|| now + KEYCHAIN_POLL_INTERVAL),
        }
    }

    #[cfg(test)]
    pub fn status(&self) -> Status {
        self.status
    }

    /// A copy of the password being checked, for the PAM call.
    pub fn current(&self) -> Zeroizing<String> {
        self.current.clone()
    }

    /// The work due at `now`, advancing the schedule. The current password is
    /// only re-checked while it is active: a rejected one is never retried.
    pub fn due(&mut self, now: Instant) -> Vec<Due> {
        let mut due = Vec::new();
        if self.status == Status::Active && now >= self.next_recheck {
            self.next_recheck = now + RECHECK_INTERVAL;
            due.push(Due::RecheckCurrent);
        }
        if self.next_poll.is_some_and(|next| now >= next) {
            self.next_poll = Some(now + KEYCHAIN_POLL_INTERVAL);
            due.push(Due::PollKeychain);
        }
        due
    }

    /// Feed the PAM verdict on the current password.
    pub fn on_recheck(&mut self, verdict: &Verdict) -> Action {
        match verdict {
            Verdict::Rejected(_) if self.status == Status::Active => {
                self.status = Status::Revoked;
                self.remember_rejected(self.current.clone());
                Action::Revoke
            }
            _ => Action::Nothing,
        }
    }

    /// Feed a Keychain read. `None` = the entry could not be read, which is
    /// not a reason to change anything.
    pub fn on_keychain(&mut self, value: Option<Zeroizing<String>>) -> Action {
        let Some(value) = value else {
            return Action::Nothing;
        };
        if value.is_empty()
            || *value == *self.current
            || self.rejected.iter().any(|r| **r == *value)
        {
            return Action::Nothing;
        }
        Action::Validate(value)
    }

    /// Feed the PAM verdict on a Keychain value from [`Action::Validate`].
    pub fn on_validated(
        &mut self,
        now: Instant,
        candidate: Zeroizing<String>,
        verdict: &Verdict,
    ) -> Action {
        match verdict {
            Verdict::Accepted => {
                self.current = candidate.clone();
                self.status = Status::Active;
                self.next_recheck = now + RECHECK_INTERVAL;
                Action::Install(candidate)
            }
            Verdict::Rejected(_) => {
                self.remember_rejected(candidate);
                Action::Nothing
            }
            Verdict::Unavailable(_) => Action::Nothing,
        }
    }

    fn remember_rejected(&mut self, password: Zeroizing<String>) {
        if self.rejected.len() >= REMEMBERED_REJECTIONS {
            self.rejected.remove(0);
        }
        self.rejected.push(password);
    }
}

/// A random password nobody knows: 32 bytes from the system RNG, hex-encoded.
fn unguessable_password() -> Zeroizing<String> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    if getrandom::getrandom(bytes.as_mut()).is_err() {
        // No RNG: fall back to something still unknown to any client. The
        // time and address are not secret, but combined with the process id
        // this is not guessable in practice, and the case should not occur.
        let seed = format!(
            "{:?}{:p}{}",
            std::time::SystemTime::now(),
            &bytes,
            std::process::id()
        );
        for (i, b) in seed.bytes().enumerate() {
            bytes[i % 32] ^= b;
        }
    }
    let mut out = Zeroizing::new(String::with_capacity(64));
    for b in bytes.iter() {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Reads the password from the Keychain. Blocking.
pub type KeychainReader = fn(&str) -> anyhow::Result<Zeroizing<String>>;

/// What the driver needs from the rest of the program.
pub struct Wiring {
    pub username: String,
    pub credentials: ironrdp_server::CredentialsHandle,
    pub secret: SecretCell,
    /// Checks a password with PAM. Blocking; run off the async threads.
    pub check: fn(&str, &str) -> Verdict,
    /// Reads the Keychain entry, when `--keychain` is in use. Blocking.
    pub read_keychain: Option<KeychainReader>,
}

impl Wiring {
    fn install(&self, password: &str) {
        let creds = ironrdp_server::Credentials {
            username: self.username.clone(),
            password: password.to_owned(),
            domain: None,
        };
        *self.credentials.write_or_recover() = Some(creds);
    }

    fn apply(&self, action: &Action) {
        match action {
            Action::Nothing | Action::Validate(_) => {}
            Action::Revoke => {
                self.install(&unguessable_password());
                *self.secret.write_or_recover() = None;
                tracing::error!(
                    user = %self.username,
                    "the account password was rejected by the system (changed, or the account \
                     was disabled): RDP logins are now refused. Update the Keychain entry \
                     (with --keychain) or restart macrdp with the new password."
                );
            }
            Action::Install(password) => {
                self.install(password);
                *self.secret.write_or_recover() = Some(password.clone());
                tracing::info!(user = %self.username, "new password from the Keychain accepted for RDP logins");
            }
        }
    }
}

/// How often the driver wakes to look for due work.
const TICK: Duration = Duration::from_secs(15);

/// Run the monitor until the process exits.
pub fn spawn(mut monitor: Monitor, wiring: Wiring) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let wiring = Arc::new(wiring);
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            for due in monitor.due(Instant::now()) {
                let action = match due {
                    Due::RecheckCurrent => {
                        let (w, pw) = (Arc::clone(&wiring), monitor.current());
                        let verdict = blocking(move || (w.check)(&w.username, &pw)).await;
                        if let Some(Verdict::Unavailable(msg)) = &verdict {
                            tracing::warn!(%msg, "periodic password check could not run; will retry");
                        }
                        verdict.map_or(Action::Nothing, |v| monitor.on_recheck(&v))
                    }
                    Due::PollKeychain => {
                        let Some(read) = wiring.read_keychain else {
                            continue;
                        };
                        let w = Arc::clone(&wiring);
                        let value = blocking(move || read(&w.username).ok()).await.flatten();
                        monitor.on_keychain(value)
                    }
                };
                let action = match action {
                    Action::Validate(candidate) => {
                        let (w, pw) = (Arc::clone(&wiring), candidate.clone());
                        match blocking(move || (w.check)(&w.username, &pw)).await {
                            Some(verdict) => {
                                if let Verdict::Rejected(msg) = &verdict {
                                    tracing::warn!(%msg, "the Keychain password was rejected by the system; keeping the current one");
                                }
                                monitor.on_validated(Instant::now(), candidate, &verdict)
                            }
                            None => Action::Nothing,
                        }
                    }
                    other => other,
                };
                wiring.apply(&action);
            }
        }
    })
}

/// Run a blocking call on the blocking pool. `None` if it panicked.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pw(s: &str) -> Zeroizing<String> {
        Zeroizing::new(s.to_owned())
    }

    fn rejected() -> Verdict {
        Verdict::Rejected("Authentication error".into())
    }

    #[test]
    fn rechecks_on_schedule_and_polls_only_with_keychain() {
        let t0 = Instant::now();
        let mut m = Monitor::new(t0, pw("a"), false);
        assert!(m.due(t0).is_empty());
        assert_eq!(m.due(t0 + RECHECK_INTERVAL), vec![Due::RecheckCurrent]);
        assert!(m.due(t0 + RECHECK_INTERVAL).is_empty(), "rescheduled");

        let mut k = Monitor::new(t0, pw("a"), true);
        assert_eq!(k.due(t0 + KEYCHAIN_POLL_INTERVAL), vec![Due::PollKeychain]);
    }

    #[test]
    fn a_rejected_password_revokes_and_is_never_rechecked() {
        let t0 = Instant::now();
        let mut m = Monitor::new(t0, pw("old"), true);
        assert_eq!(m.on_recheck(&Verdict::Accepted), Action::Nothing);
        assert_eq!(m.on_recheck(&rejected()), Action::Revoke);
        assert_eq!(m.status(), Status::Revoked);

        // Much later: only Keychain polls are due, never another PAM check of
        // the rejected password.
        let later = t0 + RECHECK_INTERVAL * 10;
        assert_eq!(m.due(later), vec![Due::PollKeychain]);
        // The Keychain still holding the rejected password triggers nothing.
        assert_eq!(m.on_keychain(Some(pw("old"))), Action::Nothing);
    }

    #[test]
    fn a_pam_error_changes_nothing() {
        let t0 = Instant::now();
        let mut m = Monitor::new(t0, pw("a"), false);
        assert_eq!(
            m.on_recheck(&Verdict::Unavailable("module error".into())),
            Action::Nothing
        );
        assert_eq!(m.status(), Status::Active);
    }

    #[test]
    fn a_new_keychain_password_is_checked_once_and_installed() {
        let t0 = Instant::now();
        let mut m = Monitor::new(t0, pw("old"), true);
        assert_eq!(m.on_keychain(Some(pw("old"))), Action::Nothing, "unchanged");
        assert_eq!(m.on_keychain(None), Action::Nothing, "unreadable");

        let Action::Validate(candidate) = m.on_keychain(Some(pw("new"))) else {
            panic!("a changed value must be validated");
        };
        assert_eq!(
            m.on_validated(t0, candidate, &Verdict::Accepted),
            Action::Install(pw("new"))
        );
        assert_eq!(*m.current(), "new");
        assert_eq!(m.on_keychain(Some(pw("new"))), Action::Nothing);
    }

    #[test]
    fn a_rejected_keychain_value_is_not_retried_and_keeps_the_current_one() {
        let t0 = Instant::now();
        let mut m = Monitor::new(t0, pw("good"), true);
        let Action::Validate(candidate) = m.on_keychain(Some(pw("typo"))) else {
            panic!("expected validation");
        };
        assert_eq!(m.on_validated(t0, candidate, &rejected()), Action::Nothing);
        assert_eq!(m.status(), Status::Active);
        assert_eq!(*m.current(), "good");
        assert_eq!(
            m.on_keychain(Some(pw("typo"))),
            Action::Nothing,
            "never retried"
        );
    }

    #[test]
    fn revocation_is_lifted_by_a_valid_keychain_password() {
        let t0 = Instant::now();
        let mut m = Monitor::new(t0, pw("old"), true);
        assert_eq!(m.on_recheck(&rejected()), Action::Revoke);
        let Action::Validate(candidate) = m.on_keychain(Some(pw("new"))) else {
            panic!("expected validation");
        };
        let later = t0 + Duration::from_secs(90);
        assert_eq!(
            m.on_validated(later, candidate, &Verdict::Accepted),
            Action::Install(pw("new"))
        );
        assert_eq!(m.status(), Status::Active);
        assert_eq!(
            m.due(later + RECHECK_INTERVAL).first(),
            Some(&Due::RecheckCurrent)
        );
    }

    #[test]
    fn unguessable_passwords_are_long_and_distinct() {
        let a = unguessable_password();
        let b = unguessable_password();
        assert_eq!(a.len(), 64);
        assert_ne!(*a, *b);
    }

    #[test]
    fn revoke_replaces_the_credentials_and_clears_the_secret() {
        let creds: ironrdp_server::CredentialsHandle = Default::default();
        let secret: SecretCell = Arc::new(RwLock::new(Some(pw("old"))));
        let wiring = Wiring {
            username: "u".into(),
            credentials: creds.clone(),
            secret: secret.clone(),
            check: |_, _| Verdict::Accepted,
            read_keychain: None,
        };
        wiring.install("old");
        wiring.apply(&Action::Revoke);
        let now = creds
            .read_or_recover()
            .clone()
            .expect("credentials still set");
        assert_ne!(now.password, "old");
        assert_eq!(now.password.len(), 64);
        assert!(secret.read_or_recover().is_none());

        wiring.apply(&Action::Install(pw("new")));
        assert_eq!(creds.read_or_recover().as_ref().unwrap().password, "new");
        assert_eq!(
            secret.read_or_recover().as_deref().map(String::as_str),
            Some("new")
        );
    }
}
