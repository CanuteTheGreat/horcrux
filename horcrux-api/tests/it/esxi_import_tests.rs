//! ESXi importer against the real binary, a mock ESXi datastore browser
//! (HTTPS with a self-signed certificate, basic auth) and the real
//! `qemu-img`: listing, preview, import with the disk's bytes intact, and the
//! refusals (wrong password, snapshots, powered-on VM, loopback target).

use crate::support::*;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const VMX: &str = "displayName = \"web01\"\nguestOS = \"ubuntu-64\"\nnumvcpus = \"2\"\nmemsize = \"512\"\nscsi0.present = \"TRUE\"\nscsi0.virtualDev = \"lsilogic\"\nscsi0:0.present = \"TRUE\"\nscsi0:0.fileName = \"web01.vmdk\"\nethernet0.present = \"TRUE\"\nethernet0.virtualDev = \"e1000\"\nethernet0.networkName = \"VM Network\"\n";

fn page(items: &[(&str, &str)]) -> Vec<u8> {
    let rows: String = items
        .iter()
        .map(|(h, t)| format!("<tr><td><a href=\"{h}\">{t}</a></td></tr>"))
        .collect();
    format!("<html><body><table>{rows}</table></body></html>").into_bytes()
}

/// path (without query) -> body
fn site(flat: Vec<u8>) -> HashMap<String, Vec<u8>> {
    let q = "?dcPath=ha%2ddatacenter&amp;dsName=datastore1";
    let mut m = HashMap::new();
    m.insert(
        "/folder".into(),
        page(&[(
            "/folder?dcPath=ha%2ddatacenter&amp;dsName=datastore1",
            "datastore1",
        )]),
    );
    m.insert(
        "/folder@ds".into(),
        page(&[
            (&format!("/folder/web01{q}"), "web01/"),
            (&format!("/folder/snapvm{q}"), "snapvm/"),
            (&format!("/folder/lockedvm{q}"), "lockedvm/"),
        ]),
    );
    m.insert(
        "/folder/web01@ds".into(),
        page(&[
            (&format!("/folder/web01/web01.vmx{q}"), "web01.vmx"),
            (&format!("/folder/web01/web01.vmdk{q}"), "web01.vmdk"),
            (
                &format!("/folder/web01/web01-flat.vmdk{q}"),
                "web01-flat.vmdk",
            ),
            (&format!("/folder/web01/vmware.log{q}"), "vmware.log"),
        ]),
    );
    m.insert("/folder/web01/web01.vmx".into(), VMX.as_bytes().to_vec());
    m.insert(
        "/folder/web01/web01.vmdk".into(),
        b"# Disk DescriptorFile\nversion=1\ncreateType=\"vmfsThin\"\nRW 2048 VMFS \"web01-flat.vmdk\"\n".to_vec(),
    );
    m.insert("/folder/web01/web01-flat.vmdk".into(), flat);
    // a VM with a snapshot: its vmx points at a delta disk
    m.insert(
        "/folder/snapvm@ds".into(),
        page(&[
            (&format!("/folder/snapvm/snapvm.vmx{q}"), "snapvm.vmx"),
            (
                &format!("/folder/snapvm/snapvm-000001.vmdk{q}"),
                "snapvm-000001.vmdk",
            ),
            (
                &format!("/folder/snapvm/snapvm-000001-delta.vmdk{q}"),
                "snapvm-000001-delta.vmdk",
            ),
        ]),
    );
    m.insert(
        "/folder/snapvm/snapvm.vmx".into(),
        VMX.replace("web01.vmdk", "snapvm-000001.vmdk").into_bytes(),
    );
    m.insert(
        "/folder/snapvm/snapvm-000001.vmdk".into(),
        b"createType=\"vmfsSparse\"\nparentFileNameHint=\"snapvm.vmdk\"\n".to_vec(),
    );
    // a powered-on VM
    m.insert(
        "/folder/lockedvm@ds".into(),
        page(&[
            (&format!("/folder/lockedvm/lockedvm.vmx{q}"), "lockedvm.vmx"),
            (
                &format!("/folder/lockedvm/lockedvm.vmx.lck{q}"),
                "lockedvm.vmx.lck",
            ),
        ]),
    );
    m
}

async fn mock_esxi(ip: std::net::IpAddr, site: HashMap<String, Vec<u8>>) -> u16 {
    let cert = rcgen::generate_simple_self_signed(vec!["esxi.test".to_string()]).unwrap();
    let chain = vec![cert.cert.der().clone()];
    let key = rustls::pki_types::PrivateKeyDer::try_from(cert.signing_key.serialize_der()).unwrap();
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let l = tokio::net::TcpListener::bind((ip, 0)).await.unwrap();
    let port = l.local_addr().unwrap().port();
    let site = Arc::new(site);
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = l.accept().await else {
                return;
            };
            let (acceptor, site) = (acceptor.clone(), site.clone());
            tokio::spawn(async move {
                let Ok(mut s) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = s.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let first = head.lines().next().unwrap_or("").to_string();
                let mut it = first.split_whitespace();
                let (method, target) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                let path = urlencoding::decode(path).unwrap_or_default().to_string();
                let authed = head.to_lowercase().contains("authorization: basic ")
                    && head.contains(&base64_basic("root", "esxi-pass"));
                let ds_key = format!("{path}@ds");
                let key = if query.contains("dsName=")
                    && (site.contains_key(&ds_key) || !site.contains_key(&path))
                {
                    ds_key
                } else {
                    path.clone()
                };
                let (status, body) = if !authed {
                    ("401 Unauthorized", b"no".to_vec())
                } else if let Some(b) = site.get(&key) {
                    ("200 OK", b.clone())
                } else {
                    ("404 Not Found", b"missing".to_vec())
                };
                let out = if method == "HEAD" {
                    Vec::new()
                } else {
                    body.clone()
                };
                let _ = s
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
                let _ = s.write_all(&out).await;
                let _ = s.shutdown().await;
            });
        }
    });
    port
}

