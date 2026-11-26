//! Remote Play readiness for VMs (docs/design/gaming-vm.md section 16):
//! Steam Remote Play / Steam Link and, as the user's choice, Moonlight
//! (Sunshine installed by the user in the guest).
//!
//! * `GET/PUT /api/vms/remote-play/settings`   node settings (ports, Wake-on-LAN
//!   ports, relay, nft table, Steam installer URL); declarative `remote_play`
//! * `GET/PUT /api/vms/:id/remote-play`        the VM's settings
//!   (`hardware.adv.remote_play`; changes apply at once, also while it runs)
//! * `GET  /api/vms/:id/remote-play/status`    prerequisite checklist, resolved
//!   network, address, the rules Horcrux keeps and the nftables text
//! * `POST /api/vms/:id/remote-play/guest-setup` through the QEMU guest agent:
//!   check the guest, install Steam (Valve's signed installer), automatic
//!   logon (LSA secret), Big Picture at logon, no sleep
//! * `POST /api/vms/:id/remote-play/lan-nic`   move the VM's first card onto
//!   the LAN (a bridge with a physical port, else macvtap on the wired
//!   uplink); the VM must be stopped
//!
//! A background service keeps the node side in step every
//! `reconcile_secs` and after every change: the nftables port forwards
//! from the allowed LAN networks to VMs behind NAT, the Steam discovery
//! relay, VM firewall allow rules for LAN VMs that have a Horcrux firewall,
//! and the Wake-on-LAN listeners that start (or resume) a VM when a magic
//! packet for its MAC arrives from an allowed network.
use crate::error::ApiError;
use crate::vm::remote_play::{self as rp, Facts, Forward, Reach, Relay, RelayTarget};
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use horcrux_common::remote_play::{
    parse_ports, ports_without, GuestReport, GuestSetupRequest, RemotePlayClient, RemotePlayRule,
    RemotePlaySettings, RemotePlayStatus, VmRemotePlay,
};
use horcrux_common::{NicAttach, VmConfig, VmStatus};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const SETTINGS_KIND: &str = "remote_play_settings";
const GUEST_KIND: &str = "remote_play_guest";
const SYS_NET: &str = "/sys/class/net";

pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    spawn_service(state.clone());
    Router::new()
        .route(
            "/api/vms/remote-play/settings",
            get(get_settings).put(put_settings),
        )
        .route("/api/vms/:id/remote-play", get(get_vm).put(put_vm))
        .route("/api/vms/:id/remote-play/status", get(status))
        .route("/api/vms/:id/remote-play/guest-setup", post(guest_setup))
        .route("/api/vms/:id/remote-play/lan-nic", post(lan_nic))
}

// ------------------------------------------------------------- settings

fn settings_cell() -> &'static Mutex<RemotePlaySettings> {
    static S: OnceLock<Mutex<RemotePlaySettings>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(RemotePlaySettings::default()))
}

pub fn settings() -> RemotePlaySettings {
    settings_cell()
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default()
}

pub async fn load_settings(state: &AppState) -> RemotePlaySettings {
    let s = crate::db::config_store::ConfigStore::new(&state.database)
        .load_values::<RemotePlaySettings>(SETTINGS_KIND)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .unwrap_or_default();
    if let Ok(mut c) = settings_cell().lock() {
        *c = s.clone();
    }
    s
}

/// Store new node settings (API and declarative apply).
pub async fn set_settings(state: &AppState, s: RemotePlaySettings) -> Result<(), String> {
    s.validate()?;
    crate::db::config_store::ConfigStore::new(&state.database)
        .put(SETTINGS_KIND, "default", &s)
        .await
        .map_err(|e| e.to_string())?;
    if let Ok(mut c) = settings_cell().lock() {
        *c = s;
    }
    kick();
    Ok(())
}

async fn get_settings(State(state): State<Arc<AppState>>) -> Json<RemotePlaySettings> {
    Json(load_settings(&state).await)
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(s): Json<RemotePlaySettings>,
) -> Result<Json<RemotePlaySettings>, ApiError> {
    set_settings(&state, s.clone())
        .await
        .map_err(ApiError::BadRequest)?;
    Ok(Json(s))
}

// ------------------------------------------------------------ per VM

async fn get_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmRemotePlay>, ApiError> {
    let vm = state.vm_manager.get_vm(&id).await?;
    Ok(Json(vm.hardware.adv.remote_play.unwrap_or_default()))
}

