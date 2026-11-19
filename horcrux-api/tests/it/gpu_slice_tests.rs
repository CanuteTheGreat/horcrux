//! Hardware GPU slicing over HTTP against a freshly spawned horcrux-api whose
//! sysfs/devfs root (`HORCRUX_SYSFS_ROOT` / `HORCRUX_DEVFS_ROOT`) is a fixture
//! tree: an Intel xe PF with two VFs bound to xe-vfio-pci, `sriov_admin`
//! profiles included. Proves the capability model, the plan/apply/rollback
//! round trip with generations, refusal reasons and the VM save-method route.
//! The kernel behaviour (VF creation, VFIO migration ioctl) needs real
//! hardware; the unit tests in `vm::gpucap` / `vm::sriov` cover the logic.

use crate::support::{scratch, Server};
use serde_json::json;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

fn w(p: &Path, v: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, v).unwrap();
}

fn pci(sys: &Path, addr: &str, vendor: &str, driver: &str, group: &str) {
    let d = sys.join("bus/pci/devices").join(addr);
    fs::create_dir_all(&d).unwrap();
    w(&d.join("vendor"), vendor);
    w(&d.join("device"), "0xe20b");
    w(&d.join("class"), "0x030000");
    let g = sys.join("kernel/iommu_groups").join(group);
    fs::create_dir_all(g.join("devices")).unwrap();
    symlink(&g, d.join("iommu_group")).unwrap();
    symlink(&d, g.join("devices").join(addr)).unwrap();
    let dd = sys.join("bus/pci/drivers").join(driver);
    fs::create_dir_all(&dd).unwrap();
    symlink(&dd, d.join("driver")).unwrap();
}

fn fixture(root: &Path) {
    let sys = root.join("sys");
    let dev = root.join("dev");
    fs::create_dir_all(&dev).unwrap();
    fs::create_dir_all(sys.join("bus/pci/drivers/vfio-pci")).unwrap();
    pci(&sys, "0000:03:00.0", "0x8086", "xe", "10");
    let pf = sys.join("bus/pci/devices/0000:03:00.0");
    w(&pf.join("sriov_totalvfs"), "7\n");
    w(&pf.join("sriov_numvfs"), "2\n");
    w(&pf.join("sriov_drivers_autoprobe"), "0\n");
    for i in 1..=7u32 {
        w(
            &pf.join(format!("sriov_admin/vf{i}/profile/vram_quota")),
            if i <= 2 { "4294967296\n" } else { "0\n" },
        );
    }
    for i in 0..2u32 {
        let a = format!("0000:03:00.{}", i + 1);
        pci(&sys, &a, "0x8086", "xe-vfio-pci", &format!("2{i}"));
        let vf = sys.join("bus/pci/devices").join(&a);
        symlink(&pf, vf.join("physfn")).unwrap();
        symlink(&vf, pf.join(format!("virtfn{i}"))).unwrap();
        fs::create_dir_all(vf.join("vfio-dev").join(format!("vfio{}", i + 1))).unwrap();
        w(&dev.join(format!("vfio/devices/vfio{}", i + 1)), "");
    }
    // a GPU that offers nothing but passthrough
    pci(&sys, "0000:01:00.0", "0x10de", "nvidia", "1");
}

fn rd(p: &Path) -> String {
    fs::read_to_string(p).unwrap().trim().to_string()
}

