//! Host name / time zone / locale, network interfaces, DNS, NTP and health.
use super::{data_dir, rooted, run, user_of, which, Init};
use crate::api::tasks;
use crate::db::config_store::{kinds, ConfigStore};
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::get,
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Arc;

pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    spawn_reapply(state.clone());
    Router::new()
        .route(
            "/api/system/config",
            get(get_config).post(set_config).put(set_config),
        )
        .route("/api/system/network/interfaces", get(list_interfaces))
        .route(
            "/api/system/network/interfaces/:name",
            axum::routing::post(set_interface).put(set_interface),
        )
        .route(
            "/api/system/network/dns",
            get(get_dns).post(set_dns).put(set_dns),
        )
        .route(
            "/api/system/time/ntp",
            get(get_ntp).post(set_ntp).put(set_ntp),
        )
        .route("/api/system/health", get(health))
}

type Res<T> = Result<Json<T>, ApiError>;
type AuthExt = Option<Extension<crate::middleware::auth::AuthUser>>;

// ---------------------------------------------------------------- config

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemConfiguration {
    pub hostname: String,
    pub domain: Option<String>,
    pub timezone: String,
    pub locale: String,
}

pub fn valid_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub fn valid_domain(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.split('.').all(valid_label)
}

pub fn valid_timezone(tz: &str) -> bool {
    !tz.is_empty()
        && tz.len() < 64
        && !tz.starts_with('/')
        && !tz.split('/').any(|p| p.is_empty() || p == "." || p == "..")
        && tz
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
        && rooted("usr/share/zoneinfo").join(tz).is_file()
}

pub fn valid_locale(l: &str) -> bool {
    !l.is_empty()
        && l.len() < 40
        && l.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | '-'))
}

fn read_trim(p: PathBuf) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn current_timezone() -> String {
    if let Ok(t) = std::fs::read_link(rooted("etc/localtime")) {
        let s = t.to_string_lossy().to_string();
        if let Some(i) = s.find("zoneinfo/") {
            return s[i + "zoneinfo/".len()..].to_string();
        }
    }
    read_trim(rooted("etc/timezone")).unwrap_or_else(|| "UTC".into())
}

/// `LANG=...` out of a `locale.conf`-style file (quotes allowed).
pub fn parse_lang(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim();
        l.strip_prefix("LANG=")
            .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|v| !v.is_empty())
    })
}

fn current_locale() -> String {
    for f in [
        "etc/locale.conf",
        "etc/default/locale",
        "etc/env.d/02locale",
    ] {
        if let Some(l) = std::fs::read_to_string(rooted(f))
            .ok()
            .and_then(|t| parse_lang(&t))
        {
            return l;
        }
    }
    std::env::var("LANG").unwrap_or_else(|_| "C".into())
}

fn current_hostname() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default()
}

async fn get_config() -> Json<SystemConfiguration> {
    let full = current_hostname();
    let (host, domain) = match full.split_once('.') {
        Some((h, d)) => (h.to_string(), Some(d.to_string())),
        None => (full.clone(), hosts_domain(&host_file_text(), &full)),
    };
    Json(SystemConfiguration {
        hostname: host,
        domain,
        timezone: current_timezone(),
        locale: current_locale(),
    })
}

fn host_file_text() -> String {
    std::fs::read_to_string(rooted("etc/hosts")).unwrap_or_default()
}

/// The domain of `short` according to the `127.0.1.1 fqdn short` line.
pub fn hosts_domain(hosts: &str, short: &str) -> Option<String> {
    hosts.lines().find_map(|l| {
        let l = l.split('#').next().unwrap_or("");
        let mut it = l.split_whitespace();
        let ip = it.next()?;
        if !ip.starts_with("127.0.1.") {
            return None;
        }
        it.find_map(|n| {
            n.strip_prefix(short)
                .and_then(|r| r.strip_prefix('.'))
                .map(|d| d.to_string())
        })
    })
}

/// `hosts` with the `127.0.1.1` line replaced (or appended) so the machine
/// resolves its own name.
pub fn set_hosts_line(hosts: &str, fqdn: &str, short: &str) -> String {
    let line = if fqdn == short {
        format!("127.0.1.1\t{short}")
    } else {
        format!("127.0.1.1\t{fqdn} {short}")
    };
    let mut out = Vec::new();
    let mut done = false;
    for l in hosts.lines() {
        if l.trim_start().starts_with("127.0.1.1") {
            if !done {
                out.push(line.clone());
                done = true;
            }
        } else {
            out.push(l.to_string());
        }
    }
    if !done {
        out.push(line);
    }
    out.join("\n") + "\n"
}

/// Replace the `key=` line of a `conf.d`/`env.d` style file, keeping others.
pub fn set_kv(text: &str, key: &str, value: &str, quote: bool) -> String {
    let new = if quote {
        format!("{key}=\"{value}\"")
    } else {
        format!("{key}={value}")
    };
    let mut done = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            if l.trim_start().starts_with(&format!("{key}=")) && !done {
                done = true;
                new.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !done {
        out.push(new);
    }
    out.join("\n") + "\n"
}

fn write_atomic(path: &std::path::Path, text: &str) -> Result<(), ApiError> {
    // Write through a symlink (resolv.conf is often one) instead of replacing it.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(dir) = target.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = target.with_extension("horcrux-new");
    let renamed = std::fs::write(&tmp, text).and_then(|_| {
        if let Ok(m) = std::fs::metadata(&target) {
            let _ = std::fs::set_permissions(&tmp, m.permissions());
        }
        std::fs::rename(&tmp, &target)
    });
    if renamed.is_ok() {
        return Ok(());
    }
    // A bind-mounted file (/etc/hosts, /etc/resolv.conf in containers) can't be
    // replaced by rename; overwrite it in place instead.
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&target, text)
        .map_err(|e| ApiError::Internal(format!("write {}: {e}", target.display())))
}

