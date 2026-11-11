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
use std::collections::HashMap;
use std::path::PathBuf;
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

/// Take the agent's singleton lock; `None` if another agent holds it.
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
    let Some(_lock) = take_lock() else {
        return 0; // another agent already runs
    };
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

/// Manager side: start the agent if none runs (detached, own session, so it
/// outlives a killed or restarted manager).
pub(crate) fn ensure_agent() {
    if agent_running() {
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
