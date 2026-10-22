//! Node disks and SMART health (Proxmox "Node > Disks"): the host's physical
//! disks and partitions from `lsblk`, and their SMART data and self-tests
//! from `smartctl` (smartmontools). Read-mostly: no partitioning, formatting
//! or wiping happens here.
//!
//! * `GET /api/storage/disks`, `/:id`, `/:id/partitions`
//! * `GET /api/storage/smart/disks`, `/:id`
//! * `POST /api/storage/smart/disks/:id/test {test_type: short|long|conveyance}`
//!   and `GET .../test/status`
//! * `POST /api/storage/smart/disks/:id/enable|disable` (`smartctl -s on|off`)
//! * `GET /api/storage/smart/alerts`, `POST .../alerts/:id/acknowledge`
//!
//! A disk id is its kernel name (`sda`, `nvme0n1`) and must be one `lsblk`
//! reports as a disk; the id is never put into a shell, only into an argv.
use crate::db::config_store::{kinds, ConfigStore};
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/storage/disks", get(list_disks))
        .route("/api/storage/disks/:id", get(get_disk))
        .route("/api/storage/disks/:id/partitions", get(get_partitions))
        .route("/api/storage/smart/disks", get(smart_disks))
        .route("/api/storage/smart/disks/:id", get(smart_disk))
        .route("/api/storage/smart/disks/:id/test", post(start_test))
        .route("/api/storage/smart/disks/:id/test/status", get(test_status))
        .route("/api/storage/smart/disks/:id/enable", post(enable_smart))
        .route("/api/storage/smart/disks/:id/disable", post(disable_smart))
        .route("/api/storage/smart/alerts", get(smart_alerts))
        .route("/api/storage/smart/alerts/:id/acknowledge", post(ack_alert))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PartitionInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub filesystem: String,
    pub mount_point: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiskInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub model: String,
    pub serial: String,
    pub disk_type: String,
    pub interface: String,
    pub partitions: Vec<PartitionInfo>,
    pub smart_status: String,
    pub temperature: Option<u32>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(|x| x.as_str())
        .map(|x| x.trim().to_string())
        .unwrap_or_default()
}

/// `lsblk` prints numbers as numbers or strings depending on its version.
fn num(v: &Value, k: &str) -> u64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(t)) => t.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn flag(v: &Value, k: &str) -> bool {
    match v.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(t)) => t == "1" || t == "true",
        Some(Value::Number(n)) => n.as_u64() == Some(1),
        _ => false,
    }
}

fn opt(v: &Value, k: &str) -> Option<String> {
    let t = s(v, k);
    (!t.is_empty()).then_some(t)
}

/// Disks (and their partitions) from `lsblk -J -b` output. Loop, ram, rom
/// and device-mapper devices are not physical disks and are left out.
pub fn parse_lsblk(json: &str) -> Result<Vec<DiskInfo>, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("lsblk output: {e}"))?;
    let devs = root
        .get("blockdevices")
        .and_then(|d| d.as_array())
        .ok_or("lsblk output has no blockdevices")?;
    let mut out = Vec::new();
    for d in devs {
        if s(d, "type") != "disk" {
            continue;
        }
        let name = s(d, "name");
        if name.starts_with("zram") {
            continue;
        }
        let tran = s(d, "tran");
        let rota = flag(d, "rota");
        let disk_type = if name.starts_with("nvme") || tran == "nvme" {
            "nvme"
        } else if rota {
            "hdd"
        } else {
            "ssd"
        };
        let mut parts = Vec::new();
        let mut used = 0;
        for p in d
            .get("children")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default()
        {
            if s(&p, "type") != "part" {
                continue;
            }
            let pused = num(&p, "fsused");
            used += pused;
            let pname = s(&p, "name");
            parts.push(PartitionInfo {
                id: pname.clone(),
                path: opt(&p, "path").unwrap_or_else(|| format!("/dev/{pname}")),
                name: pname,
                size_bytes: num(&p, "size"),
                used_bytes: pused,
                filesystem: s(&p, "fstype"),
                mount_point: opt(&p, "mountpoint"),
                label: opt(&p, "label"),
            });
        }
        out.push(DiskInfo {
            id: name.clone(),
            path: opt(d, "path").unwrap_or_else(|| format!("/dev/{name}")),
            name,
            size_bytes: num(d, "size"),
            used_bytes: used,
            model: s(d, "model"),
            serial: s(d, "serial"),
            disk_type: disk_type.into(),
            interface: if tran.is_empty() {
                "unknown".into()
            } else {
                tran
            },
            partitions: parts,
            smart_status: "unknown".into(),
            temperature: None,
        });
    }
    Ok(out)
}