fn set_timezone(tz: &str) -> Result<(), ApiError> {
    let link = rooted("etc/localtime");
    let target = PathBuf::from("/usr/share/zoneinfo").join(tz);
    let tmp = link.with_extension("horcrux-new");
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(&target, &tmp)
        .and_then(|_| std::fs::rename(&tmp, &link))
        .map_err(|e| ApiError::Internal(format!("set /etc/localtime: {e}")))?;
    // Debian/Gentoo also keep the name in /etc/timezone.
    write_atomic(&rooted("etc/timezone"), &format!("{tz}\n"))
}

fn apply_config(c: &SystemConfiguration) -> Result<String, ApiError> {
    if !valid_label(&c.hostname) {
        return Err(ApiError::ValidationError(
            "hostname must be 1-63 letters, digits or '-' (no dots; the domain is a separate field)".into(),
        ));
    }
    if let Some(d) = c.domain.as_deref().filter(|d| !d.is_empty()) {
        if !valid_domain(d) {
            return Err(ApiError::ValidationError(format!("invalid domain '{d}'")));
        }
    }
    if !valid_timezone(&c.timezone) {
        return Err(ApiError::ValidationError(format!(
            "unknown time zone '{}' (expected e.g. Europe/Berlin)",
            c.timezone
        )));
    }
    if !valid_locale(&c.locale) {
        return Err(ApiError::ValidationError(format!(
            "invalid locale '{}'",
            c.locale
        )));
    }
    let short = c.hostname.to_ascii_lowercase();
    let fqdn = match c.domain.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => format!("{short}.{}", d.to_ascii_lowercase()),
        None => short.clone(),
    };
    let mut done = Vec::new();

    write_atomic(&rooted("etc/hostname"), &format!("{short}\n"))?;
    // Gentoo OpenRC reads the name from /etc/conf.d/hostname.
    let confd = rooted("etc/conf.d/hostname");
    if confd.exists() || rooted("etc/conf.d").is_dir() {
        let old = std::fs::read_to_string(&confd).unwrap_or_default();
        write_atomic(&confd, &set_kv(&old, "hostname", &short, true))?;
    }
    write_atomic(
        &rooted("etc/hosts"),
        &set_hosts_line(&host_file_text(), &fqdn, &short),
    )?;
    if std::env::var_os("HORCRUX_SYSROOT").is_none() {
        std::fs::write("/proc/sys/kernel/hostname", &short)
            .map_err(|e| ApiError::Internal(format!("set kernel hostname: {e}")))?;
    }
    done.push(format!("hostname {fqdn}"));

    set_timezone(&c.timezone)?;
    done.push(format!("time zone {}", c.timezone));

    write_atomic(
        &rooted("etc/locale.conf"),
        &set_kv(
            &std::fs::read_to_string(rooted("etc/locale.conf")).unwrap_or_default(),
            "LANG",
            &c.locale,
            false,
        ),
    )?;
    if rooted("etc/env.d").is_dir() {
        write_atomic(
            &rooted("etc/env.d/02locale"),
            &set_kv(
                &std::fs::read_to_string(rooted("etc/env.d/02locale")).unwrap_or_default(),
                "LANG",
                &c.locale,
                true,
            ),
        )?;
    }
    done.push(format!("locale {}", c.locale));
    Ok(done.join(", "))
}

async fn set_config(u: AuthExt, Json(c): Json<SystemConfiguration>) -> Res<SystemConfiguration> {
    let r = apply_config(&c);
    tasks::record_done(
        "system-config",
        &c.hostname,
        &user_of(&u),
        &r.as_ref().map(|s| s.clone()).map_err(|e| format!("{e:?}")),
    );
    r?;
    Ok(get_config().await)
}

// --------------------------------------------------------------- network

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NetworkInterface {
    pub name: String,
    pub method: String,
    pub address: Option<String>,
    pub netmask: Option<String>,
    pub gateway: Option<String>,
    pub mtu: Option<u16>,
    pub auto: bool,
    pub bridge: Option<String>,
    pub bridge_ports: Option<Vec<String>>,
    pub vlan_id: Option<u16>,
}

pub fn prefix_to_netmask(p: u8) -> Option<String> {
    if p > 32 {
        return None;
    }
    let m: u32 = if p == 0 { 0 } else { u32::MAX << (32 - p) };
    Some(Ipv4Addr::from(m).to_string())
}

pub fn netmask_to_prefix(m: &str) -> Option<u8> {
    let ip: Ipv4Addr = m.parse().ok()?;
    let b = u32::from(ip);
    // a valid mask is ones followed by zeros
    if b.leading_ones() + b.trailing_zeros() == 32 || b == 0 {
        Some(b.leading_ones() as u8)
    } else {
        None
    }
}

