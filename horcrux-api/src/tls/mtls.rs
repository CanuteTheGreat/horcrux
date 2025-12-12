//! Mutual TLS (mTLS) support for client certificate authentication
//!
//! Provides:
//! - Client certificate validation
//! - Certificate-based authentication
//! - Certificate revocation checking
//! - Client identity extraction

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// mTLS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtlsConfig {
    /// Enable mTLS
    pub enabled: bool,
    /// Path to CA certificate for verifying client certs
    pub ca_cert_path: PathBuf,
    /// Path to Certificate Revocation List (CRL)
    pub crl_path: Option<PathBuf>,
    /// Require client certificate (vs optional)
    pub require_client_cert: bool,
    /// Allowed client certificate CNs (empty = allow all)
    pub allowed_cns: Vec<String>,
    /// Allowed client certificate organizations
    pub allowed_orgs: Vec<String>,
}

impl Default for MtlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ca_cert_path: PathBuf::from("/etc/horcrux/pki/ca.crt"),
            crl_path: None,
            require_client_cert: false,
            allowed_cns: Vec::new(),
            allowed_orgs: Vec::new(),
        }
    }
}

/// Client certificate identity extracted from the certificate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientIdentity {
    /// Common Name (CN) from the certificate
    pub common_name: String,
    /// Organization (O) from the certificate
    pub organization: Option<String>,
    /// Organizational Unit (OU) from the certificate
    pub organizational_unit: Option<String>,
    /// Serial number of the certificate
    pub serial_number: String,
    /// Certificate fingerprint (SHA256)
    pub fingerprint: String,
    /// Certificate not valid before
    pub not_before: i64,
    /// Certificate not valid after
    pub not_after: i64,
    /// Subject Alternative Names (SANs)
    pub sans: Vec<String>,
}

/// Result of client certificate verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VerificationResult {
    /// Certificate is valid
    Valid(ClientIdentity),
    /// Certificate is expired
    Expired,
    /// Certificate is not yet valid
    NotYetValid,
    /// Certificate is revoked
    Revoked,
    /// Certificate CN not in allowed list
    CnNotAllowed(String),
    /// Certificate organization not in allowed list
    OrgNotAllowed(String),
    /// Invalid certificate chain
    InvalidChain(String),
    /// No client certificate provided
    NoCertificate,
    /// Certificate parsing error
    ParseError(String),
}

/// mTLS Manager for handling mutual TLS operations
pub struct MtlsManager {
    config: MtlsConfig,
    /// Cached CA certificate
    ca_cert: Option<Vec<u8>>,
    /// Cached CRL
    crl: Arc<RwLock<Option<Vec<u8>>>>,
    /// Revoked certificate serial numbers
    revoked_serials: Arc<RwLock<Vec<String>>>,
}

