//! Keeping a macvtap NIC's host-side receive filter in step with the guest.
//!
//! A macvtap device only passes the multicast frames its own filter lists,
//! and the kernel has no idea which groups the guest listens on: the guest's
//! subscriptions (IPv6 solicited-node groups `33:33:ff:xx:xx:xx` above all)
//! live in the virtio-net device. Without them the router's neighbour
//! solicitation for the guest's global address is dropped and nothing routes
//! over IPv6, although the link-local address (all-nodes group) still works.
//!
//! QEMU reports the guest's filter (`NIC_RX_FILTER_CHANGED` event, then
//! `query-rx-filter`) and libvirt copies it to the macvtap device
//! (`trustGuestRxFilters`). Horcrux does the same: every VM with a macvtap
//! NIC gets a second QMP monitor (`<disk>.rxf.sock`) that only this watcher
//! uses, so it never competes with the one-client-at-a-time main QMP socket.
//! Per NIC, [`MacvtapMulticast`] chooses between following the guest's filter
//! and passing all multicast.

use super::qemu::{tap_name, HostNet, QemuManager};
use horcrux_common::{MacvtapMulticast, NicAttach, VmNic};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// The monitor socket only the rx-filter watcher talks to.
pub fn rxfilter_socket(disk: &Path) -> PathBuf {
    disk.with_extension("rxf.sock")
}

/// QEMU arguments for the watcher's monitor; empty when the VM has no
/// macvtap NIC.
pub fn monitor_args(nets: &[VmNic], disk: &Path) -> Vec<String> {
    if !nets.iter().any(|n| n.attach == NicAttach::Macvtap) {
        return Vec::new();
    }
    vec![
        "-chardev".into(),
        format!(
            "socket,id=hxrxf,path={},server=on,wait=off",
            rxfilter_socket(disk).display()
        ),
        "-mon".into(),
        "chardev=hxrxf,mode=control".into(),
    ]
}

/// The QEMU device id of NIC `i` (`-device ...,id=nic<i>`).
pub fn nic_device_id(i: usize) -> String {
    format!("nic{i}")
}

/// What the macvtap device's filter should look like.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceFilter {
    pub promisc: bool,
    pub allmulti: bool,
    /// Multicast MACs to accept (lower-case, colon form) when `allmulti` is off.
    pub multicast: BTreeSet<String>,
}

/// One entry of `query-rx-filter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestRxFilter {
    pub name: String,
    pub promiscuous: bool,
    /// "normal" (use the table), "none" or "all".
    pub multicast: String,
    pub multicast_overflow: bool,
    pub multicast_table: Vec<String>,
}