/// Interfaces from `ip -j addr` + the default route from `ip -j route`,
/// plus the methods Horcrux itself applied.
pub fn interfaces_from_ip(
    addr: &serde_json::Value,
    routes: &serde_json::Value,
    methods: &std::collections::HashMap<String, String>,
) -> Vec<NetworkInterface> {
    let mut out = Vec::new();
    for l in addr.as_array().into_iter().flatten() {
        let name = l["ifname"].as_str().unwrap_or("").to_string();
        if name.is_empty() || name == "lo" {
            continue;
        }
        let v4 = l["addr_info"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["family"] == "inet");
        let flags: Vec<&str> = l["flags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f.as_str())
            .collect();
        let gateway = routes
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| r["dst"] == "default" && r["dev"].as_str() == Some(name.as_str()))
            .and_then(|r| r["gateway"].as_str().map(|s| s.to_string()));
        let kind = l["linkinfo"]["info_kind"].as_str().unwrap_or("");
        let method = methods.get(&name).cloned().unwrap_or_else(|| {
            if v4.is_some_and(|a| a["dynamic"] == true) {
                "dhcp".into()
            } else if v4.is_some() {
                "static".into()
            } else {
                "manual".into()
            }
        });
        let ports = if kind == "bridge" {
            std::fs::read_dir(format!("/sys/class/net/{name}/brif"))
                .ok()
                .map(|d| {
                    let mut v: Vec<String> = d
                        .flatten()
                        .map(|e| e.file_name().to_string_lossy().to_string())
                        .collect();
                    v.sort();
                    v
                })
        } else {
            None
        };
        out.push(NetworkInterface {
            method,
            address: v4.and_then(|a| a["local"].as_str().map(|s| s.to_string())),
            netmask: v4
                .and_then(|a| a["prefixlen"].as_u64())
                .and_then(|p| prefix_to_netmask(p as u8)),
            gateway,
            mtu: l["mtu"].as_u64().map(|m| m as u16),
            auto: flags.contains(&"UP"),
            bridge: l["master"].as_str().map(|s| s.to_string()),
            bridge_ports: ports,
            vlan_id: if kind == "vlan" {
                l["linkinfo"]["info_data"]["id"].as_u64().map(|v| v as u16)
            } else {
                None
            },
            name,
        });
    }
    out
}

async fn ip_json(args: &[&str]) -> serde_json::Value {
    let mut a = vec!["-j", "-d"];
    a.extend_from_slice(args);
    run("ip", &a)
        .await
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

async fn applied_methods(state: &Arc<AppState>) -> std::collections::HashMap<String, String> {
    ConfigStore::new(&state.database)
        .load::<NetworkInterface>(kinds::SYSTEM_IFACE)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, v.method))
        .collect()
}

async fn list_interfaces(State(state): State<Arc<AppState>>) -> Res<Vec<NetworkInterface>> {
    let addr = ip_json(&["addr", "show"]).await;
    if addr.is_null() {
        return Err(ApiError::ServiceUnavailable(
            "`ip` (iproute2) is not available on this host".into(),
        ));
    }
    let routes = ip_json(&["route", "show"]).await;
    Ok(Json(interfaces_from_ip(
        &addr,
        &routes,
        &applied_methods(&state).await,
    )))
}

pub fn validate_iface(i: &NetworkInterface) -> Result<(), String> {
    if !["dhcp", "static", "manual"].contains(&i.method.as_str()) {
        return Err(format!(
            "method must be dhcp, static or manual, not '{}'",
            i.method
        ));
    }
    if i.method == "static" {
        let a = i.address.as_deref().unwrap_or("");
        let a: Ipv4Addr = a
            .parse()
            .map_err(|_| format!("'{a}' is not an IPv4 address"))?;
        let m = i.netmask.as_deref().unwrap_or("");
        let p = netmask_to_prefix(m).ok_or_else(|| format!("'{m}' is not a netmask"))?;
        if let Some(g) = i.gateway.as_deref().filter(|g| !g.is_empty()) {
            let g: Ipv4Addr = g
                .parse()
                .map_err(|_| format!("'{g}' is not an IPv4 gateway"))?;
            let mask: u32 = if p == 0 { 0 } else { u32::MAX << (32 - p) };
            if u32::from(g) & mask != u32::from(a) & mask {
                return Err(format!("gateway {g} is not inside {a}/{p}"));
            }
        }
    }
    if let Some(m) = i.mtu {
        if !(68..=9216).contains(&m) {
            return Err(format!("mtu {m} is outside 68-9216"));
        }
    }
    Ok(())
}

fn valid_ifname(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 15
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
}

