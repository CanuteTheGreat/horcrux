//! Opt-in port mapping for the console relay (`[console] wan.upnp`): asks
//! the router to forward the relay port, first with NAT-PMP (RFC 6886,
//! answered by most routers that also speak PCP), then with UPnP IGD
//! (SSDP discovery + WANIPConnection/WANPPPConnection SOAP). The mapping is
//! renewed at half its lease and removed when WAN access or UPnP is turned
//! off. Clean-room from the RFC and the UPnP Forum IGD specifications; no
//! third-party code or service.
use horcrux_common::console::WanSettings;
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::task::JoinHandle;

const NATPMP_PORT: u16 = 5351;

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct MappingStatus {
    pub enabled: bool,
    /// `nat-pmp` or `upnp` once a mapping exists.
    pub method: Option<String>,
    pub external_address: Option<String>,
    pub external_port: Option<u16>,
    pub lease_secs: u32,
    pub last_renewed: Option<String>,
    pub error: Option<String>,
}

struct Task {
    key: (u16, bool, u32, String, String),
    handle: JoinHandle<()>,
}

static STATUS: Mutex<Option<MappingStatus>> = Mutex::new(None);
static TASK: Mutex<Option<Task>> = Mutex::new(None);

pub fn status() -> MappingStatus {
    STATUS
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_default()
}

fn set_status(f: impl FnOnce(&mut MappingStatus)) {
    if let Ok(mut g) = STATUS.lock() {
        let s = g.get_or_insert_with(MappingStatus::default);
        f(s);
    }
}

/// The default route's gateway (IPv4) from /proc/net/route.
pub fn default_gateway() -> Option<Ipv4Addr> {
    let t = std::fs::read_to_string("/proc/net/route").ok()?;
    for line in t.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() > 2 && f[1] == "00000000" {
            let g = u32::from_str_radix(f[2], 16).ok()?;
            if g != 0 {
                return Some(Ipv4Addr::from(u32::from_be(g)));
            }
        }
    }
    None
}

fn gateway(w: &WanSettings) -> Option<SocketAddr> {
    if let Ok(a) = w.upnp_gateway.parse::<SocketAddr>() {
        return Some(a);
    }
    if let Ok(ip) = w.upnp_gateway.parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, NATPMP_PORT));
    }
    default_gateway().map(|g| SocketAddr::new(IpAddr::V4(g), NATPMP_PORT))
}

/// Start, keep or stop the mapping task to match `w`.
pub async fn apply(w: &WanSettings) {
    let want = w.enabled && w.upnp;
    let key = (
        w.relay_port,
        w.relay_tcp,
        w.upnp_lease_secs,
        w.upnp_gateway.clone(),
        w.upnp_ssdp_address.clone(),
    );
    let old = {
        let Ok(mut g) = TASK.lock() else { return };
        if want && g.as_ref().map(|t| t.key == key).unwrap_or(false) {
            return;
        }
        g.take()
    };
    if let Some(t) = old {
        t.handle.abort();
        // remove what the old task mapped (best effort)
        let st = status();
        if let Some(m) = st.method.as_deref() {
            let (port, tcp, _, gw, ssdp) = t.key;
            let w2 = WanSettings {
                relay_port: port,
                relay_tcp: tcp,
                upnp_gateway: gw,
                upnp_ssdp_address: ssdp,
                ..Default::default()
            };
            let _ = tokio::time::timeout(Duration::from_secs(5), unmap(&w2, m, st.external_port))
                .await;
        }
    }
    if let Ok(mut g) = STATUS.lock() {
        *g = Some(MappingStatus {
            enabled: want,
            ..Default::default()
        });
    }
    if !want {
        return;
    }
    let w = w.clone();
    let handle = tokio::spawn(async move { keep_mapped(w).await });
    if let Ok(mut g) = TASK.lock() {
        *g = Some(Task { key, handle });
    }
}

