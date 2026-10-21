//! Networking > SDN client (mirrors horcrux-api `api::sdn_cni`; checked
//! against the live API by tests/api_contract.rs).
use super::{delete_json, fetch_json, post_json, ApiError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A CNI network (`GET /api/cni/networks`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CniNetwork {
    pub name: String,
    pub plugin_type: String,
    pub bridge: Option<String>,
    pub ipam: CniIpam,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CniIpam {
    #[serde(default)]
    pub ipam_type: String,
    pub subnet: Option<String>,
    pub gateway: Option<String>,
    #[serde(default)]
    pub range_start: Option<String>,
    #[serde(default)]
    pub range_end: Option<String>,
}

/// A container attached to a CNI network (`GET /api/cni/attachments`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CniAttachment {
    pub container_id: String,
    pub network_name: String,
    pub interface_name: String,
    pub ip_address: String,
    pub mac_address: String,
    pub gateway: Option<String>,
    #[serde(default)]
    pub guest: Option<String>,
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// `POST /api/cni/attach` for a Horcrux container (the API looks up its
/// network namespace from the container runtime).
#[derive(Debug, Clone, Default, Serialize)]
pub struct AttachRequest {
    pub container_id: String,
    pub network_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    pub labels: HashMap<String, String>,
}

/// `POST /api/cni/detach`: interface and netns are the recorded ones.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DetachRequest {
    pub container_id: String,
    pub network_name: String,
}

pub async fn list_networks() -> Result<Vec<CniNetwork>, ApiError> {
    fetch_json("/cni/networks").await
}

pub async fn delete_network(name: &str) -> Result<(), ApiError> {
    delete_json(&format!("/cni/networks/{}", urlencoding::encode(name))).await
}

pub async fn list_attachments() -> Result<Vec<CniAttachment>, ApiError> {
    fetch_json("/cni/attachments").await
}

pub async fn attach(req: &AttachRequest) -> Result<CniAttachment, ApiError> {
    post_json("/cni/attach", req).await
}

pub async fn detach(req: &DetachRequest) -> Result<String, ApiError> {
    post_json("/cni/detach", req).await
}

/// `k=v, k2=v2` -> labels (blank entries ignored, `k` alone = empty value).
pub fn parse_labels(text: &str) -> HashMap<String, String> {
    text.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|kv| match kv.split_once('=') {
            Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
            None => (kv.to_string(), String::new()),
        })
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_labels;

    #[test]
    fn labels_are_parsed() {
        let l = parse_labels(" app=web, tier = db ,,flag");
        assert_eq!(l.get("app").map(String::as_str), Some("web"));
        assert_eq!(l.get("tier").map(String::as_str), Some("db"));
        assert_eq!(l.get("flag").map(String::as_str), Some(""));
        assert_eq!(l.len(), 3);
        assert!(parse_labels("").is_empty());
    }
}
