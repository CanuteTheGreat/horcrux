//! Manager-independent dead-man fence for the HA guests of this node.
//!
//! The in-manager self-fence (`ha_loop::self_fence_watchdog`) only protects
//! against a lost network: a manager that is hung (SIGSTOP, deadlock) or dead
//! cannot kill anything, yet its QEMU processes keep running the guests. Without
//! a fencing device the cluster leader used to answer that ambiguity with "never
//! recover a node whose kernel still answers TCP". That also blocked the plain
//! case of a node that crashed, rebooted and came back WITHOUT a running manager
//! (its kernel refuses the probe, its QEMUs are long gone): the guest stayed
//! down for minutes (lab cluster-hasuite j01330, 'hard crash, node stays down':
//! 331 s against the 90 s goal).
//!
//! The agent turns the ambiguity into a guarantee instead of a long blind wait:
//!
//! * the manager rewrites a heartbeat file (`beat`, every second, from its own
//!   task: a hung or dead manager stops writing) listing the HA guests that run
//!   on this node;
//! * the agent, a tiny separate process (`horcrux-api ha-fence-agent`, own
//!   session, renamed `hx-ha-fence` so `pkill horcrux-api` does not take it down
//!   with the manager) kills those guests when the heartbeat is older than
//!   [`fence_secs`];
//! * the manager advertises the running agent to its peers in every placement
//!   sync, so the leader knows that a silent manager on a live kernel means
//!   "guests dead within `fence_secs + kill budget`" and may recover them without
//!   waiting for the node to come back. A node without an agent (older version,
//!   agent not running) is never trusted: the old split-brain guard holds.
//!
//! ## Independent of the manager (j01743: 'hard crash, node stays down' 322 s)
//!
//! A node that rebooted with its manager DOWN (service disabled, failed to start)
//! never ran the agent (the manager spawned it) and never advertised it (the
//! advert lived in the manager's placement sync), so the leader saw 'kernel
//! alive, manager silent, no agent' and held the guests for the whole outage.
//! Now the agent is its own boot service (`horcrux-ha-fence`, started before the
//! manager, installed by the manager/package, see [`ensure_service`]) and
//! answers `GET /attest` on [`attest_port`] itself, without the manager. The
//! reply is a first-hand proof, computed per request from the node's state:
//!
//! * manager heartbeat stale AND no HA guest of this node running (the agent
//!   kills them), or
//! * no heartbeat since boot AND no VM process running AND the manager cannot
//!   start by itself (service not in a runlevel), or did not start within
//!   [`boot_settle_secs`] (guests only ever start through the manager).
//!
//! The leader recovers on a `safe` attestation instead of waiting for a node
//! that may never come back. Read-only: the endpoint can never kill anything.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Heartbeat older than this and the agent kills the HA guests.
pub(crate) fn fence_secs() -> i64 {
    std::env::var("HORCRUX_HA_AGENT_FENCE_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| *v >= 3)
        .unwrap_or(15)
}

/// Longest time between the manager's last answer and the guests being gone,
/// beyond `fence_secs`: one heartbeat period, one agent poll period,
/// `kill_qemu` (SIGTERM, SIGKILL after 5 s, gives up after 10 s, all guests
/// concurrently) and clock drift between the two nodes. The leader waits
/// `fence_secs + AGENT_SETTLE_SECS` from the moment it FIRST sees the manager
/// silent, which is never before the manager's last answer.
pub(crate) const AGENT_SETTLE_SECS: i64 = 1 + 1 + 10 + 2;

fn prefix() -> String {
    std::env::var("HORCRUX_HA_FENCE_PREFIX").unwrap_or_else(|_| "/var/run/horcrux-ha-fence".into())
}

fn hb_path() -> PathBuf {
    PathBuf::from(format!("{}.hb", prefix()))
}

fn lock_path() -> PathBuf {
    PathBuf::from(format!("{}.lock", prefix()))
}

/// What the manager writes: the HA guests of this node.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Beat {
    vms: Vec<String>,
}

