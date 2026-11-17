//! What the Create VM form needs from the API: host capabilities for the
//! sliders, the default network (and its global override), and ISO
//! storages with upload / download-with-checksum / delete, against a freshly
//! spawned horcrux-api.

use crate::support::{scratch, Server};
use serde_json::json;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn host_options_report_capabilities_default_network_and_iso_storage() {
    let dir = scratch("vm-form-options");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    let (s, o) = api.get("/vms/host-options").await;
    assert_eq!(s, 200, "{o}");
    // slider bounds
    let host = &o["host"];
    assert!(host["cpus"].as_u64().unwrap() >= 1, "{o}");
    let (total, reserve, max) = (
        host["memory_mb"].as_u64().unwrap(),
        host["memory_reserve_mb"].as_u64().unwrap(),
        host["max_vm_memory_mb"].as_u64().unwrap(),
    );
    assert!(total > 0 && reserve > 0 && max + reserve == total, "{o}");
    assert!(host["default_disk_available_gb"].as_u64().is_some(), "{o}");

    // a fresh install has a default ISO-capable storage
    let stores = o["iso_storages"].as_array().unwrap();
    assert!(stores.iter().any(|s| s["id"] == "local"), "{o}");
    assert!(stores.iter().any(|s| s["id"] == ""), "image library: {o}");
    assert_eq!(o["default_iso_storage"], "local", "{o}");
    let (_, pools) = api.get("/storage/pools").await;
    assert!(
        pools.as_array().unwrap().iter().any(|p| p["id"] == "local"),
        "{pools}"
    );

    // global default network: unset, a bogus bridge is refused, empty clears
    let (s, d) = api.get("/vms/defaults").await;
    assert_eq!(s, 200, "{d}");
    assert!(d["default_network"].is_null(), "{d}");
    let (s, e) = api
        .call(
            reqwest::Method::PUT,
            "/vms/defaults",
            Some(json!({"default_network": "no-such-bridge0"})),
        )
        .await;
    assert_eq!(s, 400, "{e}");
    let (s, d) = api
        .call(
            reqwest::Method::PUT,
            "/vms/defaults",
            Some(json!({"default_network": ""})),
        )
        .await;
    assert_eq!(s, 200, "{d}");
    assert!(d["default_network"].is_null(), "{d}");
    assert!(o["default_networks"]["override"].is_null(), "{o}");
}

#[tokio::test]
async fn default_pool_is_seeded_once() {
    let dir = scratch("vm-form-seed-once");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let st = api.delete("/storage/pools/local").await;
    assert!(st == 200 || st == 204, "operator can remove it: {st}");
    drop(api);
    srv.kill9();
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;
    let (_, pools) = api.get("/storage/pools").await;
    assert!(
        pools.as_array().unwrap().iter().all(|p| p["id"] != "local"),
        "a removed default pool must stay removed after restart: {pools}"
    );
    // ISOs still have the image library
    let (_, o) = api.get("/vms/host-options").await;
    assert_eq!(o["default_iso_storage"], "", "{o}");
}

