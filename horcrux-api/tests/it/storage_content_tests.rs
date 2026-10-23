//! Storage content types over HTTP against a freshly spawned horcrux-api:
//! a real streamed upload lands byte-for-byte in the pool's content
//! directory, the checksum is enforced, bad names/extensions are refused, the
//! ISO library sees an uploaded ISO, and delete removes the file.

use crate::support::{scratch, Server};
use serde_json::json;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn upload_list_and_delete_pool_content() {
    let dir = scratch("storage-content");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    let pool_dir = dir.join("pool");
    std::fs::create_dir_all(&pool_dir).unwrap();
    let pool = api
        .ok(
            reqwest::Method::POST,
            "/storage/pools",
            Some(json!({"name": "content", "storage_type": "directory",
                        "path": pool_dir.to_string_lossy()})),
        )
        .await;
    let pid = pool["id"].as_str().unwrap().to_string();

    let (s, types) = api.get("/storage/content-types").await;
    assert_eq!(s, 200);
    assert!(types
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"] == "snippets" && t["uploadable"] == true));

    // 3 MiB of non-trivial data, streamed.
    let data: Vec<u8> = (0..3 * 1024 * 1024u32)
        .map(|i| (i * 7 % 251) as u8)
        .collect();
    let sha = hex::encode(Sha256::digest(&data));
    let up = |content: &str, name: &str, sha: Option<&str>, body: Vec<u8>| {
        let mut url = format!(
            "{}/storage/pools/{pid}/upload?content={content}&filename={name}",
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

    // wrong checksum: refused, nothing left behind (not even the .part file)
    let (s, m) = up("iso", "t.iso", Some(&"0".repeat(64)), data.clone()).await;
    assert_eq!(s, 400, "{m}");
    assert!(m.contains("sha256 mismatch"), "{m}");
    let iso_dir = pool_dir.join("template/iso");
    assert_eq!(std::fs::read_dir(&iso_dir).unwrap().count(), 0);

    // names / extensions / types
    for (c, n) in [
        ("iso", "t.sh"),
        ("iso", "..%2Fx.iso"),
        ("iso", "a%2Fb.iso"),
        ("snippets", ".hidden"),
        ("nope", "t.iso"),
        ("rootdir", "t.iso"),
    ] {
        // small body: the server refuses before reading, a big one would hit a closed pipe
        let (s, m) = up(c, n, None, data[..1000].to_vec()).await;
        assert_eq!(s, 400, "{c}/{n}: {m}");
    }
    assert!(!pool_dir.join("x.iso").exists());
    let (s, _) = up("iso", "empty.iso", None, Vec::new()).await;
    assert_eq!(s, 400);

    // good upload
    let (s, m) = up("iso", "t.iso", Some(&sha), data.clone()).await;
    assert_eq!(s, 201, "{m}");
    assert_eq!(std::fs::read(iso_dir.join("t.iso")).unwrap(), data);
    assert_eq!(std::fs::read_dir(&iso_dir).unwrap().count(), 1, "no .part");
    let (s, _) = up("iso", "t.iso", None, data[..1000].to_vec()).await;
    assert_eq!(s, 409, "no overwrite");

    // a snippet, then list per type
    let (s, m) = up(
        "snippets",
        "user-data.yaml",
        None,
        b"#cloud-config\n".to_vec(),
    )
    .await;
    assert_eq!(s, 201, "{m}");
    let (_, l) = api
        .get(&format!("/storage/pools/{pid}/content?content=iso"))
        .await;
    assert_eq!(l.as_array().unwrap().len(), 1);
    assert_eq!(l[0]["name"], "t.iso");
    assert_eq!(l[0]["size"], data.len());
    let (_, all) = api.get(&format!("/storage/pools/{pid}/content")).await;
    assert_eq!(all.as_array().unwrap().len(), 2);

    // the ISO library (VM CD-ROM picker) sees the uploaded ISO
    let (_, isos) = api.get("/storage/isos").await;
    assert!(
        isos.as_array()
            .unwrap()
            .iter()
            .any(|i| i["name"] == "t.iso"),
        "{isos}"
    );

    // delete
    assert_eq!(
        api.delete(&format!("/storage/pools/{pid}/content/iso/t.iso"))
            .await,
        204
    );
    assert!(!iso_dir.join("t.iso").exists());
    assert_eq!(
        api.delete(&format!("/storage/pools/{pid}/content/iso/t.iso"))
            .await,
        404
    );
    assert_eq!(
        api.delete(&format!("/storage/pools/{pid}/content/iso/..%2Fx.iso"))
            .await,
        400
    );
    let (s, _) = api.get("/storage/pools/nope/content").await;
    assert_eq!(s, 404);
}
