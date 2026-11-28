//! Import from Proxmox VE (`/api/import/proxmox/*`).
use super::{post_json, ApiError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PveRow {
    pub key: String,
    #[serde(default)]
    pub value: String,
    /// `carried`, `changed`, `not-carried`, `ignored`.
    pub carry: String,
    #[serde(default)]
    pub horcrux: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PveVolume {
    pub role: String,
    pub key: String,
    pub source: String,
    #[serde(default)]
    pub resolved: Option<serde_json::Value>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PveDiffRow {
    pub fact: String,
    pub proxmox: String,
    pub horcrux: String,
    pub same: bool,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PveGuest {
    /// `vm` or `ct`.
    pub kind: String,
    pub vmid: u32,
    pub name: String,
    pub target_id: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub volumes: Vec<PveVolume>,
    #[serde(default)]
    pub rows: Vec<PveRow>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub qemu_diff: Option<Vec<PveDiffRow>>,
    #[serde(default)]
    pub horcrux_cmdline: Option<String>,
    #[serde(default)]
    pub exists: bool,
    #[serde(default)]
    pub imported_as: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PvePlan {
    pub source: String,
    pub guests: Vec<PveGuest>,
    #[serde(default)]
    pub bridges: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PveGuestResult {
    pub vmid: u32,
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub id: Option<String>,
    pub state: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub link: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PveJob {
    pub id: String,
    pub state: String,
    pub percent: f32,
    pub message: String,
    pub source: String,
    pub guests: Vec<PveGuestResult>,
}

/// The source form, as the API takes it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PveSourceForm {
    /// `dir`, `ssh` or `vzdump`.
    pub kind: String,
    pub path: String,
    pub host: String,
    pub port: String,
    pub identity: String,
    /// `storage=zfs:dataset` / `storage=dir:/path` lines.
    pub storage_map: String,
    /// `vmbr0=br0` lines.
    pub bridge_map: String,
    /// `8:16=/dev/sdb` lines.
    pub io_devices: String,
}

fn lines(s: &str) -> Vec<(String, String)> {
    s.lines()
        .filter_map(|l| {
            let (a, b) = l.split_once('=')?;
            let (a, b) = (a.trim(), b.trim());
            (!a.is_empty() && !b.is_empty()).then(|| (a.to_string(), b.to_string()))
        })
        .collect()
}

impl PveSourceForm {
    pub fn source(&self) -> serde_json::Value {
        match self.kind.as_str() {
            "ssh" => serde_json::json!({
                "type": "ssh",
                "host": self.host.trim(),
                "port": self.port.trim().parse::<u16>().ok(),
                "identity": (!self.identity.trim().is_empty()).then(|| self.identity.trim().to_string()),
            }),
            "vzdump" => serde_json::json!({"type": "vzdump", "path": self.path.trim()}),
            _ => {
                let mut m = serde_json::Map::new();
                for (k, v) in lines(&self.storage_map) {
                    let loc = match v.split_once(':') {
                        Some(("zfs", ds)) => serde_json::json!({"zfs": ds}),
                        Some(("dir", d)) => serde_json::json!({"dir": d}),
                        _ if v.starts_with('/') => serde_json::json!({"dir": v}),
                        _ => serde_json::json!({"zfs": v}),
                    };
                    m.insert(k, loc);
                }
                serde_json::json!({"type": "dir", "path": self.path.trim(), "storage_map": m})
            }
        }
    }

    pub fn body(&self) -> serde_json::Value {
        let map = |s: &str| -> BTreeMap<String, String> { lines(s).into_iter().collect() };
        serde_json::json!({
            "source": self.source(),
            "bridge_map": map(&self.bridge_map),
            "io_devices": map(&self.io_devices),
        })
    }
}

pub async fn plan(form: &PveSourceForm) -> Result<PvePlan, ApiError> {
    post_json("/import/proxmox/plan", &form.body()).await
}

#[derive(Deserialize)]
struct Started {
    job: String,
}

pub async fn start_import(
    form: &PveSourceForm,
    guests: &[u32],
    ids: &BTreeMap<u32, String>,
    storage: &str,
) -> Result<String, ApiError> {
    let mut b = form.body();
    b["guests"] = serde_json::json!(guests);
    b["ids"] = serde_json::json!(ids);
    if !storage.trim().is_empty() {
        b["storage"] = serde_json::json!(storage.trim());
    }
    let s: Started = post_json("/import/proxmox", &b).await?;
    Ok(s.job)
}

pub async fn status(job: &str) -> Result<PveJob, ApiError> {
    post_json("/import/proxmox/status", &serde_json::json!({ "job": job })).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_builds_the_api_body() {
        let f = PveSourceForm {
            kind: "dir".into(),
            path: "/tank/pve ".into(),
            storage_map: "local-zfs=zfs:tank/pve/data\nlocal=/tank/pve/dump\nbad".into(),
            bridge_map: "vmbr0=br0\n".into(),
            io_devices: "8:16=/dev/sdb".into(),
            ..Default::default()
        };
        let b = f.body();
        assert_eq!(b["source"]["path"], "/tank/pve");
        assert_eq!(
            b["source"]["storage_map"]["local-zfs"]["zfs"],
            "tank/pve/data"
        );
        assert_eq!(b["source"]["storage_map"]["local"]["dir"], "/tank/pve/dump");
        assert_eq!(b["bridge_map"]["vmbr0"], "br0");
        assert_eq!(b["io_devices"]["8:16"], "/dev/sdb");
        let s = PveSourceForm {
            kind: "ssh".into(),
            host: "root@pve1".into(),
            port: "2222".into(),
            ..Default::default()
        };
        assert_eq!(s.source()["port"], 2222);
        assert!(s.source()["identity"].is_null());
    }
}
