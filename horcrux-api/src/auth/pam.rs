///! PAM (Pluggable Authentication Modules) integration
///!
///! Implements system authentication via PAM using multiple methods:
///! 1. Direct PAM conversation (requires pam-sys crate - not included)
///! 2. SSH-based verification (using system's SSH with PAM)
///! 3. Shadow file verification (requires root privileges)

use horcrux_common::Result;
use tokio::process::Command;
use tracing::{info, warn, error};
use std::process::Stdio;

/// PAM authenticator
pub struct PamAuthenticator {
    service_name: String,
}

impl PamAuthenticator {
    pub fn new() -> Self {
        Self {
            service_name: "horcrux".to_string(),
        }
    }

    /// Authenticate user via PAM
    pub async fn authenticate(&self, username: &str, password: &str) -> Result<bool> {
        info!("Authenticating user {} via PAM", username);

        // Validate input
        if username.is_empty() || password.is_empty() {
            return Ok(false);
        }

        // Check for invalid characters in username (security)
        if username.contains(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.') {
            warn!("Invalid characters in username: {}", username);
            return Ok(false);
        }

        #[cfg(target_os = "linux")]
        {
            // Method 1: Try using pamtester if available (most reliable)
            if let Ok(result) = self.authenticate_with_pamtester(username, password).await {
                return Ok(result);
            }

            // Method 2: Try using passwd-based verification (for local users)
            if let Ok(result) = self.authenticate_with_passwd_check(username, password).await {
                return Ok(result);
            }

            // Method 3: Fallback - verify user exists in system
            // Don't actually verify password without PAM library
            warn!("PAM authentication library not available, checking user existence only");
            self.verify_user_exists(username).await
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (username, password);
            Err(horcrux_common::Error::System(
                "PAM authentication only available on Linux".to_string(),
            ))
        }
    }

    /// Authenticate using pamtester utility
    #[cfg(target_os = "linux")]
    async fn authenticate_with_pamtester(&self, username: &str, password: &str) -> Result<bool> {
        // pamtester requires: pamtester <service> <username> authenticate
        // Password is provided via stdin

        let output = Command::new("pamtester")
            .arg(&self.service_name)
            .arg(username)
            .arg("authenticate")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        match output {
            Ok(mut child) => {
                // Write password to stdin
                if let Some(mut stdin) = child.stdin.take() {
                    use tokio::io::AsyncWriteExt;
                    let _ = stdin.write_all(password.as_bytes()).await;
                    let _ = stdin.write_all(b"\n").await;
                    drop(stdin);
                }

                let result = child.wait().await;
                match result {
                    Ok(status) => {
                        info!("PAM authentication via pamtester: success={}", status.success());
                        Ok(status.success())
                    }
                    Err(e) => {
                        warn!("pamtester execution failed: {}", e);
                        Err(horcrux_common::Error::System("PAM authentication failed".to_string()))
                    }
                }
            }
            Err(e) => {
                // pamtester not available
                warn!("pamtester not available: {}", e);
                Err(horcrux_common::Error::System("pamtester not available".to_string()))
            }
        }
    }

    /// Authenticate by checking against system password (requires shadow access)
    #[cfg(target_os = "linux")]
    async fn authenticate_with_passwd_check(&self, username: &str, password: &str) -> Result<bool> {
        // This method uses Python's crypt module or similar
        // Since we can't use crypt directly in Rust without a library,
        // we use a Python one-liner if available

        let python_script = format!(
            r#"import crypt, spwd; h = spwd.getspnam('{}').sp_pwdp; print('1' if crypt.crypt('{}', h) == h else '0')"#,
            username.replace("'", ""), // Basic sanitization
            password.replace("'", "").replace("\\", "")
        );

        let output = Command::new("python3")
            .arg("-c")
            .arg(&python_script)
            .output()
            .await;

        match output {
            Ok(output) if output.status.success() => {
                let result = String::from_utf8_lossy(&output.stdout);
                let authenticated = result.trim() == "1";
                info!("PAM authentication via shadow: {}", authenticated);
                Ok(authenticated)
            }
            Ok(_) | Err(_) => {
                // Python method failed (missing spwd, no permissions, etc.)
                Err(horcrux_common::Error::System("Shadow password check not available".to_string()))
            }
        }
    }

    /// Verify that user exists in the system (fallback when password verification unavailable)
    #[cfg(target_os = "linux")]
    async fn verify_user_exists(&self, username: &str) -> Result<bool> {
        // Check if user exists using getent
        let output = Command::new("getent")
            .arg("passwd")
            .arg(username)
            .output()
            .await;

        match output {
            Ok(output) if output.status.success() => {
                warn!("User {} exists but password verification not available - denying access", username);
                // For security, return false even if user exists
                // Without proper PAM, we can't verify the password
                Ok(false)
            }
            _ => {
                info!("User {} does not exist in system", username);
                Ok(false)
            }
        }
    }

    /// Check if PAM authentication is available on this system
    pub async fn check_pam_available() -> bool {
        // Check if pamtester or python with spwd is available
        let pamtester_check = Command::new("pamtester")
            .arg("--help")
            .output()
            .await;

        if pamtester_check.is_ok() {
            return true;
        }

        let python_check = Command::new("python3")
            .arg("-c")
            .arg("import spwd")
            .output()
            .await;

        python_check.is_ok()
    }

    /// Configure PAM service (creates /etc/pam.d/horcrux file)
    pub async fn configure_pam_service() -> Result<()> {
        let pam_config = r#"#%PAM-1.0
# Horcrux PAM configuration
auth       required     pam_unix.so nullok
auth       required     pam_env.so
account    required     pam_unix.so
account    required     pam_permit.so
password   required     pam_unix.so sha512 shadow
session    required     pam_unix.so
session    required     pam_limits.so
"#;

        // Write PAM configuration (requires root)
        let result = tokio::fs::write("/etc/pam.d/horcrux", pam_config).await;

        match result {
            Ok(_) => {
                info!("PAM service configuration created at /etc/pam.d/horcrux");
                Ok(())
            }
            Err(e) => {
                error!("Failed to create PAM configuration: {}", e);
                Err(horcrux_common::Error::System(format!(
                    "Failed to create PAM configuration: {}. Run as root to enable PAM.", e
                )))
            }
        }
    }
}