async fn keep_mapped(w: WanSettings) {
    loop {
        let res = map_once(&w).await;
        let wait = match res {
            Ok(m) => {
                let lease = m.lease_secs.max(120);
                tracing::info!(
                    "console relay port mapping via {}: {}:{} (lease {lease} s)",
                    m.method.as_deref().unwrap_or("?"),
                    m.external_address.as_deref().unwrap_or("?"),
                    m.external_port.unwrap_or(0)
                );
                set_status(|s| {
                    *s = MappingStatus {
                        enabled: true,
                        last_renewed: Some(chrono::Utc::now().to_rfc3339()),
                        ..m
                    }
                });
                Duration::from_secs(lease as u64 / 2)
            }
            Err(e) => {
                tracing::warn!("console relay port mapping failed: {e}");
                set_status(|s| {
                    s.enabled = true;
                    s.error = Some(e);
                });
                Duration::from_secs(60)
            }
        };
        tokio::time::sleep(wait).await;
    }
}

async fn map_once(w: &WanSettings) -> Result<MappingStatus, String> {
    let mut errors = Vec::new();
    match natpmp_map(w, w.upnp_lease_secs).await {
        Ok(m) => return Ok(m),
        Err(e) => errors.push(format!("NAT-PMP: {e}")),
    }
    match upnp_map(w).await {
        Ok(m) => return Ok(m),
        Err(e) => errors.push(format!("UPnP: {e}")),
    }
    Err(errors.join("; "))
}

async fn unmap(w: &WanSettings, method: &str, external: Option<u16>) -> Result<(), String> {
    match method {
        "nat-pmp" => natpmp_map(w, 0).await.map(|_| ()),
        _ => upnp_unmap(w, external.unwrap_or(w.relay_port)).await,
    }
}

// ---------------- NAT-PMP (RFC 6886) ----------------

async fn natpmp_request(gw: SocketAddr, req: &[u8], expect_op: u8) -> Result<Vec<u8>, String> {
    let s = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    let mut wait = Duration::from_millis(250);
    let mut buf = [0u8; 64];
    // RFC 6886 3.1: 250 ms doubling; give up after ~2 s here (UPnP next)
    for _ in 0..4 {
        s.send_to(req, gw).await.map_err(|e| e.to_string())?;
        if let Ok(Ok((n, from))) = tokio::time::timeout(wait, s.recv_from(&mut buf)).await {
            if from.ip() == gw.ip() && n >= 8 && buf[0] == 0 && buf[1] == 128 + expect_op {
                let rc = u16::from_be_bytes([buf[2], buf[3]]);
                if rc != 0 {
                    return Err(format!("result code {rc}"));
                }
                return Ok(buf[..n].to_vec());
            }
        }
        wait *= 2;
    }
    Err(format!("no answer from {gw}"))
}

async fn natpmp_map(w: &WanSettings, lease: u32) -> Result<MappingStatus, String> {
    let gw = gateway(w).ok_or("no default gateway")?;
    let ext = natpmp_request(gw, &[0, 0], 0).await?;
    if ext.len() < 12 {
        return Err("short external-address answer".into());
    }
    let ext_ip = Ipv4Addr::new(ext[8], ext[9], ext[10], ext[11]);
    let mut external_port = None;
    let mut granted = lease;
    let mut ops = vec![1u8];
    if w.relay_tcp {
        ops.push(2);
    }
    for op in ops {
        let mut req = vec![0, op, 0, 0];
        req.extend_from_slice(&w.relay_port.to_be_bytes());
        // suggested external port: the same (0 when deleting)
        let sugg = if lease == 0 { 0 } else { w.relay_port };
        req.extend_from_slice(&sugg.to_be_bytes());
        req.extend_from_slice(&lease.to_be_bytes());
        let r = natpmp_request(gw, &req, op).await?;
        if r.len() < 16 {
            return Err("short mapping answer".into());
        }
        let ep = u16::from_be_bytes([r[10], r[11]]);
        granted = u32::from_be_bytes([r[12], r[13], r[14], r[15]]);
        // UDP decides the advertised port; TCP should match it
        external_port.get_or_insert(ep);
    }
    Ok(MappingStatus {
        enabled: true,
        method: Some("nat-pmp".into()),
        external_address: Some(ext_ip.to_string()),
        external_port,
        lease_secs: granted,
        ..Default::default()
    })
}

// ---------------- UPnP IGD ----------------

const IGD_SERVICES: [&str; 3] = [
    "urn:schemas-upnp-org:service:WANIPConnection:2",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];

struct Igd {
    control: SocketAddr,
    control_path: String,
    service: String,
    local_ip: IpAddr,
}

