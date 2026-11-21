//! Remote Play client (mirrors horcrux-api `api::remote_play`; types are
//! shared through horcrux-common).
use super::{fetch_json, post_json, put_json, ApiError};
use horcrux_common::remote_play::{
    GuestReport, GuestSetupRequest, RemotePlaySettings, RemotePlayStatus, VmRemotePlay,
};
use horcrux_common::VmConfig;

fn vm_path(vm: &str, rest: &str) -> String {
    format!("/vms/{}/remote-play{}", urlencoding::encode(vm), rest)
}

pub async fn get(vm: &str) -> Result<VmRemotePlay, ApiError> {
    fetch_json(&vm_path(vm, "")).await
}

pub async fn put(vm: &str, rp: &VmRemotePlay) -> Result<VmRemotePlay, ApiError> {
    put_json(&vm_path(vm, ""), rp).await
}

pub async fn status(vm: &str) -> Result<RemotePlayStatus, ApiError> {
    fetch_json(&vm_path(vm, "/status")).await
}

pub async fn guest_setup(vm: &str, req: &GuestSetupRequest) -> Result<GuestReport, ApiError> {
    post_json(&vm_path(vm, "/guest-setup"), req).await
}

pub async fn lan_nic(vm: &str) -> Result<VmConfig, ApiError> {
    post_json(&vm_path(vm, "/lan-nic"), &serde_json::json!({})).await
}

pub async fn get_settings() -> Result<RemotePlaySettings, ApiError> {
    fetch_json("/vms/remote-play/settings").await
}

pub async fn put_settings(s: &RemotePlaySettings) -> Result<RemotePlaySettings, ApiError> {
    put_json("/vms/remote-play/settings", s).await
}
