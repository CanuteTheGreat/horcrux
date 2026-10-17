//! Customer journey: a VM from a cloud image, all the way to a restored copy.
//!
//! Real QEMU (KVM when the host has /dev/kvm, else TCG), a real CirrOS
//! guest, every step through the HTTP API, every effect checked inside the
//! guest over its serial console:
//!
//! create VM -> import the cloud image as its disk -> boot -> log in on the
//! serial console and write a marker file -> live snapshot with RAM ->
//! write a second file -> live disk-only snapshot -> roll back (running) to
//! the first snapshot: the second file is gone, the marker is there ->
//! disk-only rollback of a running VM is refused -> stop -> full clone ->
//! backup -> restore to a new VM (disk content identical) -> boot the
//! clone and the restored VM: each one has the marker.
//!
//! Nightly (`--ignored`): needs qemu-system-x86_64 + qemu-img and
//! HORCRUX_DEPTH_IMAGE pointing at a CirrOS qcow2. Set
//! HORCRUX_DEPTH_REQUIRE_KVM=1 to fail unless the guest really ran on KVM.

use crate::support::*;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn sock(disk: &Path, ext: &str) -> PathBuf {
    disk.with_extension(ext)
}

async fn disk_of(a: &Api, id: &str) -> PathBuf {
    let (s, vm) = a.get(&format!("/vms/{id}")).await;
    assert_eq!(s, 200, "{vm}");
    PathBuf::from(vm["disks"][0]["path"].as_str().expect("disk path"))
}

async fn start(a: &Api, id: &str) -> u32 {
    let (s, b) = a.post(&format!("/vms/{id}/start"), json!({})).await;
    assert_eq!(s, 200, "start {id}: {b}");
    assert_eq!(b["status"], "running", "{b}");
    let mut pid = None;
    wait_until(&format!("QEMU of {id}"), 30, || {
        pid = qemu_pid(id);
        pid.is_some()
    })
    .await;
    pid.unwrap()
}

async fn stop(a: &Api, id: &str) {
    let (s, b) = a.post(&format!("/vms/{id}/stop"), json!({})).await;
    assert_eq!(s, 200, "stop {id}: {b}");
    wait_until(&format!("QEMU of {id} to exit"), 60, || {
        qemu_pid(id).is_none()
    })
    .await;
    let (_, vm) = a.get(&format!("/vms/{id}")).await;
    assert_eq!(vm["status"], "stopped", "{vm}");
}

fn boot_secs() -> u64 {
    if Path::new("/dev/kvm").exists() {
        180
    } else {
        900
    }
}

async fn seed(a: &Api, id: &str) {
    let (s, ci) = a
        .post(
            &format!("/cloudinit/{id}"),
            json!({"hostname": "journey", "user": null, "ssh_keys": [], "network": null,
                   "packages": [], "runcmd": []}),
        )
        .await;
    assert_eq!(s, 200, "cloud-init seed for {id}: {ci}");
}

async fn read_marker(a: &Api, id: &str) -> String {
    seed(a, id).await;
    start(a, id).await;
    let disk = disk_of(a, id).await;
    let mut con = Serial::connect(&sock(&disk, "serial.sock"));
    con.cirros_login(boot_secs());
    let m = con.sh("cat /home/cirros/marker", 60);
    stop(a, id).await;
    m
}

