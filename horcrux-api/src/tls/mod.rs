//! TLS for the API/web UI listener.
//!
//! The API is served over HTTPS by default (like Proxmox VE's pveproxy on
//! :8006). On first start the node creates its own local CA (like PVE's
//! pve-root-ca) and issues the HTTPS certificate for the host's names from
//! it; importing that CA (`GET /api/tls/ca.pem`) once makes browsers trust
//! the node. An administrator can replace the certificate with an uploaded
//! one or one issued through ACME (Let's Encrypt or any RFC 8555 CA). Certificates are swapped in without a restart: the listener asks
//! [`CertResolver`] for the current key on every handshake.
//!
//! Plain-HTTP requests arriving on the HTTPS port are answered with a
//! redirect to `https://` (see [`server`]), so `http://host:8006` keeps
//! working in a browser.
pub mod acme;
pub mod mtls;
pub mod server;

use horcrux_common::{Error, Result};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

static SERVING_TLS: AtomicBool = AtomicBool::new(false);

/// Whether the API listener is serving HTTPS (cookies get `Secure`,
/// OpenID redirect URIs use `https://`).
pub fn serving_tls() -> bool {
    SERVING_TLS.load(Ordering::Relaxed)
}

pub(crate) fn set_serving_tls(on: bool) {
    SERVING_TLS.store(on, Ordering::Relaxed);
}

/// What the TLS settings page shows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub enabled: bool,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    /// Extra port answering plain HTTP with a redirect (and ACME HTTP-01).
    pub http_redirect_port: Option<u16>,
    pub min_version: String,
}

/// Certificate information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CertificateInfo {
    pub subject: String,
    pub issuer: String,
    pub valid_from: String,
    pub valid_until: String,
    pub not_after_unix: i64,
    pub serial_number: String,
    pub fingerprint_sha256: String,
    pub san: Vec<String>, // Subject Alternative Names
    /// `self-signed`, `custom` (uploaded) or `acme`
    pub source: String,
    pub self_signed: bool,
}

/// Parse the first certificate of a PEM chain.
pub fn certificate_info(cert_pem: &str) -> Result<CertificateInfo> {
    use sha2::Digest;
    let der = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .next()
        .ok_or_else(|| Error::Validation("no certificate in PEM".into()))?
        .map_err(|e| Error::Validation(format!("bad certificate PEM: {}", e)))?;
    let (_, cert) = x509_parser::parse_x509_certificate(der.as_ref())
        .map_err(|e| Error::Validation(format!("bad certificate: {}", e)))?;
    let mut san = vec![];
    if let Ok(Some(ext)) = cert.subject_alternative_name() {
        for n in &ext.value.general_names {
            match n {
                x509_parser::extensions::GeneralName::DNSName(d) => san.push(d.to_string()),
                x509_parser::extensions::GeneralName::IPAddress(ip) => {
                    let s = match ip.len() {
                        4 => std::net::IpAddr::from(<[u8; 4]>::try_from(*ip).unwrap()).to_string(),
                        16 => {
                            std::net::IpAddr::from(<[u8; 16]>::try_from(*ip).unwrap()).to_string()
                        }
                        _ => continue,
                    };
                    san.push(s);
                }
                _ => {}
            }
        }
    }
    let fp = sha2::Sha256::digest(der.as_ref());
    let fingerprint_sha256 = fp
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(":");
    let to_rfc = |t: x509_parser::time::ASN1Time| {
        chrono::DateTime::from_timestamp(t.timestamp(), 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default()
    };
    let subject = cert.subject().to_string();
    let issuer = cert.issuer().to_string();
    Ok(CertificateInfo {
        self_signed: subject == issuer,
        subject,
        issuer,
        valid_from: to_rfc(cert.validity().not_before),
        valid_until: to_rfc(cert.validity().not_after),
        not_after_unix: cert.validity().not_after.timestamp(),
        serial_number: cert.raw_serial_as_string(),
        fingerprint_sha256,
        san,
        source: String::new(),
    })
}

/// Hands rustls the current certificate on every handshake.
#[derive(Debug)]
pub struct CertResolver {
    current: RwLock<Option<Arc<CertifiedKey>>>,
}

impl ResolvesServerCert for CertResolver {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.current.read().ok()?.clone()
    }
}