/// Manager side: refresh the heartbeat (atomic replace, fresh mtime).
pub(crate) fn beat(vms: &[String]) -> std::io::Result<()> {
    let body = serde_json::to_vec(&Beat { vms: vms.to_vec() })?;
    let tmp = PathBuf::from(format!("{}.hb.tmp", prefix()));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, hb_path())
}

/// Seconds since the heartbeat was last written; `None` if there is none
/// (the node booted and its manager has not run yet: nothing to fence).
fn heartbeat_age() -> Option<i64> {
    let m = std::fs::metadata(hb_path()).ok()?.modified().ok()?;
    Some(
        std::time::SystemTime::now()
            .duration_since(m)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    )
}

/// Pure decision of one agent poll.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AgentStep {
    /// Heartbeat fresh (or none yet): nothing to do, re-arm.
    Idle,
    /// Heartbeat stale and not yet fenced: kill the guests now.
    Fence,
    /// Already fenced for this outage.
    Fenced,
}

pub(crate) fn agent_step(age: Option<i64>, fence_after: i64, fenced: bool) -> AgentStep {
    match age {
        Some(a) if a > fence_after => {
            if fenced {
                AgentStep::Fenced
            } else {
                AgentStep::Fence
            }
        }
        _ => AgentStep::Idle,
    }
}

/// Wire/behaviour version of the agent. 1 = 3bf9f4eb (watcher only, spawned by
/// the manager); 2 = attestation endpoint, own boot service.
pub(crate) const AGENT_VERSION: u32 = 2;

/// Take the agent's singleton lock; `None` if another agent holds it. The
/// holder writes `<pid> <version>` into the file.
fn take_lock() -> Option<nix::fcntl::Flock<std::fs::File>> {
    use nix::fcntl::{Flock, FlockArg};
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path())
        .ok()?;
    Flock::lock(f, FlockArg::LockExclusiveNonblock).ok()
}

/// Version of the agent holding the lock (1 when the file is empty: 3bf9f4eb
/// agents wrote nothing).
fn lock_holder_version() -> u32 {
    std::fs::read_to_string(lock_path())
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse().ok())
        .unwrap_or(1)
}

/// Pids of other processes named `hx-ha-fence`.
fn other_agent_pids() -> Vec<u32> {
    let me = std::process::id();
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    rd.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|p| *p != me)
        .filter(|p| {
            std::fs::read_to_string(format!("/proc/{p}/comm"))
                .map(|c| c.trim() == "hx-ha-fence")
                .unwrap_or(false)
        })
        .collect()
}

/// Is an agent running (its lock is held)?
pub(crate) fn agent_running() -> bool {
    take_lock().is_none()
}

/// Kill the guests listed in the heartbeat file.
async fn fence_guests() -> usize {
    let vms = std::fs::read(hb_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Beat>(&b).ok())
        .unwrap_or_default()
        .vms;
    let kills = vms.iter().map(|vm| async move {
        if crate::vm::qemu::QemuManager::process_running(vm) {
            eprintln!("ha-fence-agent: manager heartbeat stale, killing HA guest {vm}");
            crate::vm::migrate::kill_qemu(vm).await;
        }
    });
    futures::future::join_all(kills).await;
    vms.len()
}