#[tokio::test]
async fn gpu_capabilities_and_sriov_plan_apply_rollback() {
    let dir = scratch("gpuslice");
    fixture(&dir);
    let pf = dir.join("sys/bus/pci/devices/0000:03:00.0");
    let mut srv = Server::new(&dir)
        .env("HORCRUX_SYSFS_ROOT", dir.join("sys").to_str().unwrap())
        .env("HORCRUX_DEVFS_ROOT", dir.join("dev").to_str().unwrap());
    srv.start();
    srv.wait_healthy().await;
    let api = srv.api().await;

    // capabilities: the xe PF with its VFs, the NVIDIA card with reasons
    let (s, caps) = api.get("/gpu/capabilities").await;
    assert_eq!(s, 200, "{caps}");
    let caps = caps.as_array().unwrap();
    assert_eq!(caps.len(), 2, "VFs are listed under their PF: {caps:?}");
    let xe = caps
        .iter()
        .find(|c| c["address"] == "0000:03:00.0")
        .unwrap();
    assert_eq!(xe["passthrough"]["available"], true);
    assert_eq!(xe["sriov"]["total_vfs"], 7);
    assert_eq!(xe["sriov"]["current_vfs"], 2);
    assert_eq!(xe["sriov"]["per_vf_resources"], true);
    let vfs = xe["sriov"]["vfs"].as_array().unwrap();
    assert_eq!(vfs.len(), 2);
    assert_eq!(vfs[0]["address"], "0000:03:00.1");
    assert_eq!(vfs[0]["vram_mib"], 4096);
    // the fixture has no /dev/iommu: the migration state is "unknown" with a
    // reason, never guessed from the driver name
    assert_eq!(vfs[0]["migration"]["state"], "unknown", "{xe}");
    assert!(!vfs[0]["migration"]["reason"].as_str().unwrap().is_empty());
    let nv = caps
        .iter()
        .find(|c| c["address"] == "0000:01:00.0")
        .unwrap();
    assert!(nv["sriov"].is_null() && nv["vendor_vgpu"].is_null());
    let why = nv["why_not"].to_string();
    assert!(why.contains("passthrough only"), "{why}");

    // config: defaults, validation
    let (s, c) = api.get("/gpu/sriov/config").await;
    assert_eq!(s, 200, "{c}");
    assert_eq!(c["apply_on_boot"], true);
    assert_eq!(c["pfs"], json!([]));
    let dup = json!({"pfs": [
        {"pf": "0000:03:00.0", "num_vfs": 1}, {"pf": "03:00.0", "num_vfs": 1}]});
    let (s, v) = api.put("/gpu/sriov/config", dup).await;
    assert!(s == 400 || s == 422, "{s} {v}");

    // refusals come back as plan errors with the reason, nothing is written
    let bad = json!({"config": {"pfs": [
        {"pf": "0000:01:00.0", "num_vfs": 2},
        {"pf": "0000:03:00.0", "num_vfs": 9},
        {"pf": "0000:09:00.0", "num_vfs": 1}]}});
    let (s, p) = api.post("/gpu/sriov/plan", bad.clone()).await;
    assert_eq!(s, 200, "{p}");
    let errs = p["errors"].to_string();
    assert!(
        errs.contains("not SR-IOV capable") && errs.contains("passthrough only"),
        "{errs}"
    );
    assert!(
        errs.contains("at most 7") && errs.contains("no such PCI device"),
        "{errs}"
    );
    let (s, r) = api.post("/gpu/sriov/apply", bad).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["ok"], false);
    assert_eq!(api.get("/gpu/sriov/generations").await.1, json!([]));
    assert_eq!(rd(&pf.join("sriov_numvfs")), "2");

    // plan + apply a real change: equal 2 GiB per VF
    let cfg = |mib: u64| {
        json!({"settle_timeout_s": 2, "pfs": [
            {"pf": "03:00.0", "num_vfs": 2, "vram_quota_mib": mib, "exec_quantum_ms": 8}]})
    };
    let (s, p) = api
        .post("/gpu/sriov/plan", json!({"config": cfg(2048)}))
        .await;
    assert_eq!(s, 200, "{p}");
    assert_eq!(p["errors"], json!([]), "{p}");
    let acts: Vec<&str> = p["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["action"].as_str().unwrap())
        .collect();
    assert_eq!(acts[0], "disable_vfs", "{acts:?}");
    assert!(
        acts.contains(&"vram_quota") && acts.contains(&"enable_vfs"),
        "{acts:?}"
    );
    assert_eq!(rd(&pf.join("sriov_numvfs")), "2", "plan changes nothing");

    let (s, r) = api
        .post("/gpu/sriov/apply", json!({"config": cfg(2048)}))
        .await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["generation"], 1);
    assert_eq!(
        rd(&pf.join("sriov_admin/vf1/profile/vram_quota")),
        (2048u64 << 20).to_string()
    );
    assert_eq!(rd(&pf.join("sriov_admin/vf2/profile/exec_quantum_ms")), "8");
    // the stored config is what GET returns, and it applies at boot
    let (_, c) = api.get("/gpu/sriov/config").await;
    assert_eq!(c["pfs"][0]["vram_quota_mib"], 2048);
    let (_, caps) = api.get("/gpu/capabilities").await;
    let vf = &caps[0]["sriov"]["vfs"][0];
    assert!(
        vf["vram_mib"] == 2048 || caps[1]["sriov"]["vfs"][0]["vram_mib"] == 2048,
        "{vf}"
    );

    // a second generation, then roll back to the first
    let (_, r) = api
        .post("/gpu/sriov/apply", json!({"config": cfg(1024)}))
        .await;
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["generation"], 2);
    assert_eq!(
        rd(&pf.join("sriov_admin/vf1/profile/vram_quota")),
        (1024u64 << 20).to_string()
    );
    let (s, r) = api.post("/gpu/sriov/rollback", json!({})).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["generation"], 3);
    assert_eq!(
        rd(&pf.join("sriov_admin/vf1/profile/vram_quota")),
        (2048u64 << 20).to_string()
    );
    let (_, g) = api.get("/gpu/sriov/generations").await;
    assert_eq!(g.as_array().unwrap().len(), 3);
    assert_eq!(g[2]["origin"], "rollback");
    let (s, _) = api
        .post("/gpu/sriov/rollback", json!({"generation": 99}))
        .await;
    assert_eq!(s, 404);

    // VM save method: a VM without host devices, and a missing VM
    let (s, v) = api
        .post(
            "/vms",
            json!({"id": "slicevm", "name": "slicevm", "hypervisor": "qemu", "architecture": "x86_64",
                   "cpus": 1, "memory": 256, "disk_size": 1, "status": "stopped"}),
        )
        .await;
    assert!(s == 200 || s == 201, "{v}");
    let (s, v) = api.get("/vms/slicevm/host-device-save").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["summary"], "no host devices");
    assert_eq!(v["host_save"], true);
    assert_eq!(api.get("/vms/nope/host-device-save").await.0, 404);
}
