//! Compare the QEMU command line a Proxmox guest runs with (`qm showcmd`)
//! against the one Horcrux will run for the imported VM: the facts that
//! decide what the guest sees (machine, firmware, TPM, NIC models and MACs,
//! disk models, throttles, pointer, clock, identity) are pulled out of both
//! and listed side by side.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Split `qm showcmd` output (or any shell command line) into words:
/// single/double quotes, backslash-newline continuations.
pub fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\'' => {
                have = true;
                for d in it.by_ref() {
                    if d == '\'' {
                        break;
                    }
                    cur.push(d);
                }
            }
            '"' => {
                have = true;
                while let Some(d) = it.next() {
                    match d {
                        '"' => break,
                        '\\' => {
                            if let Some(e) = it.next() {
                                cur.push(e);
                            }
                        }
                        _ => cur.push(d),
                    }
                }
            }
            '\\' => {
                if let Some(&n) = it.peek() {
                    it.next();
                    if n != '\n' {
                        cur.push(n);
                        have = true;
                    }
                }
            }
            c if c.is_whitespace() => {
                if have {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            c => {
                cur.push(c);
                have = true;
            }
        }
    }
    if have {
        out.push(cur);
    }
    out
}

/// Pairs of `-opt value` (flags without a value are skipped by the readers).
fn opts(argv: &[String]) -> Vec<(&str, &str)> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        if a.starts_with('-') && i + 1 < argv.len() && !argv[i + 1].starts_with('-') {
            v.push((a, argv[i + 1].as_str()));
            i += 2;
        } else {
            i += 1;
        }
    }
    v
}

fn kv(s: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for (i, p) in s.split(',').enumerate() {
        match p.split_once('=') {
            Some((a, b)) => {
                m.insert(a.to_string(), b.to_string());
            }
            None if i == 0 => {
                m.insert(String::new(), p.to_string());
            }
            None => {
                m.insert(p.to_string(), String::new());
            }
        }
    }
    m
}

const NIC_MODELS: [&str; 5] = ["virtio-net-pci", "e1000", "e1000e", "rtl8139", "vmxnet3"];