/// `horcrux-api ha-fence-agent`: runs until killed.
pub async fn agent_main() -> i32 {
    let _ = nix::sys::prctl::set_name(c"hx-ha-fence");
    let started = std::time::Instant::now();
    let mut lock = take_lock();
    if lock.is_none() && lock_holder_version() < AGENT_VERSION {
        // an older agent (3bf9f4eb: no attestation endpoint) outlived a manager upgrade:
        // replace it, its guests-killing job is the same and ours also answers peers
        for pid in other_agent_pids() {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            lock = take_lock();
            if lock.is_some() {
                break;
            }
        }
    }
    let Some(mut lock) = lock else {
        return 0; // another (current) agent already runs
    };
    {
        use std::io::{Seek, Write};
        let _ = lock.set_len(0);
        let _ = lock.rewind();
        let _ = writeln!(&mut *lock, "{} {}", std::process::id(), AGENT_VERSION);
    }
    tokio::spawn(serve_attest(started));
    let mut fenced = false;
    loop {
        match agent_step(heartbeat_age(), fence_secs(), fenced) {
            AgentStep::Fence => {
                fence_guests().await;
                fenced = true;
            }
            AgentStep::Idle => fenced = false,
            AgentStep::Fenced => {}
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

// ------------------------------------------------ attestation (no manager)

/// TCP port of the attestation endpoint (`GET /attest?n=<nonce>`).
pub(crate) fn attest_port() -> u16 {
    std::env::var("HORCRUX_HA_FENCE_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(8026)
}

/// How long after the agent started a manager that is enabled but not running is
/// still given to start by itself before the node is declared manager-less.
pub(crate) fn boot_settle_secs() -> i64 {
    std::env::var("HORCRUX_HA_AGENT_BOOT_SETTLE_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| *v >= 0)
        .unwrap_or(20)
}

/// What the agent saw when it answered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Attestation {
    pub v: u32,
    pub nonce: String,
    pub boot_id: String,
    pub agent_age: i64,
    pub hb_age: Option<i64>,
    pub fence_secs: i64,
    /// HA guests (heartbeat list) or, without a heartbeat, every VM process alive.
    pub running: Vec<String>,
    pub manager_enabled: bool,
    pub manager_alive: bool,
    pub safe: bool,
    pub reason: String,
}

/// Facts the verdict is made from.
pub(crate) struct AttestFacts {
    pub hb_age: Option<i64>,
    pub fence_after: i64,
    pub agent_age: i64,
    /// HA guests of the heartbeat that are running now.
    pub ha_running: Vec<String>,
    /// Every VM process that is running now.
    pub all_running: Vec<String>,
    pub manager_enabled: bool,
    pub manager_alive: bool,
    pub settle: i64,
}

/// The verdict: are this node's HA guests provably not running, and can nothing
/// start them behind the leader's back? Pure, unit-tested.
pub(crate) fn attest_decide(f: &AttestFacts) -> (bool, &'static str) {
    match f.hb_age {
        Some(a) if a <= f.fence_after => (false, "manager heartbeat is fresh"),
        Some(_) if f.ha_running.is_empty() => (true, "manager heartbeat stale, HA guests are gone"),
        Some(_) => (false, "manager heartbeat stale, HA guests not killed yet"),
        None if !f.all_running.is_empty() => (false, "no heartbeat since boot, a VM is running"),
        None if f.manager_alive => (false, "manager process is up but has not beaten yet"),
        None if !f.manager_enabled => (true, "manager is not enabled at boot, no guest runs"),
        None if f.agent_age >= f.settle => (
            true,
            "manager did not start within the boot window, no guest runs",
        ),
        None => (false, "manager may still start at boot"),
    }
}

fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Is a horcrux-api process (the manager, or a `horcrux-api guests` call) alive?
fn manager_alive() -> bool {
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return true; // cannot tell: assume it is
    };
    rd.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .any(|p| {
            std::fs::read_to_string(format!("/proc/{p}/comm"))
                .map(|c| c.trim() == "horcrux-api")
                .unwrap_or(false)
        })
}

/// Will the init system start the manager by itself at boot? OpenRC: the service
/// is in a runlevel. Unknown init systems answer yes (the safe side).
fn manager_enabled() -> bool {
    let Ok(rd) = std::fs::read_dir("/etc/runlevels") else {
        return true;
    };
    rd.flatten().any(|d| {
        std::fs::symlink_metadata(d.path().join("horcrux-api")).is_ok()
            || std::fs::symlink_metadata(d.path().join("horcrux")).is_ok()
    })
}

/// Every VM whose pidfile names a live QEMU.
fn running_vms() -> Vec<String> {
    let Ok(rd) = std::fs::read_dir("/var/run") else {
        return vec![];
    };
    let mut v: Vec<String> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_str()?.to_string();
            let id = n
                .strip_prefix("horcrux-vm-")?
                .strip_suffix(".pid")?
                .to_string();
            crate::vm::qemu::QemuManager::process_running(&id).then_some(id)
        })
        .collect();
    v.sort();
    v
}

fn listed_guests() -> Vec<String> {
    std::fs::read(hb_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Beat>(&b).ok())
        .unwrap_or_default()
        .vms
}

fn attest_now(nonce: &str, started: std::time::Instant) -> Attestation {
    let hb_age = heartbeat_age();
    let all_running = running_vms();
    let ha_running: Vec<String> = listed_guests()
        .into_iter()
        .filter(|g| all_running.contains(g))
        .collect();
    let f = AttestFacts {
        hb_age,
        fence_after: fence_secs(),
        agent_age: started.elapsed().as_secs() as i64,
        ha_running,
        all_running,
        manager_enabled: manager_enabled(),
        manager_alive: manager_alive(),
        settle: boot_settle_secs(),
    };
    let (safe, reason) = attest_decide(&f);
    Attestation {
        v: AGENT_VERSION,
        nonce: nonce.to_string(),
        boot_id: boot_id(),
        agent_age: f.agent_age,
        hb_age,
        fence_secs: f.fence_after,
        running: if hb_age.is_some() {
            f.ha_running
        } else {
            f.all_running
        },
        manager_enabled: f.manager_enabled,
        manager_alive: f.manager_alive,
        safe,
        reason: reason.to_string(),
    }
}

/// `n=<nonce>` of a request line like `GET /attest?n=abc HTTP/1.0`.
fn request_nonce(head: &str) -> Option<String> {
    let line = head.lines().next()?;
    let mut it = line.split_whitespace();
    if it.next()? != "GET" {
        return None;
    }
    let target = it.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/attest" {
        return None;
    }
    let n = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("n="))
        .unwrap_or("");
    Some(
        n.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(64)
            .collect(),
    )
}