/// Build a rustls key from PEM, checking the key belongs to the certificate.
pub fn certified_key(cert_pem: &str, key_pem: &str) -> Result<CertifiedKey> {
    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| Error::Validation(format!("bad certificate PEM: {}", e)))?;
    if certs.is_empty() {
        return Err(Error::Validation("no certificate in PEM".into()));
    }
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .map_err(|e| Error::Validation(format!("bad key PEM: {}", e)))?
        .ok_or_else(|| Error::Validation("no private key in PEM".into()))?;
    let signing = rustls::crypto::ring::sign::any_supported_type(&key)
        .map_err(|e| Error::Validation(format!("unsupported private key: {}", e)))?;
    let ck = CertifiedKey::new(certs, signing);
    match ck.keys_match() {
        Ok(()) => Ok(ck),
        Err(rustls::Error::InconsistentKeys(rustls::InconsistentKeys::Unknown)) => Ok(ck),
        Err(_) => Err(Error::Validation(
            "the private key does not belong to the certificate".into(),
        )),
    }
}

/// A self-signed certificate (PEM cert, PEM key) for `names`.
pub fn generate_self_signed(
    common_name: &str,
    organization: &str,
    names: &[String],
    validity_days: u32,
) -> Result<(String, String)> {
    let mut params = rcgen::CertificateParams::new(names.to_vec())
        .map_err(|e| Error::System(format!("certificate parameters: {}", e)))?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    params
        .distinguished_name
        .push(rcgen::DnType::OrganizationName, organization);
    set_validity(&mut params, validity_days);
    let key = rcgen::KeyPair::generate().map_err(|e| Error::System(format!("key: {}", e)))?;
    let cert = params
        .self_signed(&key)
        .map_err(|e| Error::System(format!("self-sign: {}", e)))?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// The node's local CA (PEM certificate, PEM key), created on first use in
/// `dir` (`ca.pem`, `ca-key.pem` mode 0600).
pub fn ensure_local_ca(dir: &Path) -> std::io::Result<(String, String)> {
    let (cert, key) = (dir.join("ca.pem"), dir.join("ca-key.pem"));
    if let (Ok(c), Ok(k)) = (
        std::fs::read_to_string(&cert),
        std::fs::read_to_string(&key),
    ) {
        return Ok((c, k));
    }
    let host = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "horcrux".into());
    let mut params = rcgen::CertificateParams::default();
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        format!("Horcrux Local CA ({})", host),
    );
    params
        .distinguished_name
        .push(rcgen::DnType::OrganizationName, "Horcrux");
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    set_validity(&mut params, 3650);
    let io = |e: rcgen::Error| std::io::Error::other(e.to_string());
    let ca_key = rcgen::KeyPair::generate().map_err(io)?;
    let ca = params.self_signed(&ca_key).map_err(io)?;
    std::fs::create_dir_all(dir)?;
    crate::middleware::auth::write_private(&key, ca_key.serialize_pem().as_bytes())?;
    std::fs::write(&cert, ca.pem())?;
    tracing::info!("Created the node's local CA at {}", cert.display());
    Ok((ca.pem(), ca_key.serialize_pem()))
}

fn set_validity(params: &mut rcgen::CertificateParams, days: u32) {
    use chrono::Datelike;
    let now = chrono::Utc::now() - chrono::Duration::days(1);
    let end = now + chrono::Duration::days(days.max(1) as i64);
    params.not_before = rcgen::date_time_ymd(now.year(), now.month() as u8, now.day() as u8);
    params.not_after = rcgen::date_time_ymd(end.year(), end.month() as u8, end.day() as u8);
}

/// A server certificate for `names` signed by the local CA. Returns the PEM
/// chain (leaf + CA) and the leaf key.
pub fn issue_from_local_ca(
    ca_pem: &str,
    ca_key_pem: &str,
    common_name: &str,
    names: &[String],
    validity_days: u32,
) -> Result<(String, String)> {
    let err = |e: rcgen::Error| Error::System(format!("certificate: {}", e));
    let ca_key = rcgen::KeyPair::from_pem(ca_key_pem).map_err(err)?;
    let issuer = rcgen::Issuer::from_ca_cert_pem(ca_pem, ca_key).map_err(err)?;
    let mut params = rcgen::CertificateParams::new(names.to_vec()).map_err(err)?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    params
        .distinguished_name
        .push(rcgen::DnType::OrganizationName, "Horcrux");
    params.is_ca = rcgen::IsCa::ExplicitNoCa;
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::DigitalSignature,
        rcgen::KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    set_validity(&mut params, validity_days);
    let key = rcgen::KeyPair::generate().map_err(err)?;
    let cert = params.signed_by(&key, &issuer).map_err(err)?;
    Ok((format!("{}{}", cert.pem(), ca_pem), key.serialize_pem()))
}

