//! Real VM consoles against a running horcrux-api (CI's Test job starts one
//! on :8006 and installs qemu-system-x86; TCG is fine, no KVM needed).
//!
//! Boots tiny QEMU VMs through the API and speaks the actual wire
//! protocols to what QEMU serves:
//!   * VNC: RFB 3.8 handshake, VNC password authentication with the
//!     per-start password the API hands out, ServerInit framebuffer size,
//!     and the same banner through the ticketed WebSocket proxy.
//!   * SPICE: link handshake on the main channel, RSA-OAEP ticket auth,
//!     then the channel list must include playback + record (audio over
//!     SPICE) and usbredir; a wrong password must be refused.
//!   * Serial: on a headless VM SeaBIOS boots a tiny MBR program that
//!     prints to COM1 and echoes input; its output arrives through the
//!     WebSocket bridge, typed input reaches the guest and the echo comes
//!     back, and `<disk>.serial.log` still records it.
//!   * Video cards / sound / clipboard agent / USB redirection: checked in
//!     QEMU itself over QMP (query-pci, query-chardev, query-spice), not
//!     just in the API's answers.
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const API: &str = "https://localhost:8006/api";
const WS: &str = "wss://localhost:8006";

/// The API serves HTTPS with a certificate from the node-local CA.
fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap()
}

/// TLS for the console WebSockets, trusting the node's own CA
/// (`/api/tls/ca.pem`) the way an operator's browser does after importing it.
async fn wss_connector() -> tokio_tungstenite::Connector {
    static CFG: tokio::sync::OnceCell<std::sync::Arc<rustls::ClientConfig>> =
        tokio::sync::OnceCell::const_new();
    let cfg = CFG
        .get_or_init(|| async {
            let pem = http()
                .get(format!("{API}/tls/ca.pem"))
                .send()
                .await
                .expect("GET /api/tls/ca.pem")
                .bytes()
                .await
                .unwrap();
            let mut roots = rustls::RootCertStore::empty();
            for cert in rustls_pemfile::certs(&mut &pem[..]) {
                roots.add(cert.expect("CA PEM")).unwrap();
            }
            assert!(!roots.is_empty(), "/api/tls/ca.pem returned no certificate");
            let cfg = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
            std::sync::Arc::new(cfg)
        })
        .await;
    tokio_tungstenite::Connector::Rustls(cfg.clone())
}

/// One admin login for the whole binary: (bearer token, `session_id=...`
/// cookie). The login endpoint allows 5 attempts/minute per IP and every
/// CI test binary logs in from 127.0.0.1, so log in once and wait out a
/// 429 (`retry_after`) instead of burning the next suite's budget.
async fn login() -> &'static (String, String) {
    static LOGIN: tokio::sync::OnceCell<(String, String)> = tokio::sync::OnceCell::const_new();
    LOGIN
        .get_or_init(|| async {
            let c = http();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(150);
            loop {
                let r = c
                    .post(format!("{API}/auth/login"))
                    .json(&json!({"username": "admin", "password": "admin"}))
                    .send()
                    .await;
                let wait = match r {
                    Ok(r) if r.status().is_success() => {
                        let cookie = r
                            .headers()
                            .get_all(reqwest::header::SET_COOKIE)
                            .iter()
                            .filter_map(|v| v.to_str().ok())
                            .find(|v| v.starts_with("session_id="))
                            .expect("login sets the session_id cookie")
                            .split(';')
                            .next()
                            .unwrap()
                            .to_string();
                        let v: Value = r.json().await.unwrap();
                        return (v["ticket"].as_str().unwrap().to_string(), cookie);
                    }
                    Ok(r) if r.status() == 429 => {
                        let v: Value = r.json().await.unwrap_or_default();
                        v["retry_after"].as_u64().unwrap_or(5).clamp(1, 61)
                    }
                    _ => 1, // server still starting
                };
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "admin login kept failing"
                );
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
        })
        .await
}

async fn token(_c: &reqwest::Client) -> String {
    login().await.0.clone()
}

