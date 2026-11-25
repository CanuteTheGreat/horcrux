//! Ceph release, kernel line and platform versions:
//! `horcrux ceph releases|install|upgrade`, `horcrux kernel status|set|apply`,
//! `horcrux versions`.
use crate::api::ApiClient;
use crate::output;
use anyhow::Result;
use clap::Subcommand;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub enum CephCommands {
    /// Ceph releases the package repositories carry for this node
    Releases,
    /// Install a Ceph release (and optionally create or join a cluster)
    Install {
        /// Release: major version (21) or name (umbrella, tentacle)
        release: String,
        /// Create a new cluster on this public network (CIDR), first monitor + manager here
        #[arg(long, conflicts_with = "join")]
        new_cluster: Option<String>,
        /// Separate replication network (CIDR) for a new cluster
        #[arg(long, requires = "new_cluster")]
        cluster_network: Option<String>,
        /// Join the cluster of this Horcrux node (host or host:port)
        #[arg(long)]
        join: Option<String>,
        /// Administrator on the --join node
        #[arg(long, requires = "join")]
        join_user: Option<String>,
        /// Its password (or HORCRUX_JOIN_PASSWORD)
        #[arg(long, env = "HORCRUX_JOIN_PASSWORD", requires = "join")]
        join_password: Option<String>,
        /// This node's monitor address (default: its address in the public network)
        #[arg(long)]
        mon_ip: Option<String>,
    },
    /// Show the upgrade state (daemon versions, next step)
    UpgradeStatus,
    /// Upgrade to a newer release, in Ceph's order
    Upgrade {
        release: String,
        /// Steps to run (packages, restart-mon, restart-mgr, restart-osd, restart-mds,
        /// restart-rgw, finish); default: all of them on this node
        #[arg(long, value_delimiter = ',')]
        steps: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum KernelCommands {
    /// The kernel line and what each line selects on this node
    Status,
    /// Choose the kernel line: default (stable) or newest (opt-in)
    Set { line: String },
    /// Run the update now (the kernel arrives; the reboot is yours)
    Apply,
}

fn s(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(x) => x.clone(),
        o => o.to_string(),
    }
}

fn emit(v: &Value, fmt: &str) -> Result<()> {
    match fmt {
        "yaml" => output::print_yaml(v),
        _ => output::print_json(v),
    }
}

pub async fn handle_ceph(cmd: CephCommands, api: &ApiClient, fmt: &str) -> Result<()> {
    match cmd {
        CephCommands::Releases => {
            let r: Value = api.get("/api/ceph/releases").await?;
            if fmt != "table" {
                return emit(&r, fmt);
            }
            output::print_info(&format!(
                "installed: {}  pinned: {}  recommended: {}",
                s(&r["installed"]),
                s(&r["pinned"]),
                s(&r["recommended"])
            ));
            for x in r["releases"].as_array().into_iter().flatten() {
                println!(
                    "{:>4}  {:<10} {:<12} {:<8} {}",
                    s(&x["major"]),
                    s(&x["name"]),
                    s(&x["newest"]),
                    s(&x["keyword"]),
                    s(&x["repo"])
                );
            }
        }
        CephCommands::Install {
            release,
            new_cluster,
            cluster_network,
            join,
            join_user,
            join_password,
            mon_ip,
        } => {
            let bootstrap = match (new_cluster, join) {
                (Some(net), _) => json!({"mode": "new", "public_network": net,
                    "cluster_network": cluster_network, "mon_ip": mon_ip}),
                (None, Some(peer)) => json!({"mode": "join", "peer": peer,
                    "username": join_user.unwrap_or_else(|| "root".into()),
                    "password": join_password.unwrap_or_default(), "mon_ip": mon_ip}),
                _ => json!({"mode": "none"}),
            };
            let r: Value = api
                .post(
                    "/api/ceph/install",
                    &json!({"release": release, "bootstrap": bootstrap}),
                )
                .await?;
            output::print_success(&format!(
                "Ceph {} install started: task {}",
                s(&r["name"]),
                s(&r["upid"])
            ));
        }
        CephCommands::UpgradeStatus => {
            let r: Value = api.get("/api/ceph/upgrade").await?;
            emit(&r, if fmt == "table" { "yaml" } else { fmt })?;
        }
        CephCommands::Upgrade { release, steps } => {
            let r: Value = api
                .post(
                    "/api/ceph/upgrade",
                    &json!({"release": release, "steps": steps}),
                )
                .await?;
            output::print_success(&format!("upgrade started: task {}", s(&r["upid"])));
        }
    }
    Ok(())
}

pub async fn handle_kernel(cmd: KernelCommands, api: &ApiClient, fmt: &str) -> Result<()> {
    let show = |r: &Value| {
        output::print_info(&format!(
            "line: {}  running: {}  default: {}  newest: {}",
            s(&r["line"]),
            s(&r["running"]),
            s(&r["default_version"]),
            s(&r["newest_version"])
        ));
        if let Some(h) = r["held_back"].as_str() {
            output::print_info(h);
        }
        if let Some(p) = r["update_pending"].as_str() {
            output::print_info(&format!(
                "{p} is selected but not installed: `horcrux kernel apply` (or wait for the nightly update)"
            ));
        }
        if r["reboot_required"].as_bool() == Some(true) {
            output::print_info("a newer kernel is installed: reboot when convenient");
        }
    };
    match cmd {
        KernelCommands::Status => {
            let r: Value = api.get("/api/system/kernel-line").await?;
            if fmt != "table" {
                return emit(&r, fmt);
            }
            show(&r);
        }
        KernelCommands::Set { line } => {
            let r: Value = api
                .put("/api/system/kernel-line", &json!({"line": line}))
                .await?;
            output::print_success("kernel line saved");
            show(&r);
        }
        KernelCommands::Apply => {
            let r: Value = api
                .post("/api/system/kernel-line/apply", &json!({}))
                .await?;
            output::print_success(&format!("update started: task {}", s(&r["upid"])));
        }
    }
    Ok(())
}

pub async fn handle_versions(api: &ApiClient, fmt: &str) -> Result<()> {
    let r: Value = api.get("/api/system/versions").await?;
    if fmt != "table" {
        return emit(&r, fmt);
    }
    println!("Horcrux {}", s(&r["horcrux"]));
    for c in r["components"].as_array().into_iter().flatten() {
        println!(
            "{:<14} {:<14} {}{}",
            s(&c["name"]),
            c["version"].as_str().unwrap_or("not installed"),
            c["running"]
                .as_str()
                .map(|x| format!("(running {x}) "))
                .unwrap_or_default(),
            c["note"].as_str().unwrap_or("")
        );
    }
    Ok(())
}