impl GuestRxFilter {
    pub fn parse(v: &Value) -> Option<Self> {
        Some(Self {
            name: v.get("name")?.as_str()?.to_string(),
            promiscuous: v["promiscuous"].as_bool().unwrap_or(false),
            multicast: v["multicast"].as_str().unwrap_or("normal").to_string(),
            multicast_overflow: v["multicast-overflow"].as_bool().unwrap_or(false),
            multicast_table: v["multicast-table"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.as_str())
                        .map(|m| m.to_ascii_lowercase())
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

/// The filter the device needs for what the guest reported. A guest table
/// that overflowed (more groups than virtio-net's table holds) or "all"
/// means every multicast frame.
pub fn plan(g: &GuestRxFilter) -> DeviceFilter {
    let allmulti = g.multicast == "all" || g.multicast_overflow;
    let multicast = if g.multicast == "normal" && !allmulti {
        g.multicast_table.iter().cloned().collect()
    } else {
        BTreeSet::new()
    };
    DeviceFilter {
        promisc: g.promiscuous,
        allmulti,
        multicast,
    }
}

impl HostNet {
    /// Set `tap`'s filter to `new`, touching only what differs from `old`
    /// (None = unknown: set everything). Individual failures are logged and
    /// skipped: a group that can't be added must not stop the others.
    pub async fn apply_macvtap_filter(
        &self,
        tap: &str,
        old: Option<&DeviceFilter>,
        new: &DeviceFilter,
    ) {
        let onoff = |b: bool| if b { "on" } else { "off" };
        if old.map(|o| o.allmulti) != Some(new.allmulti) {
            self.ip_cmd(&["link", "set", tap, "allmulticast", onoff(new.allmulti)])
                .await;
        }
        if old.map(|o| o.promisc) != Some(new.promisc) {
            self.ip_cmd(&["link", "set", tap, "promisc", onoff(new.promisc)])
                .await;
        }
        let empty = BTreeSet::new();
        let prev = old.map(|o| &o.multicast).unwrap_or(&empty);
        for m in new.multicast.difference(prev) {
            self.ip_cmd(&["maddr", "add", m, "dev", tap]).await;
        }
        for m in prev.difference(&new.multicast) {
            self.ip_cmd(&["maddr", "del", m, "dev", tap]).await;
        }
    }

    /// The device's filter right after creation: all multicast passes, so the
    /// guest works until it has reported its own filter (and always, for
    /// [`MacvtapMulticast::All`]).
    pub async fn init_macvtap_filter(&self, tap: &str) {
        // A new macvtap device has promisc off and an empty list.
        self.apply_macvtap_filter(
            tap,
            Some(&DeviceFilter::default()),
            &DeviceFilter {
                promisc: false,
                allmulti: true,
                multicast: BTreeSet::new(),
            },
        )
        .await;
    }
}

fn running() -> &'static Mutex<HashSet<String>> {
    static R: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Start the watcher for `vm` (once per VM id; a second call is a no-op).
/// The task ends by itself when QEMU is gone.
pub fn spawn_watcher(vm_id: &str, disk: &Path, nets: &[VmNic]) {
    let follow: Vec<usize> = nets
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            n.attach == NicAttach::Macvtap && n.macvtap_multicast == MacvtapMulticast::Follow
        })
        .map(|(i, _)| i)
        .collect();
    if follow.is_empty() {
        return;
    }
    if !running().lock().unwrap().insert(vm_id.to_string()) {
        return;
    }
    let (id, disk) = (vm_id.to_string(), disk.to_path_buf());
    tokio::spawn(async move {
        let host = HostNet::system();
        let alive = {
            let id = id.clone();
            move || QemuManager::process_running(&id)
        };
        watch(
            &host,
            &id,
            &rxfilter_socket(&disk),
            &follow,
            alive,
            Duration::from_secs(1),
        )
        .await;
        running().lock().unwrap().remove(&id);
    });
}

/// Re-attach watchers to VMs that kept running across an API restart.
pub async fn adopt_running(vms: &super::VmManager) {
    for vm in vms.list_vms().await {
        if vm.hypervisor != horcrux_common::VmHypervisor::Qemu
            || !QemuManager::process_running(&vm.id)
        {
            continue;
        }
        // A VM started before this feature has no watcher monitor; it keeps
        // the filter it was started with until its next start.
        if let Some(d) = vm.disks.first() {
            if rxfilter_socket(Path::new(&d.path)).exists() {
                spawn_watcher(&vm.id, Path::new(&d.path), &vm.hardware.nets);
            }
        }
    }
}

struct Qmp {
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    w: tokio::net::unix::OwnedWriteHalf,
}

impl Qmp {
    async fn connect(sock: &Path) -> std::io::Result<Self> {
        let s = tokio::net::UnixStream::connect(sock).await?;
        let (r, w) = s.into_split();
        let mut q = Self {
            lines: BufReader::new(r).lines(),
            w,
        };
        q.next_json().await?; // greeting
        q.w.write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
            .await?;
        loop {
            let v = q.next_json().await?;
            if v.get("return").is_some() || v.get("error").is_some() {
                return Ok(q);
            }
        }
    }

    async fn next_json(&mut self) -> std::io::Result<Value> {
        let line = self
            .lines
            .next_line()
            .await?
            .ok_or_else(|| std::io::Error::other("QMP closed"))?;
        serde_json::from_str(&line).map_err(std::io::Error::other)
    }

    /// `query-rx-filter` for one device (events that arrive meanwhile are
    /// returned for the caller to act on).
    async fn query(
        &mut self,
        name: &str,
        events: &mut Vec<Value>,
    ) -> std::io::Result<Vec<GuestRxFilter>> {
        let cmd = json!({"execute": "query-rx-filter", "arguments": {"name": name}});
        self.w.write_all(format!("{cmd}\n").as_bytes()).await?;
        loop {
            let v = self.next_json().await?;
            if v.get("event").is_some() {
                events.push(v);
                continue;
            }
            if v.get("error").is_some() {
                // Not (yet) a NIC with a filter, e.g. a model that doesn't report.
                return Ok(Vec::new());
            }
            if let Some(a) = v.get("return").and_then(Value::as_array) {
                return Ok(a.iter().filter_map(GuestRxFilter::parse).collect());
            }
        }
    }
}

/// Watch the monitor at `sock` and keep each NIC in `follow` in step. Runs
/// until `alive()` turns false. Reconnects after a lost connection (an API
/// restart leaves QEMU's monitor free for the next client).
pub async fn watch(
    host: &HostNet,
    vm_id: &str,
    sock: &Path,
    follow: &[usize],
    alive: impl Fn() -> bool,
    retry: Duration,
) {
    let mut state: Vec<(usize, Option<DeviceFilter>)> = follow.iter().map(|&i| (i, None)).collect();
    while alive() {
        let mut q = match Qmp::connect(sock).await {
            Ok(q) => q,
            Err(_) => {
                tokio::time::sleep(retry).await;
                continue;
            }
        };
        let r = session(host, vm_id, &mut q, &mut state).await;
        if let Err(e) = r {
            tracing::debug!("VM {vm_id}: rx-filter monitor: {e}");
        }
        tokio::time::sleep(retry).await;
    }
}

async fn session(
    host: &HostNet,
    vm_id: &str,
    q: &mut Qmp,
    state: &mut [(usize, Option<DeviceFilter>)],
) -> std::io::Result<()> {
    let mut pending: Vec<Value> = Vec::new();
    // Initial state of every NIC (the guest may have reported before we connected).
    for (i, st) in state.iter_mut() {
        sync_nic(host, vm_id, q, *i, st, &mut pending).await?;
    }
    loop {
        let ev = match pending.pop() {
            Some(e) => e,
            None => q.next_json().await?,
        };
        if ev["event"] != "NIC_RX_FILTER_CHANGED" {
            continue;
        }
        let name = ev["data"]["name"].as_str().unwrap_or("");
        if let Some((i, st)) = state.iter_mut().find(|(i, _)| nic_device_id(*i) == name) {
            sync_nic(host, vm_id, q, *i, st, &mut pending).await?;
        }
    }
}

async fn sync_nic(
    host: &HostNet,
    vm_id: &str,
    q: &mut Qmp,
    i: usize,
    st: &mut Option<DeviceFilter>,
    pending: &mut Vec<Value>,
) -> std::io::Result<()> {
    let name = nic_device_id(i);
    let got = q.query(&name, pending).await?;
    let Some(g) = got.iter().find(|g| g.name == name) else {
        return Ok(());
    };
    let new = plan(g);
    if st.as_ref() == Some(&new) {
        return Ok(());
    }
    let tap = tap_name(vm_id, i);
    // After the first report the baseline "all multicast" of the fresh
    // device is the known old state; after an API restart it is unknown
    // (None), which re-applies everything.
    host.apply_macvtap_filter(&tap, st.as_ref(), &new).await;
    *st = Some(new);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::UnixListener;

    fn guest(v: Value) -> GuestRxFilter {
        GuestRxFilter::parse(&v).unwrap()
    }

    #[test]
    fn plan_follows_the_guest_table() {
        let p = plan(&guest(json!({
            "name": "nic0", "promiscuous": false, "multicast": "normal",
            "multicast-overflow": false,
            "multicast-table": ["33:33:FF:00:11:22", "33:33:00:00:00:01"]
        })));
        assert!(!p.allmulti && !p.promisc);
        assert_eq!(
            p.multicast.into_iter().collect::<Vec<_>>(),
            vec!["33:33:00:00:00:01", "33:33:ff:00:11:22"]
        );
    }

    #[test]
    fn plan_all_overflow_and_none() {
        for (mode, overflow, allmulti, listed) in [
            ("all", false, true, 0),
            ("normal", true, true, 0),
            ("none", false, false, 0),
        ] {
            let p = plan(&guest(json!({
                "name": "nic0", "multicast": mode, "multicast-overflow": overflow,
                "multicast-table": ["33:33:00:00:00:01"]
            })));
            assert_eq!(
                (p.allmulti, p.multicast.len()),
                (allmulti, listed),
                "{mode}"
            );
        }
        assert!(plan(&guest(json!({"name": "n", "promiscuous": true}))).promisc);
    }

    #[test]
    fn monitor_args_only_for_macvtap() {
        let mut n = VmNic {
            model: Default::default(),
            bridge: "eth0".into(),
            attach: NicAttach::Bridge,
            mac: None,
            vlan: None,
            rate_mbps: None,
            macvtap_multicast: Default::default(),
        };
        let disk = Path::new("/vms/a/a.qcow2");
        assert!(monitor_args(&[n.clone()], disk).is_empty());
        n.attach = NicAttach::Macvtap;
        let a = monitor_args(&[n], disk);
        assert!(a
            .join(" ")
            .contains("path=/vms/a/a.rxf.sock,server=on,wait=off"));
        assert!(a.join(" ").contains("-mon chardev=hxrxf,mode=control"));
    }

    fn fake_host() -> (tempfile::TempDir, HostNet) {
        let d = tempfile::tempdir().unwrap();
        let script = "echo \"$*\" >> \"$(dirname \"$0\")/calls\"\n";
        std::fs::write(d.path().join("ip"), script).unwrap();
        let h = HostNet::with_ip(vec![
            "sh".into(),
            d.path().join("ip").to_string_lossy().into(),
        ]);
        (d, h)
    }

    fn calls(d: &tempfile::TempDir) -> Vec<String> {
        std::fs::read_to_string(d.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[tokio::test]
    async fn apply_sends_only_the_difference() {
        let (d, h) = fake_host();
        let a = DeviceFilter {
            promisc: false,
            allmulti: true,
            multicast: BTreeSet::new(),
        };
        h.apply_macvtap_filter("tapx", None, &a).await;
        assert_eq!(
            calls(&d),
            vec!["link set tapx allmulticast on", "link set tapx promisc off"]
        );
        let b = DeviceFilter {
            promisc: false,
            allmulti: false,
            multicast: ["33:33:ff:00:11:22".to_string()].into(),
        };
        h.apply_macvtap_filter("tapx", Some(&a), &b).await;
        let c = calls(&d);
        assert_eq!(
            &c[2..],
            [
                "link set tapx allmulticast off",
                "maddr add 33:33:ff:00:11:22 dev tapx"
            ]
        );
        let c2 = DeviceFilter {
            multicast: ["33:33:ff:99:99:99".to_string()].into(),
            ..b.clone()
        };
        h.apply_macvtap_filter("tapx", Some(&b), &c2).await;
        let c = calls(&d);
        assert_eq!(
            &c[4..],
            [
                "maddr add 33:33:ff:99:99:99 dev tapx",
                "maddr del 33:33:ff:00:11:22 dev tapx"
            ]
        );
    }

    /// A fake QEMU monitor: answers query-rx-filter with a table that grows
    /// after each NIC_RX_FILTER_CHANGED event, like a guest joining groups.
    #[tokio::test]
    async fn watcher_follows_events_from_the_monitor() {
        let (d, h) = fake_host();
        let sock = d.path().join("vm.rxf.sock");
        let l = UnixListener::bind(&sock).unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            s.write_all(b"{\"QMP\":{}}\n").await.unwrap();
            let mut buf = vec![0u8; 4096];
            let mut tables: Vec<Vec<&str>> = vec![
                vec!["33:33:00:00:00:01"],
                vec!["33:33:00:00:00:01", "33:33:ff:aa:bb:cc"],
            ];
            tables.reverse();
            let mut cur: Vec<&str> = tables.pop().unwrap();
            loop {
                let n = s.read(&mut buf).await.unwrap();
                if n == 0 {
                    return;
                }
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                for cmd in text.lines() {
                    if cmd.contains("qmp_capabilities") {
                        s.write_all(b"{\"return\":{}}\n").await.unwrap();
                    } else if cmd.contains("query-rx-filter") {
                        let r = json!({"return": [{"name": "nic1", "promiscuous": false,
                            "multicast": "normal", "multicast-overflow": false,
                            "multicast-table": cur}]});
                        s.write_all(format!("{r}\n").as_bytes()).await.unwrap();
                        if let Some(next) = tables.pop() {
                            cur = next;
                            s.write_all(
                                b"{\"event\":\"NIC_RX_FILTER_CHANGED\",\"data\":{\"name\":\"nic1\",\"path\":\"/x\"}}\n",
                            )
                            .await
                            .unwrap();
                        }
                    }
                }
            }
        });
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let alive = {
            let stop = stop.clone();
            move || stop.load(std::sync::atomic::Ordering::SeqCst)
        };
        let w = tokio::spawn({
            let sock = sock.clone();
            async move {
                watch(&h, "vmid", &sock, &[1], alive, Duration::from_millis(50)).await;
            }
        });
        let tap = tap_name("vmid", 1);
        let want = format!("maddr add 33:33:ff:aa:bb:cc dev {tap}");
        for _ in 0..100 {
            if calls(&d).contains(&want) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let c = calls(&d);
        stop.store(false, std::sync::atomic::Ordering::SeqCst);
        server.abort();
        let _ = tokio::time::timeout(Duration::from_secs(3), w).await;
        assert!(
            c.contains(&format!("link set {tap} allmulticast off")),
            "{c:?}"
        );
        assert!(
            c.contains(&format!("maddr add 33:33:00:00:00:01 dev {tap}")),
            "{c:?}"
        );
        assert!(c.contains(&want), "{c:?}");
    }
}

/// Real-kernel proof (needs root, `ip`, netns and macvtap support; run by
/// `horcrux-api/tests/macvtap_ndp_e2e.sh`): a router in a network namespace
/// can resolve the IPv6 global address of a "guest" behind a macvtap device
/// only once the device's filter carries the guest's solicited-node group.
#[cfg(test)]
mod netns_e2e {
    use super::*;
    use std::io::{Read, Write};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    fn sh(args: &[&str]) -> String {
        let o = Command::new(args[0]).args(&args[1..]).output().unwrap();
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).to_string()
    }

    fn csum(src: &[u8; 16], dst: &[u8; 16], icmp: &[u8]) -> u16 {
        let mut b = Vec::new();
        b.extend_from_slice(src);
        b.extend_from_slice(dst);
        b.extend_from_slice(&(icmp.len() as u32).to_be_bytes());
        b.extend_from_slice(&[0, 0, 0, 58]);
        b.extend_from_slice(icmp);
        if b.len() % 2 == 1 {
            b.push(0);
        }
        let mut sum: u32 = b
            .chunks(2)
            .map(|c| u32::from(u16::from_be_bytes([c[0], c[1]])))
            .sum();
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        !(sum as u16)
    }

    /// The reply of a host owning `target` to the neighbour solicitation
    /// frame `ns`, or None when `ns` isn't one for it.
    fn answer(ns: &[u8], my_mac: [u8; 6], target: [u8; 16]) -> Option<Vec<u8>> {
        if ns.len() < 78 || ns[12..14] != [0x86, 0xdd] || ns[20] != 58 || ns[54] != 135 {
            return None;
        }
        if ns[62..78] != target {
            return None;
        }
        let src_ip: [u8; 16] = ns[22..38].try_into().unwrap();
        let mut icmp = vec![136, 0, 0, 0, 0x60, 0, 0, 0];
        icmp.extend_from_slice(&target);
        icmp.extend_from_slice(&[2, 1]);
        icmp.extend_from_slice(&my_mac);
        let c = csum(&target, &src_ip, &icmp);
        icmp[2..4].copy_from_slice(&c.to_be_bytes());
        let mut f = Vec::new();
        f.extend_from_slice(&ns[6..12]);
        f.extend_from_slice(&my_mac);
        f.extend_from_slice(&[0x86, 0xdd, 0x60, 0, 0, 0]);
        f.extend_from_slice(&(icmp.len() as u16).to_be_bytes());
        f.extend_from_slice(&[58, 255]);
        f.extend_from_slice(&target);
        f.extend_from_slice(&src_ip);
        f.extend_from_slice(&icmp);
        Some(f)
    }

    fn cleanup() {
        let _ = Command::new("ip").args(["link", "del", "hxnu"]).output();
        let _ = Command::new("ip").args(["netns", "del", "hxnr"]).output();
    }

    #[tokio::test]
    #[ignore = "needs root, netns and macvtap: run horcrux-api/tests/macvtap_ndp_e2e.sh"]
    async fn router_resolves_the_guest_global_address_only_with_its_group() {
        cleanup();
        let mac = [0x02u8, 0, 0, 0x5e, 0x10, 0x07];
        let mac_s = "02:00:00:5e:10:07";
        let guest_ip: [u8; 16] = "2001:db8:77::2"
            .parse::<std::net::Ipv6Addr>()
            .unwrap()
            .octets();
        sh(&["ip", "netns", "add", "hxnr"]);
        sh(&[
            "ip", "link", "add", "hxnu", "type", "veth", "peer", "name", "hxnp",
        ]);
        sh(&["ip", "link", "set", "hxnp", "netns", "hxnr"]);
        sh(&["ip", "link", "set", "hxnu", "up"]);
        sh(&[
            "ip",
            "netns",
            "exec",
            "hxnr",
            "sysctl",
            "-qw",
            "net.ipv6.conf.hxnp.accept_dad=0",
        ]);
        sh(&[
            "ip", "netns", "exec", "hxnr", "ip", "link", "set", "hxnp", "up",
        ]);
        sh(&[
            "ip",
            "netns",
            "exec",
            "hxnr",
            "ip",
            "addr",
            "add",
            "2001:db8:77::1/64",
            "dev",
            "hxnp",
            "nodad",
        ]);
        // The macvtap device the way Horcrux makes it (VM id any, NIC 0).
        let host = HostNet::system();
        let tap = tap_name("netns-e2e", 0);
        let _ = Command::new("ip").args(["link", "del", &tap]).output();
        sh(&[
            "ip", "link", "add", "link", "hxnu", "name", &tap, "type", "macvtap", "mode", "bridge",
        ]);
        sh(&["ip", "link", "set", &tap, "address", mac_s]);
        sh(&["ip", "link", "set", &tap, "up"]);
        let ifindex = std::fs::read_to_string(format!("/sys/class/net/{tap}/ifindex")).unwrap();
        let ifindex = ifindex.trim();
        let node = format!("/dev/tap{ifindex}");
        for _ in 0..30 {
            if Path::new(&node).exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if !Path::new(&node).exists() {
            let dev =
                std::fs::read_to_string(format!("/sys/class/net/{tap}/macvtap/tap{ifindex}/dev"))
                    .unwrap();
            let (a, b) = dev.trim().split_once(':').unwrap();
            sh(&["mknod", &node, "c", a, b]);
        }
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&node)
            .unwrap();
        // The "guest": answers solicitations for its global address, counts them.
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(AtomicUsize::new(0));
        let guest = {
            let (stop, seen) = (stop.clone(), seen.clone());
            std::thread::spawn(move || {
                use std::os::fd::AsRawFd;
                let mut buf = vec![0u8; 4096];
                while !stop.load(Ordering::SeqCst) {
                    let mut p = [nix::poll::PollFd::new(
                        unsafe { std::os::fd::BorrowedFd::borrow_raw(f.as_raw_fd()) },
                        nix::poll::PollFlags::POLLIN,
                    )];
                    if nix::poll::poll(&mut p, 100u16).unwrap_or(0) == 0 {
                        continue;
                    }
                    // A macvtap queue speaks virtio-net frames: 10 header bytes first.
                    let n = f.read(&mut buf).unwrap_or(0);
                    if n <= 10 {
                        continue;
                    }
                    if let Some(r) = answer(&buf[10..n], mac, guest_ip) {
                        seen.fetch_add(1, Ordering::SeqCst);
                        let mut framed = vec![0u8; 10];
                        framed.extend_from_slice(&r);
                        let _ = f.write_all(&framed);
                    }
                }
            })
        };
        let resolved = || {
            let o = Command::new("ip")
                .args([
                    "netns",
                    "exec",
                    "hxnr",
                    "ip",
                    "-6",
                    "neigh",
                    "show",
                    "2001:db8:77::2",
                    "dev",
                    "hxnp",
                ])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).contains(mac_s)
        };
        let poke = || {
            let _ = Command::new("ip")
                .args([
                    "netns",
                    "exec",
                    "hxnr",
                    "ping",
                    "-6",
                    "-c1",
                    "-W1",
                    "2001:db8:77::2",
                ])
                .output();
        };
        // The bug: a freshly made device, filter empty (what the kernel has
        // before anyone tells it the guest's groups): the router can't resolve.
        sh(&["ip", "link", "set", &tap, "allmulticast", "off"]);
        poke();
        assert!(!resolved(), "resolved without the guest's group");
        assert_eq!(seen.load(Ordering::SeqCst), 0, "guest saw a solicitation");
        // The fix: the filter the guest reports, applied by the product code
        // (solicited-node group of 2001:db8:77::2 = ff02::1:ff00:2).
        let g = GuestRxFilter {
            name: "nic0".into(),
            promiscuous: false,
            multicast: "normal".into(),
            multicast_overflow: false,
            multicast_table: vec!["33:33:00:00:00:01".into(), "33:33:ff:00:00:02".into()],
        };
        host.apply_macvtap_filter(&tap, None, &plan(&g)).await;
        let _ = Command::new("ip")
            .args([
                "netns", "exec", "hxnr", "ip", "-6", "neigh", "flush", "dev", "hxnp",
            ])
            .output();
        poke();
        let ok = resolved();
        let n = seen.load(Ordering::SeqCst);
        stop.store(true, Ordering::SeqCst);
        guest.join().unwrap();
        let _ = Command::new("ip").args(["link", "del", &tap]).output();
        cleanup();
        assert!(
            ok && n >= 1,
            "router did not resolve the guest (guest saw {n} solicitations)"
        );
    }
}
