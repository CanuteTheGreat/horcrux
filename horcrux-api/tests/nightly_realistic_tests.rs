//! Nightly "realistic" end-to-end tests for Horcrux.
//!
//! Unlike `integration_tests.rs` (which only talks HTTP to a running
//! `horcrux-api` server and checks status codes / JSON shapes), these tests
//! call the production manager code directly and assert on *real* external
//! effects:
//!   - a genuine QEMU guest OS actually boots to a login prompt (serial
//!     console output is captured and inspected, not faked)
//!   - a genuine Docker container is created, started, and a command is
//!     executed inside it via `docker exec`, with real stdout checked
//!   - genuine CNI plugins (bridge + host-local) are exec'd against real
//!     Linux network namespaces, and real ICMP connectivity between two
//!     namespaces over the resulting veth/bridge is verified
//!
//! These tests are expensive, require root/CAP_NET_ADMIN, a working Docker
//! daemon, qemu-system-x86_64/qemu-img on PATH, and real CNI plugin
//! binaries, so they are `#[ignore]`d by default. They are NOT run by the
//! per-push CI (`cargo test --workspace`) - only by
//! `.forgejo/workflows/nightly-realistic-test.yml`, explicitly via:
//!   cargo test --test nightly_realistic_tests -- --ignored --test-threads=1 --nocapture
//!
//! Run with: cargo test --test nightly_realistic_tests -- --ignored

use horcrux_api::container::ContainerManager;
use horcrux_api::sdn::cni::{CniConfig, CniManager, CniPluginType, IpamConfig};
use horcrux_api::vm::qemu::QemuManager;
use horcrux_common::{
    ContainerConfig, ContainerRuntime, ContainerStatus, VmArchitecture, VmConfig, VmDisk,
    VmHypervisor, VmStatus,
};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::time::sleep;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn run(cmd: &str, args: &[&str]) -> std::process::Output {
    Command::new(cmd)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn `{} {:?}`: {}", cmd, args, e))
}