#[tokio::test]
#[ignore = "nightly depth suite: real QEMU guest (HORCRUX_DEPTH_IMAGE)"]
async fn vm_from_cloud_image_through_snapshot_clone_backup_restore() {
    let t0 = Instant::now();
    let image = cloud_image();
    let dir = scratch("vm-journey");
    let mut srv = Server::new(&dir);
    srv.start();
    srv.wait_healthy().await;
    let a = srv.api().await;

    let tag = std::process::id();
    let id = format!("jv{tag}");
    let clone_id = format!("jv{tag}-clone");
    let restored_id = format!("jv{tag}-restored");

    // The image is put where an admin puts images: <data>/images.
    let images = srv.data().join("images");
    std::fs::create_dir_all(&images).unwrap();
    let src = images.join("cirros.qcow2");
    std::fs::copy(&image, &src).unwrap();

    let (s, b) = a
        .post(
            "/vms",
            json!({"id": id, "name": "journey", "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 256, "disk_size": 1, "status": "stopped",
                   "display": {"protocol": "none"}}),
        )
        .await;
    assert_eq!(s, 201, "create vm: {b}");
    let (s, b) = a
        .post(
            &format!("/vms/{id}/import-disk"),
            json!({"source": src.to_string_lossy()}),
        )
        .await;
    assert_eq!(s, 200, "import cloud image: {b}");
    let disk = disk_of(&a, &id).await;
    let info = run(
        "qemu-img",
        &["info", "--output=json", &disk.to_string_lossy()],
    );
    let info: serde_json::Value = serde_json::from_str(&info).unwrap();
    assert_eq!(info["format"], "qcow2");
    assert!(
        info["virtual-size"].as_u64().unwrap() >= 1 << 30,
        "disk grown to the VM's 1 GB: {info}"
    );
    // Same guest-visible content (the grown tail reads as zeros).
    let cmp = run(
        "qemu-img",
        &["compare", &src.to_string_lossy(), &disk.to_string_lossy()],
    );
    assert!(
        cmp.contains("identical"),
        "imported disk = cloud image: {cmp}"
    );

    // Cloud-init seed (NoCloud "cidata" CD-ROM), so the guest configures
    // itself from it instead of waiting minutes for an EC2 metadata server.
    let (s, ci) = a
        .post(
            &format!("/cloudinit/{id}"),
            json!({"hostname": "journey", "user": null, "ssh_keys": [], "network": null,
                   "packages": [], "runcmd": []}),
        )
        .await;
    assert_eq!(s, 200, "cloud-init seed: {ci}");
    assert_eq!(
        ci["attached_as"].as_str(),
        Some(disk.with_extension("cidata.iso").to_string_lossy().as_ref()),
        "{ci}"
    );

    // Boot and log in on the serial console.
    let pid = start(&a, &id).await;
    let blocks = qmp(&sock(&disk, "qmp.sock"), json!({"execute": "query-block"}));
    assert!(
        blocks.to_string().contains("cidata.iso"),
        "seed attached as a drive: {blocks}"
    );
    let qmp_sock = sock(&disk, "qmp.sock");
    let kvm = qmp(&qmp_sock, json!({"execute": "query-kvm"}));
    eprintln!("query-kvm: {kvm}");
    if std::env::var("HORCRUX_DEPTH_REQUIRE_KVM").is_ok() {
        assert_eq!(
            kvm["return"]["enabled"], true,
            "guest must run on KVM: {kvm}"
        );
    }
    let mut con = Serial::connect(&sock(&disk, "serial.sock"));
    let tb = Instant::now();
    con.cirros_login(boot_secs());
    eprintln!("guest booted to a shell in {:?}", tb.elapsed());
    let host = con.sh("hostname", 30);
    assert!(
        host.contains("journey"),
        "cloud-init set the hostname: {host:?}"
    );
    let marker = format!("horcrux-journey-{tag}");
    let out = con.sh(
        &format!("echo {marker} > /home/cirros/marker; sync; cat /home/cirros/marker"),
        60,
    );
    assert!(
        out.contains(&marker),
        "marker written in the guest: {out:?}"
    );
    assert!(
        std::fs::read_to_string(sock(&disk, "serial.log"))
            .unwrap_or_default()
            .contains("login:"),
        "serial log records the console"
    );

    // Live snapshot with RAM: same QEMU process, guest keeps running.
    let (s, s1) = a
        .post(
            &format!("/vms/{id}/snapshots"),
            json!({"name": "s1", "description": "with RAM", "include_memory": true}),
        )
        .await;
    assert_eq!(s, 200, "live snapshot with RAM: {s1}");
    assert_eq!(s1["vm_state"], "Running", "{s1}");
    assert_eq!(qemu_pid(&id), Some(pid), "same QEMU process");
    let st = qmp(&qmp_sock, json!({"execute": "query-status"}));
    assert_eq!(st["return"]["status"], "running", "{st}");
    let s1_id = s1["id"].as_str().unwrap().to_string();

    let out = con.sh("echo later > /home/cirros/after; sync; ls /home/cirros", 60);
    assert!(out.contains("after"), "{out:?}");

    // Live disk-only snapshot: paused for the snapshot, resumed after.
    let (s, s2) = a
        .post(
            &format!("/vms/{id}/snapshots"),
            json!({"name": "s2", "include_memory": false}),
        )
        .await;
    assert_eq!(s, 200, "live disk-only snapshot: {s2}");
    assert_eq!(s2["vm_state"], "Paused", "{s2}");
    let st = qmp(&qmp_sock, json!({"execute": "query-status"}));
    assert_eq!(
        st["return"]["status"], "running",
        "resumed after snapshot: {st}"
    );
    let s2_id = s2["id"].as_str().unwrap().to_string();
    let snaps = run(
        "qemu-img",
        &["snapshot", "-U", "-l", &disk.to_string_lossy()],
    );
    assert!(snaps.contains("s1") && snaps.contains("s2_0"), "{snaps}");

    // A disk-only snapshot can't be rolled back under a running guest.
    let (s, b) = a
        .post(&format!("/vms/{id}/snapshots/{s2_id}/restore"), json!({}))
        .await;
    assert_eq!(s, 409, "disk-only rollback of a running VM refused: {b}");

    // Rolling back (running) to s1 restores RAM and disk to that point.
    let (s, b) = a
        .post(&format!("/vms/{id}/snapshots/{s1_id}/restore"), json!({}))
        .await;
    assert_eq!(s, 200, "live rollback: {b}");
    assert_eq!(qemu_pid(&id), Some(pid), "rollback keeps the QEMU process");
    con.send("\n");
    let out = con.sh("ls /home/cirros; cat /home/cirros/marker", 60);
    assert!(out.contains(&marker), "marker still there: {out:?}");
    assert!(
        !out.contains("after"),
        "file written after s1 is gone: {out:?}"
    );

    // A snapshot id of this VM is not addressable through another VM.
    let (s, _) = a
        .post(
            &format!("/vms/nope-{tag}/snapshots/{s1_id}/restore"),
            json!({}),
        )
        .await;
    assert_eq!(s, 404);

    stop(&a, &id).await;
    drop(con);

    // Delete the disk-only snapshot offline: gone from the image too.
    assert_eq!(a.delete(&format!("/vms/{id}/snapshots/{s2_id}")).await, 200);
    let snaps = run("qemu-img", &["snapshot", "-l", &disk.to_string_lossy()]);
    assert!(!snaps.contains("s2_0"), "{snaps}");

    // Full clone.
    let (s, b) = a
        .post(
            &format!("/vms/{id}/clone"),
            json!({"name": "journey clone", "id": clone_id, "mode": "full"}),
        )
        .await;
    assert_eq!(s, 200, "clone: {b}");

    // Backup + restore to a new VM.
    let (s, bk) = a
        .post(
            "/backups",
            json!({"target_id": id, "mode": "stop", "compression": "zstd", "notes": "journey"}),
        )
        .await;
    assert_eq!(s, 201, "backup: {bk}");
    let bid = bk["id"].as_str().unwrap().to_string();
    let (s, v) = a.post(&format!("/backups/{bid}/verify"), json!({})).await;
    assert_eq!((s, v["ok"].clone()), (200, json!(true)), "{v}");
    let (s, r) = a
        .post(
            &format!("/backups/{bid}/restore"),
            json!({"target_id": restored_id, "name": "journey restored"}),
        )
        .await;
    assert_eq!(s, 200, "restore to new VM: {r}");
    let restored_disk = disk_of(&a, &restored_id).await;
    assert_ne!(restored_disk, disk);
    assert_eq!(
        content_sha(&restored_disk),
        content_sha(&disk),
        "restored disk content identical"
    );

    // Each copy boots and has the marker.
    for vid in [&clone_id, &restored_id] {
        let m = read_marker(&a, vid).await;
        assert!(m.contains(&marker), "{vid}: marker {m:?}");
    }
    // ... and so does the original, after all of that.
    let m = read_marker(&a, &id).await;
    assert!(m.contains(&marker), "original: {m:?}");

    // Clean up through the API: VMs and their disks are removed.
    for vid in [&id, &clone_id, &restored_id] {
        let d = disk_of(&a, vid).await;
        assert!(
            (200..300).contains(&a.delete(&format!("/vms/{vid}")).await),
            "delete {vid}"
        );
        assert!(!d.exists(), "{} removed", d.display());
    }
    let _ = a.delete(&format!("/backups/{bid}")).await;
    srv.term();
    eprintln!("VM journey passed in {:?}", t0.elapsed());
    let _ = std::fs::remove_dir_all(&dir);
}
