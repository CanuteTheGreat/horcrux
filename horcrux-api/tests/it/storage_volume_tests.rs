//! Storage pool volumes over HTTP against a freshly spawned horcrux-api
//! (support::Server): names that would leave the pool (`../x`,
//! `../../tmp/x`, `a/b`, absolute paths, `.`, `..`, empty, option-like)
//! are refused with 400 on create, delete and resize, and nothing appears
//! or disappears outside the pool directory; valid names create, list,
//! grow and delete real qcow2 files.

use crate::support::{scratch, Server};
use serde_json::json;
use std::path::{Path, PathBuf};

fn tree(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(tree(&p));
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[tokio::test]
async fn volume_names_cannot_escape_the_pool_over_http() {
    let dir = scratch("storage-volumes");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    // <outside>/pool is the pool; <outside>/victim.qcow2 is where
    // "../victim" would point.
    let outside = dir.join("outside");
    let pool_dir = outside.join("pool");
    std::fs::create_dir_all(&pool_dir).unwrap();
    std::fs::write(outside.join("victim.qcow2"), b"keep me").unwrap();
    let tmp_marker = std::env::temp_dir().join(format!("hx-trav-{}", std::process::id()));
    let tmp_name = tmp_marker
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let pool = api
        .ok(
            reqwest::Method::POST,
            "/storage/pools",
            Some(json!({"name": "trav", "storage_type": "directory",
                        "path": pool_dir.to_string_lossy()})),
        )
        .await;
    let pid = pool["id"].as_str().unwrap().to_string();
    let before = tree(&outside);

    let tmp_rel = format!("../../../../../../../..{}", tmp_marker.display());
    let bad = [
        "../x".to_string(),
        "../victim".to_string(),
        "../../tmp/x".to_string(),
        tmp_rel.clone(),
        "a/b".to_string(),
        tmp_marker.to_string_lossy().to_string(),
        "/etc/passwd".to_string(),
        String::new(),
        ".".to_string(),
        "..".to_string(),
        "-rf".to_string(),
        "x@y".to_string(),
    ];
    for name in &bad {
        let (s, v) = api
            .post(
                &format!("/storage/pools/{pid}/volumes"),
                json!({"name": name, "size": 1}),
            )
            .await;
        assert_eq!(s, 400, "create {name:?}: {v}");

        // In a URL path segment: percent-encode everything so `/` and dot
        // segments reach the handler as part of the name.
        let enc: String = name.bytes().map(|b| format!("%{b:02X}")).collect();
        if enc.is_empty() || name == "." || name == ".." {
            continue; // not expressible as a path segment (URL dot segments)
        }
        let s = api
            .delete(&format!("/storage/pools/{pid}/volumes/{enc}"))
            .await;
        assert_eq!(s, 400, "delete {name:?}");
        let (s, v) = api
            .post(
                &format!("/storage/pools/{pid}/volumes/{enc}/resize"),
                json!({"size": 2}),
            )
            .await;
        assert_eq!(s, 400, "resize {name:?}: {v}");
    }
    assert_eq!(tree(&outside), before, "nothing changed outside the pool");
    assert_eq!(
        std::fs::read(outside.join("victim.qcow2")).unwrap(),
        b"keep me"
    );
    assert!(!tmp_marker.with_extension("qcow2").exists(), "{tmp_name}");

    // Valid names: real files in the pool, listed, grown, deleted.
    let gb: u64 = 1024 * 1024 * 1024;
    let v = api
        .ok(
            reqwest::Method::POST,
            &format!("/storage/pools/{pid}/volumes"),
            Some(json!({"name": "data-1", "size": 1})),
        )
        .await;
    assert_eq!(v["name"], "data-1");
    assert_eq!(v["size_bytes"], gb);
    assert!(pool_dir.join("data-1.qcow2").is_file());
    let (s, _) = api
        .post(
            &format!("/storage/pools/{pid}/volumes"),
            json!({"name": "data-1", "size": 1}),
        )
        .await;
    assert_eq!(s, 409, "same name again must not truncate the image");

    let (s, list) = api.get(&format!("/storage/pools/{pid}/volumes")).await;
    assert_eq!(s, 200);
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["data-1"]);

    let v = api
        .ok(
            reqwest::Method::POST,
            &format!("/storage/pools/{pid}/volumes/data-1/resize"),
            Some(json!({"size": 2})),
        )
        .await;
    assert_eq!(v["size_bytes"], 2 * gb);
    let (s, _) = api
        .post(
            &format!("/storage/pools/{pid}/volumes/data-1/resize"),
            json!({"size": 1}),
        )
        .await;
    assert_eq!(s, 400, "shrink refused");

    assert_eq!(
        api.delete(&format!("/storage/pools/{pid}/volumes/data-1"))
            .await,
        200
    );
    assert!(!pool_dir.join("data-1.qcow2").exists());
    assert_eq!(
        api.delete(&format!("/storage/pools/{pid}/volumes/data-1"))
            .await,
        404
    );
    let (s, _) = api.get("/storage/pools/no-such-pool/volumes").await;
    assert_eq!(s, 404);
    assert_eq!(tree(&outside), before);
    let _ = std::fs::remove_dir_all(&dir);
}
