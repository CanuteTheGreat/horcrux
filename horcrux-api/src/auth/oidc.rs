//! OpenID Connect (OIDC) authentication provider
//!
//! Provides SSO integration with identity providers like
//! Keycloak, Auth0, Okta, Google, Microsoft Azure AD

use horcrux_common::Result;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

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

/// JSON Web Key Set (JWKS)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

/// JSON Web Key
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Jwk {
    pub kty: String,           // Key type (RSA, EC, etc.)
    pub kid: Option<String>,   // Key ID
    pub alg: Option<String>,   // Algorithm
    #[serde(rename = "use")]
    pub use_: Option<String>,  // Public key use (sig, enc)
    pub n: Option<String>,     // RSA modulus
    pub e: Option<String>,     // RSA exponent
    pub x: Option<String>,     // EC x coordinate
    pub y: Option<String>,     // EC y coordinate
    pub crv: Option<String>,   // EC curve
}

/// Cached JWKS with timestamp
#[derive(Debug, Clone)]
struct JwksCache {
    jwks: Jwks,
    fetched_at: std::time::SystemTime,
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
    jwks_cache: Arc<RwLock<Option<JwksCache>>>,
    client: reqwest::Client,
}

impl OidcProvider {
    pub fn new(config: OidcConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            discovery: Arc::new(RwLock::new(None)),
            jwks_cache: Arc::new(RwLock::new(None)),
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

    /// Fetch JWKS from the OIDC provider
    async fn fetch_jwks(&self) -> Result<Jwks> {
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

        info!("Fetching JWKS from {}", discovery.jwks_uri);

        let response = self.client
            .get(&discovery.jwks_uri)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to fetch JWKS: {}", e)))?;

        if !response.status().is_success() {
            return Err(horcrux_common::Error::System(format!(
                "JWKS fetch failed with status: {}",
                response.status()
            )));
        }

        let jwks: Jwks = response
            .json()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to parse JWKS: {}", e)))?;

        info!("JWKS fetched successfully, {} keys available", jwks.keys.len());

        Ok(jwks)
    }

    /// Get JWKS from cache or fetch if expired (cache TTL: 1 hour)
    async fn get_jwks(&self) -> Result<Jwks> {
        let cache = self.jwks_cache.read().await;

        if let Some(cached) = cache.as_ref() {
            let age = std::time::SystemTime::now()
                .duration_since(cached.fetched_at)
                .unwrap_or_default();

            // Cache valid for 1 hour
            if age.as_secs() < 3600 {
                return Ok(cached.jwks.clone());
            }
        }

        drop(cache); // Release read lock

        // Fetch fresh JWKS
        let jwks = self.fetch_jwks().await?;

        // Update cache
        let mut cache = self.jwks_cache.write().await;
        *cache = Some(JwksCache {
            jwks: jwks.clone(),
            fetched_at: std::time::SystemTime::now(),
        });

        Ok(jwks)
    }

    /// Find a JWK by key ID (kid)
    fn find_jwk<'a>(&self, jwks: &'a Jwks, kid: &str) -> Option<&'a Jwk> {
        jwks.keys.iter().find(|key| {
            key.kid.as_ref().map(|k| k == kid).unwrap_or(false)
        })
    }

    /// Convert JWK to DecodingKey
    fn jwk_to_decoding_key(&self, jwk: &Jwk) -> Result<DecodingKey> {
        match jwk.kty.as_str() {
            "RSA" => {
                // RSA key
                let n = jwk.n.as_ref()
                    .ok_or_else(|| horcrux_common::Error::System("Missing RSA modulus (n)".to_string()))?;
                let e = jwk.e.as_ref()
                    .ok_or_else(|| horcrux_common::Error::System("Missing RSA exponent (e)".to_string()))?;

                // Decode base64url encoded modulus and exponent
                use base64::engine::general_purpose::URL_SAFE_NO_PAD;
                use base64::Engine;

                let _n_bytes = URL_SAFE_NO_PAD.decode(n)
                    .map_err(|e| horcrux_common::Error::System(format!("Failed to decode RSA modulus: {}", e)))?;
                let _e_bytes = URL_SAFE_NO_PAD.decode(e)
                    .map_err(|e| horcrux_common::Error::System(format!("Failed to decode RSA exponent: {}", e)))?;

                DecodingKey::from_rsa_components(n, e)
                    .map_err(|e| horcrux_common::Error::System(format!("Failed to create RSA key: {}", e)))
            }
            "EC" => {
                // Elliptic Curve key
                let x = jwk.x.as_ref()
                    .ok_or_else(|| horcrux_common::Error::System("Missing EC x coordinate".to_string()))?;
                let y = jwk.y.as_ref()
                    .ok_or_else(|| horcrux_common::Error::System("Missing EC y coordinate".to_string()))?;

                DecodingKey::from_ec_components(x, y)
                    .map_err(|e| horcrux_common::Error::System(format!("Failed to create EC key: {}", e)))
            }
            kty => {
                Err(horcrux_common::Error::System(format!("Unsupported key type: {}", kty)))
            }
        }
    }

    /// Verify and decode ID token with full signature validation
    pub async fn verify_id_token(&self, id_token: &str) -> Result<IdTokenClaims> {
        // Step 1: Decode JWT header to get kid (key ID)
        let header = decode_header(id_token)
            .map_err(|e| horcrux_common::Error::System(format!("Failed to decode JWT header: {}", e)))?;

        let kid = header.kid
            .ok_or_else(|| horcrux_common::Error::System("JWT header missing kid (key ID)".to_string()))?;

        info!("Verifying ID token with kid: {}", kid);

        // Step 2: Fetch JWKS (from cache or provider)
        let jwks = self.get_jwks().await?;

        // Step 3: Find matching public key
        let jwk = self.find_jwk(&jwks, &kid)
            .ok_or_else(|| horcrux_common::Error::System(format!("No matching key found for kid: {}", kid)))?;

        // Step 4: Convert JWK to DecodingKey
        let decoding_key = self.jwk_to_decoding_key(jwk)?;

        // Step 5: Determine algorithm
        let algorithm = match jwk.alg.as_deref() {
            Some("RS256") => Algorithm::RS256,
            Some("RS384") => Algorithm::RS384,
            Some("RS512") => Algorithm::RS512,
            Some("ES256") => Algorithm::ES256,
            Some("ES384") => Algorithm::ES384,
            Some(alg) => {
                warn!("Unknown algorithm '{}', defaulting to RS256", alg);
                Algorithm::RS256
            }
            None => {
                warn!("No algorithm specified in JWK, defaulting to RS256");
                Algorithm::RS256
            }
        };

        // Step 6: Set up validation
        let config = self.config.read().await;
        let mut validation = Validation::new(algorithm);
        validation.set_audience(&[&config.client_id]);
        validation.set_issuer(&[&config.issuer_url]);

        // Note: Nonce validation would happen at the application level
        // where the nonce from the session can be compared
        validation.validate_nbf = true; // Validate "not before" claim

        // Step 7: Verify signature and decode claims
        let token_data = decode::<IdTokenClaims>(id_token, &decoding_key, &validation)
            .map_err(|e| horcrux_common::Error::System(format!("JWT verification failed: {}", e)))?;

        info!("ID token verified successfully for subject: {}", token_data.claims.sub);

        Ok(token_data.claims)
    }

    /// Verify ID token with nonce validation
    pub async fn verify_id_token_with_nonce(&self, id_token: &str, expected_nonce: &str) -> Result<IdTokenClaims> {
        let claims = self.verify_id_token(id_token).await?;

        // Verify nonce matches
        match &claims.nonce {
            Some(nonce) if nonce == expected_nonce => Ok(claims),
            Some(nonce) => Err(horcrux_common::Error::System(
                format!("Nonce mismatch: expected '{}', got '{}'", expected_nonce, nonce)
            )),
            None => Err(horcrux_common::Error::System(
                "ID token missing nonce claim".to_string()
            )),
        }
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
