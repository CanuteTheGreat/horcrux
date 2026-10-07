//! Real end-to-end test of the VM serial console pipeline: enable -> connect
//! -> read/write, exercised through `horcrux_api::console::ConsoleManager`
//! exactly as the HTTP `/api/console/:vm_id/serial` route and its WebSocket
//! bridge use it (see `horcrux-api/src/console/{mod,serial,serial_ws}.rs`).
//!
//! `ConsoleManager::ensure_serial_enabled` requires a running process whose
//! command line matches `qemu.*<vm_id>` (via `pgrep -f`) before it will hand
//! back a console. Spinning up a full bootable QEMU guest is unnecessary (and
//! slow/flaky) just to prove the *console plumbing* works, so this test
//! stands up a lightweight stand-in process: a shell renamed (via `exec -a`)
//! to look like a qemu process for that VM ID, backed by `socat` serving the
//! exact Unix socket path the production code computes
//! (`/var/run/qemu-server/<vm_id>.serial`) with a `tee` sink so writes are
//! independently verifiable and echoed back for reads. This is a real
//! process, a real Unix socket, and the real `SerialManager`/`ConsoleManager`
//! code path - only the "QEMU" at the far end is a stub, as the task allows.

use horcrux_api::console::{ConsoleManager, ConsoleType};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::timeout;

struct StubQemu {
    child: Child,
    socket_path: PathBuf,
    log_path: PathBuf,
}

impl Drop for StubQemu {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_file(&self.log_path);
    }
}