/// SSDP M-SEARCH for an Internet Gateway Device; returns LOCATION URLs.
async fn ssdp_search(w: &WanSettings) -> Result<Vec<String>, String> {
    let to: SocketAddr = w
        .upnp_ssdp_address
        .parse()
        .map_err(|_| "bad upnp_ssdp_address")?;
    let s = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for st in [
        "urn:schemas-upnp-org:device:InternetGatewayDevice:2",
        "urn:schemas-upnp-org:device:InternetGatewayDevice:1",
    ] {
        let msg = format!(
            "M-SEARCH * HTTP/1.1\r\nHOST: {to}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {st}\r\n\r\n"
        );
        let _ = s.send_to(msg.as_bytes(), to).await;
    }
    let mut buf = vec![0u8; 2048];
    let deadline = tokio::time::Instant::now() + Duration::from_millis(2500);
    while let Ok(Ok((n, _))) = tokio::time::timeout_at(deadline, s.recv_from(&mut buf)).await {
        let text = String::from_utf8_lossy(&buf[..n]);
        for line in text.lines() {
            if let Some((k, v)) = line.split_once(':') {
                if k.trim().eq_ignore_ascii_case("location") {
                    let v = v.trim().to_string();
                    if !out.contains(&v) {
                        out.push(v);
                    }
                }
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    if out.is_empty() {
        return Err("no Internet Gateway Device answered".into());
    }
    Ok(out)
}

/// `http://host:port/path` -> (address, path). Plain HTTP only (UPnP).
fn split_url(u: &str) -> Option<(SocketAddr, String)> {
    let rest = u.strip_prefix("http://")?;
    let (hp, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    let addr = if hp.contains(':') {
        hp.parse().ok()?
    } else {
        SocketAddr::new(hp.parse().ok()?, 80)
    };
    Some((addr, path))
}

/// One HTTP/1.1 exchange (Connection: close); returns (status, body).
async fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: &str,
) -> Result<(u16, String), String> {
    let fut = async {
        let mut s = TcpStream::connect(addr).await.map_err(|e| e.to_string())?;
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nUser-Agent: Horcrux/1 UPnP/1.1\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        s.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
        let mut resp = Vec::new();
        s.read_to_end(&mut resp).await.map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&resp).to_string();
        let code = text
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .ok_or("bad HTTP answer")?;
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
        Ok::<_, String>((code, body))
    };
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .map_err(|_| "HTTP timeout".to_string())?
}

fn xml_text<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let i = xml.find(&open)? + open.len();
    let j = xml[i..].find(&close)? + i;
    Some(xml[i..j].trim())
}

/// Find a WAN connection service and its control URL in a description.
fn find_service(desc: &str) -> Option<(String, String)> {
    for svc in IGD_SERVICES {
        let mut rest = desc;
        while let Some(i) = rest.find("<service>") {
            let block_end = rest[i..].find("</service>").map(|j| i + j)?;
            let block = &rest[i..block_end];
            if xml_text(block, "serviceType") == Some(svc) {
                return Some((svc.to_string(), xml_text(block, "controlURL")?.to_string()));
            }
            rest = &rest[block_end..];
        }
    }
    None
}

async fn discover_igd(w: &WanSettings) -> Result<Igd, String> {
    for loc in ssdp_search(w).await? {
        let Some((addr, path)) = split_url(&loc) else { continue };
        let Ok((200, desc)) = http(addr, "GET", &path, &[], "").await else {
            continue;
        };
        let Some((service, control)) = find_service(&desc) else {
            continue;
        };
        let (control, control_path) = if control.starts_with("http://") {
            split_url(&control).ok_or("bad controlURL")?
        } else {
            let p = if control.starts_with('/') { control } else { format!("/{control}") };
            (addr, p)
        };
        // our address as the router sees it
        let probe = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
        probe.connect(control).map_err(|e| e.to_string())?;
        let local_ip = probe.local_addr().map_err(|e| e.to_string())?.ip();
        return Ok(Igd {
            control,
            control_path,
            service,
            local_ip,
        });
    }
    Err("no WANIPConnection/WANPPPConnection service found".into())
}

async fn soap(igd: &Igd, action: &str, args: &[(&str, String)]) -> Result<String, String> {
    let mut a = String::new();
    for (k, v) in args {
        a.push_str(&format!("<{k}>{v}</{k}>"));
    }
    let body = format!(
        "<?xml version=\"1.0\"?>\r\n<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>\
         <u:{action} xmlns:u=\"{svc}\">{a}</u:{action}></s:Body></s:Envelope>",
        svc = igd.service
    );
    let (code, resp) = http(
        igd.control,
        "POST",
        &igd.control_path,
        &[
            ("Content-Type", "text/xml; charset=\"utf-8\"".into()),
            ("SOAPAction", format!("\"{}#{action}\"", igd.service)),
        ],
        &body,
    )
    .await?;
    if code != 200 {
        let e = xml_text(&resp, "errorDescription").unwrap_or("").to_string();
        return Err(format!("{action}: HTTP {code} {e}"));
    }
    Ok(resp)
}

async fn upnp_map(w: &WanSettings) -> Result<MappingStatus, String> {
    let igd = discover_igd(w).await?;
    let ext = soap(&igd, "GetExternalIPAddress", &[]).await?;
    let ext_ip = xml_text(&ext, "NewExternalIPAddress").unwrap_or("").to_string();
    let mut protos = vec!["UDP"];
    if w.relay_tcp {
        protos.push("TCP");
    }
    for p in protos {
        soap(
            &igd,
            "AddPortMapping",
            &[
                ("NewRemoteHost", String::new()),
                ("NewExternalPort", w.relay_port.to_string()),
                ("NewProtocol", p.into()),
                ("NewInternalPort", w.relay_port.to_string()),
                ("NewInternalClient", igd.local_ip.to_string()),
                ("NewEnabled", "1".into()),
                ("NewPortMappingDescription", "Horcrux console relay".into()),
                ("NewLeaseDuration", w.upnp_lease_secs.to_string()),
            ],
        )
        .await?;
    }
    Ok(MappingStatus {
        enabled: true,
        method: Some("upnp".into()),
        external_address: (!ext_ip.is_empty()).then_some(ext_ip),
        external_port: Some(w.relay_port),
        lease_secs: w.upnp_lease_secs,
        ..Default::default()
    })
}

async fn upnp_unmap(w: &WanSettings, external: u16) -> Result<(), String> {
    let igd = discover_igd(w).await?;
    let mut protos = vec!["UDP"];
    if w.relay_tcp {
        protos.push("TCP");
    }
    for p in protos {
        let _ = soap(
            &igd,
            "DeletePortMapping",
            &[
                ("NewRemoteHost", String::new()),
                ("NewExternalPort", external.to_string()),
                ("NewProtocol", p.into()),
            ],
        )
        .await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::net::TcpListener;

    /// A NAT-PMP router: external address 203.0.113.9, maps every request
    /// to external port +1000, records requests.
    async fn fake_natpmp() -> (SocketAddr, Arc<Mutex<Vec<Vec<u8>>>>) {
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let a = s.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let l2 = log.clone();
        tokio::spawn(async move {
            let mut b = [0u8; 64];
            while let Ok((n, from)) = s.recv_from(&mut b).await {
                l2.lock().unwrap().push(b[..n].to_vec());
                let op = b[1];
                let mut r = vec![0, 128 + op, 0, 0, 0, 0, 0, 1];
                if op == 0 {
                    r.extend_from_slice(&[203, 0, 113, 9]);
                } else {
                    r.extend_from_slice(&b[4..6]);
                    let ext = u16::from_be_bytes([b[4], b[5]]) + 1000;
                    r.extend_from_slice(&ext.to_be_bytes());
                    r.extend_from_slice(&b[8..12]);
                }
                let _ = s.send_to(&r, from).await;
            }
        });
        (a, log)
    }

    #[tokio::test]
    async fn natpmp_maps_udp_and_tcp_and_deletes() {
        let (gw, log) = fake_natpmp().await;
        let w = WanSettings {
            relay_port: 3478,
            upnp_gateway: gw.to_string(),
            ..Default::default()
        };
        let m = natpmp_map(&w, 3600).await.unwrap();
        assert_eq!(m.method.as_deref(), Some("nat-pmp"));
        assert_eq!(m.external_address.as_deref(), Some("203.0.113.9"));
        assert_eq!(m.external_port, Some(4478));
        assert_eq!(m.lease_secs, 3600);
        let ops: Vec<u8> = log.lock().unwrap().iter().map(|r| r[1]).collect();
        assert_eq!(ops, vec![0, 1, 2]);
        natpmp_map(&w, 0).await.unwrap();
        let last = log.lock().unwrap().last().unwrap().clone();
        assert_eq!(&last[8..12], &[0, 0, 0, 0], "lease 0 deletes");
    }

    /// A UPnP IGD: SSDP answer on a unicast socket, description + SOAP over
    /// HTTP; records SOAP actions.
    async fn fake_igd() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
        let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ha = http.local_addr().unwrap();
        let ssdp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sa = ssdp.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(async move {
            let mut b = [0u8; 1024];
            while let Ok((_, from)) = ssdp.recv_from(&mut b).await {
                let r = format!("HTTP/1.1 200 OK\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nLOCATION: http://{ha}/desc.xml\r\n\r\n");
                let _ = ssdp.send_to(r.as_bytes(), from).await;
            }
        });
        let l2 = log.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = http.accept().await {
                let mut buf = vec![0u8; 8192];
                let mut got = Vec::new();
                // read headers + body (Content-Length)
                loop {
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    if n == 0 { break; }
                    got.extend_from_slice(&buf[..n]);
                    let t = String::from_utf8_lossy(&got).to_string();
                    if let Some((h, b)) = t.split_once("\r\n\r\n") {
                        let cl = h.lines().find_map(|l| l.strip_prefix("Content-Length: ")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                        if b.len() >= cl { break; }
                    }
                }
                let t = String::from_utf8_lossy(&got).to_string();
                let body = if t.starts_with("GET /desc.xml") {
                    "<root><device><serviceList><service><serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType><controlURL>/l3</controlURL></service>\
                     <service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType><controlURL>/ctl/IPConn</controlURL></service></serviceList></device></root>".to_string()
                } else {
                    let action = t.lines().find_map(|l| l.strip_prefix("SOAPAction: ")).unwrap_or("").to_string();
                    l2.lock().unwrap().push(format!("{action} {}", t.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("")));
                    if action.contains("GetExternalIPAddress") {
                        "<s:Envelope><s:Body><u:GetExternalIPAddressResponse><NewExternalIPAddress>198.51.100.7</NewExternalIPAddress></u:GetExternalIPAddressResponse></s:Body></s:Envelope>".to_string()
                    } else {
                        "<s:Envelope><s:Body/></s:Envelope>".to_string()
                    }
                };
                let r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                let _ = s.write_all(r.as_bytes()).await;
            }
        });
        (sa, log)
    }

    #[tokio::test]
    async fn upnp_discovers_and_maps_both_protocols() {
        let (ssdp, log) = fake_igd().await;
        let w = WanSettings {
            relay_port: 3478,
            upnp_ssdp_address: ssdp.to_string(),
            upnp_lease_secs: 600,
            ..Default::default()
        };
        let m = upnp_map(&w).await.unwrap();
        assert_eq!(m.method.as_deref(), Some("upnp"));
        assert_eq!(m.external_address.as_deref(), Some("198.51.100.7"));
        assert_eq!(m.external_port, Some(3478));
        let l = log.lock().unwrap().clone();
        assert!(l[0].contains("WANIPConnection:1#GetExternalIPAddress"), "{l:?}");
        assert!(l[1].contains("#AddPortMapping") && l[1].contains("<NewProtocol>UDP</NewProtocol>"));
        assert!(l[1].contains("<NewInternalClient>127.0.0.1</NewInternalClient>"));
        assert!(l[1].contains("<NewLeaseDuration>600</NewLeaseDuration>"));
        assert!(l[2].contains("<NewProtocol>TCP</NewProtocol>"));
        upnp_unmap(&w, 3478).await.unwrap();
        let l = log.lock().unwrap().clone();
        assert!(l.iter().any(|x| x.contains("#DeletePortMapping")));
    }

    #[test]
    fn urls_and_xml() {
        assert_eq!(
            split_url("http://192.168.1.1:5000/rootDesc.xml"),
            Some(("192.168.1.1:5000".parse().unwrap(), "/rootDesc.xml".into()))
        );
        assert_eq!(split_url("https://x/"), None);
        assert_eq!(xml_text("<a><b> x </b></a>", "b"), Some("x"));
    }
}