/// Apply `i` to the running host.
async fn apply_iface(
    i: &NetworkInterface,
    current: &[NetworkInterface],
) -> Result<String, ApiError> {
    let cur = current
        .iter()
        .find(|c| c.name == i.name)
        .ok_or_else(|| ApiError::NotFound(format!("interface {} not found", i.name)))?;
    let carries_default = cur.gateway.is_some();
    let new_addr = if i.method == "static" {
        i.address.clone()
    } else {
        None
    };
    // Re-addressing the uplink that carries the default route would cut off
    // the very request configuring it.
    if carries_default
        && (i.method != "static" || new_addr != cur.address || i.netmask != cur.netmask)
    {
        return Err(ApiError::Conflict(format!(
            "{} carries the default route; changing its address or bringing it down would disconnect this node. Use the console or a bridge instead",
            i.name
        )));
    }
    if carries_default && !i.auto {
        return Err(ApiError::Conflict(format!(
            "{} carries the default route and cannot be taken down from here",
            i.name
        )));
    }
    let n = i.name.as_str();
    let e = |r: Result<String, String>| r.map_err(ApiError::Internal);
    let mut done = Vec::new();
    if let Some(m) = i.mtu {
        if Some(m) != cur.mtu {
            e(run("ip", &["link", "set", "dev", n, "mtu", &m.to_string()]).await)?;
            done.push(format!("mtu {m}"));
        }
    }
    match i.method.as_str() {
        "static" => {
            let p = netmask_to_prefix(i.netmask.as_deref().unwrap_or("")).unwrap_or(24);
            let cidr = format!("{}/{p}", i.address.clone().unwrap_or_default());
            if !carries_default {
                e(run("ip", &["-4", "addr", "flush", "dev", n]).await)?;
                e(run("ip", &["addr", "add", &cidr, "dev", n]).await)?;
                done.push(format!("address {cidr}"));
            }
        }
        "manual" => {
            if !carries_default {
                e(run("ip", &["-4", "addr", "flush", "dev", n]).await)?;
                done.push("addresses cleared".into());
            }
        }
        _ => {
            let client = ["dhcpcd", "dhclient", "udhcpc"]
                .into_iter()
                .find(|c| which(c).is_some())
                .ok_or_else(|| {
                    ApiError::ServiceUnavailable(
                        "no DHCP client (dhcpcd, dhclient, udhcpc) is installed".into(),
                    )
                })?;
            if !carries_default {
                e(run("ip", &["-4", "addr", "flush", "dev", n]).await)?;
            }
            let args: Vec<String> = match client {
                "dhcpcd" => vec!["-n".into(), n.into()],
                "dhclient" => vec!["-1".into(), n.into()],
                _ => vec!["-n".into(), "-q".into(), "-i".into(), n.into()],
            };
            let _ = tasks::spawn_command("net-dhcp", n, "system", client, &args, &[])?;
            done.push(format!("{client} started"));
        }
    }
    if i.method == "static" {
        if let Some(g) = i.gateway.as_deref().filter(|g| !g.is_empty()) {
            if !carries_default {
                e(run("ip", &["route", "replace", "default", "via", g, "dev", n]).await)?;
                done.push(format!("default via {g}"));
            }
        }
    }
    // Bring the link up/down last so a down link isn't re-addressed in vain.
    if i.auto != cur.auto || i.method != "manual" {
        let st = if i.auto { "up" } else { "down" };
        e(run("ip", &["link", "set", "dev", n, st]).await)?;
        done.push(format!("link {st}"));
    }
    Ok(done.join(", "))
}

async fn set_interface(
    State(state): State<Arc<AppState>>,
    u: AuthExt,
    Path(name): Path<String>,
    Json(mut i): Json<NetworkInterface>,
) -> Res<NetworkInterface> {
    if !valid_ifname(&name) {
        return Err(ApiError::BadRequest("bad interface name".into()));
    }
    i.name = name.clone();
    validate_iface(&i).map_err(ApiError::ValidationError)?;
    let addr = ip_json(&["addr", "show"]).await;
    let routes = ip_json(&["route", "show"]).await;
    let current = interfaces_from_ip(&addr, &routes, &Default::default());
    let r = apply_iface(&i, &current).await;
    tasks::record_done(
        "net-config",
        &name,
        &user_of(&u),
        &r.as_ref().map(|s| s.clone()).map_err(|e| format!("{e:?}")),
    );
    r?;
    ConfigStore::new(&state.database)
        .put(kinds::SYSTEM_IFACE, &name, &i)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let addr = ip_json(&["addr", "show"]).await;
    let routes = ip_json(&["route", "show"]).await;
    interfaces_from_ip(&addr, &routes, &applied_methods(&state).await)
        .into_iter()
        .find(|c| c.name == name)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("interface {name} disappeared")))
}

/// Re-apply what the operator configured after a reboot (the kernel forgets
/// `ip addr add`).
fn spawn_reapply(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let saved = ConfigStore::new(&state.database)
            .load::<NetworkInterface>(kinds::SYSTEM_IFACE)
            .await
            .unwrap_or_default();
        for (name, i) in saved {
            let addr = ip_json(&["addr", "show"]).await;
            let routes = ip_json(&["route", "show"]).await;
            let current = interfaces_from_ip(&addr, &routes, &Default::default());
            if let Err(e) = apply_iface(&i, &current).await {
                tracing::warn!("re-applying network config of {name}: {e:?}");
            }
        }
    });
}

// ------------------------------------------------------------------- DNS

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DnsConfiguration {
    pub servers: Vec<String>,
    pub search_domains: Vec<String>,
}

pub fn parse_resolv(text: &str) -> DnsConfiguration {
    let mut servers = Vec::new();
    let mut search = Vec::new();
    for l in text.lines() {
        let l = l.trim();
        if let Some(r) = l.strip_prefix("nameserver") {
            if let Some(s) = r.split_whitespace().next() {
                servers.push(s.to_string());
            }
        } else if let Some(r) = l
            .strip_prefix("search")
            .or_else(|| l.strip_prefix("domain"))
        {
            search = r.split_whitespace().map(|s| s.to_string()).collect();
        }
    }
    DnsConfiguration {
        servers,
        search_domains: search,
    }
}