/// The guest-visible facts of a QEMU command line.
pub fn facts(argv: &[String]) -> BTreeMap<String, String> {
    let mut f: BTreeMap<String, String> = BTreeMap::new();
    let mut nics = 0;
    let mut disks: Vec<String> = Vec::new();
    let mut throttles: Vec<String> = Vec::new();
    let mut hostpci: Vec<String> = Vec::new();
    let mut serial = 0;
    let mut discard = 0;
    let mut iothreads = 0;
    let mut pflash = 0;
    f.insert("firmware".into(), "seabios".into());
    f.insert("tablet".into(), "no".into());
    f.insert("clock".into(), "utc".into());
    f.insert("guest-agent".into(), "no".into());
    f.insert("balloon".into(), "no".into());
    f.insert("tpm".into(), "none".into());
    f.insert("machine".into(), "i440fx".into());
    f.insert("video".into(), "none".into());
    for (o, v) in opts(argv) {
        match o {
            "-machine" | "-M" => {
                let m = kv(v);
                let t = m.get("type").or(m.get("")).cloned().unwrap_or_default();
                if t.contains("q35") {
                    f.insert("machine".into(), "q35".into());
                }
                if m.contains_key("pflash0") {
                    pflash += 1;
                }
            }
            "-cpu" => {
                f.insert("cpu".into(), v.split(',').next().unwrap_or_default().into());
            }
            "-m" => {
                let m = kv(v);
                let s = m.get("size").or(m.get("")).cloned().unwrap_or_default();
                let n: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
                let mb = match s.chars().last() {
                    Some('G') | Some('g') => n.parse::<u64>().unwrap_or(0) * 1024,
                    _ => n.parse::<u64>().unwrap_or(0),
                };
                f.insert("memory-mb".into(), mb.to_string());
            }
            "-smp" => {
                let m = kv(v);
                let n = m.get("").or(m.get("cpus")).cloned().unwrap_or_default();
                let s = m.get("sockets").cloned().unwrap_or_else(|| "1".into());
                f.insert("cpus".into(), format!("{n} (sockets {s})"));
            }
            "-smbios" => {
                let m = kv(v);
                if m.get("type").map(String::as_str) == Some("1") {
                    if let Some(u) = m.get("uuid") {
                        f.insert("smbios-uuid".into(), u.to_ascii_lowercase());
                    }
                }
            }
            "-rtc" => {
                if v.contains("base=localtime") {
                    f.insert("clock".into(), "localtime".into());
                }
            }
            "-bios" => {}
            "-object" => {
                if v.starts_with("iothread,") {
                    iothreads += 1;
                }
                if v.starts_with('{') {
                    if let Ok(j) = serde_json::from_str::<serde_json::Value>(v) {
                        if j["qom-type"] == "throttle-group" {
                            if let Some(t) = throttle_of(&j["limits"]) {
                                throttles.push(t);
                            }
                        }
                    }
                }
            }
            "-drive" => {
                let m = kv(v);
                if m.get("if").map(String::as_str) == Some("pflash") {
                    pflash += 1;
                }
                if m.get("media").map(String::as_str) != Some("cdrom")
                    && m.get("if").map(String::as_str) != Some("pflash")
                {
                    if m.get("discard").map(String::as_str) == Some("unmap") {
                        discard += 1;
                    }
                    let t = serde_json::json!({
                        "bps-read": m.get("throttling.bps-read").and_then(|x| x.parse::<u64>().ok()),
                        "bps-write": m.get("throttling.bps-write").and_then(|x| x.parse::<u64>().ok()),
                        "iops-read": m.get("throttling.iops-read").and_then(|x| x.parse::<u64>().ok()),
                        "iops-write": m.get("throttling.iops-write").and_then(|x| x.parse::<u64>().ok()),
                    });
                    if let Some(t) = throttle_of(&t) {
                        throttles.push(t);
                    }
                }
            }
            "-blockdev" => {
                if let Ok(j) = serde_json::from_str::<serde_json::Value>(v) {
                    let name = j["node-name"].as_str().unwrap_or_default();
                    if name.starts_with("drive-")
                        && !name.contains("efidisk")
                        && !name.contains("cloudinit")
                        && j["read-only"] != true
                        && j["discard"] == "unmap"
                    {
                        discard += 1;
                    }
                }
            }
            "-device" => {
                let m = kv(v);
                let dev = m.get("").cloned().unwrap_or_default();
                if NIC_MODELS.contains(&dev.as_str()) {
                    let mac = m
                        .get("mac")
                        .cloned()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    f.insert(format!("net{nics}"), format!("{dev} {mac}"));
                    nics += 1;
                }
                match dev.as_str() {
                    "scsi-hd" | "virtio-blk-pci" | "ide-hd" => {
                        let ssd = if m.get("rotation_rate").map(String::as_str) == Some("1") {
                            " ssd"
                        } else {
                            ""
                        };
                        let bus = match dev.as_str() {
                            "scsi-hd" => "scsi",
                            "virtio-blk-pci" => "virtio",
                            _ if m.get("bus").is_some_and(|b| {
                                b.starts_with("ahci") || b.starts_with("sata")
                            }) =>
                            {
                                "sata"
                            }
                            _ => "ide",
                        };
                        disks.push(format!("{bus}{ssd}"));
                    }
                    "usb-tablet" => {
                        f.insert("tablet".into(), "yes".into());
                    }
                    "tpm-tis" | "tpm-crb" => {
                        f.insert("tpm".into(), dev.trim_start_matches("tpm-").into());
                    }
                    "vmgenid" => {
                        f.insert(
                            "vmgenid".into(),
                            m.get("guid")
                                .cloned()
                                .unwrap_or_default()
                                .to_ascii_lowercase(),
                        );
                    }
                    "virtserialport" => {
                        if m.get("name").map(String::as_str) == Some("org.qemu.guest_agent.0") {
                            f.insert("guest-agent".into(), "yes".into());
                        }
                    }
                    "isa-serial" | "pci-serial" => serial += 1,
                    "vfio-pci" => {
                        if let Some(h) = m.get("host") {
                            let h = h.to_ascii_lowercase();
                            hostpci.push(if h.matches(':').count() == 1 {
                                format!("0000:{h}")
                            } else {
                                h
                            });
                        }
                    }
                    d if d.starts_with("virtio-balloon") => {
                        f.insert("balloon".into(), "yes".into());
                    }
                    "VGA" | "vga" => {
                        f.insert("video".into(), "std".into());
                    }
                    "virtio-vga" | "virtio-gpu-pci" | "virtio-vga-gl" => {
                        f.insert("video".into(), "virtio".into());
                    }
                    "qxl-vga" => {
                        f.insert("video".into(), "qxl".into());
                    }
                    _ => {}
                }
            }
            "-vga" => {
                f.insert(
                    "video".into(),
                    match v {
                        "std" => "std",
                        "virtio" => "virtio",
                        "qxl" => "qxl",
                        "none" => "none",
                        o => o,
                    }
                    .into(),
                );
            }
            "-serial" if v != "none" => serial += 1,
            _ => {}
        }
    }
    if pflash > 0 {
        f.insert("firmware".into(), "ovmf".into());
    }
    for (i, d) in disks.iter().enumerate() {
        f.insert(format!("disk{i}"), d.clone());
    }
    throttles.sort();
    hostpci.sort();
    f.insert("disk-throttles".into(), throttles.join("; "));
    f.insert("disks-with-discard".into(), discard.to_string());
    f.insert("iothreads".into(), iothreads.to_string());
    f.insert("serial-ports".into(), serial.to_string());
    f.insert("hostpci".into(), hostpci.join(" "));
    f
}