#[tokio::test]
async fn iso_upload_download_and_delete_per_storage() {
    let dir = scratch("vm-form-iso");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    let data: Vec<u8> = (0..2 * 1024 * 1024u32)
        .map(|i| (i * 13 % 251) as u8)
        .collect();
    let sha = hex::encode(Sha256::digest(&data));
    let up = |name: &str, storage: &str, sha: Option<&str>, body: Vec<u8>| {
        let mut url = format!(
            "{}/storage/isos/upload?filename={name}&storage={storage}",
            api.base
        );
        if let Some(s) = sha {
            url.push_str(&format!("&sha256={s}"));
        }
        let r = api.c.post(url).bearer_auth(&api.token).body(body);
        async move {
            let r = r.send().await.unwrap();
            (r.status().as_u16(), r.text().await.unwrap())
        }
    };

    // wrong checksum / bad name / unknown storage: refused, nothing left
    let (s, m) = up("x.iso", "local", Some(&"0".repeat(64)), data.clone()).await;
    assert_eq!((s, m.contains("sha256 mismatch")), (400, true), "{m}");
    let (s, m) = up("x.sh", "local", None, vec![1, 2, 3]).await;
    assert_eq!(s, 400, "{m}");
    let (s, m) = up("x.iso", "nope", None, vec![1, 2, 3]).await;
    assert_eq!(s, 404, "{m}");
    let local_iso = dir.join("data/storage/local/template/iso");
    if local_iso.exists() {
        assert_eq!(std::fs::read_dir(&local_iso).unwrap().count(), 0);
    }

    // good uploads: default pool and the image library
    let (s, m) = up("a.iso", "local", Some(&sha), data.clone()).await;
    assert_eq!(s, 201, "{m}");
    let v: serde_json::Value = serde_json::from_str(&m).unwrap();
    assert_eq!(
        (v["storage"].as_str(), v["size"].as_u64()),
        (Some("local"), Some(data.len() as u64))
    );
    assert_eq!(std::fs::read(v["path"].as_str().unwrap()).unwrap(), data);
    let (s, m) = up("b.iso", "", None, data.clone()).await;
    assert_eq!(s, 201, "{m}");
    // small body: the handler refuses before reading it, and a big unread
    // body breaks the pooled connection for the next request
    let (s, _) = up("a.iso", "local", None, vec![1, 2, 3]).await;
    assert_eq!(s, 409, "duplicate");

    let (s, isos) = api.get("/storage/isos").await;
    assert_eq!(s, 200, "GET /storage/isos: {isos}");
    let find = |n: &str| {
        isos.as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == n)
            .cloned()
            .unwrap_or_else(|| panic!("{n} not listed: {isos}"))
    };
    assert_eq!(find("a.iso")["storage"], "local");
    assert_eq!(find("a.iso")["storage_name"], "local");
    assert_eq!(find("b.iso")["storage"], "");

    // download from a URL with a checksum (a non-loopback address: loopback
    // is refused by design)
    if let Some(ip) = primary_ip() {
        let listener = tokio::net::TcpListener::bind((ip, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = data.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                let body = body.clone();
                tokio::spawn(async move {
                    let mut b = [0u8; 2048];
                    let _ = s.read(&mut b).await;
                    let _ = s
                        .write_all(
                            format!(
                                "HTTP/1.0 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                    let _ = s.write_all(&body).await;
                });
            }
        });
        let url = format!("http://{ip}:{port}/mirror.iso");
        // wrong checksum: the file is discarded and the list reports why
        let (s, m) = api
            .post(
                "/storage/isos/download",
                json!({"url": url, "storage": "local", "filename": "bad.iso", "sha256": "0".repeat(64)}),
            )
            .await;
        assert_eq!(s, 202, "{m}");
        let err = wait_iso(&api, "bad.iso", true).await;
        assert!(
            err["error"].as_str().unwrap().contains("sha256 mismatch"),
            "{err}"
        );
        assert!(!local_iso.join("bad.iso").exists());
        // right checksum
        let (s, m) = api
            .post(
                "/storage/isos/download",
                json!({"url": url, "storage": "local", "filename": "good.iso", "sha256": sha}),
            )
            .await;
        assert_eq!(s, 202, "{m}");
        let ok = wait_iso(&api, "good.iso", false).await;
        assert_eq!(ok["storage"], "local", "{ok}");
        assert_eq!(std::fs::read(local_iso.join("good.iso")).unwrap(), data);
        assert_eq!(
            api.delete("/storage/isos/good.iso?storage=local").await,
            204
        );
    } else {
        eprintln!("no non-loopback address: download case skipped");
    }

    // delete addresses the storage
    assert_eq!(api.delete("/storage/isos/a.iso?storage=local").await, 204);
    assert!(!local_iso.join("a.iso").exists());
    assert_eq!(api.delete("/storage/isos/b.iso").await, 204);
}

fn primary_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// Poll the ISO list until `name` finished (or, with `failed`, errored).
async fn wait_iso(api: &crate::support::Api, name: &str, failed: bool) -> serde_json::Value {
    for _ in 0..100 {
        let (_, l) = api.get("/storage/isos").await;
        if let Some(i) = l.as_array().unwrap().iter().find(|i| i["name"] == name) {
            if failed && i["error"].is_string() {
                return i.clone();
            }
            if !failed && i["downloading"] == false && i["error"].is_null() {
                return i.clone();
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    panic!("ISO {name} did not reach the expected state");
}