async fn serve_attest(started: std::time::Instant) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let port = attest_port();
    let listener = loop {
        match tokio::net::TcpListener::bind(("::", port)).await {
            Ok(l) => break l,
            Err(_) => match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
                Ok(l) => break l,
                Err(e) => {
                    eprintln!("ha-fence-agent: cannot listen on {port}: {e}");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            },
        }
    };
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            let n = match tokio::time::timeout(Duration::from_secs(2), sock.read(&mut buf)).await {
                Ok(Ok(n)) if n > 0 => n,
                _ => return,
            };
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let reply = match request_nonce(&head) {
                Some(nonce) => {
                    let body =
                        serde_json::to_string(&attest_now(&nonce, started)).unwrap_or_default();
                    format!(
                        "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                }
                None => "HTTP/1.0 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_string(),
            };
            let _ = tokio::time::timeout(Duration::from_secs(2), sock.write_all(reply.as_bytes()))
                .await;
            let _ = sock.shutdown().await;
        });
    }
}

/// Leader side: ask the agent of `host` for its attestation. `None`: no answer,
/// a wrong nonce (not our reply) or an agent older than the endpoint.
pub(crate) async fn fetch_attestation(host: &str) -> Option<Attestation> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let nonce = format!(
        "{:x}{:x}",
        chrono::Utc::now().timestamp_micros(),
        std::process::id()
    );
    let port = attest_port();
    let work = async {
        let mut s = tokio::net::TcpStream::connect((host, port)).await.ok()?;
        s.write_all(format!("GET /attest?n={nonce} HTTP/1.0\r\nHost: {host}\r\n\r\n").as_bytes())
            .await
            .ok()?;
        let mut out = Vec::new();
        s.take(16 * 1024).read_to_end(&mut out).await.ok()?;
        let text = String::from_utf8_lossy(&out).to_string();
        let (head, body) = text.split_once("\r\n\r\n")?;
        if !head.starts_with("HTTP/1.0 200") && !head.starts_with("HTTP/1.1 200") {
            return None;
        }
        serde_json::from_str::<Attestation>(body).ok()
    };
    let att = tokio::time::timeout(Duration::from_secs(4), work)
        .await
        .ok()??;
    (att.v >= 2 && att.nonce == nonce).then_some(att)
}

