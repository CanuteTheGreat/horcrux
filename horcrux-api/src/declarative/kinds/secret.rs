//! Secret references. A spec never carries a secret value: where a kind
//! needs one (a user's password) it names where the server reads it from at
//! apply time. Export never writes a reference it cannot know.
//!
//! ```yaml
//! password_ref: {file: /etc/horcrux/secrets/alice}     # trailing newline trimmed
//! password_ref: {env: ALICE_PASSWORD}                  # the API server's environment
//! password_ref: {vault: kv/horcrux/alice, key: password}
//! ```
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SecretRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<String>,
    /// Key inside the vault secret (default `value`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl SecretRef {
    /// Exactly one source, well formed.
    pub fn validate(&self) -> Result<(), String> {
        let n = [&self.file, &self.env, &self.vault]
            .iter()
            .filter(|x| x.is_some())
            .count();
        if n != 1 {
            return Err("a secret reference names exactly one of file, env or vault".into());
        }
        if self.key.is_some() && self.vault.is_none() {
            return Err("`key` only applies to a vault reference".into());
        }
        if let Some(f) = &self.file {
            if !f.starts_with('/') || f.split('/').any(|s| s == "..") {
                return Err(format!(
                    "secret file {f:?} must be an absolute path without .."
                ));
            }
        }
        if let Some(e) = &self.env {
            let ok = !e.is_empty()
                && e.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !e.starts_with(|c: char| c.is_ascii_digit());
            if !ok {
                return Err(format!("invalid environment variable name {e:?}"));
            }
        }
        if let Some(v) = &self.vault {
            if v.trim().is_empty() {
                return Err("vault path is empty".into());
            }
        }
        Ok(())
    }

    /// Where it comes from, for messages (never the value).
    pub fn describe(&self) -> String {
        if let Some(f) = &self.file {
            format!("file {f}")
        } else if let Some(e) = &self.env {
            format!("env {e}")
        } else if let Some(v) = &self.vault {
            format!("vault {v}#{}", self.key.as_deref().unwrap_or("value"))
        } else {
            "empty reference".into()
        }
    }

    /// Read the value without waiting: files and environment variables.
    /// `Ok(None)` for a vault reference (that needs the async client).
    pub fn resolve_sync(&self) -> Result<Option<String>, String> {
        self.validate()?;
        if let Some(f) = &self.file {
            let raw = std::fs::read_to_string(f).map_err(|e| format!("secret file {f}: {e}"))?;
            return Ok(Some(raw.trim_end_matches(['\n', '\r']).to_string()));
        }
        if let Some(e) = &self.env {
            return std::env::var(e)
                .map(Some)
                .map_err(|_| format!("environment variable {e} is not set"));
        }
        Ok(None)
    }

    /// Read the value. Errors name the reference, never the content.
    pub async fn resolve(&self, vault: &crate::secrets::VaultManager) -> Result<String, String> {
        self.validate()?;
        if let Some(f) = &self.file {
            let raw = tokio::fs::read_to_string(f)
                .await
                .map_err(|e| format!("secret file {f}: {e}"))?;
            return Ok(raw.trim_end_matches(['\n', '\r']).to_string());
        }
        if let Some(e) = &self.env {
            return std::env::var(e).map_err(|_| format!("environment variable {e} is not set"));
        }
        let path = self.vault.as_deref().unwrap_or_default();
        let key = self.key.as_deref().unwrap_or("value");
        let s = vault
            .read_secret(path)
            .await
            .map_err(|e| format!("vault {path}: {e:?}"))?;
        s.data
            .get(key)
            .cloned()
            .ok_or_else(|| format!("vault {path} has no key {key}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(file: Option<&str>, env: Option<&str>, vault: Option<&str>) -> SecretRef {
        SecretRef {
            file: file.map(Into::into),
            env: env.map(Into::into),
            vault: vault.map(Into::into),
            key: None,
        }
    }

    #[test]
    fn exactly_one_source() {
        assert!(r(None, None, None).validate().is_err());
        assert!(r(Some("/a"), Some("X"), None).validate().is_err());
        assert!(r(Some("/etc/s"), None, None).validate().is_ok());
        assert!(r(None, Some("MY_PW"), None).validate().is_ok());
        assert!(r(None, None, Some("kv/x")).validate().is_ok());
    }

    #[test]
    fn rejects_relative_and_traversal_files_and_bad_env_names() {
        assert!(r(Some("rel/path"), None, None).validate().is_err());
        assert!(r(Some("/etc/../shadow"), None, None).validate().is_err());
        assert!(r(None, Some("1BAD"), None).validate().is_err());
        assert!(r(None, Some("A-B"), None).validate().is_err());
        let mut k = r(Some("/x"), None, None);
        k.key = Some("k".into());
        assert!(k.validate().is_err());
    }

    #[test]
    fn describe_never_contains_a_value() {
        assert_eq!(r(Some("/x/y"), None, None).describe(), "file /x/y");
        assert_eq!(r(None, Some("PW"), None).describe(), "env PW");
    }
}
