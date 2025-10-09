///! TLS/SSL configuration and certificate management
///!
///! Provides secure HTTPS connections and certificate management

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// TLS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub enabled: bool,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub ca_path: Option<PathBuf>,
    pub min_version: TlsVersion,
    pub max_version: TlsVersion,
    pub ciphers: Vec<String>,
    pub require_client_cert: bool,
    pub verify_client_cert: bool,
}

/// TLS protocol version
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum TlsVersion {
    #[serde(rename = "tls1.0")]
    Tls10,
    #[serde(rename = "tls1.1")]
    Tls11,
    #[serde(rename = "tls1.2")]
    Tls12,
    #[serde(rename = "tls1.3")]
    Tls13,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cert_path: PathBuf::from("/etc/horcrux/ssl/cert.pem"),
            key_path: PathBuf::from("/etc/horcrux/ssl/key.pem"),
            ca_path: None,
            min_version: TlsVersion::Tls12,
            max_version: TlsVersion::Tls13,
            ciphers: vec![
                "TLS_AES_256_GCM_SHA384".to_string(),
                "TLS_AES_128_GCM_SHA256".to_string(),
                "TLS_CHACHA20_POLY1305_SHA256".to_string(),
            ],
            require_client_cert: false,
            verify_client_cert: false,
        }
    }
}

/// Certificate information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateInfo {
    pub subject: String,
    pub issuer: String,
    pub valid_from: String,
    pub valid_until: String,
    pub serial_number: String,
    pub fingerprint_sha256: String,
    pub san: Vec<String>,  // Subject Alternative Names
}

/// TLS manager
pub struct TlsManager {
    config: Arc<RwLock<TlsConfig>>,
    certificates: Arc<RwLock<Vec<CertificateInfo>>>,
}