async fn put_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut want): Json<VmRemotePlay>,
) -> Result<Json<VmRemotePlay>, ApiError> {
    want.clients.sort();
    want.clients.dedup();
    want.lan_sources = want
        .lan_sources
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    want.validate().map_err(ApiError::BadRequest)?;
    let vm = state.vm_manager.get_vm(&id).await?;
    if vm.hypervisor != horcrux_common::VmHypervisor::Qemu {
        return Err(ApiError::BadRequest(
            "Remote Play is offered for QEMU VMs".into(),
        ));
    }
    let mut hw = vm.hardware.clone();
    hw.adv.remote_play = Some(want.clone());
    state
        .vm_manager
        .update_config(
            &id,
            crate::vm::hardware::VmConfigUpdate {
                hardware: Some(hw),
                ..Default::default()
            },
        )
        .await?;
    kick();
    Ok(Json(want))
}

/// `POST /api/vms/:id/remote-play/lan-nic`: the first card goes onto the
/// LAN, keeping its MAC (Wake-on-LAN and the guest's DHCP lease follow it).
async fn lan_nic(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    let vm = state.vm_manager.get_vm(&id).await?;
    let sys = std::path::Path::new(SYS_NET);
    let mut hw = vm.hardware.clone();
    let lan_bridge = crate::api::vm_hw::host_bridges()
        .into_iter()
        .map(|b| b.name)
        .find(|b| !rp::bridge_uplinks(sys, b).is_empty());
    let (attach, dev) = match lan_bridge {
        Some(b) => (NicAttach::Bridge, b),
        None => match crate::api::vm_hw::macvtap_uplinks().into_iter().next() {
            Some(i) => (NicAttach::Macvtap, i),
            None => {
                return Err(ApiError::BadRequest(
                    "this host has no bridge with a physical port and no wired interface for macvtap: create a bridge on the LAN uplink (Network page) first".into(),
                ))
            }
        },
    };
    match hw.nets.first_mut() {
        Some(n) => {
            n.attach = attach;
            n.bridge = dev;
            if attach == NicAttach::Macvtap {
                n.vlan = None;
            }
        }
        None => hw.nets.push(horcrux_common::VmNic {
            model: Default::default(),
            bridge: dev,
            attach,
            mac: None,
            vlan: None,
            rate_mbps: None,
            macvtap_multicast: Default::default(),
        }),
    }
    crate::api::vm_hw::check_host_hardware(&state, &hw).await?;
    let out = state
        .vm_manager
        .update_config(
            &id,
            crate::vm::hardware::VmConfigUpdate {
                hardware: Some(hw),
                ..Default::default()
            },
        )
        .await?;
    kick();
    Ok(Json(out))
}

// ------------------------------------------------------------ runtime

/// What the service found for one VM.
#[derive(Debug, Clone, Default)]
struct VmRt {
    network: String,
    nic: String,
    bridge: String,
    mac: Option<String>,
    address: Option<Ipv4Addr>,
    sources: Vec<String>,
    rules: Vec<RemotePlayRule>,
    firewall: bool,
    firewall_allows: bool,
}

#[derive(Default)]
struct Runtime {
    relay: Relay,
    /// None = not wanted (no VM behind NAT with Steam, relay off).
    relay_bound: Option<Result<(), String>>,
    relay_port: u16,
    wol: BTreeMap<u16, Result<(), String>>,
    nft_text: String,
    nft_result: Option<Result<(), String>>,
    vms: HashMap<String, VmRt>,
    /// MAC -> (VM, allowed sources)
    wake_macs: HashMap<[u8; 6], (String, Vec<String>)>,
    last_wake: HashMap<String, Instant>,
    /// Last known address per VM (kept while the VM is off).
    known_addr: HashMap<String, Ipv4Addr>,
}

fn rt() -> &'static Mutex<Runtime> {
    static R: OnceLock<Mutex<Runtime>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Runtime::default()))
}

fn kick_cell() -> &'static tokio::sync::Notify {
    static N: OnceLock<tokio::sync::Notify> = OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

/// Re-run the reconcile now (after a settings or VM change).
pub fn kick() {
    kick_cell().notify_one();
}

fn spawn_service(state: Arc<AppState>) {
    tokio::spawn(async move {
        load_settings(&state).await;
        let mut listeners = Listeners::default();
        loop {
            reconcile(&state, &mut listeners).await;
            let secs = settings().reconcile_secs.max(2);
            let _ = tokio::time::timeout(Duration::from_secs(secs), kick_cell().notified()).await;
        }
    });
}

