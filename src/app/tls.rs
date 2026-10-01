//! The TLS identity: loading, generating and persisting the certificate, and
//! building the acceptor.

use super::*;

pub(super) fn default_cert_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home)
        .join("Library/Application Support")
        .join(crate::brand::NAME))
}

pub(super) fn load_pem_cert_and_key(
    cert_path: &Path,
    key_path: &Path,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let cert_file =
        fs::File::open(cert_path).with_context(|| format!("open cert {}", cert_path.display()))?;
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut BufReader::new(cert_file))
        .collect::<std::io::Result<_>>()
        .with_context(|| format!("parse cert {}", cert_path.display()))?;
    if certs.is_empty() {
        return Err(anyhow!("no certificates in {}", cert_path.display()));
    }

    let key_meta =
        fs::metadata(key_path).with_context(|| format!("stat key {}", key_path.display()))?;
    let mode = key_meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(anyhow!(
            "private key {} is group/world-accessible (mode {:o}); refusing to use it. \
             Fix with: chmod 600 {}",
            key_path.display(),
            mode,
            key_path.display(),
        ));
    }

    let key_file =
        fs::File::open(key_path).with_context(|| format!("open key {}", key_path.display()))?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(key_file))
        .with_context(|| format!("parse key {}", key_path.display()))?
        .ok_or_else(|| anyhow!("no private key in {}", key_path.display()))?;

    Ok((certs, key))
}

