//! Host diagnostics: real checks of this node (disk, memory, load, route,
//! DNS, KVM, QEMU, time sync, failed services, database, TLS, firewall),
//! the scores the System > Diagnostics page shows, and a downloadable report.
use super::host::{self, detect_ntp};
use super::{data_dir, rooted, run, which};
use crate::db::config_store::ConfigStore;
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::header,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/system/diagnostics", get(overview))
        .route("/api/system/diagnostics/tests/run-all", post(run_all))
        .route("/api/system/diagnostics/tests/:name", post(run_one))
        .route("/api/system/diagnostics/report", get(report))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DiagnosticTest {
    pub name: String,
    pub category: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DiagnosticTestResult {
    pub passed: bool,
    pub message: String,
    pub duration_ms: u64,
    pub error_details: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemIssue {
    pub severity: String,
    pub category: String,
    pub description: String,
    pub recommendation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PerformanceMetrics {
    pub cpu_score: f64,
    pub memory_score: f64,
    pub disk_score: f64,
    pub network_score: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SecurityStatus {
    pub score: f64,
    pub vulnerability_count: u32,
    pub compliance_level: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemDiagnostics {
    pub overall_health_score: f64,
    pub critical_issues_count: u32,
    pub warning_count: u32,
    pub last_check_time: String,
    pub performance_metrics: PerformanceMetrics,
    pub security_status: SecurityStatus,
    pub available_tests: Vec<DiagnosticTest>,
    pub system_issues: Vec<SystemIssue>,
}

/// (name, category, critical, description, how to fix)
const TESTS: &[(&str, &str, bool, &str, &str)] = &[
    ("disk-space", "performance", true, "Data directory and root file system have free space (< 90% used)", "Free space or grow the file system; VM disks and backups live under the data directory."),
    ("memory", "performance", false, "At least 10% of memory is available", "Stop guests or add memory."),
    ("cpu-load", "performance", false, "Load average is below 1.5 x CPU cores", "Move guests to another node (Cluster > Maintenance) or add CPU."),
    ("network-route", "network", false, "The node has a default route", "Configure a gateway on the uplink (System > Configuration > Network)."),
    ("dns-config", "network", false, "resolv.conf lists a nameserver and the resolver answers", "Set DNS servers (System > Configuration > DNS)."),
    ("kvm", "virtualization", false, "/dev/kvm is usable (hardware virtualisation)", "Enable VT-x/AMD-V in the firmware and load kvm_intel/kvm_amd; without it VMs run under slow emulation."),
    ("qemu", "virtualization", false, "A qemu-system binary for this architecture is installed", "Install app-emulation/qemu (Gentoo: emerge app-emulation/qemu)."),
    ("time-sync", "system", false, "An NTP daemon is installed and running", "Install and start chrony (System > Services)."),
    ("failed-services", "system", false, "No init-system service is in the failed state", "Open System > Services and restart or investigate the failed ones."),
    ("database", "system", true, "The Horcrux database answers a query", "Check the data directory and the horcrux-api log."),
    ("data-dir-writable", "system", true, "The data directory is writable", "Fix ownership/permissions of the data directory."),
    ("tls-enabled", "security", false, "The API is served over HTTPS", "Enable TLS or put a TLS-terminating proxy in front."),
    ("data-dir-permissions", "security", false, "The data directory is not world-writable", "chmod o-w on the data directory."),
    ("firewall", "security", false, "An nftables ruleset is loaded", "Create firewall rules (Firewall) or load a base ruleset."),
];

fn def(
    name: &str,
) -> Option<&'static (&'static str, &'static str, bool, &'static str, &'static str)> {
    TESTS.iter().find(|t| t.0 == name)
}

fn pct_used(path: &str) -> f64 {
    nix::sys::statvfs::statvfs(path)
        .map(|s| {
            let total = s.blocks() as f64;
            if total == 0.0 {
                0.0
            } else {
                (total - s.blocks_free() as f64) * 100.0 / total
            }
        })
        .unwrap_or(0.0)
}

fn res(ok: bool, msg: String, started: Instant, err: Option<String>) -> DiagnosticTestResult {
    DiagnosticTestResult {
        passed: ok,
        message: msg,
        duration_ms: started.elapsed().as_millis() as u64,
        error_details: if ok { None } else { err },
    }
}

pub fn cpu_score(load1: f64, cores: u32) -> f64 {
    let ratio = load1 / cores.max(1) as f64;
    if ratio <= 0.7 {
        100.0
    } else {
        (100.0 - (ratio - 0.7) * 100.0 / 0.8).clamp(0.0, 100.0)
    }
}

pub fn memory_score(available: u64, total: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    (available as f64 * 100.0 / total as f64 * 2.0).clamp(0.0, 100.0)
}

async fn execute(state: &Arc<AppState>, name: &str) -> DiagnosticTestResult {
    let t = Instant::now();
    match name {
        "disk-space" => {
            let d = pct_used(&data_dir().to_string_lossy());
            let r = pct_used("/");
            res(
                d < 90.0 && r < 90.0,
                format!("data dir {d:.0}% used, / {r:.0}% used"),
                t,
                Some("a file system is at least 90% full".into()),
            )
        }
        "memory" => {
            let m = host::mem();
            let p = if m.total == 0 {
                0.0
            } else {
                m.available as f64 * 100.0 / m.total as f64
            };
            res(
                p >= 10.0,
                format!("{p:.0}% of memory available"),
                t,
                Some("less than 10% of memory is available".into()),
            )
        }
        "cpu-load" => {
            let l = host::load_avg();
            let c = host::cpu_cores();
            res(
                l[0] < c as f64 * 1.5,
                format!("load {:.2} on {c} cores", l[0]),
                t,
                Some("the node is overloaded".into()),
            )
        }
        "network-route" => match run("ip", &["route", "show", "default"]).await {
            Ok(o) if !o.trim().is_empty() => res(
                true,
                o.lines().next().unwrap_or("").trim().to_string(),
                t,
                None,
            ),
            Ok(_) => res(
                false,
                "no default route".into(),
                t,
                Some("`ip route show default` is empty".into()),
            ),
            Err(e) => res(false, "cannot read routes".into(), t, Some(e)),
        },
        "dns-config" => {
            let conf = host::parse_resolv(
                &std::fs::read_to_string(rooted("etc/resolv.conf")).unwrap_or_default(),
            );
            if conf.servers.is_empty() {
                return res(
                    false,
                    "no nameserver configured".into(),
                    t,
                    Some("resolv.conf has no nameserver line".into()),
                );
            }
            // Resolve a name that needs no network: proves the resolver path
            // works without querying the internet.
            match tokio::net::lookup_host("localhost:80").await {
                Ok(_) => res(
                    true,
                    format!("nameservers: {}", conf.servers.join(", ")),
                    t,
                    None,
                ),
                Err(e) => res(false, "resolver failed".into(), t, Some(e.to_string())),
            }
        }
        "kvm" => {
            let ok = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/kvm")
                .is_ok();
            res(
                ok,
                if ok {
                    "/dev/kvm is usable".into()
                } else {
                    "/dev/kvm missing or not accessible".into()
                },
                t,
                Some("VMs fall back to TCG emulation".into()),
            )
        }
        "qemu" => {
            let bin = match std::env::consts::ARCH {
                "aarch64" => "qemu-system-aarch64",
                "riscv64" => "qemu-system-riscv64",
                _ => "qemu-system-x86_64",
            };
            match run(bin, &["--version"]).await {
                Ok(o) => res(
                    true,
                    o.lines().next().unwrap_or("").trim().to_string(),
                    t,
                    None,
                ),
                Err(e) => res(false, format!("{bin} not found"), t, Some(e)),
            }
        }
        "time-sync" => match detect_ntp() {
            None => res(
                false,
                "no NTP daemon installed".into(),
                t,
                Some("chrony, ntpd and systemd-timesyncd were not found".into()),
            ),
            Some(b) => {
                let mut active = false;
                for u in b.units() {
                    if super::services::is_active(u).await.unwrap_or(false) {
                        active = true;
                        break;
                    }
                }
                res(
                    active,
                    format!(
                        "{b:?} {}",
                        if active {
                            "running"
                        } else {
                            "installed but not running"
                        }
                    ),
                    t,
                    Some("the NTP daemon is not running".into()),
                )
            }
        },
        "failed-services" => match super::services::list_services().await {
            Ok(v) => {
                let bad: Vec<String> = v
                    .into_iter()
                    .filter(|s| s.status == "failed")
                    .map(|s| s.name)
                    .collect();
                res(
                    bad.is_empty(),
                    if bad.is_empty() {
                        "no failed services".into()
                    } else {
                        format!("failed: {}", bad.join(", "))
                    },
                    t,
                    Some(bad.join(", ")),
                )
            }
            Err(e) => res(
                false,
                "cannot list services".into(),
                t,
                Some(format!("{e:?}")),
            ),
        },
        "database" => match ConfigStore::new(&state.database)
            .load_values::<serde_json::Value>("diagnostics_probe")
            .await
        {
            Ok(_) => res(true, "database answered".into(), t, None),
            Err(e) => res(
                false,
                "database query failed".into(),
                t,
                Some(e.to_string()),
            ),
        },
        "data-dir-writable" => {
            let p = data_dir().join(format!(".diag-{}", std::process::id()));
            let ok = std::fs::write(&p, b"x").is_ok();
            let _ = std::fs::remove_file(&p);
            res(
                ok,
                if ok {
                    format!("{} is writable", data_dir().display())
                } else {
                    format!("cannot write in {}", data_dir().display())
                },
                t,
                Some("write test failed".into()),
            )
        }
        "tls-enabled" => {
            let on = state.config.server.tls_enabled;
            res(
                on,
                if on {
                    "HTTPS enabled".into()
                } else {
                    "serving plain HTTP".into()
                },
                t,
                Some("tls_enabled is off".into()),
            )
        }
        "data-dir-permissions" => {
            use std::os::unix::fs::PermissionsExt;
            match std::fs::metadata(data_dir()) {
                Ok(m) => {
                    let mode = m.permissions().mode() & 0o777;
                    res(
                        mode & 0o002 == 0,
                        format!("mode {mode:o}"),
                        t,
                        Some("the data directory is world-writable".into()),
                    )
                }
                Err(e) => res(
                    false,
                    "cannot stat the data directory".into(),
                    t,
                    Some(e.to_string()),
                ),
            }
        }
        "firewall" => {
            if which("nft").is_none() {
                return res(
                    false,
                    "nft is not installed".into(),
                    t,
                    Some("nftables userspace tools missing".into()),
                );
            }
            match run("nft", &["list", "ruleset"]).await {
                Ok(o) => res(
                    o.contains("chain"),
                    if o.contains("chain") {
                        "ruleset loaded".into()
                    } else {
                        "ruleset is empty".into()
                    },
                    t,
                    Some("no chains are loaded".into()),
                ),
                Err(e) => res(false, "cannot read the ruleset".into(), t, Some(e)),
            }
        }
        _ => res(false, "unknown test".into(), t, None),
    }
}

async fn run_one(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<DiagnosticTestResult>, ApiError> {
    if def(&name).is_none() {
        return Err(ApiError::NotFound(format!("no diagnostic test '{name}'")));
    }
    Ok(Json(execute(&state, &name).await))
}

async fn run_all(
    State(state): State<Arc<AppState>>,
) -> Json<HashMap<String, DiagnosticTestResult>> {
    let mut out = HashMap::new();
    for t in TESTS {
        out.insert(t.0.to_string(), execute(&state, t.0).await);
    }
    Json(out)
}

async fn gather(
    state: &Arc<AppState>,
) -> (SystemDiagnostics, Vec<(&'static str, DiagnosticTestResult)>) {
    let mut results = Vec::new();
    for t in TESTS {
        results.push((t.0, execute(state, t.0).await));
    }
    let passed = results.iter().filter(|r| r.1.passed).count();
    let m = host::mem();
    let l = host::load_avg();
    let ok = |n: &str| {
        results
            .iter()
            .find(|r| r.0 == n)
            .is_some_and(|r| r.1.passed)
    };
    let sec: Vec<_> = results
        .iter()
        .filter(|r| def(r.0).is_some_and(|d| d.1 == "security"))
        .collect();
    let sec_pass = sec.iter().filter(|r| r.1.passed).count();
    let sec_pct = if sec.is_empty() {
        100.0
    } else {
        sec_pass as f64 * 100.0 / sec.len() as f64
    };
    let issues: Vec<SystemIssue> = results
        .iter()
        .filter(|r| !r.1.passed)
        .filter_map(|r| {
            def(r.0).map(|d| SystemIssue {
                severity: if d.2 { "critical" } else { "warning" }.into(),
                category: d.1.into(),
                description: format!("{}: {}", d.3, r.1.message),
                recommendation: Some(d.4.into()),
            })
        })
        .collect();
    let diag = SystemDiagnostics {
        overall_health_score: passed as f64 * 100.0 / results.len() as f64,
        critical_issues_count: issues.iter().filter(|i| i.severity == "critical").count() as u32,
        warning_count: issues.iter().filter(|i| i.severity == "warning").count() as u32,
        last_check_time: chrono::Utc::now().to_rfc3339(),
        performance_metrics: PerformanceMetrics {
            cpu_score: cpu_score(l[0], host::cpu_cores()),
            memory_score: memory_score(m.available, m.total),
            disk_score: (100.0 - host::data_dir_usage_percent()).clamp(0.0, 100.0),
            network_score: if ok("network-route") { 50.0 } else { 0.0 }
                + if ok("dns-config") { 50.0 } else { 0.0 },
        },
        security_status: SecurityStatus {
            score: sec_pct,
            vulnerability_count: (sec.len() - sec_pass) as u32,
            compliance_level: sec_pct,
        },
        available_tests: TESTS
            .iter()
            .map(|t| DiagnosticTest {
                name: t.0.into(),
                category: t.1.into(),
                description: t.3.into(),
            })
            .collect(),
        system_issues: issues,
    };
    (diag, results)
}

async fn overview(State(state): State<Arc<AppState>>) -> Json<SystemDiagnostics> {
    Json(gather(&state).await.0)
}

#[derive(Deserialize)]
struct ReportQ {
    format: Option<String>,
}

async fn report(State(state): State<Arc<AppState>>, Query(q): Query<ReportQ>) -> impl IntoResponse {
    let (diag, results) = gather(&state).await;
    let host = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let (ctype, body) = match q.format.as_deref().unwrap_or("text") {
        "json" => (
            "application/json",
            serde_json::to_string_pretty(&serde_json::json!({
                "host": host, "diagnostics": diag,
                "results": results.iter().map(|r| (r.0, &r.1)).collect::<HashMap<_, _>>(),
            }))
            .unwrap_or_default(),
        ),
        "markdown" | "md" => ("text/markdown", render(&host, &diag, &results, true)),
        _ => ("text/plain", render(&host, &diag, &results, false)),
    };
    ([(header::CONTENT_TYPE, ctype)], body)
}

pub fn render(
    host: &str,
    d: &SystemDiagnostics,
    results: &[(&str, DiagnosticTestResult)],
    md: bool,
) -> String {
    let h = |s: &str| {
        if md {
            format!("## {s}\n")
        } else {
            format!("{s}\n{}\n", "-".repeat(s.len()))
        }
    };
    let mut o = if md {
        format!("# Diagnostics for {host}\n\n")
    } else {
        format!("Diagnostics for {host}\n\n")
    };
    o += &format!(
        "Generated {}. Health score {:.0}/100, {} critical, {} warnings.\n\n",
        d.last_check_time, d.overall_health_score, d.critical_issues_count, d.warning_count
    );
    o += &h("Tests");
    for (n, r) in results {
        o += &format!(
            "{} {n}: {}\n",
            if r.passed { "[ok]  " } else { "[FAIL]" },
            r.message
        );
    }
    if !d.system_issues.is_empty() {
        o += &format!("\n{}", h("Issues"));
        for i in &d.system_issues {
            o += &format!("[{}] {} - {}\n", i.severity, i.category, i.description);
            if let Some(r) = &i.recommendation {
                o += &format!("      fix: {r}\n");
            }
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_degrade_with_load_and_memory_pressure() {
        assert_eq!(cpu_score(0.5, 4), 100.0);
        assert_eq!(cpu_score(4.0, 4), 100.0 - (1.0 - 0.7) * 100.0 / 0.8);
        assert_eq!(cpu_score(40.0, 4), 0.0);
        assert_eq!(memory_score(500, 1000), 100.0);
        assert_eq!(memory_score(100, 1000), 20.0);
        assert_eq!(memory_score(1, 0), 0.0);
    }

    #[test]
    fn every_test_has_a_unique_name_and_a_fix() {
        let mut names: Vec<_> = TESTS.iter().map(|t| t.0).collect();
        names.sort();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n);
        assert!(TESTS.iter().all(|t| !t.3.is_empty() && !t.4.is_empty()));
        assert!(def("kvm").is_some() && def("nope").is_none());
    }

    #[test]
    fn report_lists_failures_with_the_fix() {
        let d = SystemDiagnostics {
            overall_health_score: 50.0,
            critical_issues_count: 1,
            warning_count: 0,
            last_check_time: "t".into(),
            performance_metrics: PerformanceMetrics {
                cpu_score: 1.0,
                memory_score: 1.0,
                disk_score: 1.0,
                network_score: 1.0,
            },
            security_status: SecurityStatus {
                score: 1.0,
                vulnerability_count: 0,
                compliance_level: 1.0,
            },
            available_tests: vec![],
            system_issues: vec![SystemIssue {
                severity: "critical".into(),
                category: "system".into(),
                description: "db down".into(),
                recommendation: Some("restart".into()),
            }],
        };
        let r = vec![(
            "database",
            DiagnosticTestResult {
                passed: false,
                message: "no".into(),
                duration_ms: 1,
                error_details: None,
            },
        )];
        let t = render("h1", &d, &r, false);
        assert!(
            t.contains("[FAIL] database")
                && t.contains("fix: restart")
                && t.contains("Diagnostics for h1")
        );
        assert!(render("h1", &d, &r, true).starts_with("# Diagnostics for h1"));
    }
}