fn run_ok(cmd: &str, args: &[&str], context: &str) {
    let out = run(cmd, args);
    assert!(
        out.status.success(),
        "{context}: `{} {:?}` failed: stdout={} stderr={}",
        cmd,
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------
// 1. Real QEMU VM boot
// ---------------------------------------------------------------------
//
// Boots the real CirrOS test-cloud-image (the same tiny ~20MB image the
// OpenStack project publishes specifically for exercising hypervisors end
// to end) under QEMU/TCG and asserts the guest kernel genuinely reaches a
// login prompt by scraping the serial console log qemu.rs now redirects
// to disk. A blank/unbootable disk would just sit forever with no serial
// output, so this cannot pass without a real guest OS actually executing.
#[tokio::test]
#[ignore]
async fn test_real_qemu_vm_boot() {
    let cirros_image = env_or(
        "NIGHTLY_CIRROS_IMAGE",
        "/tmp/horcrux-nightly/cirros-disk.img",
    );
    assert!(
        PathBuf::from(&cirros_image).exists(),
        "CirrOS test image not found at {cirros_image} - the workflow's \
         download step must run before this test"
    );

    let storage_dir = PathBuf::from(env_or("NIGHTLY_VM_STORAGE", "/tmp/horcrux-nightly/vms"));
    tokio::fs::create_dir_all(&storage_dir).await.unwrap();

    let manager = QemuManager::with_storage_path(storage_dir.clone());

    let vm_id = "nightly-cirros-boot-test";
    let config = VmConfig {
        id: vm_id.to_string(),
        name: "Nightly CirrOS Boot Test".to_string(),
        hypervisor: VmHypervisor::Qemu,
        memory: 256,
        cpus: 1,
        disk_size: 1,
        status: VmStatus::Stopped,
        architecture: VmArchitecture::X86_64,
        disks: Vec::<VmDisk>::new(),
    };

    // create_vm() lays down a blank qcow2 at storage_dir/{id}.qcow2 via the
    // real `qemu-img create` code path - exercise that for real, then swap
    // in genuine bootable guest content before starting.
    let vm = manager
        .create_vm(&config)
        .await
        .expect("create_vm should succeed");

    tokio::fs::copy(&cirros_image, &vm.disk_path)
        .await
        .expect("failed to install CirrOS image onto the VM disk");

    manager
        .start_vm(&vm)
        .await
        .expect("start_vm should spawn a real qemu-system-x86_64 process");

    let serial_log = vm.disk_path.with_extension("serial.log");
    let deadline = Instant::now() + Duration::from_secs(120);
    let boot_marker = "login as 'cirros' user";
    let mut last_seen = String::new();
    let mut booted = false;

    while Instant::now() < deadline {
        if let Ok(contents) = tokio::fs::read_to_string(&serial_log).await {
            last_seen = contents.clone();
            if contents.contains(boot_marker) {
                booted = true;
                break;
            }
        }
        sleep(Duration::from_secs(2)).await;
    }

    // Always try to stop/delete the VM, even if the boot assertion below
    // fails, so a nightly failure doesn't leak a running qemu process.
    let stop_result = manager.stop_vm(&vm).await;
    let _ = tokio::fs::remove_file(&serial_log).await;
    let delete_result = manager.delete_vm(&vm).await;

    assert!(
        booted,
        "CirrOS never reached its login prompt within 120s. Last serial \
         output (<=4000 chars shown):\n{}",
        &last_seen[last_seen.len().saturating_sub(4000)..]
    );
    stop_result.expect("stop_vm should succeed after a real boot");
    delete_result.expect("delete_vm should succeed after stopping");
}

// ---------------------------------------------------------------------
// 2. Real Docker container lifecycle
// ---------------------------------------------------------------------
//
// Goes through ContainerManager (the same routing code horcrux-api's HTTP
// handlers use) with runtime=Docker, so this exercises the real
// ContainerManager -> DockerManager dispatch plus genuine `docker create`
// / `docker start` / `docker exec` / `docker stop` / `docker rm` shell-outs
// against docker-pulled-from-the-real-daemon (automounted
// /var/run/docker.sock on the Forgejo runner host, not a mock).
#[tokio::test]
#[ignore]
async fn test_real_docker_container_lifecycle() {
    let manager = ContainerManager::new();
    let container_id = "nightly-docker-test";
    let container_name = "horcrux-nightly-docker-test";

    // Best-effort cleanup from a previous failed run.
    let _ = run("docker", &["rm", "-f", container_name]);

    let config = ContainerConfig {
        id: container_id.to_string(),
        name: container_name.to_string(),
        runtime: ContainerRuntime::Docker,
        memory: 128,
        cpus: 1,
        rootfs: "alpine:3.20".to_string(),
        status: ContainerStatus::Stopped,
    };

    manager
        .create_container(config)
        .await
        .expect("create_container (docker) should succeed");

    manager
        .start_container(container_id)
        .await
        .expect("start_container (docker) should succeed");

    // Confirm with `docker ps` directly (bypassing horcrux) that a real
    // running container exists - proves this isn't just an in-memory
    // status flag flip.
    let ps = run(
        "docker",
        &[
            "ps",
            "--filter",
            &format!("name={container_name}"),
            "--filter",
            "status=running",
            "--format",
            "{{.Names}}",
        ],
    );
    let ps_out = String::from_utf8_lossy(&ps.stdout);
    assert!(
        ps_out.contains(container_name),
        "docker ps does not show {container_name} as running: {ps_out}"
    );

    let exec_output = manager
        .exec_command(
            container_id,
            vec!["echo".to_string(), "horcrux-nightly-ok".to_string()],
        )
        .await
        .expect("exec_command (docker) should succeed");
    assert!(
        exec_output.contains("horcrux-nightly-ok"),
        "docker exec output did not contain expected marker: {exec_output}"
    );

    manager
        .stop_container(container_id)
        .await
        .expect("stop_container (docker) should succeed");

    manager
        .delete_container(container_id)
        .await
        .expect("delete_container (docker) should succeed");

    // Verify real cleanup: the container must be genuinely gone from Docker.
    let ps_after = run(
        "docker",
        &[
            "ps",
            "-a",
            "--filter",
            &format!("name={container_name}"),
            "--format",
            "{{.Names}}",
        ],
    );
    assert!(
        String::from_utf8_lossy(&ps_after.stdout).trim().is_empty(),
        "container {container_name} should no longer exist in docker ps -a"
    );
}

// ---------------------------------------------------------------------
// 3. Real CNI networking between two network namespaces
// ---------------------------------------------------------------------
//
// Creates two real Linux network namespaces with `ip netns add`, then
// drives CniManager::add_container() - the exact code the SDN module uses
// in production - which execs the real `bridge` and `host-local` CNI
// plugin binaries following the standard CNI exec protocol (CNI_COMMAND/
// CNI_NETNS/CNI_IFNAME env vars + JSON config on stdin). Success is judged
// by genuine ICMP connectivity between the two namespaces across the
// bridge CNI created, not by checking that the API call merely returned
// 200.
#[tokio::test]
#[ignore]
async fn test_real_cni_networking() {
    let cni_bin_dir = PathBuf::from(env_or("NIGHTLY_CNI_BIN_DIR", "/opt/cni/bin"));
    assert!(
        cni_bin_dir.join("bridge").exists() && cni_bin_dir.join("host-local").exists(),
        "real CNI plugin binaries (bridge, host-local) not found under {} - \
         the workflow must install containernetworking-plugins first",
        cni_bin_dir.display()
    );
    let cni_conf_dir = PathBuf::from(env_or(
        "NIGHTLY_CNI_CONF_DIR",
        "/tmp/horcrux-nightly/cni-conf",
    ));

    let ns1 = "hcx-nightly-ns1";
    let ns2 = "hcx-nightly-ns2";
    let bridge_name = "hcx-nightly0";
    let network_name = "nightly-test-bridge";

    // Clean slate.
    let _ = run("ip", &["netns", "del", ns1]);
    let _ = run("ip", &["netns", "del", ns2]);
    let _ = run("ip", &["link", "del", bridge_name]);

    run_ok("ip", &["netns", "add", ns1], "create netns 1");
    run_ok("ip", &["netns", "add", ns2], "create netns 2");

    let mut cni = CniManager::new(cni_bin_dir, cni_conf_dir);

    let network_config = CniConfig {
        cni_version: "1.0.0".to_string(),
        name: network_name.to_string(),
        plugin_type: CniPluginType::Bridge,
        bridge: Some(bridge_name.to_string()),
        ipam: IpamConfig {
            ipam_type: "host-local".to_string(),
            subnet: Some("10.250.77.0/24".to_string()),
            range_start: Some("10.250.77.10".parse().unwrap()),
            range_end: Some("10.250.77.250".parse().unwrap()),
            gateway: Some("10.250.77.1".parse().unwrap()),
            routes: Vec::new(),
        },
        dns: None,
        capabilities: Default::default(),
    };

    cni.create_network(network_config)
        .await
        .expect("create_network should write a real CNI conflist");

    let netns1_path = format!("/var/run/netns/{ns1}");
    let netns2_path = format!("/var/run/netns/{ns2}");

    let result1 = cni
        .add_container("nightly-container-1", network_name, "eth0", &netns1_path)
        .await
        .expect("CNI ADD into ns1 should succeed (real bridge+host-local plugin exec)");
    let result2 = cni
        .add_container("nightly-container-2", network_name, "eth0", &netns2_path)
        .await
        .expect("CNI ADD into ns2 should succeed (real bridge+host-local plugin exec)");

    let ip1 = result1
        .ips
        .first()
        .expect("CNI ADD result for ns1 should include an assigned IP")
        .address
        .split('/')
        .next()
        .unwrap()
        .to_string();
    let ip2 = result2
        .ips
        .first()
        .expect("CNI ADD result for ns2 should include an assigned IP")
        .address
        .split('/')
        .next()
        .unwrap()
        .to_string();

    assert_ne!(
        ip1, ip2,
        "the two namespaces must get distinct IPAM addresses"
    );

    // The real test: genuine ICMP across the veth/bridge CNI just built.
    let ping = run(
        "ip",
        &["netns", "exec", ns1, "ping", "-c", "3", "-W", "2", &ip2],
    );
    let ping_out = format!(
        "{}{}",
        String::from_utf8_lossy(&ping.stdout),
        String::from_utf8_lossy(&ping.stderr)
    );

    // Best-effort teardown regardless of the ping result, so failures
    // don't leak namespaces/bridges between nightly runs.
    let _ = cni
        .del_container("nightly-container-1", network_name, "eth0", &netns1_path)
        .await;
    let _ = cni
        .del_container("nightly-container-2", network_name, "eth0", &netns2_path)
        .await;
    let _ = cni.delete_network(network_name).await;
    let _ = run("ip", &["netns", "del", ns1]);
    let _ = run("ip", &["netns", "del", ns2]);
    let _ = run("ip", &["link", "del", bridge_name]);

    assert!(
        ping.status.success(),
        "real ping from {ns1} ({ip1}) to {ns2} ({ip2}) over the CNI bridge \
         failed:\n{ping_out}"
    );
}
