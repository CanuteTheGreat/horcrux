//! ACME (RFC 8555) certificates - Let's Encrypt or any compatible CA - with
//! the HTTP-01 challenge.
//!
//! The CA validates by fetching `http://<domain>/.well-known/acme-challenge/<token>`
//! on port 80, which the plain-HTTP side of the listener answers (see
//! `tls::server`). So for ACME the redirect port must be 80
//! (`HORCRUX_HTTP_REDIRECT_PORT=80`, or a port forward). The issued
//! certificate is installed and activated without a restart, and a renewal
//! task re-orders it when it gets close to expiry.

use super::{CertificateInfo, TlsManager};
use horcrux_common::{Error, Result};
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, NewAccount,
    NewOrder, OrderStatus, RetryPolicy,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;

pub const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcmeConfig {
    #[serde(default = "default_directory")]
    pub directory_url: String,
    #[serde(default)]
    pub contact_email: Option<String>,
    pub domains: Vec<String>,
    /// PEM file with an extra trusted root for the ACME server's own HTTPS
    /// (private CAs, test CAs like Pebble).
    #[serde(default)]
    pub ca_cert: Option<String>,
    #[serde(default = "yes")]
    pub auto_renew: bool,
    #[serde(default = "thirty")]
    pub renew_days_before: i64,
    /// Must be true: the CA's terms of service.
    #[serde(default)]
    pub accept_tos: bool,
}

fn default_directory() -> String {
    LETS_ENCRYPT.into()
}
fn yes() -> bool {
    true
}
fn thirty() -> i64 {
    30
}

const CONFIG_KEY: &str = "acme_config";
const ACCOUNT_KEY: &str = "acme_account";
const LAST_RESULT_KEY: &str = "acme_last_result";

async fn get(pool: &SqlitePool, key: &str) -> Option<String> {
    crate::auth::store::get_setting(pool, key)
        .await
        .ok()
        .flatten()
}

pub async fn load_config(pool: &SqlitePool) -> Option<AcmeConfig> {
    get(pool, CONFIG_KEY)
        .await
        .and_then(|s| serde_json::from_str(&s).ok())
}

pub async fn last_result(pool: &SqlitePool) -> Option<serde_json::Value> {
    get(pool, LAST_RESULT_KEY)
        .await
        .and_then(|s| serde_json::from_str(&s).ok())
}

pub fn validate(cfg: &AcmeConfig) -> Result<()> {
    if !cfg.accept_tos {
        return Err(Error::Validation(
            "accept the ACME CA's terms of service (accept_tos)".into(),
        ));
    }
    if !cfg.directory_url.starts_with("https://") {
        return Err(Error::Validation("directory_url must be https://".into()));
    }
    if cfg.domains.is_empty() {
        return Err(Error::Validation("at least one domain is required".into()));
    }
    for d in &cfg.domains {
        let ok = !d.is_empty()
            && d.len() <= 253
            && d.contains('.')
            && d.split('.').all(|l| {
                !l.is_empty()
                    && l.len() <= 63
                    && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            });
        if !ok {
            return Err(Error::Validation(format!("'{}' is not a DNS name", d)));
        }
    }
    Ok(())
}

pub async fn save_config(pool: &SqlitePool, cfg: &AcmeConfig) -> Result<()> {
    validate(cfg)?;
    let old = load_config(pool).await;
    if old
        .map(|o| o.directory_url != cfg.directory_url)
        .unwrap_or(false)
    {
        // a different CA needs a different account
        crate::auth::store::set_setting(pool, ACCOUNT_KEY, "").await?;
    }
    crate::auth::store::set_setting(
        pool,
        CONFIG_KEY,
        &serde_json::to_string(cfg).map_err(|e| Error::System(e.to_string()))?,
    )
    .await
}

fn acme_err(e: instant_acme::Error) -> Error {
    Error::System(format!("ACME: {}", e))
}

async fn account(pool: &SqlitePool, cfg: &AcmeConfig) -> Result<Account> {
    let builder = || -> Result<instant_acme::AccountBuilder> {
        match &cfg.ca_cert {
            Some(ca) if !ca.is_empty() => Account::builder_with_root(ca).map_err(acme_err),
            _ => Account::builder().map_err(acme_err),
        }
    };
    if let Some(creds) = get(pool, ACCOUNT_KEY)
        .await
        .filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str::<AccountCredentials>(&s).ok())
    {
        if let Ok(acct) = builder()?.from_credentials(creds).await {
            return Ok(acct);
        }
    }
    let contact: Vec<String> = cfg
        .contact_email
        .iter()
        .filter(|e| !e.is_empty())
        .map(|e| format!("mailto:{}", e))
        .collect();
    let contact_refs: Vec<&str> = contact.iter().map(String::as_str).collect();
    let (acct, creds) = builder()?
        .create(
            &NewAccount {
                contact: &contact_refs,
                terms_of_service_agreed: cfg.accept_tos,
                only_return_existing: false,
            },
            cfg.directory_url.clone(),
            None,
        )
        .await
        .map_err(acme_err)?;
    crate::auth::store::set_setting(
        pool,
        ACCOUNT_KEY,
        &serde_json::to_string(&creds).map_err(|e| Error::System(e.to_string()))?,
    )
    .await?;
    Ok(acct)
}