async fn run(cmd: &str, args: &[&str]) -> Result<(i32, String, String), ApiError> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        tokio::process::Command::new(cmd).args(args).output(),
    )
    .await
    .map_err(|_| ApiError::Internal(format!("{cmd} timed out")))?
    .map_err(|e| ApiError::Internal(format!("cannot run {cmd}: {e}")))?;
    Ok((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

async fn lsblk() -> Result<Vec<DiskInfo>, ApiError> {
    let (code, out, err) = run(
        "lsblk",
        &[
            "-J",
            "-b",
            "-o",
            "NAME,PATH,SIZE,MODEL,SERIAL,TYPE,ROTA,TRAN,FSTYPE,MOUNTPOINT,LABEL,FSUSED",
        ],
    )
    .await?;
    if code != 0 {
        return Err(ApiError::Internal(format!("lsblk failed: {err}")));
    }
    parse_lsblk(&out).map_err(ApiError::Internal)
}

async fn disk_by_id(id: &str) -> Result<DiskInfo, ApiError> {
    lsblk()
        .await?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("no disk {id}")))
}

// ---------------------------------------------------------------------- SMART

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SmartAttribute {
    pub id: u32,
    pub name: String,
    pub value: u64,
    pub worst: u64,
    pub threshold: u64,
    pub raw_value: String,
    pub status: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SmartTestResult {
    pub test_type: String,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub lifetime_hours: u64,
    pub error_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SmartDiskInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub capacity_bytes: u64,
    pub disk_type: String,
    pub interface: String,
    pub smart_enabled: bool,
    /// "Healthy" | "Warning" | "Critical" | "Unknown"
    pub smart_status: String,
    pub temperature: Option<u32>,
    pub power_on_hours: Option<u64>,
    pub power_cycle_count: Option<u64>,
    pub attributes: Vec<SmartAttribute>,
    pub last_test: Option<SmartTestResult>,
    pub health_score: u32,
}

/// SMART view of `disk` from `smartctl -j -a` output (`None` = smartctl
/// could not read the device: no SMART, virtual disk, not installed).
pub fn parse_smartctl(disk: &DiskInfo, json: &str) -> SmartDiskInfo {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    let mut info = SmartDiskInfo {
        id: disk.id.clone(),
        name: disk.name.clone(),
        path: disk.path.clone(),
        model: opt(&v, "model_name").unwrap_or_else(|| disk.model.clone()),
        serial: opt(&v, "serial_number").unwrap_or_else(|| disk.serial.clone()),
        firmware: s(&v, "firmware_version"),
        capacity_bytes: disk.size_bytes,
        disk_type: disk.disk_type.clone(),
        interface: disk.interface.clone(),
        smart_enabled: v
            .pointer("/smart_support/enabled")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        smart_status: "Unknown".into(),
        temperature: v
            .pointer("/temperature/current")
            .and_then(|x| x.as_u64())
            .map(|t| t as u32),
        power_on_hours: v.pointer("/power_on_time/hours").and_then(|x| x.as_u64()),
        power_cycle_count: v.get("power_cycle_count").and_then(|x| x.as_u64()),
        attributes: Vec::new(),
        last_test: None,
        health_score: 0,
    };
    if let Some(table) = v
        .pointer("/ata_smart_attributes/table")
        .and_then(|t| t.as_array())
    {
        for a in table {
            let value = num(a, "value");
            let thresh = num(a, "thresh");
            let failing = thresh > 0 && value <= thresh;
            info.attributes.push(SmartAttribute {
                id: num(a, "id") as u32,
                name: s(a, "name"),
                value,
                worst: num(a, "worst"),
                threshold: thresh,
                raw_value: a
                    .pointer("/raw/string")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string(),
                status: if failing { "failing" } else { "ok" }.into(),
                description: s(a, "name").replace('_', " "),
            });
        }
    }
    // NVMe health log -> the same attribute list.
    if let Some(n) = v.get("nvme_smart_health_information_log") {
        for (i, (k, label)) in [
            ("percentage_used", "Percentage Used"),
            ("available_spare", "Available Spare"),
            ("media_errors", "Media Errors"),
            ("unsafe_shutdowns", "Unsafe Shutdowns"),
            ("critical_warning", "Critical Warning"),
        ]
        .iter()
        .enumerate()
        {
            if let Some(x) = n.get(*k).and_then(|x| x.as_u64()) {
                info.attributes.push(SmartAttribute {
                    id: i as u32 + 1,
                    name: label.replace(' ', "_"),
                    value: x,
                    worst: x,
                    threshold: 0,
                    raw_value: x.to_string(),
                    status: "ok".into(),
                    description: label.to_string(),
                });
            }
        }
        if info.temperature.is_none() {
            info.temperature = n
                .get("temperature")
                .and_then(|x| x.as_u64())
                .map(|t| t as u32);
        }
        info.power_on_hours = info
            .power_on_hours
            .or_else(|| n.get("power_on_hours").and_then(|x| x.as_u64()));
        info.power_cycle_count = info
            .power_cycle_count
            .or_else(|| n.get("power_cycles").and_then(|x| x.as_u64()));
    }
    // Last self-test.
    if let Some(t) = v
        .pointer("/ata_smart_self_test_log/standard/table/0")
        .or_else(|| v.pointer("/nvme_self_test_log/table/0"))
    {
        info.last_test = Some(SmartTestResult {
            test_type: t
                .pointer("/type/string")
                .and_then(|x| x.as_str())
                .unwrap_or("self-test")
                .to_string(),
            status: t
                .pointer("/status/string")
                .and_then(|x| x.as_str())
                .unwrap_or("unknown")
                .to_string(),
            started_at: String::new(),
            completed_at: None,
            lifetime_hours: num(t, "lifetime_hours"),
            error_count: u32::from(t.pointer("/status/passed") == Some(&Value::Bool(false))),
        });
    }
    let passed = v.pointer("/smart_status/passed").and_then(|x| x.as_bool());
    let realloc = info
        .attributes
        .iter()
        .find(|a| a.id == 5)
        .map(|a| {
            a.raw_value
                .split_whitespace()
                .next()
                .unwrap_or("0")
                .parse::<u64>()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let failing = info.attributes.iter().any(|a| a.status == "failing");
    let hot = info.temperature.map(|t| t >= 60).unwrap_or(false);
    info.smart_status = match passed {
        Some(false) => "Critical",
        Some(true) if failing || realloc > 0 || hot => "Warning",
        Some(true) => "Healthy",
        None => "Unknown",
    }
    .into();
    info.health_score = match info.smart_status.as_str() {
        "Healthy" => 100,
        "Warning" => 60,
        "Critical" => 10,
        _ => 0,
    };
    info
}

async fn smart_of(disk: &DiskInfo) -> SmartDiskInfo {
    // smartctl exits non-zero with bit flags for "something found": the
    // JSON is what matters.
    match run("smartctl", &["-j", "-a", &disk.path]).await {
        Ok((_, out, _)) if out.trim_start().starts_with('{') => parse_smartctl(disk, &out),
        _ => parse_smartctl(disk, "{}"),
    }
}

async fn list_disks() -> Result<Json<Vec<DiskInfo>>, ApiError> {
    let mut disks = lsblk().await?;
    for d in &mut disks {
        let sm = smart_of(d).await;
        d.smart_status = sm.smart_status.to_lowercase();
        d.temperature = sm.temperature;
    }
    Ok(Json(disks))
}

async fn get_disk(Path(id): Path<String>) -> Result<Json<DiskInfo>, ApiError> {
    let mut d = disk_by_id(&id).await?;
    let sm = smart_of(&d).await;
    d.smart_status = sm.smart_status.to_lowercase();
    d.temperature = sm.temperature;
    Ok(Json(d))
}

async fn get_partitions(Path(id): Path<String>) -> Result<Json<Vec<PartitionInfo>>, ApiError> {
    Ok(Json(disk_by_id(&id).await?.partitions))
}

async fn smart_disks() -> Result<Json<Vec<SmartDiskInfo>>, ApiError> {
    let mut out = Vec::new();
    for d in lsblk().await? {
        out.push(smart_of(&d).await);
    }
    Ok(Json(out))
}

async fn smart_disk(Path(id): Path<String>) -> Result<Json<SmartDiskInfo>, ApiError> {
    let d = disk_by_id(&id).await?;
    Ok(Json(smart_of(&d).await))
}

#[derive(Deserialize)]
struct TestRequest {
    test_type: String,
}

async fn start_test(
    Path(id): Path<String>,
    Json(req): Json<TestRequest>,
) -> Result<Json<Value>, ApiError> {
    let kind = match req.test_type.as_str() {
        "short" | "long" | "conveyance" => req.test_type.as_str(),
        other => {
            return Err(ApiError::ValidationError(format!(
                "test_type must be short, long or conveyance, not {other}"
            )))
        }
    };
    let d = disk_by_id(&id).await?;
    let (code, out, err) = run("smartctl", &["-t", kind, &d.path]).await?;
    // Bits 0-2 of smartctl's exit status are command-line/device-open/
    // SMART-command failures.
    if code < 0 || code & 0b111 != 0 {
        return Err(ApiError::Conflict(format!(
            "smartctl could not start the {kind} test on {}: {}",
            d.path,
            if err.trim().is_empty() { out } else { err }.trim()
        )));
    }
    Ok(Json(
        serde_json::json!({"started": true, "test_type": kind, "disk": d.id}),
    ))
}

async fn test_status(Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let d = disk_by_id(&id).await?;
    let (_, out, _) = run("smartctl", &["-j", "-c", "-l", "selftest", &d.path]).await?;
    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    let running = v
        .pointer("/ata_smart_data/self_test/status/value")
        .and_then(|x| x.as_u64())
        .map(|x| x >> 4 == 0xF)
        .unwrap_or(false);
    let pct = v
        .pointer("/ata_smart_data/self_test/status/remaining_percent")
        .and_then(|x| x.as_u64());
    Ok(Json(serde_json::json!({
        "running": running,
        "remaining_percent": pct,
        "last": smart_of(&d).await.last_test,
    })))
}

async fn set_smart(id: &str, on: bool) -> Result<Json<SmartDiskInfo>, ApiError> {
    let d = disk_by_id(id).await?;
    let (code, out, err) = run("smartctl", &["-s", if on { "on" } else { "off" }, &d.path]).await?;
    if code < 0 || code & 0b111 != 0 {
        return Err(ApiError::Conflict(format!(
            "smartctl -s failed on {}: {}",
            d.path,
            if err.trim().is_empty() { out } else { err }.trim()
        )));
    }
    Ok(Json(smart_of(&d).await))
}

async fn enable_smart(Path(id): Path<String>) -> Result<Json<SmartDiskInfo>, ApiError> {
    set_smart(&id, true).await
}

async fn disable_smart(Path(id): Path<String>) -> Result<Json<SmartDiskInfo>, ApiError> {
    set_smart(&id, false).await
}

// ---------------------------------------------------------------------- alerts

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SmartAlert {
    pub id: String,
    pub disk_id: String,
    pub disk_name: String,
    pub alert_type: String,
    pub severity: String,
    pub message: String,
    pub attribute_id: Option<u32>,
    pub attribute_name: Option<String>,
    pub created_at: String,
    pub acknowledged: bool,
}

/// What is wrong with a disk right now (no acknowledgement applied).
pub fn alerts_for(s: &SmartDiskInfo) -> Vec<SmartAlert> {
    let now = chrono::Utc::now().to_rfc3339();
    let mk = |kind: &str, sev: &str, msg: String, attr: Option<&SmartAttribute>| SmartAlert {
        id: format!("{}-{}", s.id, kind),
        disk_id: s.id.clone(),
        disk_name: s.name.clone(),
        alert_type: kind.into(),
        severity: sev.into(),
        message: msg,
        attribute_id: attr.map(|a| a.id),
        attribute_name: attr.map(|a| a.name.clone()),
        created_at: now.clone(),
        acknowledged: false,
    };
    let mut v = Vec::new();
    if s.smart_status == "Critical" {
        v.push(mk(
            "smart_failed",
            "critical",
            format!(
                "{} reports SMART overall-health FAILED: back it up and replace it",
                s.name
            ),
            None,
        ));
    }
    for a in s.attributes.iter().filter(|a| a.status == "failing") {
        v.push(mk(
            &format!("attr_{}", a.id),
            "warning",
            format!(
                "{}: attribute {} is at or below its threshold",
                s.name, a.name
            ),
            Some(a),
        ));
    }
    if let Some(t) = s.temperature.filter(|t| *t >= 60) {
        v.push(mk(
            "temperature",
            "warning",
            format!("{} is at {t} C", s.name),
            None,
        ));
    }
    v
}

async fn smart_alerts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<SmartAlert>>, ApiError> {
    let acked: Vec<String> = ConfigStore::new(&state.database)
        .load::<bool>(kinds::SMART_ALERT_ACK)
        .await?
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let mut out = Vec::new();
    for d in lsblk().await? {
        for mut a in alerts_for(&smart_of(&d).await) {
            a.acknowledged = acked.contains(&a.id);
            out.push(a);
        }
    }
    Ok(Json(out))
}

async fn ack_alert(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    ConfigStore::new(&state.database)
        .put(kinds::SMART_ALERT_ACK, &id, &true)
        .await?;
    Ok(Json(serde_json::json!({"acknowledged": id})))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSBLK: &str = r#"{"blockdevices":[
      {"name":"sda","path":"/dev/sda","size":1000204886016,"model":"Samsung SSD 860 ","serial":"S3Z9","type":"disk","rota":false,"tran":"sata","fstype":null,"mountpoint":null,"label":null,"fsused":null,
       "children":[{"name":"sda1","path":"/dev/sda1","size":536870912,"type":"part","fstype":"vfat","mountpoint":"/boot","label":"EFI","fsused":"1048576"},
                   {"name":"sda2","path":"/dev/sda2","size":999000000000,"type":"part","fstype":"ext4","mountpoint":"/","label":null,"fsused":"500000000"}]},
      {"name":"nvme0n1","path":"/dev/nvme0n1","size":512110190592,"model":"WD","serial":"X","type":"disk","rota":"0","tran":"nvme"},
      {"name":"loop0","path":"/dev/loop0","size":1,"type":"loop"},
      {"name":"zram0","path":"/dev/zram0","size":1,"type":"disk","rota":false}]}"#;

    #[test]
    fn lsblk_disks_partitions_and_types() {
        let d = parse_lsblk(LSBLK).unwrap();
        assert_eq!(
            d.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
            ["sda", "nvme0n1"]
        );
        assert_eq!(d[0].disk_type, "ssd");
        assert_eq!(d[0].model, "Samsung SSD 860");
        assert_eq!(d[0].partitions.len(), 2);
        assert_eq!(d[0].partitions[1].mount_point.as_deref(), Some("/"));
        assert_eq!(d[0].used_bytes, 501_048_576);
        assert_eq!(d[1].disk_type, "nvme");
    }

    #[test]
    fn smartctl_json_becomes_health_attributes_and_alerts() {
        let disk = parse_lsblk(LSBLK).unwrap().remove(0);
        let j = r#"{"model_name":"Samsung SSD 860","serial_number":"S3Z9","firmware_version":"RVT04B6Q",
          "smart_support":{"enabled":true},"smart_status":{"passed":true},
          "temperature":{"current":64},"power_on_time":{"hours":1234},"power_cycle_count":77,
          "ata_smart_attributes":{"table":[
            {"id":5,"name":"Reallocated_Sector_Ct","value":100,"worst":100,"thresh":10,"raw":{"string":"3"}},
            {"id":9,"name":"Power_On_Hours","value":99,"worst":99,"thresh":0,"raw":{"string":"1234"}}]},
          "ata_smart_self_test_log":{"standard":{"table":[{"type":{"string":"Short offline"},"status":{"string":"Completed without error","passed":true},"lifetime_hours":1200}]}}}"#;
        let s = parse_smartctl(&disk, j);
        assert!(s.smart_enabled);
        assert_eq!(s.power_on_hours, Some(1234));
        assert_eq!(s.attributes.len(), 2);
        assert_eq!(s.smart_status, "Warning"); // reallocated sectors + 64 C
        assert_eq!(
            s.last_test.as_ref().unwrap().status,
            "Completed without error"
        );
        let a = alerts_for(&s);
        assert!(a.iter().any(|x| x.alert_type == "temperature"));
        // A failed overall-health is critical.
        let bad = parse_smartctl(&disk, r#"{"smart_status":{"passed":false}}"#);
        assert_eq!(bad.smart_status, "Critical");
        assert_eq!(alerts_for(&bad)[0].severity, "critical");
        // smartctl missing/unreadable -> unknown, not an error.
        assert_eq!(parse_smartctl(&disk, "").smart_status, "Unknown");
    }

    /// The real `lsblk` of the machine running the tests parses.
    #[tokio::test]
    async fn real_lsblk_parses() {
        if std::process::Command::new("lsblk")
            .arg("--version")
            .output()
            .is_err()
        {
            return; // not installed here
        }
        let disks = lsblk().await.expect("lsblk");
        for d in disks {
            assert!(d.path.starts_with("/dev/"));
        }
    }
}