/// Names a fresh self-signed certificate is issued for.
pub fn default_names() -> Vec<String> {
    let mut names = vec![];
    if let Ok(h) = hostname::get() {
        let h = h.to_string_lossy().to_string();
        if !h.is_empty() {
            if let Some(short) = h.split('.').next() {
                names.push(short.to_string());
            }
            names.push(h);
        }
    }
    names.extend(["localhost".to_string(), "127.0.0.1".into(), "::1".into()]);
    if let Ok(extra) = std::env::var("HORCRUX_TLS_SAN") {
        names.extend(
            extra
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }
    names.sort();
    names.dedup();
    names
}

/// TLS manager: owns the certificate files and the live certificate.
pub struct TlsManager {
    paths: RwLock<(PathBuf, PathBuf)>,
    fallback_dir: PathBuf,
    resolver: Arc<CertResolver>,
    info: RwLock<CertificateInfo>,
    /// ACME HTTP-01 tokens -> key authorizations, served on plain HTTP.
    acme_challenges: RwLock<HashMap<String, String>>,
    http_redirect_port: RwLock<Option<u16>>,
}

impl Default for TlsManager {
    fn default() -> Self {
        Self::new()
    }
}

fn source_file(cert: &Path) -> PathBuf {
    cert.with_extension("source")
}

impl TlsManager {
    pub fn new() -> Self {
        Self::with_paths(
            PathBuf::from("/etc/horcrux/ssl/cert.pem"),
            PathBuf::from("/etc/horcrux/ssl/key.pem"),
            PathBuf::from("/var/lib/horcrux/ssl"),
        )
    }

    /// `fallback_dir` is used when the configured location isn't writable
    /// (e.g. running unprivileged).
    pub fn with_paths(cert: PathBuf, key: PathBuf, fallback_dir: PathBuf) -> Self {
        Self {
            paths: RwLock::new((cert, key)),
            fallback_dir,
            resolver: Arc::new(CertResolver {
                current: RwLock::new(None),
            }),
            info: RwLock::new(CertificateInfo::default()),
            acme_challenges: RwLock::new(HashMap::new()),
            http_redirect_port: RwLock::new(None),
        }
    }

    pub fn resolver(&self) -> Arc<CertResolver> {
        self.resolver.clone()
    }

    pub fn paths(&self) -> (PathBuf, PathBuf) {
        self.paths.read().unwrap().clone()
    }

    pub fn set_http_redirect_port(&self, p: Option<u16>) {
        *self.http_redirect_port.write().unwrap() = p;
    }

    pub fn config(&self) -> TlsConfig {
        let (cert_path, key_path) = self.paths();
        TlsConfig {
            enabled: serving_tls(),
            cert_path,
            key_path,
            http_redirect_port: *self.http_redirect_port.read().unwrap(),
            min_version: "tls1.2".into(),
        }
    }

    pub fn current_info(&self) -> CertificateInfo {
        self.info.read().unwrap().clone()
    }

    /// Load the configured certificate, generating a self-signed one on
    /// first start.
    pub fn ensure_certificate(&self) -> Result<CertificateInfo> {
        let (cert, key) = self.paths();
        if !(cert.exists() && key.exists()) {
            // maybe a previous unprivileged start already used the fallback
            let fb_cert = self.fallback_dir.join("cert.pem");
            let fb_key = self.fallback_dir.join("key.pem");
            if fb_cert.exists() && fb_key.exists() && std::fs::read(&cert).is_err() {
                *self.paths.write().unwrap() = (fb_cert, fb_key);
            } else {
                let info = self.issue_local(&[], 3650)?;
                tracing::info!(
                    "Issued the HTTPS certificate for {:?} from the local CA ({})",
                    info.san,
                    self.paths().0.display()
                );
                return Ok(info);
            }
        }
        self.reload()
    }

    /// Directory holding the local CA (next to the certificate).
    pub fn ca_dir(&self) -> PathBuf {
        self.paths()
            .0
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.fallback_dir.clone())
    }

    /// The local CA certificate (PEM), creating it if needed.
    pub fn local_ca_pem(&self) -> Result<String> {
        let dir = self.ca_dir();
        match ensure_local_ca(&dir) {
            Ok((c, _)) => Ok(c),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                ensure_local_ca(&self.fallback_dir)
                    .map(|(c, _)| c)
                    .map_err(|e| Error::System(format!("local CA: {}", e)))
            }
            Err(e) => Err(Error::System(format!("local CA: {}", e))),
        }
    }

    /// Issue (and switch to) a certificate from the local CA for this host's
    /// names plus `extra_names`.
    pub fn issue_local(
        &self,
        extra_names: &[String],
        validity_days: u32,
    ) -> Result<CertificateInfo> {
        let dir = self.ca_dir();
        let (ca, ca_key) = match ensure_local_ca(&dir) {
            Ok(x) => x,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                ensure_local_ca(&self.fallback_dir)
                    .map_err(|e| Error::System(format!("local CA: {}", e)))?
            }
            Err(e) => return Err(Error::System(format!("local CA: {}", e))),
        };
        let mut names = default_names();
        names.extend(extra_names.iter().filter(|n| !n.trim().is_empty()).cloned());
        names.sort();
        names.dedup();
        let cn = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .ok()
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "horcrux".into());
        let (chain, key) = issue_from_local_ca(&ca, &ca_key, &cn, &names, validity_days)?;
        self.write_files(&chain, &key, "local-ca")?;
        self.reload()
    }

    /// (Re)read the certificate files into the live listener.
    pub fn reload(&self) -> Result<CertificateInfo> {
        let (cert, key) = self.paths();
        let cert_pem = std::fs::read_to_string(&cert)
            .map_err(|e| Error::System(format!("reading {}: {}", cert.display(), e)))?;
        let key_pem = std::fs::read_to_string(&key)
            .map_err(|e| Error::System(format!("reading {}: {}", key.display(), e)))?;
        let ck = certified_key(&cert_pem, &key_pem)?;
        let mut info = certificate_info(&cert_pem)?;
        info.source = std::fs::read_to_string(source_file(&cert))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| {
                if info.self_signed {
                    "self-signed".into()
                } else {
                    "custom".into()
                }
            });
        *self.resolver.current.write().unwrap() = Some(Arc::new(ck));
        *self.info.write().unwrap() = info.clone();
        Ok(info)
    }

    /// Validate and install a new certificate + key, then switch to it.
    pub fn install(&self, cert_pem: &str, key_pem: &str, source: &str) -> Result<CertificateInfo> {
        certified_key(cert_pem, key_pem)?;
        let info = certificate_info(cert_pem)?;
        if info.not_after_unix < chrono::Utc::now().timestamp() {
            return Err(Error::Validation("certificate has expired".into()));
        }
        self.write_files(cert_pem, key_pem, source)?;
        self.reload()
    }

    fn write_files(&self, cert_pem: &str, key_pem: &str, source: &str) -> Result<()> {
        let (cert, key) = self.paths();
        match write_pair(&cert, &key, cert_pem, key_pem, source) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                let fb_cert = self.fallback_dir.join("cert.pem");
                let fb_key = self.fallback_dir.join("key.pem");
                tracing::warn!(
                    "{} is not writable ({}); keeping the certificate in {} instead",
                    cert.display(),
                    e,
                    self.fallback_dir.display()
                );
                write_pair(&fb_cert, &fb_key, cert_pem, key_pem, source)
                    .map_err(|e| Error::System(format!("writing certificate: {}", e)))?;
                *self.paths.write().unwrap() = (fb_cert, fb_key);
                Ok(())
            }
            Err(e) => Err(Error::System(format!("writing certificate: {}", e))),
        }
    }

    pub fn set_acme_challenge(&self, token: &str, key_authorization: &str) {
        self.acme_challenges
            .write()
            .unwrap()
            .insert(token.to_string(), key_authorization.to_string());
    }

    pub fn clear_acme_challenge(&self, token: &str) {
        self.acme_challenges.write().unwrap().remove(token);
    }

    pub fn acme_challenge(&self, token: &str) -> Option<String> {
        self.acme_challenges.read().unwrap().get(token).cloned()
    }
}

