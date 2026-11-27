//! Guest provisioning secrets (docs/design/security-suite.md, guest
//! secrets): cloud-init passwords / SSH keys / tokens and Windows answer-file
//! passwords live in an encrypted store and are referenced by name. Values
//! are write-only through the API and UI.
use crate::cloudinit::CloudInitConfig;
use serde::{Deserialize, Serialize};

/// Reference syntax: a whole value `secret:<name>`, or `{{secret:<name>}}`
/// inside a longer string (a token in a runcmd line).
pub const REF_PREFIX: &str = "secret:";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SecretKind {
    #[default]
    Password,
    Sshkey,
    Token,
    Other,
}

/// What the API shows about a secret: never the value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuestSecretMeta {
    pub name: String,
    #[serde(default)]
    pub kind: SecretKind,
    #[serde(default)]
    pub description: String,
    /// Unix seconds.
    pub created: i64,
    /// Last value change (create or rotate).
    pub rotated: i64,
    /// Starts at 1, +1 per rotation.
    pub version: u32,
    /// Times a provisioning run injected it.
    #[serde(default)]
    pub used_count: u64,
    #[serde(default)]
    pub last_used: Option<i64>,
    /// Older than `rotation_warn_days` (computed by the API).
    #[serde(default)]
    pub rotation_due: bool,
}

/// Create (POST) or change (PUT) a secret. On PUT a missing `value` only
/// updates the description/kind; a present one rotates the secret.
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct GuestSecretPut {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<SecretKind>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
}

impl std::fmt::Debug for GuestSecretPut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestSecretPut")
            .field("name", &self.name)
            .field("value", &self.value.as_ref().map(|_| "<hidden>"))
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuestSecretAudit {
    pub ts: i64,
    pub actor: String,
    /// created | rotated | updated | deleted | injected | refused
    pub action: String,
    pub name: String,
    #[serde(default)]
    pub detail: String,
}

/// What a VM is provisioned with at its first start (references only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GuestProvisioning {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_init: Option<CloudInitConfig>,
}

impl GuestProvisioning {
    pub fn secret_refs(&self) -> std::collections::BTreeSet<String> {
        self.cloud_init
            .as_ref()
            .map(|c| c.secret_refs())
            .unwrap_or_default()
    }

    /// Why this cannot be stored/declared, if so.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(c) = &self.cloud_init {
            if c.hostname.trim().is_empty() {
                return Err("cloud_init.hostname is required".into());
            }
            let lit = c.literal_secret_fields();
            if !lit.is_empty() {
                return Err(format!(
                    "{} must be a secret reference (secret:<name>), not the value itself",
                    lit.join(", ")
                ));
            }
            for n in c.secret_refs() {
                valid_name(&n)?;
            }
        }
        Ok(())
    }
}

/// Settings (`[security] guest_secrets`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GuestSecretsSettings {
    /// Delete the cloud-init seed disc once the guest has booted (its agent
    /// answers, or `seed_grace_secs` of uptime passed).
    pub remove_seed_after_first_boot: bool,
    /// Uptime after which the seed goes even if no guest agent answers.
    pub seed_grace_secs: u64,
    /// Seconds between checks of a running guest.
    pub seed_poll_secs: u64,
    /// A secret whose value is older than this many days is flagged for
    /// rotation (0 = never).
    pub rotation_warn_days: u32,
    /// Audit entries kept.
    pub audit_keep: u32,
    /// Largest accepted secret value in bytes.
    pub max_value_bytes: u32,
    /// Absolute path of the key file that encrypts the store (hex, 0600,
    /// created on first use). Empty = `<data dir>/guest-secrets.key`. Keep
    /// it out of backups of the database to keep the two apart.
    pub key_file: String,
}

impl Default for GuestSecretsSettings {
    fn default() -> Self {
        Self {
            remove_seed_after_first_boot: true,
            seed_grace_secs: 600,
            seed_poll_secs: 15,
            rotation_warn_days: 90,
            audit_keep: 2000,
            max_value_bytes: 4096,
            key_file: String::new(),
        }
    }
}

impl GuestSecretsSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(30..=86_400).contains(&self.seed_grace_secs) {
            return Err("guest_secrets.seed_grace_secs: 30 to 86400".into());
        }
        if !(5..=3600).contains(&self.seed_poll_secs) {
            return Err("guest_secrets.seed_poll_secs: 5 to 3600".into());
        }
        if self.rotation_warn_days > 3650 {
            return Err("guest_secrets.rotation_warn_days: 0 (off) to 3650".into());
        }
        if !(100..=100_000).contains(&self.audit_keep) {
            return Err("guest_secrets.audit_keep: 100 to 100000".into());
        }
        if !(16..=65_536).contains(&self.max_value_bytes) {
            return Err("guest_secrets.max_value_bytes: 16 to 65536".into());
        }
        if !self.key_file.is_empty()
            && (!self.key_file.starts_with('/') || self.key_file.contains(".."))
        {
            return Err("guest_secrets.key_file: empty or an absolute path without '..'".into());
        }
        Ok(())
    }
}