pub(super) fn generate_and_persist(
    cert_dir: &Path,
    cert_path: &Path,
    key_path: &Path,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    fs::create_dir_all(cert_dir)
        .with_context(|| format!("create cert dir {}", cert_dir.display()))?;

    let CertifiedKey { cert, key_pair } =
        generate_simple_self_signed(vec!["localhost".to_string()])
            .context("generate self-signed cert")?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    fs::write(cert_path, &cert_pem)
        .with_context(|| format!("write cert {}", cert_path.display()))?;

    // Create key with 0600 from the start — never let it briefly exist 0644.
    {
        use std::io::Write as _;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(key_path)
            .with_context(|| format!("create key {}", key_path.display()))?;
        f.write_all(key_pem.as_bytes())
            .with_context(|| format!("write key {}", key_path.display()))?;
    }
    // If the file pre-existed with looser perms, OpenOptions::mode is a no-op
    // on truncate — re-assert 0600 explicitly.
    fs::set_permissions(key_path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod key {}", key_path.display()))?;

    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::try_from(key_pair.serialize_der())
        .map_err(|e| anyhow!("convert key DER: {e}"))?;
    Ok((vec![cert_der], key_der))
}

/// The TLS material derived from the persisted cert/key, shared across the TCP
/// connection and the UDP multitransport flows.
pub(super) struct TlsMaterial {
    /// rustls acceptor for the main TCP RDP connection.
    pub(super) acceptor: TlsAcceptor,
    /// Raw `subjectPublicKey` BIT STRING for CredSSP channel binding.
    pub(super) spki_der: Vec<u8>,
    /// rustls config reused to secure the reliable (`UdpFecR`) UDP flow.
    pub(super) config: Arc<ServerConfig>,
    /// Cert DER, for building the lossy (`UdpFecL`) flow's DTLS context (Phase 2).
    pub(super) cert_der: Vec<u8>,
    /// Private-key DER, same purpose as `cert_der`.
    pub(super) key_der: Vec<u8>,
}

/// Expiry classification for an operator-supplied cert. Pure (testable) split
/// from the logging in [`warn_if_expiring`].
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CertExpiry {
    Expired,
    /// Within the warn window; carries days remaining.
    Soon(u64),
    Ok,
}

pub(super) fn classify_expiry(
    not_after: std::time::SystemTime,
    now: std::time::SystemTime,
    warn_days: u64,
) -> CertExpiry {
    match not_after.duration_since(now) {
        Err(_) => CertExpiry::Expired,
        Ok(remaining) => {
            let days = remaining.as_secs() / 86_400;
            if days <= warn_days {
                CertExpiry::Soon(days)
            } else {
                CertExpiry::Ok
            }
        }
    }
}

/// Warn (never refuse) if an operator's leaf cert is expired or near expiry.
/// Refusing would risk locking an operator out of their own box over a stale
/// cert; a loud warning is the right call. Best-effort: a parse failure is
/// silently skipped (the cert already loaded into rustls, so it's structurally
/// fine; only the validity read is advisory).
pub(super) fn warn_if_expiring(certs: &[CertificateDer<'static>]) {
    let Some(leaf) = certs.first() else { return };
    let Ok(parsed) = Certificate::from_der(leaf.as_ref()) else {
        return;
    };
    let not_after = parsed.tbs_certificate.validity.not_after.to_system_time();
    match classify_expiry(not_after, std::time::SystemTime::now(), 14) {
        CertExpiry::Expired => warn!(
            "operator TLS certificate has EXPIRED — clients will reject it; renew it and restart macrdp"
        ),
        CertExpiry::Soon(days) => warn!(
            days_left = days,
            "operator TLS certificate expires soon — renew before it lapses"
        ),
        CertExpiry::Ok => {}
    }
}

/// Build the TLS material. With `cert`/`key` set (operator-supplied), load
/// exactly those files — a missing/unreadable/bad-permission file is a hard
/// error, NEVER a silent self-signed fallback. With neither set, use the
/// self-signed default in `cert_dir` (load if present, else generate+persist).
/// Callers must validate the both-or-neither invariant first.
pub(super) fn make_tls_acceptor(
    cert_dir: &Path,
    cert: Option<&Path>,
    key: Option<&Path>,
) -> Result<TlsMaterial> {
    let (certs, key) = match (cert, key) {
        (Some(c), Some(k)) => {
            info!(cert = %c.display(), key = %k.display(), "loading operator-supplied TLS cert");
            let loaded = load_pem_cert_and_key(c, k)?;
            warn_if_expiring(&loaded.0);
            loaded
        }
        (None, None) => {
            let cert_path = cert_dir.join("cert.pem");
            let key_path = cert_dir.join("key.pem");
            if cert_path.exists() && key_path.exists() {
                info!(dir = %cert_dir.display(), "loading persisted TLS cert");
                load_pem_cert_and_key(&cert_path, &key_path)?
            } else {
                info!(dir = %cert_dir.display(), "generating new self-signed TLS cert");
                generate_and_persist(cert_dir, &cert_path, &key_path)?
            }
        }
        _ => anyhow::bail!("--cert and --key must be supplied together (or neither)"),
    };

    // CredSSP's public-key channel-binding hashes the raw `subjectPublicKey`
    // BIT STRING contents from the X.509 cert — i.e. the DER-encoded
    // RSAPublicKey itself, NOT the full SubjectPublicKeyInfo wrapper. See
    // sspi's `raw_peer_public_key()`. Re-deriving from a keypair, or
    // passing the SPKI sequence, both produce different bytes and the
    // server/client hashes disagree.
    let cert_der_bytes = certs
        .first()
        .ok_or_else(|| anyhow!("empty cert chain"))?
        .as_ref();
    let parsed = Certificate::from_der(cert_der_bytes).context("parse cert DER for SPKI")?;
    let spki_der = parsed
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .raw_bytes()
        .to_vec();

    // Capture the cert + key DER before they're moved into the rustls config —
    // the lossy UDP flow's DTLS server (Phase 2) is built from the same bytes.
    let cert_der = cert_der_bytes.to_vec();
    let key_der = key.secret_der().to_vec();

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("build rustls ServerConfig")?;
    // Honor SSLKEYLOGFILE (Wireshark TLS decryption) for protocol debugging.
    // KeyLogFile is a no-op unless the env var is set. Covers the TCP RDP
    // connection AND the reliable-UDP multitransport flow (same config); the
    // lossy flow's DTLS (boring) is not covered.
    config.key_log = Arc::new(rustls::KeyLogFile::new());
    if std::env::var_os("SSLKEYLOGFILE").is_some() {
        warn!(
            "SSLKEYLOGFILE is set — TLS session keys are being written to that file for \
             debugging; unset it outside of protocol-capture sessions"
        );
    }
    let config = Arc::new(config);
    // The same cert/config also secures the auxiliary UDP multitransport (MS-RDPEMT
    // over TLS for the reliable flow; DTLS for the lossy flow) — the client trusts
    // it via the main connection's TOFU. Returned so the UDP listener can reuse it
    // without re-loading the cert.
    Ok(TlsMaterial {
        acceptor: TlsAcceptor::from(Arc::clone(&config)),
        spki_der,
        config,
        cert_der,
        key_der,
    })
}

/// Load the operator's certificate and key, or the persisted self-signed pair
/// (generating it on first run), and build the TLS acceptor from them.
pub(super) fn load_material(args: &Args) -> Result<TlsMaterial> {
    let cert_dir = match args.cert_dir.clone() {
        Some(p) => p,
        None => default_cert_dir()?,
    };
    // Operator-supplied cert/key are all-or-nothing: one without the other is a
    // config error (we won't guess the missing half or silently self-sign).
    if args.cert.is_some() != args.key.is_some() {
        return Err(anyhow!(
            "--cert and --key must be supplied together (or neither, for the self-signed default)"
        ));
    }
    make_tls_acceptor(&cert_dir, args.cert.as_deref(), args.key.as_deref())
}

#[cfg(test)]
mod tls_tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "macrdp-tlstest-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write a fresh self-signed cert+key as operator-style PEM files (key 0600,
    /// as `load_pem_cert_and_key` requires). Returns (cert_path, key_path).
    fn write_operator_pem(dir: &Path) -> (PathBuf, PathBuf) {
        let ck = rcgen::generate_simple_self_signed(vec!["macrdp.example".to_string()])
            .expect("gen cert");
        let cert_path = dir.join("operator-cert.pem");
        let key_path = dir.join("operator-key.pem");
        fs::write(&cert_path, ck.cert.pem()).unwrap();
        fs::write(&key_path, ck.key_pair.serialize_pem()).unwrap();
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
        (cert_path, key_path)
    }

    #[test]
    fn classify_expiry_covers_expired_soon_ok() {
        let now = SystemTime::now();
        assert_eq!(
            classify_expiry(now - Duration::from_secs(60), now, 14),
            CertExpiry::Expired
        );
        assert_eq!(
            classify_expiry(now + Duration::from_secs(3 * 86_400), now, 14),
            CertExpiry::Soon(3)
        );
        assert_eq!(
            classify_expiry(now + Duration::from_secs(60 * 86_400), now, 14),
            CertExpiry::Ok
        );
    }

    #[test]
    fn operator_cert_loads_without_self_signing() {
        let store = unique_dir("op");
        let (cert, key) = write_operator_pem(&store);
        // cert_dir is a DIFFERENT, empty dir — proving we don't touch it.
        let cert_dir = unique_dir("op-certdir");

        let mat = make_tls_acceptor(&cert_dir, Some(&cert), Some(&key)).expect("operator load");
        assert!(
            !mat.spki_der.is_empty(),
            "SPKI must be extracted for CredSSP"
        );
        assert!(!mat.cert_der.is_empty() && !mat.key_der.is_empty());
        // No self-signed material written into cert_dir.
        assert!(!cert_dir.join("cert.pem").exists());
        assert!(!cert_dir.join("key.pem").exists());

        fs::remove_dir_all(&store).ok();
        fs::remove_dir_all(&cert_dir).ok();
    }

    #[test]
    fn operator_missing_file_is_hard_error_not_self_sign() {
        let cert_dir = unique_dir("op-missing");
        let missing_cert = cert_dir.join("nope-cert.pem");
        let missing_key = cert_dir.join("nope-key.pem");
        let res = make_tls_acceptor(&cert_dir, Some(&missing_cert), Some(&missing_key));
        assert!(
            res.is_err(),
            "a missing operator cert must error, never self-sign"
        );
        // And it must NOT have generated a fallback self-signed cert.
        assert!(!cert_dir.join("cert.pem").exists());
        fs::remove_dir_all(&cert_dir).ok();
    }

    #[test]
    fn operator_cert_without_key_is_error() {
        let store = unique_dir("op-onearg");
        let (cert, _key) = write_operator_pem(&store);
        let cert_dir = unique_dir("op-onearg-certdir");
        // Only --cert, no --key: make_tls_acceptor rejects (defensive; async_main
        // also validates before calling).
        let res = make_tls_acceptor(&cert_dir, Some(&cert), None);
        assert!(res.is_err());
        fs::remove_dir_all(&store).ok();
        fs::remove_dir_all(&cert_dir).ok();
    }

    #[test]
    fn default_path_still_generates_self_signed() {
        let cert_dir = unique_dir("default");
        let mat = make_tls_acceptor(&cert_dir, None, None).expect("default self-signed");
        assert!(!mat.spki_der.is_empty());
        // Default path persists into cert_dir for stable TOFU.
        assert!(cert_dir.join("cert.pem").exists());
        assert!(cert_dir.join("key.pem").exists());
        fs::remove_dir_all(&cert_dir).ok();
    }
}