#[derive(Default)]
struct Listeners {
    wol: HashMap<u16, tokio::task::JoinHandle<()>>,
    relay: Option<(u16, tokio::task::JoinHandle<()>)>,
}

/// A UDP socket on 0.0.0.0:port that shares the port with other programs
/// that also allow it (SO_REUSEADDR: broadcasts reach every such socket).
fn udp_listener(port: u16) -> Result<tokio::net::UdpSocket, String> {
    use nix::sys::socket::{
        bind, setsockopt, socket, sockopt, AddressFamily, SockFlag, SockType, SockaddrIn,
    };
    use std::os::fd::AsRawFd;
    let fd = socket(
        AddressFamily::Inet,
        SockType::Datagram,
        SockFlag::SOCK_CLOEXEC | SockFlag::SOCK_NONBLOCK,
        None,
    )
    .map_err(|e| e.to_string())?;
    setsockopt(&fd, sockopt::ReuseAddr, &true).map_err(|e| e.to_string())?;
    setsockopt(&fd, sockopt::Broadcast, &true).map_err(|e| e.to_string())?;
    bind(fd.as_raw_fd(), &SockaddrIn::new(0, 0, 0, 0, port)).map_err(|e| e.to_string())?;
    let std_sock = std::net::UdpSocket::from(fd);
    tokio::net::UdpSocket::from_std(std_sock).map_err(|e| e.to_string())
}

async fn wol_loop(state: Arc<AppState>, sock: tokio::net::UdpSocket) {
    let mut buf = vec![0u8; 2048];
    loop {
        let Ok((n, from)) = sock.recv_from(&mut buf).await else {
            continue;
        };
        let std::net::SocketAddr::V4(from) = from else {
            continue;
        };
        let Some(mac) = rp::parse_wol(&buf[..n]) else {
            continue;
        };
        let target = rt()
            .lock()
            .ok()
            .and_then(|r| r.wake_macs.get(&mac).cloned());
        let Some((vm, sources)) = target else {
            continue;
        };
        if !rp::source_allowed(&sources, *from.ip()) {
            tracing::info!(
                "remote play: Wake-on-LAN for VM {vm} from {} ignored (not an allowed network)",
                from.ip()
            );
            continue;
        }
        wake(&state, &vm, &format!("Wake-on-LAN from {}", from.ip())).await;
    }
}

async fn relay_loop(state: Arc<AppState>, sock: tokio::net::UdpSocket, port: u16) {
    let mut buf = vec![0u8; 4096];
    loop {
        let Ok((n, from)) = sock.recv_from(&mut buf).await else {
            continue;
        };
        let std::net::SocketAddr::V4(from) = from else {
            continue;
        };
        let ttl = Duration::from_secs(settings().relay_client_secs);
        let acts = match rt().lock() {
            Ok(mut r) => r
                .relay
                .on_datagram(from, &buf[..n], port, ttl, Instant::now()),
            Err(_) => continue,
        };
        for to in acts.send {
            if let Err(e) = sock.send_to(&buf[..n], to).await {
                tracing::debug!("remote play relay: send to {to}: {e}");
            }
        }
        for vm in acts.wake {
            wake(&state, &vm, &format!("Steam discovery from {}", from.ip())).await;
        }
    }
}

/// Start (or resume) a VM for a client, at most once per cooldown.
async fn wake(state: &Arc<AppState>, vm: &str, why: &str) {
    let cooldown = Duration::from_secs(settings().wake_cooldown_secs);
    {
        let Ok(mut r) = rt().lock() else { return };
        if r.last_wake.get(vm).is_some_and(|t| t.elapsed() < cooldown) {
            return;
        }
        r.last_wake.insert(vm.to_string(), Instant::now());
    }
    match state.vm_manager.get_vm(vm).await {
        Ok(c) if c.status == VmStatus::Running => return,
        Ok(_) => {}
        Err(_) => return,
    }
    let res = crate::app::start_vm_by_id(state, vm).await;
    let ok = res.is_ok();
    match &res {
        Ok(_) => tracing::info!("remote play: VM {vm} started ({why})"),
        Err(e) => tracing::warn!("remote play: VM {vm} not started ({why}): {e:?}"),
    }
    use crate::audit::{AuditEvent, AuditEventType, AuditResult, AuditSeverity};
    state
        .audit_logger
        .log(AuditEvent {
            timestamp: chrono::Utc::now(),
            event_type: AuditEventType::VmStarted,
            severity: if ok {
                AuditSeverity::Info
            } else {
                AuditSeverity::Warning
            },
            user: None,
            source_ip: None,
            resource: Some(vm.to_string()),
            action: "remote_play_wake".into(),
            result: if ok {
                AuditResult::Success
            } else {
                AuditResult::Failure
            },
            details: Some(match res {
                Ok(_) => why.to_string(),
                Err(e) => format!("{why}: {e:?}"),
            }),
            session_id: None,
        })
        .await;
}