fn throttle_of(l: &serde_json::Value) -> Option<String> {
    let mut parts = vec![];
    for k in ["bps-read", "bps-write", "iops-read", "iops-write"] {
        if let Some(n) = l[k].as_u64().filter(|n| *n > 0) {
            parts.push(format!("{k}={n}"));
        }
    }
    (!parts.is_empty()).then(|| parts.join(","))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffRow {
    pub fact: String,
    pub proxmox: String,
    pub horcrux: String,
    pub same: bool,
    /// Why a difference is expected (empty = an unexplained difference).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// Side-by-side facts; `expected` explains known, deliberate differences
/// (fact -> reason, e.g. a Proxmox CPU flag Horcrux does not add).
pub fn diff(
    pve: &BTreeMap<String, String>,
    hx: &BTreeMap<String, String>,
    expected: &BTreeMap<String, String>,
) -> Vec<DiffRow> {
    let mut keys: Vec<&String> = pve.keys().chain(hx.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .map(|k| {
            let a = pve.get(k).cloned().unwrap_or_default();
            let b = hx.get(k).cloned().unwrap_or_default();
            DiffRow {
                fact: k.clone(),
                same: a == b,
                note: if a == b {
                    String::new()
                } else {
                    expected.get(k).cloned().unwrap_or_default()
                },
                proxmox: a,
                horcrux: b,
            }
        })
        .collect()
}

/// The `===== VM <id>` blocks of a saved multi-guest `qm showcmd` dump.
pub fn split_showcmd_dump(text: &str) -> BTreeMap<u32, String> {
    let mut out = BTreeMap::new();
    let mut cur: Option<u32> = None;
    let mut buf = String::new();
    for l in text.lines() {
        if let Some(id) = l
            .strip_prefix("===== VM ")
            .and_then(|r| r.trim().parse::<u32>().ok())
        {
            if let Some(c) = cur.take() {
                out.insert(c, std::mem::take(&mut buf));
            }
            cur = Some(id);
            continue;
        }
        if cur.is_some() {
            buf.push_str(l);
            buf.push('\n');
        }
    }
    if let Some(c) = cur {
        out.insert(c, buf);
    }
    out
}
