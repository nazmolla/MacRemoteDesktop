//! The account credentials: reading them at startup (Keychain, flag or
//! prompt), checking them with PAM, and handing them to the server with the
//! monitor that keeps them in step with the account.

use super::*;

/// Shell out to `security find-generic-password -s portico -a <user> -w`,
/// which prints the password on stdout. Falls back to the upstream service
/// name `macrdp`, so an entry stored before the rename keeps working. The
/// Keychain entry has to be created out-of-band; this never prompts the user
/// interactively. The tool is named by absolute path so a `security` earlier
/// on `PATH` cannot stand in for it (and the Keychain ACL is tied to
/// `/usr/bin/security` anyway, see docs/macos-gotchas.md).
pub(super) fn read_password_from_keychain(username: &str) -> Result<Zeroizing<String>> {
    const LEGACY_SERVICE: &str = "macrdp";
    let lookup = |service: &str| {
        std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", service, "-a", username, "-w"])
            .output()
            .context("invoke security(1)")
    };
    let mut out = lookup(crate::brand::ID)?;
    if !out.status.success() {
        out = lookup(LEGACY_SERVICE)?;
    }
    if !out.status.success() {
        return Err(anyhow!(
            "keychain entry not found (run: security add-generic-password -s {} -a {username} -w)",
            crate::brand::ID
        ));
    }
    // Wrap as soon as we touch the bytes so the underlying allocation is
    // zeroed when this scope drops. `security` appends a single \n.
    let mut s = Zeroizing::new(
        String::from_utf8(out.stdout).map_err(|_| anyhow!("keychain returned non-UTF8 bytes"))?,
    );
    if s.ends_with('\n') {
        s.pop();
    }
    if s.is_empty() {
        return Err(anyhow!("keychain entry for macrdp is empty"));
    }
    Ok(s)
}

/// Work out the username and password and, unless `--skip-auth`, check them
/// with PAM. Takes `--password` out of `args` so the plaintext copy there is
/// not kept around.
pub(super) fn obtain(args: &mut Args) -> Result<(String, Zeroizing<String>)> {
    let username = args
        .username
        .clone()
        .or_else(|| std::env::var("USER").ok())
        .ok_or_else(|| anyhow!("no username: pass --username or set $USER"))?;
    let password: Zeroizing<String> = if args.keychain {
        read_password_from_keychain(&username)?
    } else if let Some(p) = args.password.take() {
        warn!(
            "--password is visible to any local user via `ps` and may be saved in \
             shell history; prefer --keychain (headless) or the interactive prompt. \
             Kept only for compatibility / scripted tests."
        );
        Zeroizing::new(p)
    } else {
        Zeroizing::new(
            rpassword::prompt_password(format!("Password for {username}: "))
                .context("read password from terminal")?,
        )
    };
    if !args.skip_auth {
        auth::authenticate(&username, password.as_str())
            .with_context(|| format!("PAM auth failed for {username}"))?;
        info!(user = %username, "PAM auth ok");
    } else {
        // --skip-auth is a dev-only escape hatch. Refuse it whenever the
        // listener is reachable beyond loopback so a fat-fingered config
        // can't accidentally expose an unauth'd RDP server to the LAN.
        if !args.bind.ip().is_loopback() {
            return Err(anyhow!(
                "--skip-auth refused on non-loopback bind {} — \
                 remove --skip-auth or rebind to 127.0.0.1",
                args.bind,
            ));
        }
        warn!("--skip-auth set; using --password verbatim without PAM check (loopback only)");
    }
    Ok((username, password))
}

/// Give the server its credentials and start the monitor that keeps them in
/// step with the account (see credential_monitor.rs).
pub(super) fn install(
    server: &mut RdpServer,
    args: &Args,
    username: &str,
    password: &Zeroizing<String>,
    secret: &credential_monitor::SecretCell,
) {
    // ironrdp_server::Credentials holds a plain String, so this copy is
    // outside our control and won't be zeroed when the server shuts down.
    // Our Zeroizing<String> still wipes its own allocation at scope exit.
    server.set_credentials(Some(Credentials {
        username: username.to_owned(),
        password: password.as_str().to_owned(),
        domain: None,
    }));
    // Keep those credentials in step with the account (see
    // credential_monitor.rs). Not under --skip-auth: that password was never
    // checked with PAM, so there is nothing to keep in step with.
    if !args.skip_auth {
        credential_monitor::spawn(
            credential_monitor::Monitor::new(
                std::time::Instant::now(),
                password.clone(),
                args.keychain,
            ),
            credential_monitor::Wiring {
                username: username.to_owned(),
                credentials: server.credentials_handle(),
                secret: Arc::clone(secret),
                check: auth::check,
                read_keychain: args
                    .keychain
                    .then_some(read_password_from_keychain as credential_monitor::KeychainReader),
            },
        );
    }
}
