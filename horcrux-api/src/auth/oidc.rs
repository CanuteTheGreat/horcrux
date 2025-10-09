//! OpenID Connect (OIDC) authentication provider
//!
//! Provides SSO integration with identity providers like
//! Keycloak, Auth0, Okta, Google, Microsoft Azure AD

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

/// OpenID Connect configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub enabled: bool,
    pub issuer_url: String,            // e.g., "https://keycloak.example.com/auth/realms/horcrux"
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,          // e.g., "https://horcrux.example.com/api/auth/oidc/callback"
    pub scopes: Vec<String>,           // e.g., ["openid", "profile", "email"]
    pub auto_create_users: bool,       // Automatically create users on first login
    pub role_claim: Option<String>,    // Claim name for role mapping (e.g., "roles")
    pub role_mapping: HashMap<String, String>, // Map OIDC roles to Horcrux roles
}

impl Default for OidcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            issuer_url: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            redirect_uri: String::new(),
            scopes: vec!["openid".to_string(), "profile".to_string(), "email".to_string()],
            auto_create_users: true,
            role_claim: Some("roles".to_string()),
            role_mapping: HashMap::new(),
        }
    }
}

/// OpenID Connect discovery document
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcDiscovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: String,
    pub jwks_uri: String,
    pub end_session_endpoint: Option<String>,
    pub scopes_supported: Vec<String>,
    pub response_types_supported: Vec<String>,
    pub grant_types_supported: Vec<String>,
}

/// OAuth2 token response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub scope: Option<String>,
}

/// OIDC user info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub sub: String,                           // Subject (unique user ID)
    pub name: Option<String>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub preferred_username: Option<String>,
    #[serde(flatten)]
    pub additional_claims: HashMap<String, serde_json::Value>,
}

/// ID token claims (JWT)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdTokenClaims {
    pub iss: String,      // Issuer
    pub sub: String,      // Subject
    pub aud: String,      // Audience (client_id)
    pub exp: u64,         // Expiration time
    pub iat: u64,         // Issued at
    pub nonce: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
    #[serde(flatten)]
    pub additional_claims: HashMap<String, serde_json::Value>,
}

/// OpenID Connect provider
pub struct OidcProvider {
    config: Arc<RwLock<OidcConfig>>,
    discovery: Arc<RwLock<Option<OidcDiscovery>>>,
    client: reqwest::Client,
}