/// Order, validate (HTTP-01), download and install a certificate.
pub async fn order_certificate(tls: &TlsManager, pool: &SqlitePool) -> Result<CertificateInfo> {
    let cfg = load_config(pool)
        .await
        .ok_or_else(|| Error::Validation("ACME is not configured".into()))?;
    validate(&cfg)?;
    let result = order_inner(tls, pool, &cfg).await;
    let record = match &result {
        Ok(info) => serde_json::json!({"ok": true, "at": chrono::Utc::now().to_rfc3339(),
                                       "valid_until": info.valid_until, "domains": cfg.domains}),
        Err(e) => serde_json::json!({"ok": false, "at": chrono::Utc::now().to_rfc3339(),
                                     "error": e.to_string(), "domains": cfg.domains}),
    };
    let _ = crate::auth::store::set_setting(pool, LAST_RESULT_KEY, &record.to_string()).await;
    result
}

async fn order_inner(
    tls: &TlsManager,
    pool: &SqlitePool,
    cfg: &AcmeConfig,
) -> Result<CertificateInfo> {
    let acct = account(pool, cfg).await?;
    let ids: Vec<Identifier> = cfg
        .domains
        .iter()
        .map(|d| Identifier::Dns(d.clone()))
        .collect();
    let mut order = acct
        .new_order(&NewOrder::new(&ids))
        .await
        .map_err(acme_err)?;
    let mut tokens = vec![];
    {
        let mut authzs = order.authorizations();
        while let Some(a) = authzs.next().await {
            let mut authz = a.map_err(acme_err)?;
            match authz.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                other => {
                    return Err(Error::System(format!("ACME authorization is {:?}", other)));
                }
            }
            let mut ch = authz.challenge(ChallengeType::Http01).ok_or_else(|| {
                Error::System("the ACME server offers no http-01 challenge".into())
            })?;
            let token = ch.token.clone();
            tls.set_acme_challenge(&token, ch.key_authorization().as_str());
            tokens.push(token);
            ch.set_ready().await.map_err(acme_err)?;
        }
    }
    let retry = RetryPolicy::default().timeout(Duration::from_secs(120));
    let status = order.poll_ready(&retry).await;
    for t in &tokens {
        tls.clear_acme_challenge(t);
    }
    let status = status.map_err(acme_err)?;
    if status != OrderStatus::Ready {
        return Err(Error::System(format!(
            "ACME order is {:?}: the CA could not validate http://{}/.well-known/acme-challenge/ (is port 80 forwarded to the redirect port?)",
            status, cfg.domains[0]
        )));
    }
    let key_pem = order.finalize().await.map_err(acme_err)?;
    let chain_pem = order.poll_certificate(&retry).await.map_err(acme_err)?;
    let info = tls.install(&chain_pem, &key_pem, "acme")?;
    tracing::info!(
        "Installed ACME certificate for {:?}, valid until {}",
        cfg.domains,
        info.valid_until
    );
    Ok(info)
}

/// Renew ACME certificates that are close to expiry (checked twice a day).
pub fn start_renewal_task(tls: Arc<TlsManager>, pool: SqlitePool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(12 * 3600));
        loop {
            interval.tick().await;
            let Some(cfg) = load_config(&pool).await else {
                continue;
            };
            if !cfg.auto_renew || tls.current_info().source != "acme" {
                continue;
            }
            let left = tls.current_info().not_after_unix - chrono::Utc::now().timestamp();
            if left > cfg.renew_days_before * 86400 {
                continue;
            }
            match order_certificate(&tls, &pool).await {
                Ok(i) => tracing::info!("ACME certificate renewed, valid until {}", i.valid_until),
                Err(e) => tracing::error!("ACME renewal failed: {}", e),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(domains: &[&str]) -> AcmeConfig {
        AcmeConfig {
            directory_url: LETS_ENCRYPT_STAGING.into(),
            contact_email: None,
            domains: domains.iter().map(|s| s.to_string()).collect(),
            ca_cert: None,
            auto_renew: true,
            renew_days_before: 30,
            accept_tos: true,
        }
    }

    #[test]
    fn config_validation() {
        assert!(validate(&cfg(&["hx.example.org"])).is_ok());
        assert!(validate(&cfg(&[])).is_err());
        assert!(validate(&cfg(&["bad_name.example"])).is_err());
        assert!(validate(&cfg(&["localhost"])).is_err());
        let mut c = cfg(&["hx.example.org"]);
        c.accept_tos = false;
        assert!(validate(&c).is_err());
        c.accept_tos = true;
        c.directory_url = "http://insecure/dir".into();
        assert!(validate(&c).is_err());
    }
}
