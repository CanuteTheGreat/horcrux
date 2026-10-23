//! Live VM devices: memory balloon (virtio-balloon over QMP), the QEMU guest
//! agent (QGA protocol over the VM's virtio-serial socket) and USB
//! passthrough (host device list from sysfs, hot-plug over QMP).
use super::display::{qmp_command, DisplayRuntime};
use super::{QemuManager, VmManager};
use horcrux_common::{Error, Result, UsbDevice, VmConfig, VmHypervisor};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// ------------------------------------------------------------------ balloon

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BalloonInfo {
    /// The VM has a balloon device (VM hardware > Memory ballooning).
    pub enabled: bool,
    pub running: bool,
    /// Configured memory (MB).
    pub memory_mb: u64,
    /// Memory the guest holds right now (MB), from QEMU; None while stopped.
    pub actual_mb: Option<u64>,
    /// The target last requested through the API since the VM started.
    pub target_mb: Option<u64>,
}

fn targets() -> &'static Mutex<HashMap<String, u64>> {
    static T: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

pub const MIN_BALLOON_MB: u64 = 16;

/// Validate a requested target against the VM's configured memory.
pub fn check_balloon_target(target_mb: u64, memory_mb: u64) -> Result<()> {
    if target_mb < MIN_BALLOON_MB {
        return Err(Error::Validation(format!(
            "balloon target must be at least {MIN_BALLOON_MB} MB"
        )));
    }
    if target_mb > memory_mb {
        return Err(Error::Validation(format!(
            "balloon target {target_mb} MB is above the VM's memory ({memory_mb} MB)"
        )));
    }
    Ok(())
}

// -------------------------------------------------------------- guest agent

/// One QGA command: `guest-sync-delimited` handshake (so stale replies from
/// an earlier session are discarded), then the command.
pub async fn qga_exec(socket: &Path, command: &str, args: Value, secs: u64) -> Result<Value> {
    let fail = |m: String| Error::System(m);
    if !socket.exists() {
        return Err(Error::Validation(
            "this VM has no guest agent channel: enable \"QEMU guest agent\" in its hardware and restart it".into(),
        ));
    }
    let run = async {
        let stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|e| format!("guest agent socket: {e}"))?;
        let (r, mut w) = stream.into_split();
        let mut r = BufReader::new(r);
        let id: u32 = rand::random::<u32>() >> 1;
        let mut sync = vec![0xFFu8];
        sync.extend_from_slice(
            json!({"execute": "guest-sync-delimited", "arguments": {"id": id}})
                .to_string()
                .as_bytes(),
        );
        sync.push(b'\n');
        w.write_all(&sync).await.map_err(|e| e.to_string())?;
        loop {
            let mut line = Vec::new();
            let n = r
                .read_until(b'\n', &mut line)
                .await
                .map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("guest agent channel closed".to_string());
            }
            // the reply to the sync is preceded by 0xFF; anything before is stale
            let Some(pos) = line.iter().rposition(|b| *b == 0xFF) else {
                continue;
            };
            if let Ok(v) = serde_json::from_slice::<Value>(&line[pos + 1..]) {
                if v["return"].as_u64() == Some(id as u64) {
                    break;
                }
            }
        }
        let req = json!({"execute": command, "arguments": args});
        w.write_all(format!("{req}\n").as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        loop {
            let mut line = String::new();
            let n = r.read_line(&mut line).await.map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("guest agent channel closed".to_string());
            }
            if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                if let Some(e) = v.get("error") {
                    return Err(format!(
                        "{}",
                        e.get("desc")
                            .and_then(|d| d.as_str())
                            .unwrap_or("guest agent error")
                    ));
                }
                if v.get("return").is_some() {
                    return Ok(v["return"].clone());
                }
            }
        }
    };
    match tokio::time::timeout(Duration::from_secs(secs), run).await {
        Err(_) => Err(fail(format!(
            "the guest agent did not answer within {secs} s: is qemu-guest-agent installed and running in the guest?"
        ))),
        Ok(Err(e)) => Err(fail(e)),
        Ok(Ok(v)) => Ok(v),
    }
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct AgentInterface {
    pub name: String,
    pub mac: Option<String>,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct AgentInfo {
    /// The VM has the agent channel configured.
    pub enabled: bool,
    pub running: bool,
    /// The agent in the guest answered.
    pub available: bool,
    pub version: Option<String>,
    pub os_name: Option<String>,
    pub os_version: Option<String>,
    pub kernel: Option<String>,
    pub hostname: Option<String>,
    pub interfaces: Vec<AgentInterface>,
    /// Commands the agent supports and allows.
    pub commands: Vec<String>,
    pub error: Option<String>,
}

/// `guest-network-get-interfaces` -> the interfaces with a MAC or address
/// (loopback omitted).
pub fn parse_interfaces(v: &Value) -> Vec<AgentInterface> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter(|i| i["name"].as_str() != Some("lo"))
        .map(|i| AgentInterface {
            name: i["name"].as_str().unwrap_or("").to_string(),
            mac: i["hardware-address"].as_str().map(|s| s.to_string()),
            addresses: i["ip-addresses"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| {
                    Some(format!(
                        "{}/{}",
                        a["ip-address"].as_str()?,
                        a["prefix"].as_u64().unwrap_or(0)
                    ))
                })
                .collect(),
        })
        .collect()
}

