//! Cloud-init provisioning config (shared by the API, the declarative spec
//! and the UI). Secret-bearing fields hold a reference (`secret:<name>` or
//! `{{secret:<name>}}` inside a longer string, see `guest_secrets`), which is
//! resolved only while the seed ISO is built.
use crate::guest_secrets::{map_refs, refs_in};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Cloud-init configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CloudInitConfig {
    pub hostname: String,
    pub user: Option<UserConfig>,
    pub ssh_keys: Vec<String>,
    pub network: Option<NetworkConfig>,
    pub packages: Vec<String>,
    pub runcmd: Vec<String>,
}

/// User configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserConfig {
    pub name: String,
    /// Already-hashed password (a hash is not a secret value, but a
    /// reference is accepted here too).
    pub password: Option<String>,
    /// Clear-text password, hashed while the seed is built. A stored config
    /// must hold a `secret:<name>` reference here, never the value.
    pub plain_password: Option<String>,
    pub sudo: bool,
    pub shell: Option<String>,
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NetworkConfig {
    pub version: u8, // Version 1 or 2
    pub ethernets: Vec<EthernetConfig>,
}

/// Ethernet interface configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EthernetConfig {
    pub name: String, // e.g., "eth0"
    pub dhcp4: bool,
    pub addresses: Vec<String>, // CIDR notation
    pub gateway4: Option<String>,
    pub nameservers: Vec<String>,
}

fn looks_public_key(k: &str) -> bool {
    let k = k.trim();
    !k.contains("PRIVATE KEY")
        && ["ssh-", "ecdsa-", "sk-ssh-", "sk-ecdsa-"]
            .iter()
            .any(|p| k.starts_with(p))
}

impl CloudInitConfig {
    /// Names of every secret this config references.
    pub fn secret_refs(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut take = |s: &str| out.extend(refs_in(s));
        if let Some(u) = &self.user {
            u.password.iter().for_each(|s| take(s));
            u.plain_password.iter().for_each(|s| take(s));
        }
        self.ssh_keys.iter().for_each(|s| take(s));
        self.runcmd.iter().for_each(|s| take(s));
        self.packages.iter().for_each(|s| take(s));
        out
    }

    /// Fields that would put a secret value in the stored config: a literal
    /// password, an SSH entry that is not a public key or a reference.
    /// (Used to refuse such a config when it is stored or declared.)
    pub fn literal_secret_fields(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(u) = &self.user {
            if let Some(p) = &u.plain_password {
                if !p.is_empty() && !crate::guest_secrets::is_whole_ref(p) {
                    out.push("user.plain_password".into());
                }
            }
        }
        for (i, k) in self.ssh_keys.iter().enumerate() {
            if !crate::guest_secrets::is_whole_ref(k) && !looks_public_key(k) {
                out.push(format!("ssh_keys[{i}]"));
            }
        }
        out
    }

    /// A copy with every secret reference replaced via `f` (name -> value).
    pub fn resolved(&self, f: &dyn Fn(&str) -> Result<String, String>) -> Result<Self, String> {
        let mut c = self.clone();
        if let Some(u) = c.user.as_mut() {
            for s in [&mut u.password, &mut u.plain_password]
                .into_iter()
                .flatten()
            {
                *s = map_refs(s, f)?;
            }
        }
        for s in c.ssh_keys.iter_mut().chain(c.runcmd.iter_mut()) {
            *s = map_refs(s, f)?;
        }
        for s in c.packages.iter_mut() {
            *s = map_refs(s, f)?;
        }
        Ok(c)
    }
}