async fn cmd_out(cmd: &str, args: &[&str]) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(cmd).args(args).output(),
    )
    .await
    .ok()?
    .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

async fn nft_apply(table: &str, body: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let script = rp::nft_apply_script(table, body);
    let mut child = tokio::process::Command::new("nft")
        .args(["-f", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("nft: {e}"))?;
    if let Some(mut i) = child.stdin.take() {
        i.write_all(script.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
    }
    let out = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .map_err(|_| "nft did not finish".to_string())?
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn qga_socket(vm: &VmConfig) -> Option<PathBuf> {
    let d = vm.disks.first()?.path.clone();
    (!d.is_empty())
        .then(|| crate::vm::display::DisplayRuntime::qga_socket(std::path::Path::new(&d)))
}

async fn guest_addresses(vm: &VmConfig) -> Vec<(Ipv4Addr, String)> {
    if !vm.hardware.guest_agent || vm.status != VmStatus::Running {
        return Vec::new();
    }
    let Some(sock) = qga_socket(vm) else {
        return Vec::new();
    };
    match crate::vm::devices::qga_exec(&sock, "guest-network-get-interfaces", json!({}), 3).await {
        Ok(v) => rp::parse_qga_interfaces(&v),
        Err(_) => Vec::new(),
    }
}

const RULE_PREFIX: &str = "remote-play-";

/// VM firewall allow rules for a LAN VM that has Horcrux firewall rules:
/// one accept rule per client, protocol and allowed network. Returns
/// (has firewall, allow rules in place).
async fn sync_vm_firewall(
    state: &Arc<AppState>,
    vm: &str,
    want: &[(RemotePlayClient, &'static str, String, String)],
) -> (bool, bool) {
    use crate::firewall::{Direction, FirewallAction, FirewallRule, FirewallScope, Protocol};
    let scope = FirewallScope::Vm(vm.to_string());
    let have = state.firewall_manager.list_rules(scope.clone()).await;
    let own: Vec<&FirewallRule> = have
        .iter()
        .filter(|r| r.id.starts_with(RULE_PREFIX))
        .collect();
    let foreign = have.len() - own.len();
    let mut wanted_ids = Vec::new();
    if foreign > 0 {
        for (i, (client, proto, ports, src)) in want.iter().enumerate() {
            let id = format!(
                "{RULE_PREFIX}{}-{proto}-{i}",
                format!("{client:?}").to_lowercase()
            );
            wanted_ids.push(id.clone());
            let rule = FirewallRule {
                id: id.clone(),
                enabled: true,
                action: FirewallAction::Accept,
                direction: Direction::In,
                protocol: Some(if *proto == "tcp" {
                    Protocol::Tcp
                } else {
                    Protocol::Udp
                }),
                source: Some(src.clone()),
                dest: None,
                sport: None,
                dport: Some(ports.replace(", ", ",").replace('-', ":")),
                comment: Some(format!("Remote Play ({})", client.label())),
                log: false,
                position: 0,
            };
            let same = own.iter().any(|r| {
                r.id == id
                    && r.source == rule.source
                    && r.dport == rule.dport
                    && r.protocol == rule.protocol
            });
            if !same {
                let _ = state.firewall_manager.remove_rule(scope.clone(), &id).await;
                if let Err(e) = state.firewall_manager.add_rule(scope.clone(), rule).await {
                    tracing::warn!("remote play: VM {vm} firewall rule {id}: {e}");
                }
            }
        }
    }
    for r in &own {
        if !wanted_ids.contains(&r.id) {
            let _ = state
                .firewall_manager
                .remove_rule(scope.clone(), &r.id)
                .await;
        }
    }
    let after = state.firewall_manager.list_rules(scope).await;
    let ok = wanted_ids
        .iter()
        .all(|id| after.iter().any(|r| &r.id == id));
    (foreign > 0, foreign > 0 && ok)
}

async fn reconcile(state: &Arc<AppState>, listeners: &mut Listeners) {
    let s = settings();
    let sys = std::path::Path::new(SYS_NET);
    let vms = state.vm_manager.list_vms().await;
    let addrs = cmd_out("ip", &["-j", "-4", "addr", "show"])
        .await
        .map(|t| rp::parse_ip_addr_json(&t))
        .unwrap_or_default();
    let host_nets = rp::host_lan_networks(&addrs, sys);
    let neigh = cmd_out("ip", &["-j", "-4", "neigh", "show"])
        .await
        .map(|t| rp::parse_ip_neigh_json(&t))
        .unwrap_or_default();
    let known = rt()
        .lock()
        .map(|r| r.known_addr.clone())
        .unwrap_or_default();

    let mut per_vm: HashMap<String, VmRt> = HashMap::new();
    let mut forwards: Vec<Forward> = Vec::new();
    let mut targets: Vec<RelayTarget> = Vec::new();
    let mut wake_macs: HashMap<[u8; 6], (String, Vec<String>)> = HashMap::new();
    let mut new_known: HashMap<String, Ipv4Addr> = HashMap::new();
    let mut relay_wanted = false;
    for vm in &vms {
        let Some(cfg) = vm.hardware.adv.remote_play.clone().filter(|c| c.enabled) else {
            // a VM that had allow rules and turned remote play off
            if !state
                .firewall_manager
                .list_rules(crate::firewall::FirewallScope::Vm(vm.id.clone()))
                .await
                .iter()
                .any(|r| r.id.starts_with(RULE_PREFIX))
            {
                continue;
            }
            sync_vm_firewall(state, &vm.id, &[]).await;
            continue;
        };
        let reach = rp::nic_reach(&vm.hardware.nets, sys);
        let network = rp::resolve_network(&cfg, &reach).to_string();
        let nic0 = vm.hardware.nets.first();
        let mac = nic0.and_then(|n| n.mac.clone());
        let bridge = nic0.map(|n| n.bridge.clone()).unwrap_or_default();
        let running = vm.status == VmStatus::Running;
        let mut address = None;
        if let Some(m) = &mac {
            let ga = guest_addresses(vm).await;
            address = rp::address_for_mac(&ga, m).or_else(|| rp::address_for_mac(&neigh, m));
        }
        let address = address.or_else(|| known.get(&vm.id).copied());
        if let Some(a) = address {
            new_known.insert(vm.id.clone(), a);
        }
        let sources = if cfg.lan_sources.is_empty() {
            host_nets.clone()
        } else {
            cfg.lan_sources.clone()
        };
        let mut v = VmRt {
            network: network.clone(),
            nic: match &reach {
                Reach::Lan { nic } => nic.clone(),
                Reach::Nat { bridge } => format!("bridge {bridge} (no LAN port)"),
                Reach::None => "no network card".into(),
            },
            bridge: bridge.clone(),
            mac: mac.clone(),
            address,
            sources: sources.clone(),
            ..Default::default()
        };
        let relay_here = s.discovery_relay && network == "nat" && cfg.has(RemotePlayClient::Steam);
        let mut allow: Vec<(RemotePlayClient, &'static str, String, String)> = Vec::new();
        for client in &cfg.clients {
            let ports = s.ports(*client);
            let tcp = parse_ports(&ports.tcp).unwrap_or_default();
            let mut udp = parse_ports(&ports.udp).unwrap_or_default();
            if *client == RemotePlayClient::Steam && relay_here {
                udp = ports_without(&udp, s.steam_discovery_port);
            }
            let to = address.map(|a| a.to_string()).unwrap_or_default();
            for (proto, set) in [("tcp", &tcp), ("udp", &udp)] {
                if set.is_empty() {
                    continue;
                }
                let p = horcrux_common::remote_play::ports_nft(set);
                v.rules.push(RemotePlayRule {
                    kind: if network == "nat" { "forward" } else { "allow" }.into(),
                    protocol: proto.into(),
                    ports: p.clone(),
                    sources: sources.clone(),
                    to: to.clone(),
                    client: *client,
                    applied: false,
                });
                if network == "lan" {
                    for src in &sources {
                        allow.push((*client, proto, p.clone(), src.clone()));
                    }
                }
            }
            if network == "nat" {
                if let Some(a) = address {
                    forwards.push(Forward {
                        vm: vm.id.clone(),
                        client: *client,
                        bridge: bridge.clone(),
                        to: a,
                        sources: sources.clone(),
                        tcp,
                        udp,
                    });
                }
            }
        }
        if relay_here {
            relay_wanted = true;
            v.rules.push(RemotePlayRule {
                kind: "relay".into(),
                protocol: "udp".into(),
                ports: s.steam_discovery_port.to_string(),
                sources: sources.clone(),
                to: address.map(|a| a.to_string()).unwrap_or_default(),
                client: RemotePlayClient::Steam,
                applied: false,
            });
            if let Some(a) = address {
                targets.push(RelayTarget {
                    vm: vm.id.clone(),
                    addr: a,
                    sources: sources.clone(),
                    running,
                    relay: true,
                    wake_on_discovery: cfg.wake_on_discovery,
                });
            }
        } else if cfg.wake_on_discovery && cfg.has(RemotePlayClient::Steam) {
            relay_wanted = true;
            targets.push(RelayTarget {
                vm: vm.id.clone(),
                addr: Ipv4Addr::UNSPECIFIED,
                sources: sources.clone(),
                running,
                relay: false,
                wake_on_discovery: true,
            });
        }
        if network == "lan" {
            let (fw, ok) = sync_vm_firewall(state, &vm.id, &allow).await;
            v.firewall = fw;
            v.firewall_allows = ok;
            for r in v.rules.iter_mut() {
                r.applied = !fw || ok;
            }
        }
        if cfg.wake_on_lan {
            if let Some(m) = mac.as_deref().and_then(rp::parse_mac) {
                wake_macs.insert(m, (vm.id.clone(), sources.clone()));
            }
        }
        per_vm.insert(vm.id.clone(), v);
    }

    // nftables: one table for every forward
    let text = rp::render_nft(&s.nft_table, &forwards);
    let (prev_text, prev_res) = rt()
        .lock()
        .map(|r| (r.nft_text.clone(), r.nft_result.clone()))
        .unwrap_or_default();
    let nft_result = if text != prev_text || !matches!(prev_res, Some(Ok(()))) {
        if text.is_empty() && prev_text.is_empty() && prev_res.is_none() {
            None
        } else {
            let r = nft_apply(&s.nft_table, &text).await;
            if let Err(e) = &r {
                tracing::warn!("remote play: nftables: {e}");
            }
            Some(r)
        }
    } else {
        prev_res
    };
    let applied = matches!(nft_result, Some(Ok(())));
    for v in per_vm.values_mut() {
        if v.network == "nat" {
            for r in v.rules.iter_mut() {
                if r.kind == "forward" {
                    r.applied = applied && !r.to.is_empty();
                }
            }
        }
    }

    // listeners: Wake-on-LAN ports while any VM wants waking
    let want_wol: Vec<u16> = if wake_macs.is_empty() {
        Vec::new()
    } else {
        s.wol_ports.clone()
    };
    let mut wol_status: BTreeMap<u16, Result<(), String>> = BTreeMap::new();
    listeners.wol.retain(|p, h| {
        let keep = want_wol.contains(p) && !h.is_finished();
        if !keep {
            h.abort();
        }
        keep
    });
    for p in &want_wol {
        if listeners.wol.contains_key(p) {
            wol_status.insert(*p, Ok(()));
            continue;
        }
        match udp_listener(*p) {
            Ok(sock) => {
                listeners
                    .wol
                    .insert(*p, tokio::spawn(wol_loop(state.clone(), sock)));
                wol_status.insert(*p, Ok(()));
            }
            Err(e) => {
                wol_status.insert(*p, Err(e));
            }
        }
    }
    // the discovery relay
    let relay_port = s.steam_discovery_port;
    if let Some((p, h)) = &listeners.relay {
        if !relay_wanted || *p != relay_port || h.is_finished() {
            h.abort();
            listeners.relay = None;
        }
    }
    let relay_bound = if !relay_wanted {
        None
    } else if listeners.relay.is_some() {
        Some(Ok(()))
    } else {
        match udp_listener(relay_port) {
            Ok(sock) => {
                listeners.relay = Some((
                    relay_port,
                    tokio::spawn(relay_loop(state.clone(), sock, relay_port)),
                ));
                Some(Ok(()))
            }
            Err(e) => Some(Err(e)),
        }
    };
    for v in per_vm.values_mut() {
        for r in v.rules.iter_mut() {
            if r.kind == "relay" {
                r.applied = matches!(relay_bound, Some(Ok(()))) && !r.to.is_empty();
            }
        }
    }

    if let Ok(mut r) = rt().lock() {
        r.relay.targets = targets;
        r.relay_bound = relay_bound;
        r.relay_port = relay_port;
        r.wol = wol_status;
        r.nft_text = text;
        r.nft_result = nft_result;
        r.vms = per_vm;
        r.wake_macs = wake_macs;
        r.known_addr = new_known;
    }
}

// --------------------------------------------------------------- status

async fn load_guest(state: &AppState, id: &str) -> Option<GuestReport> {
    crate::db::config_store::ConfigStore::new(&state.database)
        .load::<GuestReport>(GUEST_KIND)
        .await
        .ok()?
        .into_iter()
        .find(|(k, _)| k == id)
        .map(|(_, r)| r)
}

async fn status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<RemotePlayStatus>, ApiError> {
    let vm = state.vm_manager.get_vm(&id).await?;
    let cfg = vm.hardware.adv.remote_play.clone().unwrap_or_default();
    let (v, relay, wol, nft_res, nft_text) = {
        let r = rt()
            .lock()
            .map_err(|_| ApiError::Internal("remote play state".into()))?;
        (
            r.vms.get(&id).cloned(),
            r.relay_bound.clone(),
            r.wol
                .iter()
                .map(|(p, x)| (*p, x.clone()))
                .collect::<Vec<_>>(),
            r.nft_result.clone(),
            r.nft_text.clone(),
        )
    };
    let v = v.unwrap_or_else(|| {
        // not reconciled yet (just turned on): resolve what we can now
        let reach = rp::nic_reach(&vm.hardware.nets, std::path::Path::new(SYS_NET));
        VmRt {
            network: rp::resolve_network(&cfg, &reach).into(),
            mac: vm.hardware.nets.first().and_then(|n| n.mac.clone()),
            ..Default::default()
        }
    });
    let guest = load_guest(&state, &id).await;
    let policy = crate::api::guest_state::policy::effective(
        &state,
        crate::api::guest_state::GuestKind::Vm,
        &id,
    )
    .await;
    use crate::api::guest_state::policy::{StartAction, StopAction};
    let host_shutdown = match policy.stop_action {
        StopAction::Save if !vm.hardware.hostpci.is_empty() => "shutdown-gpu",
        StopAction::Save => "save",
        StopAction::Shutdown => "shutdown",
        StopAction::Poweroff => "poweroff",
        StopAction::None => "none",
    };
    let mut gpus: Vec<(String, String)> = Vec::new();
    let mut vgpu = false;
    for h in &vm.hardware.hostpci {
        match h.kind() {
            Ok(horcrux_common::HostPciKind::Pci(addr)) => {
                let dev = std::path::Path::new("/sys/bus/pci/devices").join(&addr);
                let class = std::fs::read_to_string(dev.join("class")).unwrap_or_default();
                // display controllers only (class 0x03xxxx)
                if class.trim().starts_with("0x03") {
                    let vendor = std::fs::read_to_string(dev.join("vendor"))
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default();
                    gpus.push((vendor, addr));
                }
            }
            Ok(horcrux_common::HostPciKind::Mdev(_)) => vgpu = true,
            Err(_) => {}
        }
    }
    let my_forwards = nft_text
        .matches(&format!("remote-play {} ", id).to_lowercase())
        .count()
        / 2;
    let facts = Facts {
        rp: cfg.clone(),
        network: v.network.clone(),
        nic: v.nic.clone(),
        mac: v.mac.clone(),
        address: v.address,
        sources: v.sources.clone(),
        running: vm.status == VmStatus::Running,
        guest_agent: vm.hardware.guest_agent,
        gpus,
        vgpu,
        relay_listening: relay,
        wol_listening: wol,
        nft_applied: nft_res,
        forwards: my_forwards,
        vm_firewall: v.firewall,
        firewall_allows: v.firewall_allows,
        guest: guest.clone(),
        host_shutdown: host_shutdown.into(),
        resume_on_boot: matches!(
            policy.start_action,
            StartAction::ResumeIfRunning | StartAction::Always
        ),
    };
    let checks = rp::checklist(&facts);
    // only this VM's part of the table
    let nft = if my_forwards > 0 {
        nft_text
    } else {
        String::new()
    };
    Ok(Json(RemotePlayStatus {
        vm: id,
        config: cfg,
        network: if facts.rp.enabled {
            v.network
        } else {
            String::new()
        },
        nic: v.nic,
        mac: v.mac,
        address: v.address.map(|a| a.to_string()),
        sources: v.sources,
        checks,
        rules: v.rules,
        nft,
        guest,
    }))
}

// ---------------------------------------------------------- guest setup

const LINUX_REPORT: &str = r#"s=false; command -v steam >/dev/null 2>&1 && s=true
[ -x /usr/games/steam ] && s=true
flatpak info com.valvesoftware.Steam >/dev/null 2>&1 && s=true
r=false; pgrep -x steam >/dev/null 2>&1 && r=true
u=false; command -v sunshine >/dev/null 2>&1 && u=true
printf 'HXRP:{"os":"linux","steam_installed":%s,"steam_running":%s,"sunshine_installed":%s,"checked_at":%s,"messages":["on Linux guests set automatic logon and Big Picture in the desktop settings"]}\n' "$s" "$r" "$u" "$(date +%s)"
"#;

async fn guest_setup(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<GuestSetupRequest>,
) -> Result<Json<GuestReport>, ApiError> {
    let vm = state.vm_manager.get_vm(&id).await?;
    if vm.status != VmStatus::Running {
        return Err(ApiError::Conflict(
            "start the VM first: the guest setup runs inside it".into(),
        ));
    }
    if !vm.hardware.guest_agent {
        return Err(ApiError::BadRequest(
            "the VM has no guest agent channel: turn it on (Hardware page) and install the QEMU guest agent in the guest".into(),
        ));
    }
    let auto = match (&req.auto_login_user, &req.password) {
        (Some(u), Some(p)) if !u.trim().is_empty() => {
            if u.len() > 104 || p.len() > 256 {
                return Err(ApiError::BadRequest(
                    "user name or password too long".into(),
                ));
            }
            Some((u.trim().to_string(), p.clone()))
        }
        (Some(u), _) if !u.trim().is_empty() => {
            return Err(ApiError::BadRequest(
                "automatic logon needs the account's password".into(),
            ))
        }
        _ => None,
    };
    let sock = qga_socket(&vm)
        .ok_or_else(|| ApiError::BadRequest("the VM has no disk (no guest agent socket)".into()))?;
    let s = settings();
    let os = crate::vm::devices::qga_exec(&sock, "guest-get-osinfo", json!({}), 10)
        .await
        .map_err(|e| ApiError::Conflict(format!("the guest agent does not answer: {e}")))?;
    let windows = os
        .get("id")
        .and_then(Value::as_str)
        .map(|i| i.starts_with("mswindows"))
        .unwrap_or(false);
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let cfg = vm.hardware.adv.remote_play.clone().unwrap_or_default();
    let exec = if windows {
        let script: Vec<u8> = rp::guest_setup_script()
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let input = rp::guest_setup_input(
            &cfg,
            req.install_steam,
            &s.steam_installer_url,
            auto.as_ref().map(|(u, p)| (u.as_str(), p.as_str())),
            req.auto_login_off,
        );
        json!({
            "path": "powershell.exe",
            "arg": ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                    "-EncodedCommand", b64.encode(script)],
            "input-data": b64.encode(input.to_string()),
            "capture-output": true,
        })
    } else {
        json!({
            "path": "/bin/sh",
            "arg": ["-c", LINUX_REPORT],
            "capture-output": true,
        })
    };
    let r = crate::vm::devices::qga_exec(&sock, "guest-exec", exec, 15)
        .await
        .map_err(|e| ApiError::Conflict(format!("guest-exec: {e}")))?;
    let pid = r
        .get("pid")
        .and_then(Value::as_i64)
        .ok_or_else(|| ApiError::Internal("guest-exec gave no pid".into()))?;
    let deadline = Instant::now() + Duration::from_secs(s.guest_setup_timeout_secs);
    let mut report = None;
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let Ok(st) =
            crate::vm::devices::qga_exec(&sock, "guest-exec-status", json!({"pid": pid}), 10).await
        else {
            continue;
        };
        if st.get("exited").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let out = st
            .get("out-data")
            .and_then(Value::as_str)
            .and_then(|x| b64.decode(x).ok())
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_default();
        report = rp::parse_guest_report(&out);
        if report.is_none() {
            let err = st
                .get("err-data")
                .and_then(Value::as_str)
                .and_then(|x| b64.decode(x).ok())
                .map(|b| String::from_utf8_lossy(&b).to_string())
                .unwrap_or_default();
            let tail: String = err
                .chars()
                .rev()
                .take(600)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            return Err(ApiError::Internal(format!(
                "the guest setup gave no report (exit {}): {}",
                st.get("exitcode").and_then(Value::as_i64).unwrap_or(-1),
                tail.trim()
            )));
        }
        break;
    }
    let report = report.ok_or_else(|| {
        ApiError::Internal(format!(
            "the guest setup did not finish within {} s",
            s.guest_setup_timeout_secs
        ))
    })?;
    crate::db::config_store::ConfigStore::new(&state.database)
        .put(GUEST_KIND, &id, &report)
        .await?;
    Ok(Json(report))
}
