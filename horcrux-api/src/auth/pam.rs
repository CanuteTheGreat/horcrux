///! PAM (Pluggable Authentication Modules) integration

use horcrux_common::Result;
use tracing::info;

/// PAM authenticator
pub struct PamAuthenticator {}

impl PamAuthenticator {
    pub fn new() -> Self {
        Self {}
    }

    /// Authenticate user via PAM
    pub async fn authenticate(&self, username: &str, password: &str) -> Result<bool> {
        info!("Authenticating user {} via PAM", username);

        #[cfg(target_os = "linux")]
        {
            // Use pam-sys crate for actual PAM authentication
            // For now, this is a placeholder
            // In production, we'd use: pam::Authenticator::with_password("horcrux")
            //     .unwrap()
            //     .authenticate(username, password)

            // Placeholder: always fail for security (no hardcoded passwords)
            let _ = (username, password);
            Ok(false)
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = (username, password);
            Err(horcrux_common::Error::System(
                "PAM authentication only available on Linux".to_string(),
            ))
        }
    }
}