fn base64_basic(u: &str, p: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"))
}

fn primary_ip() -> std::net::IpAddr {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    s.connect("192.0.2.1:9").unwrap();
    s.local_addr().unwrap().ip()
}

#[tokio::test]
async fn esxi_vm_is_listed_previewed_imported_and_unsafe_ones_refused() {
    let dir = scratch("esxi-import");
    let ip = primary_ip();
    // a 1 MiB flat disk with a pattern
    let flat: Vec<u8> = (0..1 << 20).map(|i| (i % 249) as u8).collect();
    let raw = dir.join("expected.raw");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&raw, &flat).unwrap();
    let want = content_sha(&raw);
    let port = mock_esxi(ip, site(flat)).await;

    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;
    let conn = |extra: serde_json::Value| {
        let mut v = json!({"host": ip.to_string(), "port": port, "username": "root", "password": "esxi-pass"});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };

    // listing
    let (s, v) = a.post("/vms/import/esxi/list", conn(json!({}))).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v[0]["name"], "datastore1");
    let folders: Vec<_> = v[0]["vms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["folder"].as_str().unwrap().to_string())
        .collect();
    assert!(
        folders.contains(&"web01".to_string()) && folders.contains(&"snapvm".to_string()),
        "{v}"
    );

    // wrong password
    let (s, v) = a
        .post("/vms/import/esxi/list", conn(json!({"password": "nope"})))
        .await;
    assert_eq!(s, 400, "{v}");
    assert!(
        v.to_string().contains("refused the user name / password"),
        "{v}"
    );
    assert!(!v.to_string().contains("nope"), "password echoed: {v}");

    // preview
    let (s, v) = a
        .post(
            "/vms/import/esxi/preview",
            conn(json!({"datastore": "datastore1", "folder": "web01"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (
            v["plan"]["name"].clone(),
            v["plan"]["cpus"].clone(),
            v["plan"]["memory_mb"].clone()
        ),
        (json!("web01"), json!(2), json!(512))
    );
    assert_eq!(v["plan"]["disks"][0]["bus"], "scsi");
    assert_eq!(v["plan"]["nics"][0]["model"], "e1000");
    let (_, vms) = a.get("/vms").await;
    assert!(vms.as_array().unwrap().is_empty(), "preview created a VM");

    // import
    let (s, v) = a
        .post(
            "/vms/import/esxi",
            conn(json!({"datastore": "datastore1", "folder": "web01", "id": "from-esxi"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["vm"]["id"], "from-esxi");
    let disk = v["vm"]["disks"][0]["path"].as_str().unwrap().to_string();
    let info = run("qemu-img", &["info", "--output=json", &disk]);
    assert!(info.contains("\"format\": \"qcow2\""), "{info}");
    assert_eq!(content_sha(std::path::Path::new(&disk)).len(), want.len());
    // the first MiB of the converted disk equals the copied flat file
    let head = dir.join("head.raw");
    run(
        "qemu-img",
        &["convert", "-O", "raw", &disk, &head.to_string_lossy()],
    );
    let got = std::fs::read(&head).unwrap();
    assert_eq!(
        &got[..1 << 20],
        &std::fs::read(&raw).unwrap()[..],
        "disk bytes survived"
    );

    // refusals
    let n = a.get("/vms").await.1.as_array().unwrap().len();
    let (s, v) = a
        .post(
            "/vms/import/esxi",
            conn(json!({"datastore": "datastore1", "folder": "snapvm"})),
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert!(v.to_string().contains("snapshot"), "{v}");
    let (s, v) = a
        .post(
            "/vms/import/esxi",
            conn(json!({"datastore": "datastore1", "folder": "lockedvm"})),
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert!(v.to_string().contains("powered on"), "{v}");
    let (s, v) = a
        .post(
            "/vms/import/esxi/preview",
            conn(json!({"datastore": "datastore1", "folder": "../etc"})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    // the node itself / cloud metadata are never contacted
    for host in ["127.0.0.1", "169.254.169.254"] {
        let (s, v) = a
            .post("/vms/import/esxi/list", conn(json!({"host": host})))
            .await;
        assert_eq!(s, 403, "{host}: {v}");
    }
    assert_eq!(
        a.get("/vms").await.1.as_array().unwrap().len(),
        n,
        "refusals must not create VMs"
    );
    let leftovers: Vec<_> = std::fs::read_dir(srv.data())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("ovf-import-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