/// Install the agent as its own boot service (OpenRC or systemd), once, next to
/// the manager, so it starts before the manager and also when the manager is
/// disabled or broken. Idempotent; never fails the manager.
pub(crate) fn ensure_service() {
    if std::env::var_os("HORCRUX_HA_FENCE_NO_SERVICE").is_some() {
        return;
    }
    if !Path::new("/usr/bin/horcrux-api").exists() {
        return;
    }
    let write_if_changed = |path: &str, body: &str, mode: u32| -> bool {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::read_to_string(path).ok().as_deref() == Some(body) {
            return false;
        }
        std::fs::write(path, body).is_ok()
            && std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).is_ok()
    };
    if Path::new("/sbin/openrc-run").exists() && Path::new("/etc/init.d").is_dir() {
        write_if_changed("/etc/init.d/horcrux-ha-fence", OPENRC_SCRIPT, 0o755);
        let link = "/etc/runlevels/default/horcrux-ha-fence";
        if Path::new("/etc/runlevels/default").is_dir() && std::fs::symlink_metadata(link).is_err()
        {
            match std::os::unix::fs::symlink("/etc/init.d/horcrux-ha-fence", link) {
                Ok(()) => tracing::info!("HA: fence agent service enabled in the default runlevel"),
                Err(e) => tracing::warn!("HA: cannot enable the fence agent service: {e}"),
            }
        }
    } else if Path::new("/run/systemd/system").is_dir() {
        let unit = "/etc/systemd/system/horcrux-ha-fence.service";
        write_if_changed(unit, SYSTEMD_UNIT, 0o644);
        let dir = "/etc/systemd/system/multi-user.target.wants";
        let link = format!("{dir}/horcrux-ha-fence.service");
        if std::fs::create_dir_all(dir).is_ok() && std::fs::symlink_metadata(&link).is_err() {
            let _ = std::os::unix::fs::symlink(unit, &link);
        }
    }
}

const OPENRC_SCRIPT: &str = include_str!("../../../deploy/openrc/horcrux-ha-fence");
const SYSTEMD_UNIT: &str = include_str!("../../../deploy/systemd/horcrux-ha-fence.service");

/// Manager side: start the agent if none runs (detached, own session, so it
/// outlives a killed or restarted manager).
pub(crate) fn ensure_agent() {
    // an older agent (no attestation endpoint) is replaced by the new one's start
    if agent_running() && lock_holder_version() >= AGENT_VERSION {
        return;
    }
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()
        .ok()
        .map(|p| PathBuf::from(p.to_string_lossy().trim_end_matches(" (deleted)")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("/usr/bin/horcrux-api"));
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("ha-fence-agent")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: setsid is async-signal-safe and runs in the forked child only.
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setsid()
                .map(|_| ())
                .map_err(std::io::Error::from)
        });
    }
    match cmd.spawn() {
        Ok(mut child) => {
            // reap it if it exits at once (lost the lock race)
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => tracing::error!("HA: cannot start the fence agent: {e}"),
    }
}

// ------------------------------------------------- leader side: who has one

static AGENTS: OnceLock<Mutex<HashMap<String, (i64, i64)>>> = OnceLock::new();