/// A secret's name: 1-64 chars of `a-z 0-9 . _ -`, starting alphanumeric.
pub fn valid_name(n: &str) -> Result<(), String> {
    let ok = !n.is_empty()
        && n.len() <= 64
        && n.chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "secret name {n:?}: 1-64 characters of a-z, 0-9, '.', '_' or '-', starting with a letter or digit"
        ))
    }
}

/// `secret:<name>` as the whole string.
pub fn is_whole_ref(s: &str) -> bool {
    s.strip_prefix(REF_PREFIX)
        .is_some_and(|n| valid_name(n).is_ok())
}

/// Every secret name referenced in `s` (whole-value or `{{secret:x}}`).
pub fn refs_in(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(n) = s.strip_prefix(REF_PREFIX) {
        if valid_name(n).is_ok() {
            out.push(n.to_string());
            return out;
        }
    }
    let mut rest = s;
    while let Some(i) = rest.find("{{secret:") {
        let after = &rest[i + 9..];
        match after.find("}}") {
            Some(j) => {
                out.push(after[..j].trim().to_string());
                rest = &after[j + 2..];
            }
            None => break,
        }
    }
    out
}

/// Replace every reference in `s` with `f(name)`.
pub fn map_refs(s: &str, f: &dyn Fn(&str) -> Result<String, String>) -> Result<String, String> {
    if let Some(n) = s.strip_prefix(REF_PREFIX) {
        if valid_name(n).is_ok() {
            return f(n);
        }
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("{{secret:") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 9..];
        match after.find("}}") {
            Some(j) => {
                out.push_str(&f(after[..j].trim())?);
                rest = &after[j + 2..];
            }
            None => {
                out.push_str(&rest[i..]);
                return Ok(out);
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("db-root.pw_1").is_ok());
        assert!(valid_name("").is_err());
        assert!(valid_name("-x").is_err());
        assert!(valid_name("Upper").is_err());
        assert!(valid_name(&"a".repeat(65)).is_err());
    }

    #[test]
    fn refs() {
        assert_eq!(refs_in("secret:admin-pw"), vec!["admin-pw"]);
        assert_eq!(
            refs_in("curl -H 'Authorization: {{secret:tok}}' {{secret:b}}"),
            vec!["tok".to_string(), "b".to_string()]
        );
        assert!(refs_in("plain").is_empty());
        let f = |n: &str| Ok(format!("<{n}>"));
        assert_eq!(map_refs("secret:a", &f).unwrap(), "<a>");
        assert_eq!(
            map_refs("x {{secret:a}} y {{secret:b}}", &f).unwrap(),
            "x <a> y <b>"
        );
        assert_eq!(map_refs("no refs {{ here", &f).unwrap(), "no refs {{ here");
        assert!(map_refs("secret:a", &|_| Err("nope".into())).is_err());
    }

    #[test]
    fn stored_config_refuses_values() {
        let mut c = CloudInitConfig {
            hostname: "h".into(),
            user: Some(crate::cloudinit::UserConfig {
                name: "u".into(),
                password: None,
                plain_password: Some("hunter2".into()),
                sudo: true,
                shell: None,
            }),
            ssh_keys: vec!["ssh-ed25519 AAAA x".into(), "secret:deploy-key".into()],
            ..Default::default()
        };
        let p = GuestProvisioning {
            cloud_init: Some(c.clone()),
        };
        assert!(p.validate().unwrap_err().contains("plain_password"));
        c.user.as_mut().unwrap().plain_password = Some("secret:admin".into());
        c.ssh_keys
            .push("-----BEGIN OPENSSH PRIVATE KEY-----".into());
        let p = GuestProvisioning {
            cloud_init: Some(c.clone()),
        };
        assert!(p.validate().unwrap_err().contains("ssh_keys[2]"));
        c.ssh_keys.pop();
        let p = GuestProvisioning {
            cloud_init: Some(c),
        };
        p.validate().unwrap();
        assert_eq!(
            p.secret_refs().into_iter().collect::<Vec<_>>(),
            vec!["admin", "deploy-key"]
        );
    }

    #[test]
    fn settings_defaults_valid() {
        GuestSecretsSettings::default().validate().unwrap();
        let s = GuestSecretsSettings {
            seed_poll_secs: 1,
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }
}
