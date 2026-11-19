//! Real-kernel end-to-end check of the gaming-VM host plumbing (managed PCI
//! hand-off, huge pages, Hyper-V arguments, CPU pinning) against a real
//! IOMMU, real drivers and a real QEMU.
//!
//! Ignored by default: it changes the driver of a PCI device. It runs as root
//! inside a throw-away VM that has an emulated IOMMU (`-device intel-iommu`)
//! and a spare emulated PCI display (`-device bochs-display`), see
//! `ci/gaming-handoff-e2e.sh`:
//!
//!   HX_HANDOFF_E2E_DEVICE=0000:01:00.0 handoff_e2e --ignored --nocapture
use horcrux_common::{VmAdvanced, VmCpuPinning, VmHostPci, VmHugePages};
use horcrux_vm::vm::handoff::{self, Host};
use horcrux_vm::vm::tuning;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

fn driver(addr: &str) -> String {
    std::fs::read_link(format!("/sys/bus/pci/devices/{addr}/driver"))
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .unwrap_or_else(|_| "none".into())
}

fn step(s: &str) {
    println!("ok - {s}");
}

async fn qmp(sock: &Path, cmd: serde_json::Value) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let s = tokio::net::UnixStream::connect(sock)
        .await
        .expect("qmp connect");
    let (r, mut w) = s.into_split();
    let mut lines = BufReader::new(r).lines();
    lines.next_line().await.unwrap(); // greeting
    w.write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
        .await
        .unwrap();
    lines.next_line().await.unwrap();
    w.write_all(format!("{cmd}\n").as_bytes()).await.unwrap();
    loop {
        let l = lines.next_line().await.unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&l).unwrap();
        if v.get("return").is_some() || v.get("error").is_some() {
            return v;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn handoff_e2e() {
    let addr = std::env::var("HX_HANDOFF_E2E_DEVICE").expect("HX_HANDOFF_E2E_DEVICE");
    let work = tempfile::tempdir().unwrap();
    let disk = work.path().join("gaming.qcow2");
    let host = Host::default();
    let host_driver = driver(&addr);
    assert_ne!(
        host_driver, "vfio-pci",
        "start with the device on its host driver"
    );
    step(&format!("{addr} starts on its host driver {host_driver}"));

    // the report sees the device, its group and the host's display elsewhere
    let rep = handoff::host_report(&host, &[]);
    let gpu = rep
        .gpus
        .iter()
        .find(|g| g.address == addr)
        .expect("device in the gaming report");
    println!("# report: {}", serde_json::to_string(gpu).unwrap());
    assert_eq!(gpu.functions, vec![addr.clone()]);
    step(&format!(
        "gaming report: verdict {} for {addr}",
        gpu.verdict
    ));

    let hp = vec![VmHostPci {
        device: addr.clone(),
        managed: true,
        // a test VM whose only connected screen is this device
        allow_display_loss: gpu.verdict == "single-gpu",
        ..Default::default()
    }];

    // 1. a program holding the device blocks the hand-off, named
    let nodes = host.device_nodes(std::slice::from_ref(&addr));
    let card = nodes
        .iter()
        .find(|p| p.to_string_lossy().contains("/dri/card"))
        .expect("a DRM card node")
        .clone();
    let mut holder = Command::new("sh")
        .arg("-c")
        .arg(format!("exec 3<{} ; exec sleep 300", card.display()))
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let e = handoff::acquire(&host, &disk, &hp)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("still in use") && e.contains("sleep"), "{e}");
    assert_eq!(driver(&addr), host_driver);
    step(&format!("hand-off refused while held: {e}"));
    holder.kill().unwrap();
    holder.wait().unwrap();

    // 2. hand-off
    handoff::acquire(&host, &disk, &hp).await.unwrap();
    assert_eq!(driver(&addr), "vfio-pci");
    assert!(handoff::record_file(&disk).exists());
    step(&format!(
        "{addr} moved {host_driver} -> vfio-pci, record written"
    ));

    // 3. huge pages for a 256 MiB guest
    let before = tuning::hugepage_pools(Path::new("/sys"));
    let res = tuning::reserve_hugepages(Path::new("/sys"), 2048, tuning::pages_needed(256, 2048))
        .unwrap();
    step(&format!(
        "huge pages reserved: {res:?} (pools before {before:?})"
    ));

    // 4. a real QEMU with the device, Hyper-V flags and huge-page memory
    let mut adv = VmAdvanced {
        hyperv: true,
        hugepages: Some(VmHugePages { size_kb: 2048 }),
        ..Default::default()
    };
    let cpu = tuning::cpu_arg(None, &adv, &Default::default()).unwrap();
    let qmp_sock = work.path().join("qmp.sock");
    let pidf = work.path().join("qemu.pid");
    let mut args: Vec<String> = vec![
        "-machine".into(),
        "q35,accel=kvm".into(),
        "-cpu".into(),
        cpu.clone(),
        "-smp".into(),
        "2".into(),
        "-m".into(),
        "256".into(),
        "-object".into(),
        format!(
            "{},id=hxnode0,size=256M",
            tuning::hugepage_backend(adv.hugepages.as_ref().unwrap(), false)
        ),
        "-numa".into(),
        "node,nodeid=0,cpus=0-1,memdev=hxnode0".into(),
        "-device".into(),
        format!("vfio-pci,host={addr},id=hostpci0"),
        "-display".into(),
        "none".into(),
        "-vga".into(),
        "none".into(),
        "-qmp".into(),
        format!("unix:{},server=on,wait=off", qmp_sock.display()),
        "-pidfile".into(),
        pidf.display().to_string(),
        "-daemonize".into(),
    ];
    args.extend(tuning::device_args(&adv));
    let out = Command::new("qemu-system-x86_64")
        .args(&args)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "qemu: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pid: i32 = std::fs::read_to_string(&pidf)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    step(&format!(
        "QEMU {pid} runs with vfio-pci {addr}, -cpu {cpu}, huge-page memory"
    ));
    let pools = tuning::hugepage_pools(Path::new("/sys"));
    let used = pools
        .iter()
        .find(|p| p.0 == 2048)
        .map(|p| p.1 - p.2)
        .unwrap_or(0);
    assert!(used >= 128, "guest memory is not on huge pages: {pools:?}");
    step(&format!("{used} huge pages in use by the guest"));

    // 5. CPU pinning from QMP thread ids
    let reply = qmp(&qmp_sock, serde_json::json!({"execute": "query-cpus-fast"})).await;
    let threads = tuning::vcpu_threads_from_qmp(&reply);
    assert_eq!(threads.len(), 2, "{reply}");
    let hc = tuning::HostCpus::read(Path::new("/sys"));
    adv.cpu_pinning = Some(VmCpuPinning {
        vcpus: "auto".into(),
        emulator: None,
    });
    let pin = tuning::resolve_pinning(adv.cpu_pinning.as_ref().unwrap(), &hc, 2).unwrap();
    tuning::apply_pinning(Path::new("/proc"), pid, &threads, &pin).unwrap();
    for (i, t) in threads.iter().enumerate() {
        let st = std::fs::read_to_string(format!("/proc/{pid}/task/{t}/status")).unwrap();
        let allowed = st
            .lines()
            .find_map(|l| l.strip_prefix("Cpus_allowed_list:"))
            .unwrap()
            .trim()
            .to_string();
        assert_eq!(allowed, pin.vcpus[i].to_string(), "vCPU {i}");
    }
    step(&format!(
        "vCPU threads {threads:?} pinned to {:?}, QEMU threads to {:?}",
        pin.vcpus, pin.emulator
    ));

    // 6. release waits while QEMU still holds the group
    handoff::release(&host, &disk, Duration::from_secs(2)).await;
    assert_eq!(driver(&addr), "vfio-pci");
    assert!(handoff::record_file(&disk).exists());
    step("release waited: QEMU still holds the VFIO group, device kept on vfio-pci");

    // 7. QEMU exits (guest power-off) -> device back on its host driver
    let _ = qmp(&qmp_sock, serde_json::json!({"execute": "quit"})).await;
    for _ in 0..50 {
        if !Path::new(&format!("/proc/{pid}")).exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    handoff::release(&host, &disk, Duration::from_secs(30)).await;
    assert_eq!(driver(&addr), host_driver);
    assert!(!handoff::record_file(&disk).exists());
    step(&format!("after QEMU exit {addr} is back on {host_driver}"));
    tuning::release_hugepages(Path::new("/sys"), &res).unwrap();
    step("huge pages given back");
    // the host driver works again: its DRM node is back
    assert!(!host.device_nodes(std::slice::from_ref(&addr)).is_empty());
    step("host driver re-created its device nodes");
}
