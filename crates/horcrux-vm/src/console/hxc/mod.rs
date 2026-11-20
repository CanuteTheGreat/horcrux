//! Horcrux Console (docs/design/horcrux-console.md): one remote display
//! for every guest, phase C1 = the shared core.
//!
//! * [`settings`]: the node's `[console]` settings (config.toml defaults,
//!   API/declarative changes in `<data_dir>/console.json`).
//! * [`Broker`]: short-lived console tickets bound to a user and the
//!   client's signalling connection, and the live sessions.
//! * [`qemu_dbus`]: QEMU's D-Bus display as the frame/input source (no guest
//!   tools, works pre-OS).
//! * [`encode`]: host encoding through the node's FFmpeg (VA-API/NVENC when
//!   they work, libx264/libvpx otherwise).
//! * [`session`]: the WebRTC transport (str0m) and the data channels.
//! * [`page`]: the browser client served by the node.
pub mod abr;
pub mod audio;
pub mod deliver;
pub mod encode;
pub mod fec;
pub mod guest;
pub mod keymap;
pub mod local;
pub mod page;
pub mod portmap;
pub mod qemu_dbus;
pub mod relay;
pub mod session;
pub use super::hxc_settings as settings;

use horcrux_common::console::{ConsoleProfile, ConsoleSettings};
use serde::Serialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A console ticket: one guest, one session, short lifetime, bound to the
/// user who asked for it and to their signalling (API WebSocket)
/// connection. Redeemed once.
#[derive(Debug, Clone)]
pub struct HxcTicket {
    pub ticket: String,
    pub vm_id: String,
    pub user_id: String,
    pub username: String,
    pub conn_id: String,
    pub profile: ConsoleProfile,
    pub allow_input: bool,
    pub allow_clipboard: bool,
    pub allow_mic: bool,
    /// ICE host candidates for this client (from the address it used to
    /// reach the node when the ticket was issued).
    pub candidate_ips: Vec<IpAddr>,
    /// Names this ticket's relay credentials (revoked when the session
    /// ends); empty when the relay is off.
    pub relay_tag: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub vm_id: String,
    pub user: String,
    pub profile: ConsoleProfile,
    pub started: String,
    pub stats: session::SessionStats,
}

/// Why a ticket could not be redeemed.
#[derive(Debug, PartialEq, Eq)]
pub enum RedeemError {
    Unknown,
    Expired,
    WrongUser,
    WrongConnection,
}

impl std::fmt::Display for RedeemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RedeemError::Unknown => "unknown or already used console ticket",
            RedeemError::Expired => "console ticket expired",
            RedeemError::WrongUser => "console ticket belongs to another user",
            RedeemError::WrongConnection => {
                "console ticket was issued for another signalling connection"
            }
        })
    }
}

#[derive(Default)]
pub struct Broker {
    tickets: Mutex<HashMap<String, HxcTicket>>,
    sessions: Mutex<HashMap<String, Arc<session::SessionHandle>>>,
}

impl Broker {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn issue(&self, mut t: HxcTicket, lifetime_secs: u32) -> HxcTicket {
        let now = chrono::Utc::now().timestamp();
        t.ticket = format!(
            "hxc-{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        t.expires_at = now + lifetime_secs as i64;
        let mut map = self.tickets.lock().await;
        map.retain(|_, v| v.expires_at >= now);
        map.insert(t.ticket.clone(), t.clone());
        t
    }

    /// Take the ticket out (single use) if it belongs to `user_id` on
    /// `conn_id` and is still valid.
    pub async fn redeem(
        &self,
        ticket: &str,
        user_id: &str,
        conn_id: &str,
    ) -> Result<HxcTicket, RedeemError> {
        let now = chrono::Utc::now().timestamp();
        let mut map = self.tickets.lock().await;
        let t = map.get(ticket).ok_or(RedeemError::Unknown)?;
        if t.user_id != user_id {
            // Not removed: a stolen ticket id must not let anyone burn it.
            return Err(RedeemError::WrongUser);
        }
        if t.conn_id != conn_id {
            return Err(RedeemError::WrongConnection);
        }
        let t = map.remove(ticket).ok_or(RedeemError::Unknown)?;
        if t.expires_at < now {
            return Err(RedeemError::Expired);
        }
        Ok(t)
    }

    /// Redeem a local (same-host viewer) ticket: issued with
    /// `conn_id = "local:<uid>"`, redeemable only over the local socket by
    /// a peer with that Unix uid.
    pub async fn redeem_local(
        &self,
        ticket: &str,
        peer_uid: u32,
    ) -> Result<HxcTicket, RedeemError> {
        let now = chrono::Utc::now().timestamp();
        let mut map = self.tickets.lock().await;
        let t = map.get(ticket).ok_or(RedeemError::Unknown)?;
        if t.conn_id != format!("local:{peer_uid}") {
            return Err(if t.conn_id.starts_with("local:") {
                RedeemError::WrongUser
            } else {
                RedeemError::WrongConnection
            });
        }
        let t = map.remove(ticket).ok_or(RedeemError::Unknown)?;
        if t.expires_at < now {
            return Err(RedeemError::Expired);
        }
        Ok(t)
    }

