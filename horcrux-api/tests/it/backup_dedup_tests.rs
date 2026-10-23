//! Horcrux's deduplicating backup format over HTTP against a freshly spawned
//! horcrux-api, with real qcow2 disks (qemu-img/qemu-io): a second backup of a
//! slightly changed disk stores only the changed chunks, both restore to the
//! exact contents of their moment, verify catches a damaged chunk, and
//! deleting backups frees exactly the chunks nobody uses.

use crate::support::{run, scratch, Server};
use serde_json::json;
use std::path::{Path, PathBuf};

fn chunk_dir(storage: &Path) -> PathBuf {
    storage.join(".hxchunks")
}

fn chunk_files(p: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                out.extend(chunk_files(&path));
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn content_sha(disk: &Path, tmp: &Path) -> String {
    let raw = tmp.join("content.raw");
    let _ = std::fs::remove_file(&raw);
    run(
        "qemu-img",
        &[
            "convert",
            "-O",
            "raw",
            &disk.to_string_lossy(),
            &raw.to_string_lossy(),
        ],
    );
    let s = run("sha256sum", &[&raw.to_string_lossy()]);
    let _ = std::fs::remove_file(&raw);
    s.split_whitespace().next().unwrap().to_string()
}

#[tokio::test]
async fn dedup_backups_are_incremental_verified_and_garbage_collected() {
    let dir = scratch("backup-dedup");
    // no grace period for unused chunks, so the test can see them freed
    let mut srv = Server::new(&dir).env("HORCRUX_BACKUP_GC_GRACE_SECS", "0");
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    let (s, v) = api
        .post(
            "/vms",
            json!({"id": "dd-vm", "name": "dd-vm", "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped"}),
        )
        .await;
    assert!(s == 200 || s == 201, "{v}");
    let (_, vm) = api.get("/vms/dd-vm").await;
    let disk = PathBuf::from(vm["disks"][0]["path"].as_str().unwrap());
    // 12 MiB of incompressible data
    let seed = dir.join("seed.raw");
    let mut x: u64 = 0x9e3779b97f4a7c15;
    let data: Vec<u8> = (0..12 * 1024 * 1024)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    std::fs::write(&seed, data).unwrap();
    run(
        "qemu-img",
        &[
            "convert",
            "-f",
            "raw",
            "-O",
            "qcow2",
            &seed.to_string_lossy(),
            &disk.to_string_lossy(),
        ],
    );
    let sha1 = content_sha(&disk, &dir);

    let storage = dir.join("bk");
    let backup = |note: &str| {
        api.post(
            "/backups",
            json!({"target_id": "dd-vm", "mode": "stop", "compression": "dedup",
                   "storage": storage.to_string_lossy(), "notes": note}),
        )
    };
    let (s, b1) = backup("first").await;
    assert_eq!(s, 201, "{b1}");
    assert_eq!(b1["compression"], "dedup");
    let idx1 = PathBuf::from(b1["path"].as_str().unwrap());
    assert!(idx1.extension().unwrap() == "hxidx" && idx1.is_file());
    let after_first = chunk_files(&chunk_dir(&storage));
    assert!(
        after_first.len() >= 3,
        "12 MiB is at least 3 chunks: {}",
        after_first.len()
    );
    let size1 = b1["size"].as_u64().unwrap();
    assert!(
        size1 > 10 * 1024 * 1024,
        "first backup stores everything: {size1}"
    );

    // change 4 KiB of the guest's disk, back up again
    run(
        "qemu-io",
        &["-c", "write -P 0x5a 0 4096", &disk.to_string_lossy()],
    );
    let sha2 = content_sha(&disk, &dir);
    assert_ne!(sha1, sha2);
    let (s, b2) = backup("second").await;
    assert_eq!(s, 201, "{b2}");
    let idx2 = PathBuf::from(b2["path"].as_str().unwrap());
    let after_second = chunk_files(&chunk_dir(&storage));
    let added = after_second.len() - after_first.len();
    assert!(
        (1..=3).contains(&added),
        "the second backup adds only the changed chunks (added {added} of {})",
        after_second.len()
    );
    let size2 = b2["size"].as_u64().unwrap();
    assert!(
        size2 * 2 < size1,
        "incremental size {size2} vs full {size1}"
    );
    // the two backups share chunks: the store is far smaller than two full copies
    let store_bytes: u64 = after_second
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    assert!(store_bytes < size1 * 3 / 2, "{store_bytes} vs {size1}");

    // both verify and both restore to their own moment
    for (b, want) in [(&b1, &sha1), (&b2, &sha2)] {
        let id = b["id"].as_str().unwrap();
        let (s, v) = api.post(&format!("/backups/{id}/verify"), json!({})).await;
        assert_eq!((s, &v["ok"]), (200, &json!(true)), "{v}");
        let copy = format!("dd-copy-{}", &id[..6]);
        let (s, r) = api
            .post(
                &format!("/backups/{id}/restore"),
                json!({"target_id": copy}),
            )
            .await;
        assert_eq!(s, 200, "{r}");
        let (_, c) = api.get(&format!("/vms/{copy}")).await;
        let cdisk = PathBuf::from(c["disks"][0]["path"].as_str().unwrap());
        assert_eq!(&content_sha(&cdisk, &dir), want, "restore of {id}");
        assert_eq!(api.delete(&format!("/vms/{copy}")).await, 204);
    }

    // a damaged chunk is found by verify
    let victim = after_first
        .iter()
        .max_by_key(|p| std::fs::metadata(p).unwrap().len())
        .unwrap();
    let saved = std::fs::read(victim).unwrap();
    let mut bad = saved.clone();
    let mid = bad.len() / 2;
    bad[mid] ^= 0xa5;
    std::fs::write(victim, bad).unwrap();
    let id1 = b1["id"].as_str().unwrap();
    let (s, v) = api.post(&format!("/backups/{id1}/verify"), json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["error"].as_str().unwrap().contains("corrupt"), "{v}");
    std::fs::write(victim, saved).unwrap();
    let (_, v) = api.post(&format!("/backups/{id1}/verify"), json!({})).await;
    assert_eq!(v["ok"], true, "{v}");

    // deleting backup 1 keeps what backup 2 needs; deleting 2 frees the rest
    assert_eq!(api.delete(&format!("/backups/{id1}")).await, 204);
    assert!(!idx1.exists());
    let id2 = b2["id"].as_str().unwrap();
    let (_, v) = api.post(&format!("/backups/{id2}/verify"), json!({})).await;
    assert_eq!(
        v["ok"], true,
        "backup 2 survives the deletion of backup 1: {v}"
    );
    assert_eq!(api.delete(&format!("/backups/{id2}")).await, 204);
    assert!(!idx2.exists());
    // nothing is left of the chunk store once no backup needs it
    assert!(
        chunk_files(&chunk_dir(&storage)).is_empty(),
        "unused chunks are freed with the last backup"
    );
    // ... and a new backup starts a fresh store
    let (s, b3) = backup("third").await;
    assert_eq!(s, 201, "{b3}");
    let (_, v) = api
        .post(
            &format!("/backups/{}/verify", b3["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    assert_eq!(v["ok"], true, "{v}");
}
