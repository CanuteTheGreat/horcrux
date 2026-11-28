//! `horcrux import proxmox ...`: plan and run imports of Proxmox VE guests
//! (`/api/import/proxmox/*`).
use crate::api::ApiClient;
use crate::output;
use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};

#[derive(Subcommand)]
pub enum ImportCommands {
    /// Proxmox VE VMs and containers (directory, SSH node or vzdump archive)
    Proxmox {
        #[command(subcommand)]
        command: ProxmoxCommands,
    },
}

#[derive(Args, Clone)]
pub struct SourceArgs {
    /// Directory with the guest configs (qemu-server/, lxc/ or a copy of /etc/pve) and the volumes
    #[arg(long, conflicts_with_all = ["ssh", "vzdump"])]
    pub dir: Option<String>,
    /// A Proxmox node over SSH ([user@]host), read only
    #[arg(long, conflicts_with = "vzdump")]
    pub ssh: Option<String>,
    #[arg(long, requires = "ssh")]
    pub ssh_port: Option<u16>,
    /// SSH key file on the Horcrux node
    #[arg(long, requires = "ssh")]
    pub identity: Option<String>,
    /// A container vzdump archive
    #[arg(long)]
    pub vzdump: Option<String>,
    /// Where a Proxmox storage's volumes are now: STORAGE=zfs:DATASET or STORAGE=dir:PATH (repeat)
    #[arg(long = "storage-map", value_name = "ID=LOC")]
    pub storage_map: Vec<String>,
    /// Proxmox bridge -> Horcrux bridge: PVE=HX (repeat)
    #[arg(long = "bridge", value_name = "PVE=HX")]
    pub bridge: Vec<String>,
    /// Source block device of a container io limit: MAJ:MIN=/dev/... (repeat)
    #[arg(long = "io-device", value_name = "MAJ:MIN=PATH")]
    pub io_device: Vec<String>,
}

#[derive(Subcommand)]
pub enum ProxmoxCommands {
    /// What each guest becomes, every setting carried / changed / not carried, the QEMU command-line diff (read only)
    Plan {
        #[command(flatten)]
        source: SourceArgs,
        /// Only these Proxmox ids (repeat; default every guest)
        #[arg(long = "guest")]
        guest: Vec<u32>,
        /// Print every row and the QEMU diff
        #[arg(long)]
        details: bool,
    },
    /// Import guests (copies their data into Horcrux storage)
    Run {
        #[command(flatten)]
        source: SourceArgs,
        /// Proxmox ids to import (repeat)
        #[arg(long = "guest", required = true)]
        guest: Vec<u32>,
        /// Horcrux id for a guest: VMID=ID (repeat)
        #[arg(long = "id", value_name = "VMID=ID")]
        id: Vec<String>,
        /// Storage pool for VM disks (default: the VM storage directory)
        #[arg(long)]
        storage: Option<String>,
        /// Return after starting the job
        #[arg(long)]
        no_wait: bool,
    },
    /// State of an import job
    Status { job: String },
}

fn pairs(items: &[String], what: &str) -> Result<Vec<(String, String)>> {
    items
        .iter()
        .map(|s| {
            s.split_once('=')
                .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
                .filter(|(a, b)| !a.is_empty() && !b.is_empty())
                .with_context(|| format!("{what} {s:?}: expected A=B"))
        })
        .collect()
}

pub fn body(s: &SourceArgs) -> Result<Value> {
    let source = if let Some(d) = &s.dir {
        let mut m = Map::new();
        for (id, loc) in pairs(&s.storage_map, "--storage-map")? {
            let v = match loc.split_once(':') {
                Some(("zfs", ds)) => json!({ "zfs": ds }),
                Some(("dir", p)) => json!({ "dir": p }),
                _ => bail!("--storage-map {id}={loc}: use zfs:DATASET or dir:PATH"),
            };
            m.insert(id, v);
        }
        json!({"type": "dir", "path": d, "storage_map": m})
    } else if let Some(h) = &s.ssh {
        json!({"type": "ssh", "host": h, "port": s.ssh_port, "identity": s.identity})
    } else if let Some(f) = &s.vzdump {
        json!({"type": "vzdump", "path": f})
    } else {
        bail!("give a source: --dir, --ssh or --vzdump");
    };
    let to_map = |v: Vec<(String, String)>| -> Map<String, Value> {
        v.into_iter().map(|(a, b)| (a, Value::String(b))).collect()
    };
    Ok(json!({
        "source": source,
        "bridge_map": to_map(pairs(&s.bridge, "--bridge")?),
        "io_devices": to_map(pairs(&s.io_device, "--io-device")?),
    }))
}

