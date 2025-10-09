///! LDAP authentication integration
///!
///! Implements LDAP authentication using:
///! 1. ldapsearch command-line tool (most compatible)
///! 2. Direct LDAP bind verification
///! 3. Group membership checking

use horcrux_common::auth::LdapConfig;
use horcrux_common::Result;
use tokio::process::Command;
use tracing::{info, warn, error};
use std::process::Stdio;

/// LDAP authenticator
pub struct LdapAuthenticator {}

impl LdapAuthenticator {
    pub fn new() -> Self {
        Self {}
    }

    /// Authenticate user via LDAP
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
        config: &LdapConfig,
    ) -> Result<bool> {
        info!("Authenticating user {} via LDAP server {}", username, config.server);

        // Validate input
        if username.is_empty() || password.is_empty() {
            return Ok(false);
        }

        // Sanitize username for LDAP injection attacks
        if username.contains(|c: char| matches!(c, '*' | '(' | ')' | '\\' | '\0')) {
            warn!("Invalid characters in username for LDAP: {}", username);
            return Ok(false);
        }

        // Method 1: Try using ldapsearch with bind
        if let Ok(result) = self.authenticate_with_ldapsearch(username, password, config).await {
            return Ok(result);
        }

        // Method 2: Try using ldapwhoami (simpler, some LDAP servers support this)
        if let Ok(result) = self.authenticate_with_ldapwhoami(username, password, config).await {
            return Ok(result);
        }

        warn!("LDAP authentication failed for user {}", username);
        Ok(false)
    }

    /// Authenticate using ldapsearch command
    async fn authenticate_with_ldapsearch(
        &self,
        username: &str,
        password: &str,
        config: &LdapConfig,
    ) -> Result<bool> {
        // Build user DN
        let user_dn = format!("{}={},{}", config.user_attr, username, config.base_dn);

        info!("LDAP: Attempting bind as {}", user_dn);

        // Use ldapsearch to verify credentials
        // -x: simple authentication
        // -H: LDAP URI
        // -D: bind DN
        // -w: bind password (use -y file for better security in production)
        // -b: search base
        // -s base: search scope (just check bind)

        let ldap_uri = if config.port == 636 {
            format!("ldaps://{}", config.server)
        } else {
            format!("ldap://{}:{}", config.server, config.port)
        };

        let output = Command::new("ldapsearch")
            .arg("-x") // Simple auth
            .arg("-H")
            .arg(&ldap_uri)
            .arg("-D")
            .arg(&user_dn)
            .arg("-w")
            .arg(password)
            .arg("-b")
            .arg(&user_dn)
            .arg("-s")
            .arg("base") // Just verify bind, don't search
            .arg("(objectClass=*)")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(output) if output.status.success() => {
                info!("LDAP authentication successful for {}", username);
                Ok(true)
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("LDAP bind failed for {}: {}", username, stderr);
                Ok(false)
            }
            Err(e) => {
                error!("ldapsearch command failed: {}", e);
                Err(horcrux_common::Error::System("ldapsearch not available".to_string()))
            }
        }
    }

    /// Authenticate using ldapwhoami command (simpler verification)
    async fn authenticate_with_ldapwhoami(
        &self,
        username: &str,
        password: &str,
        config: &LdapConfig,
    ) -> Result<bool> {
        let user_dn = format!("{}={},{}", config.user_attr, username, config.base_dn);

        let ldap_uri = if config.port == 636 {
            format!("ldaps://{}", config.server)
        } else {
            format!("ldap://{}:{}", config.server, config.port)
        };

        let output = Command::new("ldapwhoami")
            .arg("-x")
            .arg("-H")
            .arg(&ldap_uri)
            .arg("-D")
            .arg(&user_dn)
            .arg("-w")
            .arg(password)
            .output()
            .await;

        match output {
            Ok(output) if output.status.success() => {
                info!("LDAP authentication (ldapwhoami) successful for {}", username);
                Ok(true)
            }
            Ok(_) => Ok(false),
            Err(e) => {
                warn!("ldapwhoami not available: {}", e);
                Err(horcrux_common::Error::System("ldapwhoami not available".to_string()))
            }
        }
    }


    /// Test LDAP connection
    pub async fn test_connection(config: &LdapConfig) -> Result<bool> {
        let ldap_uri = if config.port == 636 {
            format!("ldaps://{}", config.server)
        } else {
            format!("ldap://{}:{}", config.server, config.port)
        };

        info!("Testing LDAP connection to {}", ldap_uri);

        // Anonymous bind test
        let output = Command::new("ldapsearch")
            .arg("-x")
            .arg("-H")
            .arg(&ldap_uri)
            .arg("-b")
            .arg("")
            .arg("-s")
            .arg("base")
            .arg("(objectClass=*)")
            .arg("namingContexts")
            .output()
            .await;

        match output {
            Ok(output) if output.status.success() => {
                info!("LDAP server is reachable");
                Ok(true)
            }
            _ => {
                error!("LDAP server is not reachable");
                Ok(false)
            }
        }
    }

    /// Search for user in LDAP directory
    pub async fn search_user(
        username: &str,
        config: &LdapConfig,
    ) -> Result<Option<String>> {
        let ldap_uri = if config.port == 636 {
            format!("ldaps://{}", config.server)
        } else {
            format!("ldap://{}:{}", config.server, config.port)
        };

        let filter = format!("({}={})", config.user_attr, username);

        let output = Command::new("ldapsearch")
            .arg("-x")
            .arg("-H")
            .arg(&ldap_uri)
            .arg("-b")
            .arg(&config.base_dn)
            .arg(&filter)
            .arg("dn")
            .output()
            .await;

        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);

                // Parse DN from output
                for line in stdout.lines() {
                    if line.starts_with("dn: ") {
                        let dn = line.strip_prefix("dn: ").unwrap_or("").to_string();
                        return Ok(Some(dn));
                    }
                }

                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Check if LDAP tools are available
    pub async fn check_ldap_available() -> bool {
        Command::new("ldapsearch")
            .arg("-VV")
            .output()
            .await
            .is_ok()
    }
}