impl OidcProvider {
    pub fn new(config: OidcConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            discovery: Arc::new(RwLock::new(None)),
            client: reqwest::Client::new(),
        }
    }

    /// Update OIDC configuration
    pub async fn update_config(&self, config: OidcConfig) -> Result<()> {
        let mut cfg = self.config.write().await;
        *cfg = config.clone();

        // Reload discovery document
        if config.enabled {
            drop(cfg); // Release write lock
            self.load_discovery().await?;
        }

        Ok(())
    }

    /// Get current configuration
    pub async fn get_config(&self) -> OidcConfig {
        self.config.read().await.clone()
    }

    /// Load OpenID Connect discovery document
    pub async fn load_discovery(&self) -> Result<OidcDiscovery> {
        let config = self.config.read().await;
        let discovery_url = format!("{}/.well-known/openid-configuration", config.issuer_url);

        info!("Loading OIDC discovery from {}", discovery_url);

        let response = self.client
            .get(&discovery_url)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to fetch OIDC discovery: {}", e)))?;

        if !response.status().is_success() {
            return Err(horcrux_common::Error::System(format!(
                "OIDC discovery failed with status: {}",
                response.status()
            )));
        }

        let discovery: OidcDiscovery = response
            .json()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to parse OIDC discovery: {}", e)))?;

        let mut disc = self.discovery.write().await;
        *disc = Some(discovery.clone());

        info!("OIDC discovery loaded successfully");

        Ok(discovery)
    }

    /// Get authorization URL for login
    pub async fn get_authorization_url(&self, state: &str, nonce: &str) -> Result<String> {
        let config = self.config.read().await;

        if !config.enabled {
            return Err(horcrux_common::Error::InvalidConfig("OIDC is not enabled".to_string()));
        }

        // Ensure discovery is loaded
        let discovery = {
            let disc = self.discovery.read().await;
            if disc.is_none() {
                drop(disc);
                self.load_discovery().await?
            } else {
                disc.clone().unwrap()
            }
        };

        let scopes = config.scopes.join(" ");

        let url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&nonce={}",
            discovery.authorization_endpoint,
            urlencoding::encode(&config.client_id),
            urlencoding::encode(&config.redirect_uri),
            urlencoding::encode(&scopes),
            urlencoding::encode(state),
            urlencoding::encode(nonce)
        );

        Ok(url)
    }

    /// Exchange authorization code for tokens
    pub async fn exchange_code(&self, code: &str) -> Result<TokenResponse> {
        let config = self.config.read().await;

        if !config.enabled {
            return Err(horcrux_common::Error::InvalidConfig("OIDC is not enabled".to_string()));
        }

        // Ensure discovery is loaded
        let discovery = {
            let disc = self.discovery.read().await;
            if disc.is_none() {
                drop(disc);
                self.load_discovery().await?
            } else {
                disc.clone().unwrap()
            }
        };

        let params = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &config.redirect_uri),
            ("client_id", &config.client_id),
            ("client_secret", &config.client_secret),
        ];

        info!("Exchanging authorization code for tokens");

        let response = self.client
            .post(&discovery.token_endpoint)
            .form(&params)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Token exchange failed: {}", e)))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!("Token exchange failed: {}", error_text);
            return Err(horcrux_common::Error::System(format!(
                "Token exchange failed: {}",
                error_text
            )));
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to parse token response: {}", e)))?;

        Ok(token_response)
    }

    /// Get user info from access token
    pub async fn get_user_info(&self, access_token: &str) -> Result<UserInfo> {
        let config = self.config.read().await;

        if !config.enabled {
            return Err(horcrux_common::Error::InvalidConfig("OIDC is not enabled".to_string()));
        }

        // Ensure discovery is loaded
        let discovery = {
            let disc = self.discovery.read().await;
            if disc.is_none() {
                drop(disc);
                self.load_discovery().await?
            } else {
                disc.clone().unwrap()
            }
        };

        info!("Fetching user info from {}", discovery.userinfo_endpoint);

        let response = self.client
            .get(&discovery.userinfo_endpoint)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("User info request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(horcrux_common::Error::System(format!(
                "User info request failed with status: {}",
                response.status()
            )));
        }

        let user_info: UserInfo = response
            .json()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to parse user info: {}", e)))?;

        Ok(user_info)
    }

    /// Verify and decode ID token (basic validation)
    pub async fn verify_id_token(&self, id_token: &str) -> Result<IdTokenClaims> {
        // In production, this should:
        // 1. Fetch JWKS from jwks_uri
        // 2. Verify signature using public key
        // 3. Verify issuer, audience, expiration
        // 4. Verify nonce (if provided)

        // For now, decode without verification (UNSAFE for production)
        let parts: Vec<&str> = id_token.split('.').collect();
        if parts.len() != 3 {
            return Err(horcrux_common::Error::System("Invalid JWT format".to_string()));
        }

        let payload = parts[1];
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        let decoded = URL_SAFE_NO_PAD.decode(payload)
            .map_err(|e| horcrux_common::Error::System(format!("Failed to decode JWT: {}", e)))?;

        let claims: IdTokenClaims = serde_json::from_slice(&decoded)
            .map_err(|e| horcrux_common::Error::System(format!("Failed to parse JWT claims: {}", e)))?;

        // Basic validation
        let config = self.config.read().await;
        if claims.aud != config.client_id {
            return Err(horcrux_common::Error::System("Invalid audience".to_string()));
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if claims.exp < now {
            return Err(horcrux_common::Error::System("Token expired".to_string()));
        }

        Ok(claims)
    }

    /// Refresh access token using refresh token
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<TokenResponse> {
        let config = self.config.read().await;

        if !config.enabled {
            return Err(horcrux_common::Error::InvalidConfig("OIDC is not enabled".to_string()));
        }

        // Ensure discovery is loaded
        let discovery = {
            let disc = self.discovery.read().await;
            if disc.is_none() {
                drop(disc);
                self.load_discovery().await?
            } else {
                disc.clone().unwrap()
            }
        };

        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &config.client_id),
            ("client_secret", &config.client_secret),
        ];

        info!("Refreshing access token");

        let response = self.client
            .post(&discovery.token_endpoint)
            .form(&params)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Token refresh failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(horcrux_common::Error::System(format!(
                "Token refresh failed with status: {}",
                response.status()
            )));
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to parse token response: {}", e)))?;

        Ok(token_response)
    }

    /// Logout (end session)
    pub async fn logout(&self, id_token_hint: Option<&str>) -> Result<String> {
        let config = self.config.read().await;

        if !config.enabled {
            return Err(horcrux_common::Error::InvalidConfig("OIDC is not enabled".to_string()));
        }

        // Ensure discovery is loaded
        let discovery = {
            let disc = self.discovery.read().await;
            if disc.is_none() {
                drop(disc);
                self.load_discovery().await?
            } else {
                disc.clone().unwrap()
            }
        };

        if let Some(end_session_endpoint) = discovery.end_session_endpoint {
            let mut url = format!(
                "{}?post_logout_redirect_uri={}",
                end_session_endpoint,
                urlencoding::encode(&config.redirect_uri)
            );

            if let Some(hint) = id_token_hint {
                url.push_str(&format!("&id_token_hint={}", urlencoding::encode(hint)));
            }

            Ok(url)
        } else {
            // No end session endpoint, just redirect to home
            Ok(config.redirect_uri.clone())
        }
    }

    /// Map OIDC roles to Horcrux roles
    pub async fn map_roles(&self, user_info: &UserInfo) -> Vec<String> {
        let config = self.config.read().await;

        let role_claim_name = config.role_claim.as_ref()
            .map(|s| s.as_str())
            .unwrap_or("roles");

        // Extract roles from user info
        let oidc_roles: Vec<String> = user_info
            .additional_claims
            .get(role_claim_name)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        // Map OIDC roles to Horcrux roles
        let mut mapped_roles = Vec::new();
        for oidc_role in oidc_roles {
            if let Some(horcrux_role) = config.role_mapping.get(&oidc_role) {
                mapped_roles.push(horcrux_role.clone());
            }
        }

        // If no roles mapped, assign default user role
        if mapped_roles.is_empty() {
            mapped_roles.push("user".to_string());
        }

        mapped_roles
    }
}