    pub async fn add_session(&self, h: session::SessionHandle) -> Arc<session::SessionHandle> {
        let h = Arc::new(h);
        let mut map = self.sessions.lock().await;
        map.retain(|_, s| !s.finished());
        map.insert(h.id.clone(), h.clone());
        h
    }

    pub async fn live_sessions_for(&self, vm_id: &str) -> usize {
        let mut map = self.sessions.lock().await;
        map.retain(|_, s| !s.finished());
        map.values().filter(|s| s.vm_id == vm_id).count()
    }

    pub async fn sessions(&self, vm_id: Option<&str>) -> Vec<SessionInfo> {
        let mut map = self.sessions.lock().await;
        map.retain(|_, s| !s.finished());
        let mut v: Vec<SessionInfo> = map
            .values()
            .filter(|s| vm_id.map(|id| s.vm_id == id).unwrap_or(true))
            .map(|s| SessionInfo {
                id: s.id.clone(),
                vm_id: s.vm_id.clone(),
                user: s.user.clone(),
                profile: s.profile,
                started: s.started.to_rfc3339(),
                stats: s.stats.lock().map(|x| x.clone()).unwrap_or_default(),
            })
            .collect();
        v.sort_by(|a, b| a.started.cmp(&b.started));
        v
    }

    pub async fn get(&self, id: &str) -> Option<Arc<session::SessionHandle>> {
        self.sessions.lock().await.get(id).cloned()
    }