/// New resolv.conf: our nameserver/search lines, every other line (options,
/// sortlist, ...) kept.
pub fn render_resolv(old: &str, c: &DnsConfiguration) -> String {
    let mut out = vec!["# Managed by Horcrux (System > Network > DNS)".to_string()];
    for s in &c.servers {
        out.push(format!("nameserver {s}"));
    }
    if !c.search_domains.is_empty() {
        out.push(format!("search {}", c.search_domains.join(" ")));
    }
    for l in old.lines() {
        let t = l.trim_start();
        if t.starts_with("nameserver")
            || t.starts_with("search")
            || t.starts_with("domain")
            || t.starts_with("# Managed by Horcrux")
            || t.is_empty()
        {
            continue;
        }
        out.push(l.to_string());
    }
    out.join("\n") + "\n"
}

async fn get_dns() -> Json<DnsConfiguration> {
    Json(parse_resolv(
        &std::fs::read_to_string(rooted("etc/resolv.conf")).unwrap_or_default(),
    ))
}

async fn set_dns(u: AuthExt, Json(c): Json<DnsConfiguration>) -> Res<DnsConfiguration> {
    if c.servers.is_empty() || c.servers.len() > 3 {
        return Err(ApiError::ValidationError(
            "give 1 to 3 DNS servers (the resolver ignores more)".into(),
        ));
    }
    for s in &c.servers {
        s.parse::<IpAddr>()
            .map_err(|_| ApiError::ValidationError(format!("'{s}' is not an IP address")))?;
    }
    if c.search_domains.len() > 6 {
        return Err(ApiError::ValidationError("at most 6 search domains".into()));
    }
    for d in &c.search_domains {
        if !valid_domain(d) {
            return Err(ApiError::ValidationError(format!(
                "invalid search domain '{d}'"
            )));
        }
    }
    let path = rooted("etc/resolv.conf");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let r = write_atomic(&path, &render_resolv(&old, &c));
    tasks::record_done(
        "dns-config",
        "resolv.conf",
        &user_of(&u),
        &r.as_ref()
            .map(|_| c.servers.join(", "))
            .map_err(|e| format!("{e:?}")),
    );
    r?;
    Ok(get_dns().await)
}

// ------------------------------------------------------------------- NTP

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NtpConfiguration {
    pub servers: Vec<String>,
    pub timezone: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NtpBackend {
    Chrony,
    Timesyncd,
    Ntpd,
}

impl NtpBackend {
    pub fn conf_path(self) -> PathBuf {
        match self {
            NtpBackend::Chrony => {
                let a = rooted("etc/chrony/chrony.conf");
                if a.exists() || !rooted("etc/chrony.conf").exists() {
                    a
                } else {
                    rooted("etc/chrony.conf")
                }
            }
            NtpBackend::Timesyncd => rooted("etc/systemd/timesyncd.conf.d/horcrux.conf"),
            NtpBackend::Ntpd => rooted("etc/ntp.conf"),
        }
    }
    pub fn units(self) -> &'static [&'static str] {
        match self {
            NtpBackend::Chrony => &["chronyd", "chrony"],
            NtpBackend::Timesyncd => &["systemd-timesyncd"],
            NtpBackend::Ntpd => &["ntpd", "ntp"],
        }
    }
}

pub fn detect_ntp() -> Option<NtpBackend> {
    if which("chronyd").is_some()
        || rooted("etc/chrony/chrony.conf").exists()
        || rooted("etc/chrony.conf").exists()
    {
        Some(NtpBackend::Chrony)
    } else if which("ntpd").is_some() || rooted("etc/ntp.conf").exists() {
        Some(NtpBackend::Ntpd)
    } else if rooted("lib/systemd/system/systemd-timesyncd.service").exists()
        || rooted("usr/lib/systemd/system/systemd-timesyncd.service").exists()
        || rooted("etc/systemd/timesyncd.conf").exists()
    {
        Some(NtpBackend::Timesyncd)
    } else {
        None
    }
}

pub fn parse_ntp_servers(backend: NtpBackend, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('#') {
            continue;
        }
        match backend {
            NtpBackend::Timesyncd => {
                if let Some(v) = l.strip_prefix("NTP=") {
                    out.extend(v.split_whitespace().map(|s| s.to_string()));
                }
            }
            _ => {
                let mut it = l.split_whitespace();
                if matches!(it.next(), Some("server") | Some("pool")) {
                    if let Some(h) = it.next() {
                        out.push(h.to_string());
                    }
                }
            }
        }
    }
    out
}

pub fn render_ntp(backend: NtpBackend, old: &str, servers: &[String]) -> String {
    match backend {
        NtpBackend::Timesyncd => format!("[Time]\nNTP={}\n", servers.join(" ")),
        _ => {
            let mut out: Vec<String> = Vec::new();
            let mut inserted = false;
            for l in old.lines() {
                let t = l.trim_start();
                let is_src = t.starts_with("server ") || t.starts_with("pool ");
                if is_src {
                    if !inserted {
                        for s in servers {
                            out.push(format!("server {s} iburst"));
                        }
                        inserted = true;
                    }
                } else {
                    out.push(l.to_string());
                }
            }
            if !inserted {
                for s in servers {
                    out.push(format!("server {s} iburst"));
                }
            }
            out.join("\n") + "\n"
        }
    }
}