fn print_plan(p: &Value, details: bool) {
    for g in p["guests"].as_array().cloned().unwrap_or_default() {
        let rows = g["rows"].as_array().cloned().unwrap_or_default();
        let n = |c: &str| rows.iter().filter(|r| r["carry"] == c).count();
        let state = if let Some(a) = g["imported_as"].as_str() {
            format!("already imported as {a}")
        } else if g["exists"].as_bool() == Some(true) {
            "a guest with this id exists (use --id)".into()
        } else if g["blockers"].as_array().is_some_and(|b| !b.is_empty()) {
            "BLOCKED".into()
        } else {
            "ready".into()
        };
        println!(
            "{} {} {:<24} -> {:<24} carried {} / changed {} / not carried {}  {}",
            g["kind"].as_str().unwrap_or("?"),
            g["vmid"],
            g["name"].as_str().unwrap_or(""),
            g["target_id"].as_str().unwrap_or(""),
            n("carried"),
            n("changed"),
            n("not-carried"),
            state
        );
        for b in g["blockers"].as_array().cloned().unwrap_or_default() {
            println!("    blocker: {}", b.as_str().unwrap_or(""));
        }
        if details {
            for r in &rows {
                println!(
                    "    {:<12} {:<11} {:<28} {}",
                    r["key"].as_str().unwrap_or(""),
                    r["carry"].as_str().unwrap_or(""),
                    r["horcrux"].as_str().unwrap_or(""),
                    r["note"].as_str().unwrap_or("")
                );
            }
            for d in g["qemu_diff"].as_array().cloned().unwrap_or_default() {
                if d["same"].as_bool() != Some(true) {
                    println!(
                        "    qemu {:<18} proxmox={} horcrux={} {}",
                        d["fact"].as_str().unwrap_or(""),
                        d["proxmox"].as_str().unwrap_or(""),
                        d["horcrux"].as_str().unwrap_or(""),
                        d["note"].as_str().unwrap_or("UNEXPLAINED")
                    );
                }
            }
        }
    }
}

fn print_job(j: &Value) {
    println!(
        "job {}: {} ({:.0}%) {}",
        j["id"].as_str().unwrap_or(""),
        j["state"].as_str().unwrap_or(""),
        j["percent"].as_f64().unwrap_or(0.0),
        j["message"].as_str().unwrap_or("")
    );
    for g in j["guests"].as_array().cloned().unwrap_or_default() {
        println!(
            "  {} {}: {} {}",
            g["kind"].as_str().unwrap_or(""),
            g["vmid"],
            g["state"].as_str().unwrap_or(""),
            g["id"]
                .as_str()
                .map(|i| format!("-> {i}"))
                .or_else(|| g["error"].as_str().map(String::from))
                .unwrap_or_default()
        );
        for w in g["warnings"].as_array().cloned().unwrap_or_default() {
            println!("      note: {}", w.as_str().unwrap_or(""));
        }
    }
}

pub async fn handle_import_command(
    command: ImportCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    let ImportCommands::Proxmox { command } = command;
    match command {
        ProxmoxCommands::Plan {
            source,
            guest,
            details,
        } => {
            let mut b = body(&source)?;
            b["guests"] = json!(guest);
            let p: Value = api.post("/api/import/proxmox/plan", &b).await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&p)?);
            } else {
                print_plan(&p, details);
            }
        }
        ProxmoxCommands::Run {
            source,
            guest,
            id,
            storage,
            no_wait,
        } => {
            let mut b = body(&source)?;
            b["guests"] = json!(guest);
            let mut ids = Map::new();
            for (k, v) in pairs(&id, "--id")? {
                k.parse::<u32>()
                    .with_context(|| format!("--id {k}={v}: {k} is not a Proxmox id"))?;
                ids.insert(k, Value::String(v));
            }
            b["ids"] = Value::Object(ids);
            if let Some(s) = storage {
                b["storage"] = json!(s);
            }
            let started: Value = api.post("/api/import/proxmox", &b).await?;
            let job = started["job"].as_str().unwrap_or_default().to_string();
            output::print_info(&format!("import job {job} started"));
            if no_wait {
                return Ok(());
            }
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let j: Value = api
                    .post("/api/import/proxmox/status", &json!({ "job": job }))
                    .await?;
                if j["state"] != "running" {
                    print_job(&j);
                    if j["state"] != "done" {
                        bail!("some guests were not imported");
                    }
                    break;
                }
            }
        }
        ProxmoxCommands::Status { job } => {
            let j: Value = api
                .post("/api/import/proxmox/status", &json!({ "job": job }))
                .await?;
            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&j)?);
            } else {
                print_job(&j);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_body() {
        let s = SourceArgs {
            dir: Some("/tank/pve".into()),
            ssh: None,
            ssh_port: None,
            identity: None,
            vzdump: None,
            storage_map: vec![
                "local-zfs=zfs:tank/pve/data".into(),
                "local=dir:/tank/vz".into(),
            ],
            bridge: vec!["vmbr0=br0".into()],
            io_device: vec!["259:0=/dev/nvme0n1".into()],
        };
        let b = body(&s).unwrap();
        assert_eq!(
            b["source"]["storage_map"]["local-zfs"]["zfs"],
            "tank/pve/data"
        );
        assert_eq!(b["source"]["storage_map"]["local"]["dir"], "/tank/vz");
        assert_eq!(b["bridge_map"]["vmbr0"], "br0");
        assert_eq!(b["io_devices"]["259:0"], "/dev/nvme0n1");
        let bad = SourceArgs {
            storage_map: vec!["x=nfs:y".into()],
            ..s.clone()
        };
        assert!(body(&bad).is_err());
        let none = SourceArgs { dir: None, ..s };
        assert!(body(&none).is_err());
    }
}
