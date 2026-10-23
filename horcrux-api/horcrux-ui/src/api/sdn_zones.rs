//! Datacenter > SDN client: zones, VNets, subnets, IPAM (mirrors horcrux-api
//! `api::sdn_zones`; checked against the live API by tests/api_contract.rs).
use super::{delete_json, fetch_json, post_json, ApiError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Zone {
    pub id: String,
    pub zone_type: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub uplink: Option<String>,
    #[serde(default)]
    pub local_ip: Option<String>,
    #[serde(default)]
    pub peers: Vec<String>,
    #[serde(default)]
    pub mtu: Option<u16>,
    #[serde(default)]
    pub dst_port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VNet {
    pub id: String,
    pub zone: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub tag: Option<u32>,
    #[serde(default)]
    pub vlan_aware: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subnet {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub vnet: String,
    pub cidr: String,
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub snat: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Allocation {
    pub subnet: String,
    pub ip: String,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub mac: Option<String>,
    pub kind: String,
    pub created: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllocateRequest {
    pub ip: Option<String>,
    pub hostname: Option<String>,
    pub mac: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpamView {
    pub subnet: String,
    pub total: u32,
    pub used: u32,
    pub free: u32,
    pub allocations: Vec<Allocation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VNetStatus {
    pub vnet: String,
    pub bridge_present: bool,
    pub up: bool,
    pub addresses: Vec<String>,
    pub ports: Vec<String>,
    pub carrier: Option<String>,
    pub carrier_present: bool,
    pub error: Option<String>,
}

pub async fn list_zones() -> Result<Vec<Zone>, ApiError> {
    fetch_json("/sdn/zones").await
}
pub async fn create_zone(z: &Zone) -> Result<Zone, ApiError> {
    post_json("/sdn/zones", z).await
}
pub async fn delete_zone(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/sdn/zones/{}", urlencoding::encode(id))).await
}
pub async fn list_vnets() -> Result<Vec<VNet>, ApiError> {
    fetch_json("/sdn/vnets").await
}
pub async fn create_vnet(v: &VNet) -> Result<VNet, ApiError> {
    post_json("/sdn/vnets", v).await
}
pub async fn delete_vnet(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/sdn/vnets/{}", urlencoding::encode(id))).await
}
pub async fn list_subnets() -> Result<Vec<Subnet>, ApiError> {
    fetch_json("/sdn/subnets").await
}
pub async fn create_subnet(vnet: &str, s: &Subnet) -> Result<Subnet, ApiError> {
    post_json(
        &format!("/sdn/vnets/{}/subnets", urlencoding::encode(vnet)),
        s,
    )
    .await
}
pub async fn delete_subnet(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/sdn/subnets/{}", urlencoding::encode(id))).await
}
pub async fn get_ipam(subnet: &str) -> Result<IpamView, ApiError> {
    fetch_json(&format!(
        "/sdn/subnets/{}/ipam",
        urlencoding::encode(subnet)
    ))
    .await
}
pub async fn allocate_ip(subnet: &str, r: &AllocateRequest) -> Result<Allocation, ApiError> {
    post_json(
        &format!("/sdn/subnets/{}/ipam", urlencoding::encode(subnet)),
        r,
    )
    .await
}
pub async fn release_ip(subnet: &str, ip: &str) -> Result<(), ApiError> {
    delete_json(&format!(
        "/sdn/subnets/{}/ipam/{}",
        urlencoding::encode(subnet),
        urlencoding::encode(ip)
    ))
    .await
}
pub async fn sdn_status() -> Result<Vec<VNetStatus>, ApiError> {
    fetch_json("/sdn/status").await
}
