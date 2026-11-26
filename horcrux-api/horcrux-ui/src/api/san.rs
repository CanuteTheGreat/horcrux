//! Storage > SAN API client (mirrors horcrux-api `api::san` and the SAN
//! routes of `api::storage_volumes`).
use super::{delete_json, fetch_json, post_json, ApiError};
use serde::{Deserialize, Serialize};

/// `GET /api/san/status`: which SAN tools this node has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SanTools {
    #[serde(default)]
    pub iscsiadm: bool,
    #[serde(default)]
    pub multipathd: bool,
    #[serde(default)]
    pub lvmlockd: bool,
    #[serde(default)]
    pub sanlock: bool,
    #[serde(default)]
    pub blkdiscard: bool,
    #[serde(default)]
    pub nvme: bool,
    #[serde(default)]
    pub qemu_img: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ApplyReport {
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SanStatus {
    pub node: String,
    pub tools: SanTools,
    #[serde(default)]
    pub host_id: Option<u16>,
    #[serde(default)]
    pub multipath_config: String,
    #[serde(default)]
    pub last_apply: Option<ApplyReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IscsiTarget {
    pub id: String,
    pub iqn: String,
    pub portals: Vec<String>,
    #[serde(default)]
    pub use_advertised_portals: bool,
    #[serde(default)]
    pub chap_user: Option<String>,
    #[serde(default)]
    pub chap_secret_set: bool,
    #[serde(default)]
    pub nodes: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NvmeTarget {
    pub id: String,
    pub subnqn: String,
    #[serde(default)]
    pub transport: String,
    pub addresses: Vec<String>,
    #[serde(default)]
    pub iopolicy: String,
    #[serde(default)]
    pub nodes: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedLvm {
    pub id: String,
    pub vg_name: String,
    pub device: String,
    #[serde(default)]
    pub wipe_removed: bool,
    #[serde(default)]
    pub discard_removed: bool,
    #[serde(default)]
    pub snapshot_as_volume_chain: bool,
    #[serde(default)]
    pub nodes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredTarget {
    pub iqn: String,
    pub portals: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoverAnswer {
    pub targets: Vec<DiscoveredTarget>,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredSubsystem {
    pub subnqn: String,
    #[serde(default)]
    pub transport: String,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NvmeDiscoverAnswer {
    pub subsystems: Vec<DiscoveredSubsystem>,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathView {
    pub dev: String,
    pub transport: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub portal: Option<String>,
    #[serde(default)]
    pub local_address: Option<String>,
    #[serde(default)]
    pub lun: Option<u32>,
    /// `active`, `standby`, `failed`, `unmapped`.
    pub state: String,
    #[serde(default)]
    pub dm_state: Option<String>,
    #[serde(default)]
    pub checker_state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LunView {
    pub wwid: String,
    #[serde(default)]
    pub map: Option<String>,
    pub device: String,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub product: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub lun: Option<u32>,
    pub paths: Vec<PathView>,
    pub active_paths: usize,
    pub failed_paths: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FcHost {
    pub name: String,
    pub host: u32,
    #[serde(default)]
    pub wwpn: Option<String>,
    #[serde(default)]
    pub wwnn: Option<String>,
    #[serde(default)]
    pub port_state: Option<String>,
    #[serde(default)]
    pub port_type: Option<String>,
    #[serde(default)]
    pub speed: Option<String>,
    #[serde(default)]
    pub fabric_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FcLun {
    pub hctl: String,
    pub lun: u32,
    #[serde(default)]
    pub dev: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FcRemotePort {
    pub name: String,
    pub host: u32,
    #[serde(default)]
    pub wwpn: Option<String>,
    #[serde(default)]
    pub port_id: Option<String>,
    #[serde(default)]
    pub port_state: Option<String>,
    #[serde(default)]
    pub roles: Option<String>,
    #[serde(default)]
    pub luns: Vec<FcLun>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FcView {
    pub hosts: Vec<FcHost>,
    pub remote_ports: Vec<FcRemotePort>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct NodePaths {
    pub node: String,
    #[serde(default)]
    pub multipathd_running: bool,
    #[serde(default)]
    pub native_nvme_multipath: Option<bool>,
    #[serde(default)]
    pub fc_hosts: Vec<FcHost>,
    pub luns: Vec<LunView>,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ChainView {
    pub volume: String,
    pub snapshots: Vec<String>,
    #[serde(default)]
    pub virtual_size_bytes: Option<u64>,
}

pub async fn get_status() -> Result<SanStatus, ApiError> {
    fetch_json("/san/status").await
}
pub async fn get_targets() -> Result<Vec<IscsiTarget>, ApiError> {
    fetch_json("/san/targets").await
}
pub async fn get_nvme_targets() -> Result<Vec<NvmeTarget>, ApiError> {
    fetch_json("/san/nvme/targets").await
}
pub async fn get_lvm() -> Result<Vec<SharedLvm>, ApiError> {
    fetch_json("/san/lvm").await
}
/// Paths of one node (`None` = the node serving the UI).
pub async fn get_paths(node: Option<&str>) -> Result<NodePaths, ApiError> {
    match node {
        Some(n) => fetch_json(&format!("/san/nodes/{}/paths", urlencoding(n))).await,
        None => fetch_json("/san/paths").await,
    }
}
pub async fn get_fc() -> Result<FcView, ApiError> {
    fetch_json("/san/fc").await
}
pub async fn fc_rescan() -> Result<serde_json::Value, ApiError> {
    post_json("/san/fc/rescan", &serde_json::json!({})).await
}
pub async fn apply() -> Result<ApplyReport, ApiError> {
    post_json("/san/apply", &serde_json::json!({})).await
}
pub async fn discover_iscsi(portals: Vec<String>) -> Result<DiscoverAnswer, ApiError> {
    post_json("/san/discover", &serde_json::json!({ "portals": portals })).await
}
pub async fn discover_nvme(addresses: Vec<String>) -> Result<NvmeDiscoverAnswer, ApiError> {
    post_json(
        "/san/nvme/discover",
        &serde_json::json!({ "addresses": addresses }),
    )
    .await
}
pub async fn create_iscsi_target(
    id: &str,
    iqn: &str,
    portals: Vec<String>,
    chap: Option<(String, String)>,
) -> Result<serde_json::Value, ApiError> {
    let mut b = serde_json::json!({ "id": id, "iqn": iqn, "portals": portals });
    if let Some((u, s)) = chap {
        b["chap_user"] = u.into();
        b["chap_secret"] = s.into();
    }
    post_json("/san/targets", &b).await
}
pub async fn create_nvme_target(
    id: &str,
    subnqn: &str,
    addresses: Vec<String>,
) -> Result<serde_json::Value, ApiError> {
    post_json(
        "/san/nvme/targets",
        &serde_json::json!({ "id": id, "subnqn": subnqn, "addresses": addresses }),
    )
    .await
}
pub async fn delete_target(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/san/targets/{}", urlencoding(id))).await
}
pub async fn delete_nvme_target(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/san/nvme/targets/{}", urlencoding(id))).await
}
pub async fn delete_lvm(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/san/lvm/{}", urlencoding(id))).await
}

/// Body of `POST /api/san/lvm`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateLvm {
    pub id: String,
    pub vg_name: String,
    pub device: String,
    pub wipe_removed: bool,
    pub discard_removed: bool,
    pub snapshot_as_volume_chain: bool,
    pub create_vg: bool,
}

pub async fn create_lvm(req: &CreateLvm) -> Result<ApplyReport, ApiError> {
    post_json("/san/lvm", req).await
}

pub async fn volume_snapshots(pool: &str, volume: &str) -> Result<ChainView, ApiError> {
    fetch_json(&format!(
        "/storage/pools/{}/volumes/{}/snapshots",
        urlencoding(pool),
        urlencoding(volume)
    ))
    .await
}

/// Minimal path-segment escaping (ids are validated server-side; this only
/// keeps a stray character from changing the route).
fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | ':' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

/// `1.5 GiB`-style size.
pub fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_escaping() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(3 << 30), "3.0 GiB");
        assert_eq!(urlencoding("a b/c"), "a%20b%2Fc");
        assert_eq!(urlencoding("node-1.lab"), "node-1.lab");
    }
}