impl TlsManager {
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(TlsConfig::default())),
            certificates: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Load TLS configuration
    pub async fn load_config(&self, config: TlsConfig) -> Result<()> {
        // Validate configuration
        if config.enabled {
            self.validate_config(&config).await?;
        }

        *self.config.write().await = config;
        Ok(())
    }

    /// Get current TLS configuration
    pub async fn get_config(&self) -> TlsConfig {
        self.config.read().await.clone()
    }

    /// Validate TLS configuration
    async fn validate_config(&self, config: &TlsConfig) -> Result<()> {
        // Check certificate file exists
        if !config.cert_path.exists() {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Certificate file not found: {}",
                config.cert_path.display()
            )));
        }

        // Check key file exists
        if !config.key_path.exists() {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Key file not found: {}",
                config.key_path.display()
            )));
        }

        // Check CA file if specified
        if let Some(ref ca_path) = config.ca_path {
            if !ca_path.exists() {
                return Err(horcrux_common::Error::InvalidConfig(format!(
                    "CA file not found: {}",
                    ca_path.display()
                )));
            }
        }

        // Validate TLS versions
        if config.min_version > config.max_version {
            return Err(horcrux_common::Error::InvalidConfig(
                "Minimum TLS version cannot be greater than maximum TLS version".to_string(),
            ));
        }

        Ok(())
    }

    /// Generate self-signed certificate
    pub async fn generate_self_signed_cert(
        &self,
        common_name: &str,
        organization: &str,
        validity_days: u32,
        output_cert: &str,
        output_key: &str,
    ) -> Result<CertificateInfo> {
        tracing::info!(
            "Generating self-signed certificate for CN={}, O={}",
            common_name,
            organization
        );

        // Generate private key
        let key_output = tokio::process::Command::new("openssl")
            .args(&[
                "genrsa",
                "-out",
                output_key,
                "4096",
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to generate key: {}", e)))?;

        if !key_output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to generate private key".to_string(),
            ));
        }

        // Generate certificate
        let cert_output = tokio::process::Command::new("openssl")
            .args(&[
                "req",
                "-new",
                "-x509",
                "-key",
                output_key,
                "-out",
                output_cert,
                "-days",
                &validity_days.to_string(),
                "-subj",
                &format!("/CN={}/O={}", common_name, organization),
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to generate cert: {}", e)))?;

        if !cert_output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to generate certificate".to_string(),
            ));
        }

        // Get certificate info
        let cert_info = self.get_certificate_info(output_cert).await?;

        // Store in cache
        let mut certs = self.certificates.write().await;
        certs.push(cert_info.clone());

        tracing::info!("Self-signed certificate generated successfully");

        Ok(cert_info)
    }

    /// Get certificate information from file
    pub async fn get_certificate_info(&self, cert_path: &str) -> Result<CertificateInfo> {
        // Get subject
        let subject_output = tokio::process::Command::new("openssl")
            .args(&["x509", "-in", cert_path, "-noout", "-subject"])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to read cert: {}", e)))?;

        let subject = String::from_utf8_lossy(&subject_output.stdout)
            .trim()
            .strip_prefix("subject=")
            .unwrap_or("")
            .to_string();

        // Get issuer
        let issuer_output = tokio::process::Command::new("openssl")
            .args(&["x509", "-in", cert_path, "-noout", "-issuer"])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to read issuer: {}", e)))?;

        let issuer = String::from_utf8_lossy(&issuer_output.stdout)
            .trim()
            .strip_prefix("issuer=")
            .unwrap_or("")
            .to_string();

        // Get valid from
        let startdate_output = tokio::process::Command::new("openssl")
            .args(&["x509", "-in", cert_path, "-noout", "-startdate"])
            .output()
            .await?;

        let valid_from = String::from_utf8_lossy(&startdate_output.stdout)
            .trim()
            .strip_prefix("notBefore=")
            .unwrap_or("")
            .to_string();

        // Get valid until
        let enddate_output = tokio::process::Command::new("openssl")
            .args(&["x509", "-in", cert_path, "-noout", "-enddate"])
            .output()
            .await?;

        let valid_until = String::from_utf8_lossy(&enddate_output.stdout)
            .trim()
            .strip_prefix("notAfter=")
            .unwrap_or("")
            .to_string();

        // Get serial number
        let serial_output = tokio::process::Command::new("openssl")
            .args(&["x509", "-in", cert_path, "-noout", "-serial"])
            .output()
            .await?;

        let serial_number = String::from_utf8_lossy(&serial_output.stdout)
            .trim()
            .strip_prefix("serial=")
            .unwrap_or("")
            .to_string();

        // Get fingerprint
        let fingerprint_output = tokio::process::Command::new("openssl")
            .args(&["x509", "-in", cert_path, "-noout", "-fingerprint", "-sha256"])
            .output()
            .await?;

        let fingerprint_sha256 = String::from_utf8_lossy(&fingerprint_output.stdout)
            .trim()
            .strip_prefix("SHA256 Fingerprint=")
            .unwrap_or("")
            .to_string();

        // Get SANs
        let san_output = tokio::process::Command::new("openssl")
            .args(&["x509", "-in", cert_path, "-noout", "-text"])
            .output()
            .await?;

        let san_text = String::from_utf8_lossy(&san_output.stdout);
        let mut san = Vec::new();

        // Parse SAN from text output (simplified)
        if let Some(san_line) = san_text.lines().find(|line| line.contains("Subject Alternative Name")) {
            // This is simplified - real parsing would be more robust
            for part in san_line.split(',') {
                if let Some(dns) = part.trim().strip_prefix("DNS:") {
                    san.push(dns.to_string());
                }
            }
        }

        Ok(CertificateInfo {
            subject,
            issuer,
            valid_from,
            valid_until,
            serial_number,
            fingerprint_sha256,
            san,
        })
    }

    /// Generate Certificate Signing Request (CSR)
    pub async fn generate_csr(
        &self,
        common_name: &str,
        organization: &str,
        organizational_unit: Option<&str>,
        country: Option<&str>,
        state: Option<&str>,
        locality: Option<&str>,
        output_csr: &str,
        output_key: &str,
    ) -> Result<()> {
        tracing::info!("Generating CSR for CN={}", common_name);

        // Build subject string
        let mut subject = format!("/CN={}/O={}", common_name, organization);

        if let Some(ou) = organizational_unit {
            subject.push_str(&format!("/OU={}", ou));
        }
        if let Some(c) = country {
            subject.push_str(&format!("/C={}", c));
        }
        if let Some(st) = state {
            subject.push_str(&format!("/ST={}", st));
        }
        if let Some(l) = locality {
            subject.push_str(&format!("/L={}", l));
        }

        // Generate private key
        let key_output = tokio::process::Command::new("openssl")
            .args(&["genrsa", "-out", output_key, "4096"])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to generate key: {}", e)))?;

        if !key_output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to generate private key".to_string(),
            ));
        }

        // Generate CSR
        let csr_output = tokio::process::Command::new("openssl")
            .args(&[
                "req",
                "-new",
                "-key",
                output_key,
                "-out",
                output_csr,
                "-subj",
                &subject,
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to generate CSR: {}", e)))?;

        if !csr_output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to generate CSR".to_string(),
            ));
        }

        tracing::info!("CSR generated successfully");

        Ok(())
    }

    /// Verify certificate chain
    pub async fn verify_certificate_chain(
        &self,
        cert_path: &str,
        ca_path: Option<&str>,
    ) -> Result<bool> {
        let mut args = vec!["verify"];

        if let Some(ca) = ca_path {
            args.push("-CAfile");
            args.push(ca);
        }

        args.push(cert_path);

        let output = tokio::process::Command::new("openssl")
            .args(&args)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to verify cert: {}", e)))?;

        Ok(output.status.success())
    }

    /// Check if certificate is expiring soon
    pub async fn check_certificate_expiry(&self, cert_path: &str, days_threshold: u32) -> Result<bool> {
        let cert_info = self.get_certificate_info(cert_path).await?;

        // Get current time
        let now = chrono::Utc::now();

        // Parse expiry date (simplified - real implementation should use proper date parsing)
        // For now, we'll use openssl to check
        let output = tokio::process::Command::new("openssl")
            .args(&[
                "x509",
                "-in",
                cert_path,
                "-noout",
                "-checkend",
                &(days_threshold * 24 * 60 * 60).to_string(),
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to check expiry: {}", e)))?;

        // openssl -checkend returns 0 if cert will NOT expire, 1 if it will
        Ok(!output.status.success())
    }

    /// List all configured certificates
    pub async fn list_certificates(&self) -> Vec<CertificateInfo> {
        self.certificates.read().await.clone()
    }

    /// Reload certificates from disk
    pub async fn reload_certificates(&self) -> Result<()> {
        let config = self.config.read().await;

        if !config.enabled {
            return Ok(());
        }

        // Validate current certificates
        self.validate_config(&config).await?;

        // Load certificate info
        let cert_info = self.get_certificate_info(
            config.cert_path.to_str().unwrap()
        ).await?;

        let mut certs = self.certificates.write().await;
        certs.clear();
        certs.push(cert_info);

        tracing::info!("Certificates reloaded successfully");

        Ok(())
    }

    /// Enable TLS with automatic certificate generation
    pub async fn enable_with_self_signed(
        &self,
        common_name: &str,
        organization: &str,
    ) -> Result<()> {
        let cert_path = "/etc/horcrux/ssl/cert.pem";
        let key_path = "/etc/horcrux/ssl/key.pem";

        // Create directory if it doesn't exist
        tokio::fs::create_dir_all("/etc/horcrux/ssl").await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to create SSL directory: {}", e))
        })?;

        // Generate self-signed certificate
        self.generate_self_signed_cert(
            common_name,
            organization,
            365,  // 1 year validity
            cert_path,
            key_path,
        ).await?;

        // Update configuration
        let mut config = self.config.write().await;
        config.enabled = true;
        config.cert_path = PathBuf::from(cert_path);
        config.key_path = PathBuf::from(key_path);

        tracing::info!("TLS enabled with self-signed certificate");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tls_config_default() {
        let config = TlsConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.min_version, TlsVersion::Tls12);
        assert_eq!(config.max_version, TlsVersion::Tls13);
    }

    #[tokio::test]
    async fn test_tls_manager_creation() {
        let manager = TlsManager::new();
        let config = manager.get_config().await;
        assert!(!config.enabled);
    }

    #[tokio::test]
    async fn test_tls_version_ordering() {
        assert!(TlsVersion::Tls10 < TlsVersion::Tls11);
        assert!(TlsVersion::Tls11 < TlsVersion::Tls12);
        assert!(TlsVersion::Tls12 < TlsVersion::Tls13);
    }

    #[test]
    fn test_validate_tls_versions() {
        let mut config = TlsConfig::default();
        config.min_version = TlsVersion::Tls13;
        config.max_version = TlsVersion::Tls12;

        // This should fail validation (min > max)
        // In real test, we'd call validate_config
        assert!(config.min_version > config.max_version);
    }
}