async fn get_ntp() -> Json<NtpConfiguration> {
    let servers = detect_ntp()
        .map(|b| {
            let mut s = parse_ntp_servers(
                b,
                &std::fs::read_to_string(b.conf_path()).unwrap_or_default(),
            );
            if s.is_empty() && b == NtpBackend::Timesyncd {
                s = parse_ntp_servers(
                    b,
                    &std::fs::read_to_string(rooted("etc/systemd/timesyncd.conf"))
                        .unwrap_or_default(),
                );
            }
            s
        })
        .unwrap_or_default();
    Json(NtpConfiguration {
        servers,
        timezone: current_timezone(),
    })
}

fn valid_ntp_host(s: &str) -> bool {
    s.parse::<IpAddr>().is_ok() || valid_domain(s)
}

async fn set_ntp(u: AuthExt, Json(c): Json<NtpConfiguration>) -> Res<NtpConfiguration> {
    if c.servers.is_empty() || c.servers.len() > 8 {
        return Err(ApiError::ValidationError("give 1 to 8 NTP servers".into()));
    }
    if let Some(bad) = c.servers.iter().find(|s| !valid_ntp_host(s)) {
        return Err(ApiError::ValidationError(format!(
            "invalid NTP server '{bad}'"
        )));
    }
    if !valid_timezone(&c.timezone) {
        return Err(ApiError::ValidationError(format!(
            "unknown time zone '{}'",
            c.timezone
        )));
    }
    let backend = detect_ntp().ok_or_else(|| {
        ApiError::ServiceUnavailable(
            "no NTP daemon (chrony, ntpd, systemd-timesyncd) is installed on this host".into(),
        )
    })?;
    let path = backend.conf_path();
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    write_atomic(&path, &render_ntp(backend, &old, &c.servers))?;
    set_timezone(&c.timezone)?;
    // Make the daemon read the new list; a daemon that isn't running is
    // left alone (enabling it is the Services page's job).
    let mut restarted = "not running; config written".to_string();
    if super::init_system() != Init::None {
        for unit in backend.units() {
            if let Ok(true) = super::services::is_active(unit).await {
                restarted = super::services::service_action(unit, "restart")
                    .await
                    .map(|_| format!("{unit} restarted"))
                    .unwrap_or_else(|e| format!("{unit} restart failed: {e}"));
                break;
            }
        }
    }
    tasks::record_done(
        "ntp-config",
        &c.servers.join(","),
        &user_of(&u),
        &Ok(restarted),
    );
    Ok(get_ntp().await)
}