/// OIDC session state (for security)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcSession {
    pub state: String,
    pub nonce: String,
    pub created_at: i64,
    pub redirect_to: Option<String>,
}

impl OidcSession {
    pub fn new(redirect_to: Option<String>) -> Self {
        Self {
            state: uuid::Uuid::new_v4().to_string(),
            nonce: uuid::Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now().timestamp(),
            redirect_to,
        }
    }

    pub fn is_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp();
        now - self.created_at > 600 // 10 minutes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oidc_config_default() {
        let config = OidcConfig::default();
        assert!(!config.enabled);
        assert!(config.scopes.contains(&"openid".to_string()));
    }

    #[test]
    fn test_oidc_session() {
        let session = OidcSession::new(Some("/dashboard".to_string()));
        assert!(!session.state.is_empty());
        assert!(!session.nonce.is_empty());
        assert!(!session.is_expired());
    }

    #[tokio::test]
    async fn test_map_roles() {
        let mut config = OidcConfig::default();
        config.role_mapping.insert("admin".to_string(), "administrator".to_string());

        let provider = OidcProvider::new(config);

        let mut additional_claims = HashMap::new();
        additional_claims.insert(
            "roles".to_string(),
            serde_json::json!(["admin", "user"])
        );

        let user_info = UserInfo {
            sub: "user123".to_string(),
            name: Some("Test User".to_string()),
            given_name: None,
            family_name: None,
            email: Some("test@example.com".to_string()),
            email_verified: Some(true),
            preferred_username: Some("testuser".to_string()),
            additional_claims,
        };

        let roles = provider.map_roles(&user_info).await;
        assert!(roles.contains(&"administrator".to_string()));
    }
}