/// `guest-info`: (version, enabled commands).
pub fn parse_guest_info(v: &Value) -> (Option<String>, Vec<String>) {
    let cmds = v["supported_commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["enabled"].as_bool().unwrap_or(false))
        .filter_map(|c| c["name"].as_str().map(|s| s.to_string()))
        .collect();
    (v["version"].as_str().map(|s| s.to_string()), cmds)
}

/// Actions `POST /api/vms/:id/agent/:action` accepts -> (QGA command, arguments).
pub fn agent_command(action: &str) -> Option<(&'static str, Value)> {
    Some(match action {
        "ping" => ("guest-ping", json!({})),
        "fsfreeze" => ("guest-fsfreeze-freeze", json!({})),
        "fsthaw" => ("guest-fsfreeze-thaw", json!({})),
        "fsfreeze-status" => ("guest-fsfreeze-status", json!({})),
        "fstrim" => ("guest-fstrim", json!({"minimum": 0})),
        "shutdown" => ("guest-shutdown", json!({"mode": "powerdown"})),
        "reboot" => ("guest-shutdown", json!({"mode": "reboot"})),
        "time" => ("guest-get-time", json!({})),
        _ => return None,
    })
}

/// A frozen guest is thawed again after this long unless someone thawed it.
pub const MAX_FREEZE: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------- USB

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HostUsbDevice {
    pub vendor_id: String,
    pub product_id: String,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub bus: Option<u32>,
    pub device: Option<u32>,
    pub speed_mbps: Option<u32>,
    /// A hub can't be passed through.
    pub is_hub: bool,
    /// The sysfs name (`1-2`).
    pub port: String,
}

/// USB devices under `<root>/sys/bus/usb/devices`.
pub fn host_usb_devices_in(root: &Path) -> Vec<HostUsbDevice> {
    let dir = root.join("sys/bus/usb/devices");
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        // interfaces are `1-2:1.0`, root hubs `usb1`
        if name.contains(':') || name.starts_with("usb") {
            continue;
        }
        let p = e.path();
        let rd = |f: &str| {
            std::fs::read_to_string(p.join(f))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let (Some(v), Some(pid)) = (rd("idVendor"), rd("idProduct")) else {
            continue;
        };
        out.push(HostUsbDevice {
            vendor_id: v.to_ascii_lowercase(),
            product_id: pid.to_ascii_lowercase(),
            manufacturer: rd("manufacturer"),
            product: rd("product"),
            bus: rd("busnum").and_then(|s| s.parse().ok()),
            device: rd("devnum").and_then(|s| s.parse().ok()),
            speed_mbps: rd("speed")
                .and_then(|s| s.parse::<f64>().ok())
                .map(|s| s as u32),
            is_hub: rd("bDeviceClass").is_some_and(|c| c == "09"),
            port: name,
        });
    }
    out.sort_by(|a, b| (a.bus, a.device, &a.port).cmp(&(b.bus, b.device, &b.port)));
    out
}

pub fn host_usb_devices() -> Vec<HostUsbDevice> {
    host_usb_devices_in(Path::new("/"))
}

/// QEMU device id of a passed-through device (stable, so it can be removed later).
pub fn usb_device_id(d: &UsbDevice) -> String {
    format!("hxusb_{}_{}", d.vendor_id, d.product_id)
}

impl VmManager {
    async fn qmp_path_of(&self, id: &str) -> Result<(VmConfig, PathBuf, bool)> {
        let vm = self.qemu_vm(id).await?;
        let running = QemuManager::process_running(&vm.id);
        let sock = DisplayRuntime::qmp_socket(&vm.disk_path);
        Ok((vm.to_config(), sock, running))
    }

    pub async fn balloon_info(&self, id: &str) -> Result<BalloonInfo> {
        let (cfg, sock, running) = self.qmp_path_of(id).await?;
        let mut info = BalloonInfo {
            enabled: cfg.hardware.balloon,
            running,
            memory_mb: cfg.memory,
            actual_mb: None,
            target_mb: targets().lock().unwrap().get(id).copied(),
        };
        if running && cfg.hardware.balloon {
            let r = qmp_command(&sock, json!({"execute": "query-balloon"})).await?;
            info.actual_mb = r["actual"].as_u64().map(|b| b / 1024 / 1024);
        } else if !running {
            targets().lock().unwrap().remove(id);
        }
        Ok(info)
    }

    /// Ask the guest to give memory back (or take it again): the balloon
    /// driver in the guest follows the target.
    pub async fn set_balloon(&self, id: &str, target_mb: u64) -> Result<BalloonInfo> {
        let (cfg, sock, running) = self.qmp_path_of(id).await?;
        if !cfg.hardware.balloon {
            return Err(Error::Validation(
                "this VM has no balloon device: enable \"Memory ballooning\" in its hardware and restart it".into(),
            ));
        }
        if !running {
            return Err(Error::Conflict("the VM is not running".into()));
        }
        check_balloon_target(target_mb, cfg.memory)?;
        qmp_command(
            &sock,
            json!({"execute": "balloon", "arguments": {"value": target_mb * 1024 * 1024}}),
        )
        .await?;
        targets().lock().unwrap().insert(id.to_string(), target_mb);
        self.balloon_info(id).await
    }

    pub async fn agent_info(&self, id: &str) -> Result<AgentInfo> {
        let (cfg, _, running) = self.qmp_path_of(id).await?;
        let vm = self.qemu_vm(id).await?;
        let sock = DisplayRuntime::qga_socket(&vm.disk_path);
        let mut info = AgentInfo {
            enabled: cfg.hardware.guest_agent,
            running,
            ..Default::default()
        };
        if !cfg.hardware.guest_agent {
            info.error = Some("the guest agent channel is not enabled for this VM".into());
            return Ok(info);
        }
        if !running {
            info.error = Some("the VM is not running".into());
            return Ok(info);
        }
        match qga_exec(&sock, "guest-info", json!({}), 4).await {
            Err(e) => {
                info.error = Some(e.to_string());
                return Ok(info);
            }
            Ok(v) => {
                info.available = true;
                let (ver, cmds) = parse_guest_info(&v);
                info.version = ver;
                info.commands = cmds;
            }
        }
        if let Ok(v) = qga_exec(&sock, "guest-get-osinfo", json!({}), 5).await {
            info.os_name = v["pretty-name"]
                .as_str()
                .or(v["name"].as_str())
                .map(String::from);
            info.os_version = v["version"].as_str().map(String::from);
            info.kernel = v["kernel-release"].as_str().map(String::from);
        }
        if let Ok(v) = qga_exec(&sock, "guest-get-host-name", json!({}), 5).await {
            info.hostname = v["host-name"].as_str().map(String::from);
        }
        if let Ok(v) = qga_exec(&sock, "guest-network-get-interfaces", json!({}), 5).await {
            info.interfaces = parse_interfaces(&v);
        }
        Ok(info)
    }

    pub async fn agent_action(&self, id: &str, action: &str) -> Result<Value> {
        let (cmd, args) = agent_command(action)
            .ok_or_else(|| Error::Validation(format!("unknown agent action '{action}'")))?;
        let (cfg, _, running) = self.qmp_path_of(id).await?;
        if !cfg.hardware.guest_agent {
            return Err(Error::Validation(
                "the guest agent channel is not enabled for this VM".into(),
            ));
        }
        if !running {
            return Err(Error::Conflict("the VM is not running".into()));
        }
        let vm = self.qemu_vm(id).await?;
        let sock = DisplayRuntime::qga_socket(&vm.disk_path);
        let value = if action == "shutdown" || action == "reboot" {
            // The agent acts instead of answering: prove it is there first,
            // then fire and don't wait for a reply that never comes.
            qga_exec(&sock, "guest-ping", json!({}), 5).await?;
            let _ = qga_exec(&sock, cmd, args, 2).await;
            Value::Null
        } else {
            qga_exec(&sock, cmd, args, if action == "fstrim" { 120 } else { 15 }).await?
        };
        if action == "fsfreeze" {
            // never leave a guest frozen for good
            let id = id.to_string();
            let sock = sock.clone();
            tokio::spawn(async move {
                tokio::time::sleep(MAX_FREEZE).await;
                if let Ok(v) = qga_exec(&sock, "guest-fsfreeze-status", json!({}), 10).await {
                    if v.as_str() == Some("frozen") {
                        tracing::warn!("VM {id}: thawing a guest frozen for over {MAX_FREEZE:?}");
                        let _ = qga_exec(&sock, "guest-fsfreeze-thaw", json!({}), 30).await;
                    }
                }
            });
        }
        Ok(json!({"action": action, "result": value}))
    }

    /// Passed-through USB devices: what the host has, and add/remove on a VM
    /// (hot-plugged when the VM runs).
    pub async fn attach_usb(&self, id: &str, dev: UsbDevice) -> Result<VmConfig> {
        let dev = dev.normalized().map_err(Error::Validation)?;
        let present = host_usb_devices()
            .into_iter()
            .find(|h| h.vendor_id == dev.vendor_id && h.product_id == dev.product_id);
        match present {
            None => {
                return Err(Error::NotFound(format!(
                    "no USB device {}:{} is plugged into this host",
                    dev.vendor_id, dev.product_id
                )))
            }
            Some(h) if h.is_hub => {
                return Err(Error::Validation(
                    "a USB hub can't be passed through".into(),
                ))
            }
            _ => {}
        }
        let (cfg, sock, running) = self.qmp_path_of(id).await?;
        if cfg.hardware.usb.contains(&dev) {
            return Err(Error::Conflict(format!(
                "USB device {}:{} is already assigned to this VM",
                dev.vendor_id, dev.product_id
            )));
        }
        if cfg.hardware.usb.len() >= horcrux_common::MAX_VM_USB {
            return Err(Error::Validation(format!(
                "a VM can have at most {} USB devices",
                horcrux_common::MAX_VM_USB
            )));
        }
        if running {
            let ctrl = qmp_command(
                &sock,
                json!({"execute": "qom-list", "arguments": {"path": "/machine/peripheral"}}),
            )
            .await?;
            if !ctrl
                .as_array()
                .is_some_and(|l| l.iter().any(|p| p["name"] == "hxusb"))
            {
                return Err(Error::Conflict(
                    "this VM was started without a USB controller: enable \"USB controller\" in its hardware and restart it".into(),
                ));
            }
            qmp_command(
                &sock,
                json!({"execute": "device_add", "arguments": {
                    "driver": "usb-host", "bus": "hxusb.0",
                    "vendorid": u32::from_str_radix(&dev.vendor_id, 16).unwrap_or(0),
                    "productid": u32::from_str_radix(&dev.product_id, 16).unwrap_or(0),
                    "id": usb_device_id(&dev)}}),
            )
            .await?;
        }
        self.persist_usb(id, |l| l.push(dev)).await
    }

    pub async fn detach_usb(&self, id: &str, dev: UsbDevice) -> Result<VmConfig> {
        let dev = dev.normalized().map_err(Error::Validation)?;
        let (cfg, sock, running) = self.qmp_path_of(id).await?;
        if !cfg.hardware.usb.contains(&dev) {
            return Err(Error::NotFound(format!(
                "USB device {}:{} is not assigned to this VM",
                dev.vendor_id, dev.product_id
            )));
        }
        if running {
            qmp_command(
                &sock,
                json!({"execute": "device_del", "arguments": {"id": usb_device_id(&dev)}}),
            )
            .await?;
        }
        self.persist_usb(id, |l| l.retain(|d| d != &dev)).await
    }

    async fn persist_usb(&self, id: &str, f: impl FnOnce(&mut Vec<UsbDevice>)) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;
        self.hydrate(&mut vms, id).await?;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| Error::VmNotFound(id.to_string()))?;
        if vm.hypervisor != VmHypervisor::Qemu {
            return Err(Error::Validation("USB passthrough is for QEMU VMs".into()));
        }
        f(&mut vm.hardware.usb);
        vm.hardware.validate().map_err(Error::Validation)?;
        let config = vm.to_config();
        if let Some(db) = &self.db {
            db.update_vm(&config).await?;
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::VmHardware;
    use tokio::net::UnixListener;

    #[test]
    fn balloon_targets_are_bounded() {
        assert!(check_balloon_target(512, 1024).is_ok());
        assert!(check_balloon_target(1024, 1024).is_ok());
        assert!(check_balloon_target(8, 1024).is_err());
        assert!(check_balloon_target(2048, 1024).is_err());
    }

    #[test]
    fn qemu_gets_the_balloon_agent_and_usb_devices() {
        let d = Path::new("/vms/a.qcow2");
        assert!(
            super::super::qemu::device_args(&VmHardware::default(), d).is_empty(),
            "nothing by default"
        );
        let hw = VmHardware {
            balloon: true,
            guest_agent: true,
            usb: vec![UsbDevice {
                vendor_id: "046d".into(),
                product_id: "c52b".into(),
            }],
            ..Default::default()
        };
        let a = super::super::qemu::device_args(&hw, d).join(" ");
        assert!(a.contains("-device virtio-balloon-pci,id=balloon0"));
        assert!(a.contains("-chardev socket,id=qga0,path=/vms/a.qga.sock,server=on,wait=off"));
        assert!(a.contains("-device virtserialport,chardev=qga0,name=org.qemu.guest_agent.0"));
        assert!(a.contains("-device qemu-xhci,id=hxusb"));
        assert!(a.contains(
            "-device usb-host,bus=hxusb.0,vendorid=0x046d,productid=0xc52b,id=hxusb_046d_c52b"
        ));
        let ctrl = VmHardware {
            usb_controller: true,
            ..Default::default()
        };
        assert_eq!(
            super::super::qemu::device_args(&ctrl, d),
            vec!["-device".to_string(), "qemu-xhci,id=hxusb".into()]
        );
    }

    #[test]
    fn agent_replies_become_rows() {
        let n = json!([
            {"name": "lo", "ip-addresses": [{"ip-address": "127.0.0.1", "ip-address-type": "ipv4", "prefix": 8}]},
            {"name": "eth0", "hardware-address": "52:54:00:12:34:56", "ip-addresses": [
                {"ip-address": "10.0.2.15", "ip-address-type": "ipv4", "prefix": 24},
                {"ip-address": "fe80::1", "ip-address-type": "ipv6", "prefix": 64}]}
        ]);
        let v = parse_interfaces(&n);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].mac.as_deref(), Some("52:54:00:12:34:56"));
        assert_eq!(
            v[0].addresses,
            vec!["10.0.2.15/24".to_string(), "fe80::1/64".into()]
        );
        let (ver, cmds) = parse_guest_info(&json!({"version": "8.2.0", "supported_commands": [
            {"name": "guest-ping", "enabled": true}, {"name": "guest-exec", "enabled": false}]}));
        assert_eq!(ver.as_deref(), Some("8.2.0"));
        assert_eq!(cmds, vec!["guest-ping".to_string()]);
        assert!(agent_command("fsfreeze").is_some() && agent_command("rm -rf").is_none());
    }

    #[test]
    fn host_usb_comes_from_sysfs_and_hubs_are_marked() {
        let d = tempfile::tempdir().unwrap();
        let w = |dev: &str, f: &str, t: &str| {
            let p = d.path().join("sys/bus/usb/devices").join(dev);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join(f), t).unwrap();
        };
        for (dev, v, p, cls, prod) in [
            ("1-2", "046D", "C52B", "00", "USB Receiver"),
            ("1-1", "1d6b", "0002", "09", "Hub"),
        ] {
            w(dev, "idVendor", &format!("{v}\\n"));
            w(dev, "idProduct", &format!("{p}\\n"));
            w(dev, "bDeviceClass", cls);
            w(dev, "product", prod);
            w(dev, "busnum", "1");
            w(dev, "devnum", if dev == "1-2" { "4" } else { "2" });
            w(dev, "speed", "12");
        }
        w("usb1", "idVendor", "1d6b"); // root hub entry: skipped
        w("1-2:1.0", "bInterfaceClass", "03"); // interface: skipped
        let v = host_usb_devices_in(d.path());
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].port.as_str(), v[0].is_hub), ("1-1", true));
        assert_eq!(
            (
                v[1].vendor_id.as_str(),
                v[1].product_id.as_str(),
                v[1].product.as_deref()
            ),
            ("046d", "c52b", Some("USB Receiver"))
        );
        assert_eq!(v[1].speed_mbps, Some(12));
        assert!(host_usb_devices_in(&d.path().join("none")).is_empty());
    }

    /// A fake agent socket that speaks the QGA wire format: stale bytes
    /// first, then the delimited sync reply, then the command reply.
    #[tokio::test]
    async fn the_qga_client_syncs_then_runs_a_command_and_reports_agent_errors() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("a.qga.sock");
        let l = UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = l.accept().await else { break };
                tokio::spawn(async move {
                    let (r, mut w) = s.into_split();
                    let mut r = BufReader::new(r);
                    // a leftover reply from an earlier session
                    w.write_all(b"{\"return\": 1}\n").await.unwrap();
                    loop {
                        let mut line = Vec::new();
                        if r.read_until(b'\n', &mut line).await.unwrap_or(0) == 0 {
                            break;
                        }
                        let text = String::from_utf8_lossy(&line);
                        let v: Value =
                            serde_json::from_str(text.trim_start_matches('\u{fffd}').trim())
                                .unwrap_or(Value::Null);
                        match v["execute"].as_str() {
                            Some("guest-sync-delimited") => {
                                let id = v["arguments"]["id"].clone();
                                let mut out = vec![0xFFu8];
                                out.extend(json!({"return": id}).to_string().into_bytes());
                                out.push(b'\n');
                                w.write_all(&out).await.unwrap();
                            }
                            Some("guest-get-host-name") => {
                                w.write_all(b"{\"return\": {\"host-name\": \"guest1\"}}\n")
                                    .await
                                    .unwrap();
                            }
                            Some(_) => {
                                w.write_all(b"{\"error\": {\"class\": \"CommandNotFound\", \"desc\": \"command is disabled\"}}\n").await.unwrap();
                            }
                            None => {}
                        }
                    }
                });
            }
        });
        let r = qga_exec(&sock, "guest-get-host-name", json!({}), 5)
            .await
            .unwrap();
        assert_eq!(r["host-name"], "guest1");
        let e = qga_exec(&sock, "guest-exec", json!({}), 5)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("command is disabled"), "{e}");
        let missing = qga_exec(&dir.path().join("none.sock"), "guest-ping", json!({}), 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(missing.contains("no guest agent channel"), "{missing}");
    }

    /// The real `qemu-ga` binary (apt: qemu-guest-agent) as the other end of
    /// the socket, for wire-format parity with the real agent. Read-only
    /// commands only: nothing here may freeze or stop the machine running the
    /// tests.
    #[tokio::test]
    async fn the_qga_client_talks_to_the_real_qemu_ga() {
        let Some(bin) = [
            "/usr/sbin/qemu-ga",
            "/usr/bin/qemu-ga",
            "/usr/local/bin/qemu-ga",
        ]
        .into_iter()
        .find(|p| Path::new(p).is_file()) else {
            eprintln!("SKIPPED: qemu-ga is not installed (apt install qemu-guest-agent)");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("real.qga.sock");
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let mut agent = tokio::process::Command::new(bin)
            .args(["-m", "unix-listen", "-p"])
            .arg(&sock)
            .arg("-t")
            .arg(&state)
            .arg("-l")
            .arg(dir.path().join("qga.log"))
            .arg("-f")
            .arg(dir.path().join("qga.pid"))
            .arg("-b")
            .arg("guest-shutdown,guest-fsfreeze-freeze,guest-fsfreeze-thaw,guest-file-open,guest-exec,guest-set-time,guest-set-user-password,guest-suspend-disk,guest-suspend-ram,guest-suspend-hybrid,guest-fstrim")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        for _ in 0..50 {
            if sock.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(sock.exists(), "qemu-ga did not create its socket");
        assert_eq!(
            qga_exec(&sock, "guest-ping", json!({}), 10).await.unwrap(),
            json!({})
        );
        let info = qga_exec(&sock, "guest-info", json!({}), 10).await.unwrap();
        let (ver, cmds) = parse_guest_info(&info);
        assert!(ver.is_some_and(|v| !v.is_empty()));
        assert!(cmds.contains(&"guest-ping".to_string()), "{cmds:?}");
        assert!(
            !cmds.contains(&"guest-fsfreeze-freeze".to_string()),
            "blocked commands are reported disabled"
        );
        let host = qga_exec(&sock, "guest-get-host-name", json!({}), 10)
            .await
            .unwrap();
        assert_eq!(
            host["host-name"].as_str().unwrap(),
            std::fs::read_to_string("/proc/sys/kernel/hostname")
                .unwrap()
                .trim()
        );
        let os = qga_exec(&sock, "guest-get-osinfo", json!({}), 10)
            .await
            .unwrap();
        assert!(
            os["kernel-release"].as_str().is_some_and(|k| !k.is_empty()),
            "{os}"
        );
        let nets = qga_exec(&sock, "guest-network-get-interfaces", json!({}), 10)
            .await
            .unwrap();
        assert!(nets.is_array());
        let _ = parse_interfaces(&nets);
        // a command the agent refuses comes back as an error with the agent's words
        let e = qga_exec(&sock, "guest-fsfreeze-freeze", json!({}), 10)
            .await
            .unwrap_err()
            .to_string();
        assert!(!e.is_empty(), "the agent's refusal is reported");
        // the client resyncs on a fresh connection too
        assert_eq!(
            qga_exec(&sock, "guest-ping", json!({}), 10).await.unwrap(),
            json!({})
        );
        let _ = agent.kill().await;
    }

    #[tokio::test]
    async fn a_silent_guest_times_out_with_a_helpful_message() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("q.sock");
        let l = UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            // accept and say nothing: a guest without the agent
            let (_s, _) = l.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
        });
        let e = qga_exec(&sock, "guest-ping", json!({}), 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("qemu-guest-agent"), "{e}");
    }
}