/// Ensure `/var/run/qemu-server` exists and is writable by the current user.
/// Works unmodified when tests run as root (common in CI containers); falls
/// back to `sudo` for an interactive/root-capable dev sandbox.
fn ensure_runtime_dir() -> bool {
    let dir = std::path::Path::new("/var/run/qemu-server");
    if dir.exists()
        && std::fs::metadata(dir)
            .map(|m| !m.permissions().readonly())
            .unwrap_or(false)
    {
        return true;
    }
    if std::fs::create_dir_all(dir).is_ok() {
        return true;
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    let mkdir = std::process::Command::new("sudo")
        .args(["-n", "mkdir", "-p", dir.to_str().unwrap()])
        .status();
    let chown = std::process::Command::new("sudo")
        .args(["-n", "chown", &user, dir.to_str().unwrap()])
        .status();
    matches!(mkdir, Ok(s) if s.success()) && matches!(chown, Ok(s) if s.success())
}

/// Spawn the stub "QEMU" process: a shell whose argv0/cmdline embeds
/// `qemu...<vm_id>` (so `pgrep -f "qemu.*<vm_id>"` finds it, exactly like the
/// production `get_vm_pid` lookup), hosting the deterministic serial socket
/// path via `socat`. Each connection is piped through `tee -a <log>` so
/// writes land in a file we can assert on, and are also echoed back to the
/// caller on the same connection (a minimal, real round trip).
async fn spawn_stub_qemu(vm_id: &str) -> StubQemu {
    let socket_path = PathBuf::from(format!("/var/run/qemu-server/{}.serial", vm_id));
    let log_path = PathBuf::from(format!("/tmp/horcrux-serial-test-{}.log", vm_id));
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(&log_path);

    let fake_name = format!("qemu-system-x86_64-{}", vm_id);
    let shell_cmd = format!(
        "exec -a {name} socat UNIX-LISTEN:{sock},fork,unlink-early EXEC:'tee -a {log}'",
        name = fake_name,
        sock = socket_path.display(),
        log = log_path.display(),
    );

    // `exec -a <name>` to fake the process name is a bashism, not POSIX sh
    // (dash lacks it) - use bash explicitly for the stub launcher.
    let child = Command::new("bash")
        .arg("-c")
        .arg(shell_cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn stub qemu process");

    // Wait for the socket to actually appear (bounded, polling).
    for _ in 0..50 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        socket_path.exists(),
        "stub qemu never created serial socket at {}",
        socket_path.display()
    );

    StubQemu {
        child,
        socket_path,
        log_path,
    }
}

fn require_tools() -> bool {
    let have = |bin: &str| {
        std::process::Command::new("which")
            .arg(bin)
            .stdout(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    have("socat") && have("pgrep")
}

#[tokio::test]
async fn test_serial_console_enable_connect_read_write() {
    if !require_tools() {
        eprintln!("skipping: socat/pgrep not available in this environment");
        return;
    }
    if !ensure_runtime_dir() {
        eprintln!("skipping: cannot create/own /var/run/qemu-server in this environment");
        return;
    }

    let vm_id = format!("serial-it-{}", std::process::id());
    let stub = spawn_stub_qemu(&vm_id).await;

    let manager = ConsoleManager::new();

    // 1. Enable + connect: create_console() is exactly what the
    //    POST /api/console/:vm_id/serial handler calls.
    let info = manager
        .create_console(&vm_id, ConsoleType::Serial)
        .await
        .expect("create_console(Serial) should succeed against the stub qemu process");

    assert_eq!(info.vm_id, vm_id);
    assert_eq!(
        info.port, 0,
        "serial console info should report port 0 (socket-based, not TCP)"
    );
    assert!(!info.ticket.is_empty());
    assert!(info.ws_port > 0, "websocket proxy port should be assigned");

    // Ticket must be valid and resolve back to this VM.
    let ticket = manager
        .verify_ticket(&info.ticket)
        .await
        .expect("freshly issued ticket should verify");
    assert_eq!(ticket.vm_id, vm_id);

    // 2. Write: go through ConsoleManager::write_serial (the real
    //    SerialManager::write_serial_input path), then confirm the byte
    //    stream actually reached the "device" via the independent log file.
    manager
        .write_serial(&vm_id, "hello-from-integration-test")
        .await
        .expect("write_serial should succeed");

    // Give the detached write a moment to land.
    let mut saw_write = false;
    for _ in 0..20 {
        if let Ok(contents) = std::fs::read_to_string(&stub.log_path) {
            if contents.contains("hello-from-integration-test") {
                saw_write = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        saw_write,
        "data written via write_serial() never reached the serial socket"
    );

    // 3. Read: exercise the real read_serial() path; it must not error.
    let _ = manager
        .read_serial(&vm_id, 10)
        .await
        .expect("read_serial should succeed (even if output is empty)");

    // 4. Exercise the actual WebSocket proxy bridge
    //    (console::websocket::WebSocketProxy::start_unix_proxy, the same
    //    bridge the /api/console/ws/serial/:ticket_id route relies on):
    //    connect a raw TCP client to the proxy port and confirm bytes sent
    //    there are forwarded all the way to the Unix socket "device".
    let mut proxy_conn = timeout(
        Duration::from_secs(2),
        TcpStream::connect(("127.0.0.1", info.ws_port)),
    )
    .await
    .expect("connecting to the unix-socket proxy should not time out")
    .expect("connecting to the unix-socket proxy should succeed");

    proxy_conn
        .write_all(b"hello-via-proxy-bridge")
        .await
        .expect("write to proxy should succeed");
    // Read back whatever tee echoes so the proxy's bidirectional path is
    // exercised too (best-effort: don't hang forever if nothing comes back).
    let mut buf = [0u8; 256];
    let _ = timeout(Duration::from_millis(500), proxy_conn.read(&mut buf)).await;
    drop(proxy_conn);

    let mut saw_proxy_write = false;
    for _ in 0..20 {
        if let Ok(contents) = std::fs::read_to_string(&stub.log_path) {
            if contents.contains("hello-via-proxy-bridge") {
                saw_proxy_write = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        saw_proxy_write,
        "data written through the WebSocket/TCP proxy bridge never reached the serial socket"
    );

    drop(stub);
}