fn write_pair(
    cert: &Path,
    key: &Path,
    cert_pem: &str,
    key_pem: &str,
    source: &str,
) -> std::io::Result<()> {
    for p in [cert, key] {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
    }
    // key first, private, then the certificate; rename for atomic swaps
    let tmp_key = key.with_extension("pem.new");
    crate::middleware::auth::write_private(&tmp_key, key_pem.as_bytes())?;
    let tmp_cert = cert.with_extension("pem.new");
    std::fs::write(&tmp_cert, cert_pem)?;
    std::fs::rename(&tmp_key, key)?;
    std::fs::rename(&tmp_cert, cert)?;
    let _ = std::fs::write(source_file(cert), source);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_signed_round_trip_and_hot_swap() {
        let dir = tempfile::tempdir().unwrap();
        let m = TlsManager::with_paths(
            dir.path().join("ssl/cert.pem"),
            dir.path().join("ssl/key.pem"),
            dir.path().join("fallback"),
        );
        let info = m.ensure_certificate().unwrap();
        assert_eq!(info.source, "local-ca");
        assert!(info.san.iter().any(|s| s == "localhost"));
        assert!(info.san.iter().any(|s| s == "127.0.0.1"));
        // key file is private
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("ssl/key.pem"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let first = info.fingerprint_sha256.clone();

        let (c, k) = generate_self_signed("other", "Org", &["other.example".into()], 30).unwrap();
        let info2 = m.install(&c, &k, "custom").unwrap();
        assert_ne!(info2.fingerprint_sha256, first);
        assert_eq!(info2.source, "custom");
        assert_eq!(m.current_info().san, vec!["other.example".to_string()]);
        // restarting picks up the installed certificate, not a new one
        let m2 = TlsManager::with_paths(
            dir.path().join("ssl/cert.pem"),
            dir.path().join("ssl/key.pem"),
            dir.path().join("fallback"),
        );
        assert_eq!(
            m2.ensure_certificate().unwrap().fingerprint_sha256,
            info2.fingerprint_sha256
        );
    }

    #[test]
    fn local_ca_issues_a_chain_that_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let m = TlsManager::with_paths(
            dir.path().join("ssl/cert.pem"),
            dir.path().join("ssl/key.pem"),
            dir.path().join("fallback"),
        );
        let info = m.ensure_certificate().unwrap();
        assert_eq!(info.source, "local-ca");
        assert!(!info.self_signed);
        assert!(info.issuer.contains("Horcrux Local CA"), "{}", info.issuer);
        let ca = m.local_ca_pem().unwrap();
        // the served chain validates against the CA with a real verifier
        let mut roots = rustls::RootCertStore::empty();
        for c in rustls_pemfile::certs(&mut ca.as_bytes()) {
            roots.add(c.unwrap()).unwrap();
        }
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .unwrap();
        let chain: Vec<_> = rustls_pemfile::certs(
            &mut std::fs::read_to_string(dir.path().join("ssl/cert.pem"))
                .unwrap()
                .as_bytes(),
        )
        .collect::<std::result::Result<_, _>>()
        .unwrap();
        use rustls::client::danger::ServerCertVerifier;
        verifier
            .verify_server_cert(
                &chain[0],
                &chain[1..],
                &rustls::pki_types::ServerName::try_from("localhost").unwrap(),
                &[],
                rustls::pki_types::UnixTime::now(),
            )
            .expect("leaf verifies against the local CA");
        // re-issuing keeps the same CA
        let again = m.issue_local(&["extra.example".into()], 30).unwrap();
        assert_eq!(again.issuer, info.issuer);
        assert!(again.san.contains(&"extra.example".to_string()));
        assert_eq!(m.local_ca_pem().unwrap(), ca);
    }

    #[test]
    fn rejects_mismatched_key() {
        let (c, _) = generate_self_signed("a", "o", &["a".into()], 30).unwrap();
        let (_, k2) = generate_self_signed("b", "o", &["b".into()], 30).unwrap();
        let err = certified_key(&c, &k2).unwrap_err();
        assert!(err.to_string().contains("does not belong"), "{}", err);
        assert!(certified_key("garbage", &k2).is_err());
    }
}