/// The browser's session cookie (the UI downloads the .vv with it).
async fn session_cookie(_c: &reqwest::Client) -> String {
    login().await.1.clone()
}

async fn call(
    c: &reqwest::Client,
    t: &str,
    m: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut rb = c.request(m, format!("{API}{path}")).bearer_auth(t);
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    let r = rb.send().await.unwrap();
    let s = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    (
        s,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn fresh_vm(c: &reqwest::Client, t: &str, id: &str, display: Option<Value>) {
    let _ = call(
        c,
        t,
        reqwest::Method::POST,
        &format!("/vms/{id}/stop"),
        None,
    )
    .await;
    let _ = call(c, t, reqwest::Method::DELETE, &format!("/vms/{id}"), None).await;
    let (s, b) = call(
        c,
        t,
        reqwest::Method::POST,
        "/vms",
        Some(json!({
        "id": id, "name": id, "hypervisor": "qemu", "architecture": "x86_64",
        "cpus": 1, "memory": 64, "disk_size": 1, "status": "stopped"})),
    )
    .await;
    assert!(s == 200 || s == 201, "create {id}: {s} {b}");
    if let Some(d) = display {
        let (s, b) = call(
            c,
            t,
            reqwest::Method::PUT,
            &format!("/vms/{id}/display"),
            Some(d),
        )
        .await;
        assert_eq!(s, 200, "set display: {b}");
    }
    let (s, b) = call(
        c,
        t,
        reqwest::Method::POST,
        &format!("/vms/{id}/start"),
        None,
    )
    .await;
    assert!((200..300).contains(&s), "start {id}: {s} {b}");
}

async fn cleanup(c: &reqwest::Client, t: &str, id: &str) {
    let _ = call(
        c,
        t,
        reqwest::Method::POST,
        &format!("/vms/{id}/stop"),
        None,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = call(c, t, reqwest::Method::DELETE, &format!("/vms/{id}"), None).await;
}

async fn read_exact(s: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    tokio::time::timeout(Duration::from_secs(20), s.read_exact(&mut b))
        .await
        .expect("timed out reading")
        .expect("read failed");
    b
}

type QmpLines = tokio::io::Lines<tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>>;

/// Next greeting/reply on a QMP connection, skipping async events.
async fn qmp_next(lines: &mut QmpLines) -> Value {
    loop {
        let l = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await
            .expect("QMP timeout")
            .unwrap()
            .expect("QMP closed");
        let v: Value = serde_json::from_str(&l).unwrap();
        if v.get("return").is_some() || v.get("error").is_some() || v.get("QMP").is_some() {
            return v;
        }
    }
}

/// One QMP command against the VM's monitor socket (`<disk>.qmp.sock`,
/// the same socket the API uses to set console passwords). Used to prove
/// which devices QEMU really has, independent of the API's own claims.
async fn qmp(c: &reqwest::Client, t: &str, id: &str, command: Value) -> Value {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let (_, vm) = call(c, t, reqwest::Method::GET, &format!("/vms/{id}"), None).await;
    let disk = vm["disks"][0]["path"]
        .as_str()
        .expect("VM disk path")
        .to_string();
    let sock = std::path::Path::new(&disk).with_extension("qmp.sock");
    let stream = tokio::net::UnixStream::connect(&sock)
        .await
        .unwrap_or_else(|e| panic!("QMP socket {}: {e}", sock.display()));
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    qmp_next(&mut lines).await;
    w.write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
        .await
        .unwrap();
    qmp_next(&mut lines).await;
    w.write_all(format!("{command}\n").as_bytes())
        .await
        .unwrap();
    let reply = qmp_next(&mut lines).await;
    assert!(reply.get("error").is_none(), "QMP {command}: {reply}");
    reply["return"].clone()
}

/// (vendor_id, device_id) of every PCI device in the guest.
async fn pci_ids(c: &reqwest::Client, t: &str, id: &str) -> Vec<(u64, u64)> {
    let buses = qmp(c, t, id, json!({"execute": "query-pci"})).await;
    let mut out = Vec::new();
    fn walk(devs: &Value, out: &mut Vec<(u64, u64)>) {
        for d in devs.as_array().into_iter().flatten() {
            out.push((
                d["id"]["vendor"].as_u64().unwrap_or(0),
                d["id"]["device"].as_u64().unwrap_or(0),
            ));
            if let Some(b) = d.get("pci_bridge") {
                walk(&b["devices"], out);
            }
        }
    }
    for bus in buses.as_array().into_iter().flatten() {
        walk(&bus["devices"], &mut out);
    }
    out
}

/// VNC authentication: DES-encrypt the 16-byte challenge with the password
/// (8 bytes, each byte bit-reversed: the RFB quirk).
fn vnc_response(password: &str, challenge: &[u8]) -> Vec<u8> {
    use des::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
    let mut key = [0u8; 8];
    for (i, b) in password.bytes().take(8).enumerate() {
        key[i] = b.reverse_bits();
    }
    let cipher = des::Des::new(GenericArray::from_slice(&key));
    let mut out = challenge.to_vec();
    for chunk in out.chunks_mut(8) {
        cipher.encrypt_block(GenericArray::from_mut_slice(chunk));
    }
    out
}

async fn ws_connect(
    path: &str,
    t: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = format!("{WS}{path}").into_client_request().unwrap();
    req.headers_mut()
        .insert("Authorization", format!("Bearer {t}").parse().unwrap());
    tokio_tungstenite::connect_async_tls_with_config(req, None, false, Some(wss_connector().await))
        .await
        .expect("ws connect")
        .0
}

#[tokio::test]
async fn vnc_and_serial_consoles_are_real() {
    let c = http();
    let t = token(&c).await;
    let id = "console-vnc";
    fresh_vm(&c, &t, id, None).await;

    let (s, info) = call(
        &c,
        &t,
        reqwest::Method::POST,
        &format!("/console/{id}/vnc"),
        None,
    )
    .await;
    assert_eq!(s, 200, "vnc console: {info}");
    let port = info["port"].as_u64().unwrap() as u16;
    let password = info["password"].as_str().expect("VNC password").to_string();
    assert_eq!(info["host"], "127.0.0.1");

    // RFB handshake straight to QEMU.
    let mut v = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("VNC port listening");
    let banner = read_exact(&mut v, 12).await;
    assert!(
        banner.starts_with(b"RFB 003."),
        "{:?}",
        String::from_utf8_lossy(&banner)
    );
    v.write_all(b"RFB 003.008\n").await.unwrap();
    let n = read_exact(&mut v, 1).await[0] as usize;
    assert!(n > 0, "server refused connection");
    let types = read_exact(&mut v, n).await;
    assert!(types.contains(&2), "VNC auth must be required: {types:?}");
    assert!(
        !types.contains(&1),
        "VNC must not allow 'None' auth: {types:?}"
    );
    v.write_all(&[2]).await.unwrap();
    let challenge = read_exact(&mut v, 16).await;
    v.write_all(&vnc_response(&password, &challenge))
        .await
        .unwrap();
    let result = u32::from_be_bytes(read_exact(&mut v, 4).await.try_into().unwrap());
    assert_eq!(result, 0, "VNC password from the API must be accepted");
    v.write_all(&[1]).await.unwrap(); // ClientInit, shared
    let init = read_exact(&mut v, 4).await;
    let (w, h) = (
        u16::from_be_bytes([init[0], init[1]]),
        u16::from_be_bytes([init[2], init[3]]),
    );
    assert!(w > 0 && h > 0, "framebuffer {w}x{h}");
    drop(v);

    // A wrong password is refused.
    let mut v = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    read_exact(&mut v, 12).await;
    v.write_all(b"RFB 003.008\n").await.unwrap();
    let n = read_exact(&mut v, 1).await[0] as usize;
    read_exact(&mut v, n).await;
    v.write_all(&[2]).await.unwrap();
    let challenge = read_exact(&mut v, 16).await;
    v.write_all(&vnc_response("wrongpw!", &challenge))
        .await
        .unwrap();
    let result = u32::from_be_bytes(read_exact(&mut v, 4).await.try_into().unwrap());
    assert_ne!(result, 0, "wrong VNC password accepted");
    drop(v);

    // Same RFB server through the ticketed WebSocket proxy (what noVNC uses).
    let (_, info) = call(
        &c,
        &t,
        reqwest::Method::POST,
        &format!("/console/{id}/vnc"),
        None,
    )
    .await;
    let mut ws = ws_connect(info["ws_path"].as_str().unwrap(), &t).await;
    let msg = tokio::time::timeout(Duration::from_secs(20), ws.next())
        .await
        .expect("no data through VNC proxy")
        .unwrap()
        .unwrap();
    assert!(
        msg.into_data().starts_with(b"RFB 003."),
        "proxy did not relay RFB"
    );
    drop(ws);

    // noVNC page carries this run's password for the RFB client.
    let r = c
        .get(format!("{API}/console/{id}/novnc"))
        .bearer_auth(&t)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r
        .text()
        .await
        .unwrap()
        .contains(&format!("password: '{password}'")));

    // SPICE isn't configured on a VNC VM: clear 422, not a dead port.
    let (s, _) = call(
        &c,
        &t,
        reqwest::Method::POST,
        &format!("/console/{id}/spice"),
        None,
    )
    .await;
    assert_eq!(s, 422);
    // Display can't be changed while running.
    let (s, _) = call(
        &c,
        &t,
        reqwest::Method::PUT,
        &format!("/vms/{id}/display"),
        Some(json!({"protocol": "spice", "video": "qxl", "sound": "none"})),
    )
    .await;
    assert_eq!(s, 409);

    // Serial: the bridge connects to QEMU's socket and stays up.
    let (s, info) = call(
        &c,
        &t,
        reqwest::Method::POST,
        &format!("/console/{id}/serial"),
        None,
    )
    .await;
    assert_eq!(s, 200, "serial console: {info}");
    let mut ws = ws_connect(info["ws_path"].as_str().unwrap(), &t).await;
    ws.send(tokio_tungstenite::tungstenite::Message::Text("\r".into()))
        .await
        .unwrap();
    match tokio::time::timeout(Duration::from_secs(3), ws.next()).await {
        Err(_) => {} // still connected, nothing to say yet: good
        Ok(Some(Ok(m))) => assert!(!m.is_close(), "serial bridge closed: socket missing"),
        Ok(other) => panic!("serial bridge failed: {other:?}"),
    }
    drop(ws);

    cleanup(&c, &t, id).await;
}

/// SPICE main-channel link + ticket auth; returns the server's channel list.
async fn spice_channels(port: u16, password: &str) -> Result<Vec<(u8, u8)>, String> {
    use rsa::pkcs8::DecodePublicKey;
    let mut s = TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|e| e.to_string())?;
    // SpiceLinkHeader + SpiceLinkMess (main channel) + 1 common cap word:
    // PROTOCOL_AUTH_SELECTION | AUTH_SPICE (no mini header -> full headers).
    let mut mess = Vec::new();
    mess.extend_from_slice(&0u32.to_le_bytes()); // connection id
    mess.push(1); // channel type: main
    mess.push(0); // channel id
    mess.extend_from_slice(&1u32.to_le_bytes()); // num common caps
    mess.extend_from_slice(&0u32.to_le_bytes()); // num channel caps
    mess.extend_from_slice(&18u32.to_le_bytes()); // caps offset
    mess.extend_from_slice(&0b11u32.to_le_bytes());
    let mut hdr = b"REDQ".to_vec();
    hdr.extend_from_slice(&2u32.to_le_bytes());
    hdr.extend_from_slice(&2u32.to_le_bytes());
    hdr.extend_from_slice(&(mess.len() as u32).to_le_bytes());
    s.write_all(&hdr).await.unwrap();
    s.write_all(&mess).await.unwrap();

    let h = read_exact(&mut s, 16).await;
    if &h[0..4] != b"REDQ" {
        return Err(format!("not a SPICE server: {:?}", &h[0..4]));
    }
    let size = u32::from_le_bytes(h[12..16].try_into().unwrap()) as usize;
    let reply = read_exact(&mut s, size).await;
    let err = u32::from_le_bytes(reply[0..4].try_into().unwrap());
    if err != 0 {
        return Err(format!("link error {err}"));
    }
    let key = rsa::RsaPublicKey::from_public_key_der(&reply[4..166]).map_err(|e| e.to_string())?;
    s.write_all(&1u32.to_le_bytes()).await.unwrap(); // auth mechanism: SPICE ticket
    let mut pw = password.as_bytes().to_vec();
    pw.push(0);
    let enc = key
        .encrypt(&mut rand::thread_rng(), rsa::Oaep::new::<sha1::Sha1>(), &pw)
        .map_err(|e| e.to_string())?;
    s.write_all(&enc).await.unwrap();
    let result = u32::from_le_bytes(read_exact(&mut s, 4).await.try_into().unwrap());
    if result != 0 {
        return Err(format!("ticket rejected ({result})"));
    }

    // Full data headers: serial u64, type u16, size u32, sub_list u32.
    let mut serial = 0u64;
    let mut send = |ty: u16| {
        serial += 1;
        let mut m = serial.to_le_bytes().to_vec();
        m.extend_from_slice(&ty.to_le_bytes());
        m.extend_from_slice(&0u32.to_le_bytes());
        m.extend_from_slice(&0u32.to_le_bytes());
        m
    };
    let mut attached = false;
    loop {
        let h = read_exact(&mut s, 18).await;
        let ty = u16::from_le_bytes([h[8], h[9]]);
        let size = u32::from_le_bytes(h[10..14].try_into().unwrap()) as usize;
        let body = read_exact(&mut s, size).await;
        match ty {
            103 if !attached => {
                // MAIN_INIT -> ask for the channel list
                s.write_all(&send(104)).await.unwrap();
                attached = true;
            }
            104 => {
                let n = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
                return Ok((0..n).map(|i| (body[4 + 2 * i], body[5 + 2 * i])).collect());
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn spice_console_carries_audio_and_usbredir() {
    let c = http();
    let t = token(&c).await;
    let id = "console-spice";
    // Sound needs SPICE: rejected on a VNC VM.
    let _ = call(&c, &t, reqwest::Method::DELETE, &format!("/vms/{id}"), None).await;
    let (s, b) = call(
        &c,
        &t,
        reqwest::Method::POST,
        "/vms",
        Some(json!({
        "id": id, "name": id, "hypervisor": "qemu", "architecture": "x86_64", "cpus": 1,
        "memory": 64, "disk_size": 1, "status": "stopped",
        "display": {"protocol": "vnc", "video": "std", "sound": "intel-hda"}})),
    )
    .await;
    assert_eq!(s, 422, "sound without SPICE must be rejected: {b}");

    for sound in ["intel-hda", "ich9-intel-hda", "ac97"] {
        fresh_vm(
            &c,
            &t,
            id,
            Some(json!({"protocol": "spice", "video": "qxl", "sound": sound})),
        )
        .await;
        let (s, vm) = call(&c, &t, reqwest::Method::GET, &format!("/vms/{id}"), None).await;
        assert_eq!(s, 200);
        assert_eq!(vm["display"]["sound"], sound, "{vm}");

        let (s, info) = call(
            &c,
            &t,
            reqwest::Method::POST,
            &format!("/console/{id}/spice"),
            None,
        )
        .await;
        assert_eq!(s, 200, "spice console: {info}");
        let port = info["port"].as_u64().unwrap() as u16;
        let password = info["password"].as_str().unwrap().to_string();

        let channels = spice_channels(port, &password)
            .await
            .expect("SPICE link/auth");
        let types: Vec<u8> = channels.iter().map(|c| c.0).collect();
        for (ty, what) in [
            (2, "display"),
            (3, "inputs"),
            (5, "playback (audio out)"),
            (6, "record (audio in)"),
            (9, "usbredir"),
        ] {
            assert!(
                types.contains(&ty),
                "{sound}: SPICE channel {what} missing: {channels:?}"
            );
        }
        assert_eq!(
            types.iter().filter(|t| **t == 9).count(),
            2,
            "two usbredir channels"
        );
        assert!(
            spice_channels(port, "wrong-password").await.is_err(),
            "wrong SPICE ticket accepted"
        );

        // .vv for remote-viewer
        let r = c
            .get(format!("{API}/console/{id}/spice.vv"))
            .bearer_auth(&t)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let vv = r.text().await.unwrap();
        assert!(
            vv.contains("type=spice")
                && vv.contains(&format!("port={port}"))
                && vv.contains(&format!("password={password}")),
            "{vv}"
        );

        // What QEMU really has, via its own monitor: the sound card on PCI,
        // the clipboard/agent port (virtio-serial + spicevmc "vdagent"
        // chardev = com.redhat.spice.0) and the USB controller the two
        // usbredir channels hang off.
        let pci = pci_ids(&c, &t, id).await;
        let sound_pci = match sound {
            "intel-hda" => (0x8086, 0x2668),
            "ich9-intel-hda" => (0x8086, 0x293e),
            _ => (0x8086, 0x2415), // AC97
        };
        assert!(
            pci.contains(&sound_pci),
            "{sound}: sound card not on PCI: {pci:x?}"
        );
        assert!(pci.contains(&(0x1b36, 0x0100)), "qxl-vga missing: {pci:x?}");
        assert!(
            pci.contains(&(0x1b36, 0x000d)),
            "qemu-xhci missing: {pci:x?}"
        );
        assert!(
            pci.contains(&(0x1af4, 0x1003)) || pci.contains(&(0x1af4, 0x1043)),
            "virtio-serial (agent/clipboard port) missing: {pci:x?}"
        );
        let chardevs = qmp(&c, &t, id, json!({"execute": "query-chardev"})).await;
        let spicevmc: Vec<&str> = chardevs
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["filename"].as_str().unwrap_or("").contains("spicevmc"))
            .filter_map(|d| d["label"].as_str())
            .collect();
        for label in ["vdagent", "usbredirchr0", "usbredirchr1"] {
            assert!(
                spicevmc.contains(&label),
                "spicevmc chardev {label} missing: {chardevs}"
            );
        }
        let spice = qmp(&c, &t, id, json!({"execute": "query-spice"})).await;
        assert_eq!(spice["enabled"], true, "{spice}");
        assert_eq!(
            spice["auth"], "spice",
            "SPICE must require a ticket: {spice}"
        );
        assert_eq!(spice["port"].as_u64(), Some(port as u64), "{spice}");

        // The UI downloads the .vv with the browser's session cookie (no
        // bearer token on a plain link): that path must work too.
        let cookie = session_cookie(&c).await;
        let r = c
            .get(format!("{API}/console/{id}/spice.vv"))
            .header(reqwest::header::COOKIE, cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "cookie-authenticated .vv download");
        assert!(r.text().await.unwrap().contains("enable-usbredir=1"));

        // VNC isn't served on a SPICE VM.
        let (s, _) = call(
            &c,
            &t,
            reqwest::Method::POST,
            &format!("/console/{id}/vnc"),
            None,
        )
        .await;
        assert_eq!(s, 422);
        cleanup(&c, &t, id).await;
    }
}

/// A 512-byte MBR that SeaBIOS boots from the VM's disk: it prints
/// "HORCRUX SERIAL OK" to COM1 about once a second and echoes every byte it
/// receives on COM1 back with the letter case flipped. Hand-assembled
/// (16-bit real mode, loaded at 0000:7C00):
///
/// ```text
/// 7C00 FA              cli
/// 7C01 FC              cld
/// 7C02 31 C0           xor ax,ax
/// 7C04 8E D8           mov ds,ax
/// 7C06 BE 2F 7C        mov si,msg          ; banner:
/// 7C09 AC              lodsb               ; next:
/// 7C0A 84 C0           test al,al
/// 7C0C 74 06           jz idle
/// 7C0E BA F8 03        mov dx,0x3f8
/// 7C11 EE              out dx,al
/// 7C12 EB F5           jmp next
/// 7C14 66 B9 00 00 10 00  mov ecx,0x100000 ; idle:
/// 7C1A BA FD 03        mov dx,0x3fd        ; poll: line status
/// 7C1D EC              in al,dx
/// 7C1E A8 01           test al,1           ; data ready?
/// 7C20 74 07           jz quiet
/// 7C22 BA F8 03        mov dx,0x3f8
/// 7C25 EC              in al,dx
/// 7C26 34 20           xor al,0x20         ; flip case
/// 7C28 EE              out dx,al           ; echo
/// 7C29 66 49           dec ecx             ; quiet:
/// 7C2B 75 ED           jnz poll
/// 7C2D EB D7           jmp banner
/// 7C2F "HORCRUX SERIAL OK\r\n\0"
/// ```
fn serial_boot_sector() -> Vec<u8> {
    let mut b: Vec<u8> = vec![
        0xFA, 0xFC, 0x31, 0xC0, 0x8E, 0xD8, 0xBE, 0x2F, 0x7C, 0xAC, 0x84, 0xC0, 0x74, 0x06, 0xBA,
        0xF8, 0x03, 0xEE, 0xEB, 0xF5, 0x66, 0xB9, 0x00, 0x00, 0x10, 0x00, 0xBA, 0xFD, 0x03, 0xEC,
        0xA8, 0x01, 0x74, 0x07, 0xBA, 0xF8, 0x03, 0xEC, 0x34, 0x20, 0xEE, 0x66, 0x49, 0x75, 0xED,
        0xEB, 0xD7,
    ];
    assert_eq!(b.len(), 0x2F);
    b.extend_from_slice(b"HORCRUX SERIAL OK\r\n\0");
    b.resize(510, 0);
    b.extend_from_slice(&[0x55, 0xAA]);
    b
}

/// Read from the serial WebSocket until `want` shows up (or 90 s pass).
async fn serial_until(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    want: &str,
) -> String {
    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    while tokio::time::Instant::now() < deadline && !seen.contains(want) {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
            Ok(Some(Ok(m))) => {
                assert!(
                    !m.is_close(),
                    "serial bridge closed; output so far: {seen:?}"
                );
                seen.push_str(&String::from_utf8_lossy(&m.into_data()));
            }
            Ok(other) => panic!("serial bridge ended: {other:?}; output so far: {seen:?}"),
            Err(_) => {}
        }
    }
    assert!(
        seen.contains(want),
        "{want:?} never arrived on the serial console: {seen:?}"
    );
    seen
}

#[tokio::test]
async fn headless_vm_serial_console_is_interactive() {
    let c = http();
    let t = token(&c).await;
    let id = "console-serial";
    let _ = call(
        &c,
        &t,
        reqwest::Method::POST,
        &format!("/vms/{id}/stop"),
        None,
    )
    .await;
    let _ = call(&c, &t, reqwest::Method::DELETE, &format!("/vms/{id}"), None).await;
    let (s, b) = call(
        &c,
        &t,
        reqwest::Method::POST,
        "/vms",
        Some(json!({
        "id": id, "name": id, "hypervisor": "qemu", "architecture": "x86_64", "cpus": 1,
        "memory": 64, "disk_size": 1, "status": "stopped",
        "display": {"protocol": "none", "video": "none", "sound": "none"}})),
    )
    .await;
    assert!(s == 200 || s == 201, "create {id}: {s} {b}");

    // Put the serial test program on the VM's own disk (the API created a
    // qcow2; replace it with one whose first sector is the MBR above).
    let (_, vm) = call(&c, &t, reqwest::Method::GET, &format!("/vms/{id}"), None).await;
    let disk = vm["disks"][0]["path"]
        .as_str()
        .expect("disk path")
        .to_string();
    let raw = std::path::Path::new(&disk).with_extension("boot.raw");
    let mut img = serial_boot_sector();
    img.resize(1024 * 1024, 0);
    std::fs::write(&raw, &img).unwrap();
    let out = std::process::Command::new("qemu-img")
        .args(["convert", "-f", "raw", "-O", "qcow2"])
        .arg(&raw)
        .arg(&disk)
        .output()
        .expect("qemu-img");
    assert!(
        out.status.success(),
        "qemu-img convert: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_file(&raw);

    let (s, b) = call(
        &c,
        &t,
        reqwest::Method::POST,
        &format!("/vms/{id}/start"),
        None,
    )
    .await;
    assert!((200..300).contains(&s), "start {id}: {s} {b}");
    // No graphical console on a headless VM.
    let (s, _) = call(
        &c,
        &t,
        reqwest::Method::POST,
        &format!("/console/{id}/vnc"),
        None,
    )
    .await;
    assert_eq!(s, 422);

    let (s, info) = call(
        &c,
        &t,
        reqwest::Method::POST,
        &format!("/console/{id}/serial"),
        None,
    )
    .await;
    assert_eq!(s, 200, "serial console: {info}");
    let mut ws = ws_connect(info["ws_path"].as_str().unwrap(), &t).await;
    // Guest -> browser: SeaBIOS booted the disk and the program's output
    // reaches the WebSocket.
    serial_until(&mut ws, "HORCRUX SERIAL OK").await;
    // Browser -> guest -> browser: typed keys reach the guest's UART and
    // its (case-flipped) echo comes back.
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        "zebra".into(),
    ))
    .await
    .unwrap();
    serial_until(&mut ws, "ZEBRA").await;
    drop(ws);

    // The boot log still lands in <disk>.serial.log (nightly test reads it).
    let log = std::fs::read_to_string(std::path::Path::new(&disk).with_extension("serial.log"))
        .unwrap_or_default();
    assert!(log.contains("HORCRUX SERIAL OK"), "serial.log: {log:?}");
    cleanup(&c, &t, id).await;
}

#[tokio::test]
async fn video_cards_are_really_attached() {
    let c = http();
    let t = token(&c).await;
    let id = "console-video";
    // (video, PCI id QEMU must expose); VNC must serve each of them.
    for (video, pci) in [
        ("std", (0x1234u64, 0x1111u64)),
        ("virtio", (0x1af4, 0x1050)),
        ("qxl", (0x1b36, 0x0100)),
    ] {
        fresh_vm(
            &c,
            &t,
            id,
            Some(json!({"protocol": "vnc", "video": video, "sound": "none"})),
        )
        .await;
        let ids = pci_ids(&c, &t, id).await;
        assert!(ids.contains(&pci), "{video}: {pci:x?} not in {ids:x?}");
        let (s, info) = call(
            &c,
            &t,
            reqwest::Method::POST,
            &format!("/console/{id}/vnc"),
            None,
        )
        .await;
        assert_eq!(s, 200, "{video}: {info}");
        let mut v = TcpStream::connect(("127.0.0.1", info["port"].as_u64().unwrap() as u16))
            .await
            .expect("VNC listening");
        assert!(
            read_exact(&mut v, 12).await.starts_with(b"RFB 003."),
            "{video}: no RFB banner"
        );
        drop(v);
        cleanup(&c, &t, id).await;
    }
    // Headless: no display adapter at all.
    fresh_vm(
        &c,
        &t,
        id,
        Some(json!({"protocol": "none", "video": "none", "sound": "none"})),
    )
    .await;
    let ids = pci_ids(&c, &t, id).await;
    for vga in [(0x1234, 0x1111), (0x1af4, 0x1050), (0x1b36, 0x0100)] {
        assert!(
            !ids.contains(&vga),
            "headless VM has a display adapter: {ids:x?}"
        );
    }
    cleanup(&c, &t, id).await;
}