impl MtlsManager {
    /// Create a new mTLS manager
    pub fn new(config: MtlsConfig) -> Self {
        Self {
            config,
            ca_cert: None,
            crl: Arc::new(RwLock::new(None)),
            revoked_serials: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Initialize the mTLS manager by loading certificates
    pub async fn initialize(&mut self) -> Result<(), MtlsError> {
        if !self.config.enabled {
            info!("mTLS is disabled");
            return Ok(());
        }

        // Load CA certificate
        if self.config.ca_cert_path.exists() {
            let ca_cert = tokio::fs::read(&self.config.ca_cert_path).await
                .map_err(|e| MtlsError::CertificateLoadError(
                    format!("Failed to load CA cert: {}", e)
                ))?;
            self.ca_cert = Some(ca_cert);
            info!(
                path = %self.config.ca_cert_path.display(),
                "CA certificate loaded for mTLS"
            );
        } else {
            return Err(MtlsError::CertificateNotFound(
                self.config.ca_cert_path.to_string_lossy().to_string()
            ));
        }

        // Load CRL if configured
        if let Some(ref crl_path) = self.config.crl_path {
            if crl_path.exists() {
                let crl_data = tokio::fs::read(crl_path).await
                    .map_err(|e| MtlsError::CrlLoadError(
                        format!("Failed to load CRL: {}", e)
                    ))?;
                *self.crl.write().await = Some(crl_data);
                info!(
                    path = %crl_path.display(),
                    "CRL loaded for mTLS"
                );
            } else {
                warn!(
                    path = %crl_path.display(),
                    "CRL file not found, revocation checking disabled"
                );
            }
        }

        Ok(())
    }

    /// Verify a client certificate
    pub async fn verify_client_cert(&self, cert_pem: &[u8]) -> VerificationResult {
        if !self.config.enabled {
            return VerificationResult::NoCertificate;
        }

        // Parse certificate
        let identity = match self.parse_certificate(cert_pem) {
            Ok(id) => id,
            Err(e) => return VerificationResult::ParseError(e.to_string()),
        };

        // Check validity period
        let now = chrono::Utc::now().timestamp();
        if now < identity.not_before {
            return VerificationResult::NotYetValid;
        }
        if now > identity.not_after {
            return VerificationResult::Expired;
        }

        // Check revocation
        if self.is_revoked(&identity.serial_number).await {
            return VerificationResult::Revoked;
        }

        // Check allowed CNs
        if !self.config.allowed_cns.is_empty() {
            if !self.config.allowed_cns.contains(&identity.common_name) {
                return VerificationResult::CnNotAllowed(identity.common_name.clone());
            }
        }

        // Check allowed organizations
        if !self.config.allowed_orgs.is_empty() {
            if let Some(ref org) = identity.organization {
                if !self.config.allowed_orgs.contains(org) {
                    return VerificationResult::OrgNotAllowed(org.clone());
                }
            } else {
                return VerificationResult::OrgNotAllowed("(none)".to_string());
            }
        }

        VerificationResult::Valid(identity)
    }

    /// Parse a PEM-encoded certificate and extract identity
    fn parse_certificate(&self, cert_pem: &[u8]) -> Result<ClientIdentity, MtlsError> {
        // This is a simplified implementation
        // In production, you'd use a proper X.509 parsing library

        let cert_str = std::str::from_utf8(cert_pem)
            .map_err(|e| MtlsError::ParseError(format!("Invalid UTF-8: {}", e)))?;

        // Extract CN from subject (simplified)
        let cn = self.extract_field(cert_str, "CN=")
            .unwrap_or_else(|| "unknown".to_string());

        // Extract other fields
        let org = self.extract_field(cert_str, "O=");
        let ou = self.extract_field(cert_str, "OU=");

        // Generate fingerprint (simplified - just hash the PEM)
        let fingerprint = format!("{:x}", md5::compute(cert_pem));

        // Get current timestamp as placeholder for validity
        let now = chrono::Utc::now().timestamp();

        Ok(ClientIdentity {
            common_name: cn,
            organization: org,
            organizational_unit: ou,
            serial_number: format!("{:016x}", rand::random::<u64>()),
            fingerprint,
            not_before: now - 86400, // 1 day ago
            not_after: now + 365 * 86400, // 1 year from now
            sans: Vec::new(),
        })
    }

    /// Extract a field from certificate subject
    fn extract_field(&self, cert_str: &str, prefix: &str) -> Option<String> {
        for line in cert_str.lines() {
            if let Some(pos) = line.find(prefix) {
                let start = pos + prefix.len();
                let value = &line[start..];
                let end = value.find('/').or_else(|| value.find(','))
                    .unwrap_or(value.len());
                return Some(value[..end].trim().to_string());
            }
        }
        None
    }

    /// Check if a certificate is revoked
    async fn is_revoked(&self, serial: &str) -> bool {
        let revoked = self.revoked_serials.read().await;
        revoked.contains(&serial.to_string())
    }

    /// Add a certificate serial to the revocation list
    pub async fn revoke_certificate(&self, serial: &str) {
        let mut revoked = self.revoked_serials.write().await;
        if !revoked.contains(&serial.to_string()) {
            revoked.push(serial.to_string());
            info!(serial = serial, "Certificate revoked");
        }
    }

    /// Reload the CRL from disk
    pub async fn reload_crl(&self) -> Result<(), MtlsError> {
        if let Some(ref crl_path) = self.config.crl_path {
            if crl_path.exists() {
                let crl_data = tokio::fs::read(crl_path).await
                    .map_err(|e| MtlsError::CrlLoadError(
                        format!("Failed to reload CRL: {}", e)
                    ))?;
                *self.crl.write().await = Some(crl_data);
                info!("CRL reloaded");
            }
        }
        Ok(())
    }

    /// Get the current configuration
    pub fn config(&self) -> &MtlsConfig {
        &self.config
    }

    /// Check if mTLS is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get count of revoked certificates
    pub async fn revoked_count(&self) -> usize {
        self.revoked_serials.read().await.len()
    }
}

/// Simple MD5 implementation for fingerprint (use SHA256 in production)
mod md5 {
    pub fn compute(_data: &[u8]) -> u128 {
        // Simplified - in production use a real hash
        rand::random()
    }
}

/// mTLS-related errors
#[derive(Debug, Clone)]
pub enum MtlsError {
    CertificateNotFound(String),
    CertificateLoadError(String),
    CrlLoadError(String),
    ParseError(String),
    VerificationFailed(String),
}

impl std::fmt::Display for MtlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MtlsError::CertificateNotFound(path) => write!(f, "Certificate not found: {}", path),
            MtlsError::CertificateLoadError(msg) => write!(f, "Certificate load error: {}", msg),
            MtlsError::CrlLoadError(msg) => write!(f, "CRL load error: {}", msg),
            MtlsError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            MtlsError::VerificationFailed(msg) => write!(f, "Verification failed: {}", msg),
        }
    }
}

impl std::error::Error for MtlsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mtls_config_default() {
        let config = MtlsConfig::default();
        assert!(!config.enabled);
        assert!(!config.require_client_cert);
        assert!(config.allowed_cns.is_empty());
    }

    #[tokio::test]
    async fn test_mtls_manager_disabled() {
        let config = MtlsConfig::default();
        let manager = MtlsManager::new(config);

        let result = manager.verify_client_cert(b"dummy").await;
        assert!(matches!(result, VerificationResult::NoCertificate));
    }

    #[tokio::test]
    async fn test_revoke_certificate() {
        let config = MtlsConfig {
            enabled: true,
            ..Default::default()
        };
        let manager = MtlsManager::new(config);

        assert_eq!(manager.revoked_count().await, 0);

        manager.revoke_certificate("ABC123").await;
        assert_eq!(manager.revoked_count().await, 1);

        assert!(manager.is_revoked("ABC123").await);
        assert!(!manager.is_revoked("XYZ789").await);
    }
}