// ---------------------------------------------------------------- health

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct MemoryUsage {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub available: u64,
    pub buffers: u64,
    pub cached: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DiskUsage {
    pub device: String,
    pub mount_point: String,
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub filesystem: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NetworkStats {
    pub interface: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SystemInfo {
    pub kernel_version: String,
    pub arch: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub total_memory: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SystemHealth {
    pub hostname: String,
    pub uptime: u64,
    pub load_average: [f64; 3],
    pub memory_usage: MemoryUsage,
    pub disk_usage: Vec<DiskUsage>,
    pub network_stats: Vec<NetworkStats>,
    pub system_info: SystemInfo,
}

pub fn parse_meminfo(text: &str) -> MemoryUsage {
    let kb = |key: &str| -> u64 {
        text.lines()
            .find_map(|l| {
                l.strip_prefix(key)
                    .and_then(|r| r.trim_start_matches(':').split_whitespace().next())
            })
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    let total = kb("MemTotal");
    let free = kb("MemFree");
    let available = kb("MemAvailable");
    let buffers = kb("Buffers");
    let cached = kb("Cached");
    MemoryUsage {
        total,
        used: total.saturating_sub(available),
        free,
        available,
        buffers,
        cached,
    }
}

pub fn parse_net_dev(text: &str) -> Vec<NetworkStats> {
    text.lines()
        .skip(2)
        .filter_map(|l| {
            let (name, rest) = l.split_once(':')?;
            let f: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            if f.len() < 12 {
                return None;
            }
            Some(NetworkStats {
                interface: name.trim().to_string(),
                rx_bytes: f[0],
                rx_packets: f[1],
                rx_errors: f[2],
                tx_bytes: f[8],
                tx_packets: f[9],
                tx_errors: f[10],
            })
        })
        .collect()
}

fn statvfs(path: &str) -> Option<(u64, u64, u64)> {
    let st = nix::sys::statvfs::statvfs(path).ok()?;
    let bs = st.fragment_size() as u64;
    let total = st.blocks() as u64 * bs;
    let avail = st.blocks_available() as u64 * bs;
    let free = st.blocks_free() as u64 * bs;
    Some((total, total.saturating_sub(free), avail))
}

fn disks() -> Vec<DiskUsage> {
    const REAL: &[&str] = &[
        "ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "f2fs", "vfat", "ntfs", "exfat", "nfs",
        "nfs4", "cifs",
    ];
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for l in std::fs::read_to_string("/proc/mounts")
        .unwrap_or_default()
        .lines()
    {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 || !REAL.contains(&f[2]) || !seen.insert(f[0].to_string() + f[1]) {
            continue;
        }
        let mp = f[1].replace("\\040", " ");
        if let Some((total, used, avail)) = statvfs(&mp) {
            if total > 0 {
                out.push(DiskUsage {
                    device: f[0].into(),
                    mount_point: mp,
                    total,
                    used,
                    available: avail,
                    filesystem: f[2].into(),
                });
            }
        }
    }
    out
}

pub(crate) fn load_avg() -> [f64; 3] {
    let t = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let v: Vec<f64> = t
        .split_whitespace()
        .take(3)
        .filter_map(|x| x.parse().ok())
        .collect();
    [
        v.first().copied().unwrap_or(0.0),
        v.get(1).copied().unwrap_or(0.0),
        v.get(2).copied().unwrap_or(0.0),
    ]
}

pub(crate) fn cpu_cores() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
}

pub(crate) fn mem() -> MemoryUsage {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").unwrap_or_default())
}

async fn health() -> Json<SystemHealth> {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let model = cpuinfo
        .lines()
        .find(|l| {
            l.starts_with("model name") || l.starts_with("Model") || l.starts_with("Hardware")
        })
        .and_then(|l| l.split_once(':').map(|x| x.1.trim().to_string()))
        .unwrap_or_else(|| "unknown".into());
    let uptime = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|t| {
            t.split_whitespace()
                .next()
                .and_then(|v| v.parse::<f64>().ok())
        })
        .unwrap_or(0.0) as u64;
    let m = mem();
    Json(SystemHealth {
        hostname: current_hostname(),
        uptime,
        load_average: load_avg(),
        system_info: SystemInfo {
            kernel_version: std::fs::read_to_string("/proc/sys/kernel/osrelease")
                .unwrap_or_default()
                .trim()
                .into(),
            arch: std::env::consts::ARCH.into(),
            cpu_model: model,
            cpu_cores: cpu_cores(),
            total_memory: m.total,
        },
        memory_usage: m,
        disk_usage: disks(),
        network_stats: parse_net_dev(&std::fs::read_to_string("/proc/net/dev").unwrap_or_default()),
    })
}

pub(crate) fn data_dir_usage_percent() -> f64 {
    statvfs(&data_dir().to_string_lossy())
        .map(|(t, used, _)| {
            if t == 0 {
                0.0
            } else {
                used as f64 * 100.0 / t as f64
            }
        })
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_zones_and_locales_are_validated() {
        assert!(
            valid_label("node-1") && !valid_label("-x") && !valid_label("a.b") && !valid_label("")
        );
        assert!(valid_domain("lab.example.org") && !valid_domain("lab..org"));
        assert!(!valid_timezone("../../etc/passwd") && !valid_timezone("/etc/passwd"));
        assert!(valid_locale("en_US.UTF-8") && !valid_locale("en US; rm"));
    }

    #[test]
    fn hosts_and_conf_lines_are_replaced_not_duplicated() {
        let h = "127.0.0.1 localhost\n127.0.1.1 old.lab old\n::1 localhost\n";
        let n = set_hosts_line(h, "new.lab.test", "new");
        assert_eq!(n.matches("127.0.1.1").count(), 1);
        assert!(n.contains("127.0.1.1\tnew.lab.test new") && n.contains("127.0.0.1 localhost"));
        assert_eq!(hosts_domain(&n, "new").as_deref(), Some("lab.test"));
        assert!(set_hosts_line("127.0.0.1 localhost\n", "h", "h").contains("127.0.1.1\th\n"));
        let c = set_kv("# x\nhostname=\"old\"\nother=1\n", "hostname", "new", true);
        assert!(c.contains("hostname=\"new\"") && c.contains("other=1") && !c.contains("old"));
        assert_eq!(
            parse_lang("LANG=\"de_DE.UTF-8\"\n").as_deref(),
            Some("de_DE.UTF-8")
        );
    }

    #[test]
    fn netmask_and_prefix_round_trip() {
        assert_eq!(prefix_to_netmask(24).as_deref(), Some("255.255.255.0"));
        assert_eq!(prefix_to_netmask(0).as_deref(), Some("0.0.0.0"));
        assert_eq!(netmask_to_prefix("255.255.255.192"), Some(26));
        assert_eq!(netmask_to_prefix("255.0.255.0"), None);
    }

    #[test]
    fn interface_json_becomes_ui_rows() {
        let addr = serde_json::json!([
            {"ifname":"lo","flags":["UP"],"mtu":65536,"addr_info":[{"family":"inet","local":"127.0.0.1","prefixlen":8}]},
            {"ifname":"eth0","flags":["UP"],"mtu":1500,"addr_info":[{"family":"inet","local":"10.0.0.5","prefixlen":24,"dynamic":true}]},
            {"ifname":"dummy0","flags":[],"mtu":1400,"addr_info":[{"family":"inet","local":"192.0.2.9","prefixlen":28}]},
            {"ifname":"vl5","flags":["UP"],"mtu":1500,"master":"br0","linkinfo":{"info_kind":"vlan","info_data":{"id":5}},"addr_info":[]}
        ]);
        let routes = serde_json::json!([{"dst":"default","gateway":"10.0.0.1","dev":"eth0"}]);
        let v = interfaces_from_ip(&addr, &routes, &Default::default());
        assert_eq!(v.len(), 3, "lo is hidden");
        assert_eq!(
            (v[0].method.as_str(), v[0].gateway.as_deref()),
            ("dhcp", Some("10.0.0.1"))
        );
        assert_eq!(v[0].netmask.as_deref(), Some("255.255.255.0"));
        assert_eq!(
            (v[1].method.as_str(), v[1].auto, v[1].mtu),
            ("static", false, Some(1400))
        );
        assert_eq!(v[1].netmask.as_deref(), Some("255.255.255.240"));
        assert_eq!(
            (v[2].method.as_str(), v[2].vlan_id, v[2].bridge.as_deref()),
            ("manual", Some(5), Some("br0"))
        );
        let mut m = std::collections::HashMap::new();
        m.insert("dummy0".to_string(), "manual".to_string());
        assert_eq!(
            interfaces_from_ip(&addr, &routes, &m)[1].method,
            "manual",
            "applied method wins"
        );
    }

    #[test]
    fn static_interface_validation() {
        let mut i = NetworkInterface {
            name: "x".into(),
            method: "static".into(),
            address: Some("10.1.2.3".into()),
            netmask: Some("255.255.255.0".into()),
            gateway: Some("10.1.2.1".into()),
            mtu: Some(1500),
            auto: true,
            bridge: None,
            bridge_ports: None,
            vlan_id: None,
        };
        assert!(validate_iface(&i).is_ok());
        i.gateway = Some("10.9.9.1".into());
        assert!(validate_iface(&i).unwrap_err().contains("not inside"));
        i.gateway = None;
        i.netmask = Some("255.0.255.0".into());
        assert!(validate_iface(&i).is_err());
        i.netmask = Some("255.255.255.0".into());
        i.mtu = Some(10);
        assert!(validate_iface(&i).is_err());
        i.mtu = None;
        i.method = "weird".into();
        assert!(validate_iface(&i).is_err());
    }

    #[test]
    fn resolv_conf_keeps_options_and_replaces_servers() {
        let old = "# generated\nnameserver 1.1.1.1\nsearch old.lan\noptions edns0 trust-ad\n";
        let c = DnsConfiguration {
            servers: vec!["10.0.0.2".into(), "2001:db8::1".into()],
            search_domains: vec!["lab.test".into()],
        };
        let n = render_resolv(old, &c);
        assert_eq!(parse_resolv(&n), c);
        assert!(n.contains("options edns0 trust-ad") && !n.contains("1.1.1.1"));
        assert_eq!(render_resolv(&n, &c), n, "idempotent");
    }

    #[test]
    fn ntp_conf_servers_are_swapped_in_place() {
        let old = "pool 2.pool.ntp.org iburst\ndriftfile /var/lib/chrony/drift\nmakestep 1 3\n";
        let s = vec!["ntp1.lab".to_string(), "10.0.0.9".to_string()];
        let n = render_ntp(NtpBackend::Chrony, old, &s);
        assert_eq!(parse_ntp_servers(NtpBackend::Chrony, &n), s);
        assert!(n.contains("driftfile") && n.contains("makestep") && !n.contains("2.pool"));
        assert_eq!(
            n.lines().next(),
            Some("server ntp1.lab iburst"),
            "keeps its position"
        );
        let t = render_ntp(NtpBackend::Timesyncd, "", &s);
        assert_eq!(parse_ntp_servers(NtpBackend::Timesyncd, &t), s);
        assert_eq!(render_ntp(NtpBackend::Ntpd, "", &s).lines().count(), 2);
    }

    #[test]
    fn proc_files_parse() {
        let m = parse_meminfo("MemTotal: 1000 kB\nMemFree: 100 kB\nMemAvailable: 400 kB\nBuffers: 10 kB\nCached: 50 kB\n");
        assert_eq!((m.total, m.used, m.available), (1024000, 614400, 409600));
        let n = parse_net_dev("h1\nh2\n  eth0: 100 2 3 0 0 0 0 0 200 4 5 0 0 0 0 0\n");
        assert_eq!(n.len(), 1);
        assert_eq!(
            (n[0].rx_bytes, n[0].rx_errors, n[0].tx_bytes, n[0].tx_errors),
            (100, 3, 200, 5)
        );
    }

    #[test]
    fn the_config_and_resolv_files_are_really_written() {
        let _g = crate::api::system_admin::SYSROOT_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HORCRUX_SYSROOT", dir.path());
        let root = dir.path();
        std::fs::create_dir_all(root.join("etc/conf.d")).unwrap();
        std::fs::create_dir_all(root.join("usr/share/zoneinfo/Europe")).unwrap();
        std::fs::write(root.join("usr/share/zoneinfo/Europe/Berlin"), b"tz").unwrap();
        let msg = apply_config(&SystemConfiguration {
            hostname: "Node-7".into(),
            domain: Some("lab.test".into()),
            timezone: "Europe/Berlin".into(),
            locale: "de_DE.UTF-8".into(),
        })
        .unwrap();
        assert!(msg.contains("node-7.lab.test"));
        assert_eq!(
            std::fs::read_to_string(root.join("etc/hostname")).unwrap(),
            "node-7\n"
        );
        assert!(std::fs::read_to_string(root.join("etc/conf.d/hostname"))
            .unwrap()
            .contains("hostname=\"node-7\""));
        assert!(std::fs::read_to_string(root.join("etc/hosts"))
            .unwrap()
            .contains("node-7.lab.test node-7"));
        assert_eq!(current_timezone(), "Europe/Berlin");
        assert_eq!(
            std::fs::read_to_string(root.join("etc/locale.conf")).unwrap(),
            "LANG=de_DE.UTF-8\n"
        );
        assert!(apply_config(&SystemConfiguration {
            hostname: "x".into(),
            domain: None,
            timezone: "Nowhere/Land".into(),
            locale: "C".into()
        })
        .is_err());
        std::env::remove_var("HORCRUX_SYSROOT");
    }
}