    /// Stop every session of `vm_id` (VM stopped) or one session.
    pub async fn stop(&self, vm_id: Option<&str>, session_id: Option<&str>) -> usize {
        let map = self.sessions.lock().await;
        let mut n = 0;
        for s in map.values() {
            if vm_id.map(|v| v == s.vm_id).unwrap_or(true)
                && session_id.map(|i| i == s.id).unwrap_or(true)
            {
                s.stop();
                n += 1;
            }
        }
        n
    }
}

/// Whether the console can show this VM right now, and why not.
#[derive(Debug, Clone, Serialize)]
pub struct Availability {
    pub available: bool,
    pub reason: Option<String>,
    pub enabled: bool,
    pub vm_has_dbus_display: bool,
    pub encoder: encode::EncoderCaps,
    pub profile: ConsoleProfile,
    pub prefer_classic: bool,
}

/// Was this run of the VM started with the D-Bus display?
pub async fn vm_dbus_display(disk: &std::path::Path) -> bool {
    crate::vm::display::DisplayRuntime::load(disk)
        .await
        .map(|rt| rt.dbus_display)
        .unwrap_or(false)
}

pub async fn availability(settings: &ConsoleSettings, disk: Option<PathBuf>) -> Availability {
    let encoder = encode::caps(settings).await;
    let dbus = match &disk {
        Some(d) => vm_dbus_display(d).await,
        None => false,
    };
    let reason = if !settings.enabled {
        Some("The Horcrux Console is turned off on this node (Settings > Console)".to_string())
    } else if disk.is_none() {
        Some("Only QEMU VMs have the Horcrux Console in this release".to_string())
    } else if !dbus {
        Some(
            "This VM is not running with the D-Bus display: start (or restart) it with the \
             console enabled; QEMU needs its D-Bus display module"
                .to_string(),
        )
    } else if encoder.codecs().is_empty() {
        Some(format!(
            "No video encoder on this node: {}",
            encoder.error.clone().unwrap_or_default()
        ))
    } else {
        None
    };
    Availability {
        available: reason.is_none(),
        reason,
        enabled: settings.enabled,
        vm_has_dbus_display: dbus,
        encoder,
        profile: settings.default_profile,
        prefer_classic: settings.prefer_classic,
    }
}

/// ICE host candidate addresses for a client that reached the node as
/// `host` (the HTTP Host header): the configured list, else that address
/// plus the node's interface addresses (IPv4, no link-local).
pub async fn candidate_ips(settings: &ConsoleSettings, host: Option<&str>) -> Vec<IpAddr> {
    let mut v: Vec<IpAddr> = settings
        .candidate_addresses
        .iter()
        .filter_map(|a| a.parse().ok())
        .collect();
    if !v.is_empty() {
        return v;
    }
    if let Some(h) = host {
        let h = h.trim();
        // strip :port, keep IPv6 literals
        let name = if let Some(rest) = h.strip_prefix('[') {
            rest.split(']').next().unwrap_or(rest).to_string()
        } else {
            h.rsplit_once(':')
                .map(|(a, _)| a.to_string())
                .unwrap_or_else(|| h.to_string())
        };
        if let Ok(ip) = name.parse::<IpAddr>() {
            v.push(ip);
        } else if let Ok(addrs) = tokio::net::lookup_host((name.as_str(), 0)).await {
            v.extend(addrs.map(|a| a.ip()));
        }
    }
    let loopback_client = v.iter().any(|ip| ip.is_loopback());
    if let Ok(ifs) = nix::ifaddrs::getifaddrs() {
        for i in ifs {
            if let Some(sin) = i.address.as_ref().and_then(|a| a.as_sockaddr_in()) {
                let ip = IpAddr::V4(sin.ip());
                if ip.is_loopback() && !loopback_client {
                    continue;
                }
                v.push(ip);
            }
        }
    }
    // IPv4 only in C1 (one socket per address; IPv6 host candidates come
    // with the relay work in C3).
    v.retain(|ip| ip.is_ipv4());
    let mut seen = std::collections::HashSet::new();
    v.retain(|ip| seen.insert(*ip));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(user: &str, conn: &str) -> HxcTicket {
        HxcTicket {
            ticket: String::new(),
            vm_id: "vm1".into(),
            user_id: user.into(),
            username: user.into(),
            conn_id: conn.into(),
            profile: ConsoleProfile::Desktop,
            allow_input: true,
            allow_clipboard: false,
            allow_mic: false,
            candidate_ips: Vec::new(),
            relay_tag: String::new(),
            expires_at: 0,
        }
    }

    #[tokio::test]
    async fn local_tickets_are_bound_to_the_viewer_uid() {
        let b = Broker::new();
        let t = b.issue(ticket("u1", "local:1000"), 60).await;
        assert_eq!(
            b.redeem_local(&t.ticket, 1001).await.unwrap_err(),
            RedeemError::WrongUser
        );
        // not redeemable as a WebRTC ticket either
        assert!(b.redeem(&t.ticket, "u1", "c1").await.is_err());
        assert_eq!(b.redeem_local(&t.ticket, 1000).await.unwrap().vm_id, "vm1");
        assert_eq!(
            b.redeem_local(&t.ticket, 1000).await.unwrap_err(),
            RedeemError::Unknown
        );
        // a WebRTC ticket is not a local one
        let w = b.issue(ticket("u1", "c1"), 60).await;
        assert_eq!(
            b.redeem_local(&w.ticket, 1000).await.unwrap_err(),
            RedeemError::WrongConnection
        );
    }

    #[tokio::test]
    async fn tickets_are_single_use_and_bound() {
        let b = Broker::new();
        let t = b.issue(ticket("u1", "c1"), 60).await;
        assert!(t.ticket.starts_with("hxc-") && t.ticket.len() > 60);
        assert_eq!(
            b.redeem(&t.ticket, "u2", "c1").await.unwrap_err(),
            RedeemError::WrongUser
        );
        assert_eq!(
            b.redeem(&t.ticket, "u1", "c2").await.unwrap_err(),
            RedeemError::WrongConnection
        );
        // the wrong attempts did not burn it
        let ok = b.redeem(&t.ticket, "u1", "c1").await.unwrap();
        assert_eq!(ok.vm_id, "vm1");
        assert_eq!(
            b.redeem(&t.ticket, "u1", "c1").await.unwrap_err(),
            RedeemError::Unknown
        );
    }

    #[tokio::test]
    async fn expired_tickets_fail() {
        let b = Broker::new();
        let mut t = b.issue(ticket("u1", "c1"), 60).await;
        t.expires_at = chrono::Utc::now().timestamp() - 1;
        b.tickets.lock().await.insert(t.ticket.clone(), t.clone());
        assert_eq!(
            b.redeem(&t.ticket, "u1", "c1").await.unwrap_err(),
            RedeemError::Expired
        );
    }

    #[tokio::test]
    async fn candidates() {
        let mut s = ConsoleSettings {
            candidate_addresses: vec!["192.0.2.7".into()],
            ..Default::default()
        };
        assert_eq!(
            candidate_ips(&s, Some("10.1.1.1:8006")).await,
            vec!["192.0.2.7".parse::<IpAddr>().unwrap()]
        );
        s.candidate_addresses.clear();
        let v = candidate_ips(&s, Some("127.0.0.1:8006")).await;
        assert_eq!(v[0], "127.0.0.1".parse::<IpAddr>().unwrap());
        let v = candidate_ips(&s, Some("[::1]:8006")).await;
        assert!(v.iter().all(|ip| ip.is_ipv4()));
    }
}