fn agents() -> &'static Mutex<HashMap<String, (i64, i64)>> {
    AGENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A peer's sync said whether it runs an agent (and its deadline).
pub(crate) fn note_peer_agent(node: &str, agent_secs: Option<i64>, now: i64) {
    let mut m = agents().lock().unwrap_or_else(|p| p.into_inner());
    match agent_secs {
        Some(s) => {
            m.insert(node.to_string(), (s, now));
        }
        None => {
            m.remove(node);
        }
    }
}

/// How long the leader trusts the last advert of a node. Not seconds: a node
/// whose manager hangs stops syncing, and the leader's own tick can sit in a
/// 60 s call to it before it even notices, so the advert must outlive that. The
/// manager respawns a dead agent within 5 s and withdraws the advert (sync with
/// `None`) when it has none, so a stale advert needs agent AND manager to die
/// together.
const ADVERT_MAX_AGE_SECS: i64 = 3600;

/// The agent deadline of `node`, if it advertised a running agent.
pub(crate) fn peer_agent_secs(node: &str, now: i64) -> Option<i64> {
    let m = agents().lock().unwrap_or_else(|p| p.into_inner());
    let (secs, seen) = *m.get(node)?;
    (now - seen <= ADVERT_MAX_AGE_SECS).then_some(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_step_fences_once_per_outage() {
        assert_eq!(agent_step(None, 20, false), AgentStep::Idle);
        assert_eq!(agent_step(Some(3), 20, false), AgentStep::Idle);
        assert_eq!(agent_step(Some(20), 20, false), AgentStep::Idle);
        assert_eq!(agent_step(Some(21), 20, false), AgentStep::Fence);
        assert_eq!(agent_step(Some(60), 20, true), AgentStep::Fenced);
        // the heartbeat is back: re-armed
        assert_eq!(agent_step(Some(1), 20, true), AgentStep::Idle);
    }

    /// Real processes: a stale heartbeat makes the agent kill exactly the HA
    /// guests it lists (a fake `qemu` binary + pidfile), a fresh one does not.
    #[tokio::test]
    async fn stale_heartbeat_kills_the_listed_guest_only() {
        let dir = std::env::temp_dir().join(format!("hx-fence-ut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var(
            "HORCRUX_HA_FENCE_PREFIX",
            dir.join("f").to_string_lossy().to_string(),
        );
        let pidfile = "/var/run/horcrux-vm-ut-fence-a.pid";
        let other_pidfile = "/var/run/horcrux-vm-ut-fence-b.pid";
        if std::fs::write(pidfile, "0").is_err() {
            eprintln!("skipped: /var/run not writable");
            return;
        }
        let fake = dir.join("qemu-fake");
        std::fs::copy("/bin/sleep", &fake).unwrap();
        let spawn = || {
            std::process::Command::new(&fake)
                .arg("300")
                .spawn()
                .unwrap()
        };
        let (mut a, mut b) = (spawn(), spawn());
        std::fs::write(pidfile, a.id().to_string()).unwrap();
        std::fs::write(other_pidfile, b.id().to_string()).unwrap();
        for _ in 0..50 {
            if crate::vm::qemu::QemuManager::process_running("ut-fence-a")
                && crate::vm::qemu::QemuManager::process_running("ut-fence-b")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // only guest a is an HA guest in the heartbeat
        beat(&["ut-fence-a".to_string()]).unwrap();
        assert!(heartbeat_age().unwrap() <= 1);
        assert_eq!(agent_step(heartbeat_age(), 20, false), AgentStep::Idle);
        // age the heartbeat: the manager "hung" 60 s ago
        let old = std::time::SystemTime::now() - Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(hb_path())
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(agent_step(heartbeat_age(), 20, false), AgentStep::Fence);
        assert_eq!(fence_guests().await, 1);
        assert!(a.try_wait().unwrap().is_some(), "HA guest a must be killed");
        assert!(
            b.try_wait().unwrap().is_none(),
            "non-HA process b must live"
        );
        let _ = b.kill();
        let _ = b.wait();
        let _ = std::fs::remove_file(pidfile);
        let _ = std::fs::remove_file(other_pidfile);
        // the singleton lock
        assert!(!agent_running());
        let held = take_lock().expect("free");
        assert!(agent_running());
        drop(held);
        assert!(!agent_running());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adverts_expire_and_are_withdrawn() {
        note_peer_agent("ut-a", Some(20), 1000);
        assert_eq!(peer_agent_secs("ut-a", 1005), Some(20));
        // survives a long leader tick, expires after an hour
        assert_eq!(peer_agent_secs("ut-a", 1090), Some(20));
        assert_eq!(
            peer_agent_secs("ut-a", 1000 + ADVERT_MAX_AGE_SECS + 1),
            None
        );
        note_peer_agent("ut-a", None, 1010);
        assert_eq!(peer_agent_secs("ut-a", 1011), None);
        assert_eq!(peer_agent_secs("ut-never", 1), None);
    }
}

#[cfg(test)]
mod attest_tests {
    use super::*;

    fn facts() -> AttestFacts {
        AttestFacts {
            hb_age: None,
            fence_after: 15,
            agent_age: 100,
            ha_running: vec![],
            all_running: vec![],
            manager_enabled: false,
            manager_alive: false,
            settle: 20,
        }
    }

    #[test]
    fn manager_off_at_boot_attests_safe() {
        assert!(attest_decide(&facts()).0);
    }

    #[test]
    fn enabled_manager_gets_the_boot_window() {
        let mut f = facts();
        f.manager_enabled = true;
        f.agent_age = 5;
        assert!(!attest_decide(&f).0);
        f.agent_age = 20;
        assert!(attest_decide(&f).0);
    }

    #[test]
    fn never_safe_while_a_vm_runs_or_the_manager_is_up_or_beating() {
        let mut f = facts();
        f.all_running = vec!["vm1".into()];
        assert!(!attest_decide(&f).0);
        let mut f = facts();
        f.manager_alive = true;
        assert!(!attest_decide(&f).0);
        let mut f = facts();
        f.hb_age = Some(3);
        assert!(!attest_decide(&f).0);
        f.hb_age = Some(15);
        assert!(!attest_decide(&f).0);
    }

    #[test]
    fn hung_manager_is_safe_only_once_its_guests_are_dead() {
        let mut f = facts();
        f.hb_age = Some(30);
        f.ha_running = vec!["vm1".into()];
        assert!(!attest_decide(&f).0);
        f.ha_running.clear();
        assert!(attest_decide(&f).0);
    }

    /// The endpoint answers a real peer with the nonce echoed back and a verdict,
    /// and a reply that does not carry our nonce is never accepted.
    #[tokio::test]
    async fn endpoint_round_trip() {
        std::env::set_var("HORCRUX_HA_FENCE_PORT", "18926");
        tokio::spawn(serve_attest(std::time::Instant::now()));
        let mut got = None;
        for _ in 0..50 {
            if let Some(a) = fetch_attestation("127.0.0.1").await {
                got = Some(a);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let a = got.expect("the agent endpoint answers");
        assert_eq!(a.v, AGENT_VERSION);
        assert!(!a.reason.is_empty());
    }

    #[test]
    fn request_parsing() {
        assert_eq!(
            request_nonce("GET /attest?n=ab12 HTTP/1.0\r\n"),
            Some("ab12".into())
        );
        assert_eq!(
            request_nonce("GET /attest HTTP/1.0\r\n"),
            Some(String::new())
        );
        assert_eq!(request_nonce("GET /other HTTP/1.0\r\n"), None);
        assert_eq!(request_nonce("POST /attest HTTP/1.0\r\n"), None);
    }
}
