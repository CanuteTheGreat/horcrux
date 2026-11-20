//! Delivering the guest tools into a VM.
//!
//! Horcrux attaches the guest tools CD (`[console] guest_tools_iso`,
//! label `HXTOOLS`) to every console VM run. The node installs the tools
//! through the QEMU guest agent, always as a managed, removable package:
//!
//! * Windows: the MSI from the CD (Windows Installer);
//! * Linux: the distribution's package from the CD
//!   (`linux/<arch>/horcrux-guest-tools.{deb,rpm,apk}`, installed with the
//!   guest's own package manager so its dependencies resolve) or, on Gentoo,
//!   `app-emulation/horcrux-guest-tools` from the Horcrux overlay; then the
//!   service is enabled and started with whichever init runs (systemd or
//!   OpenRC).
//!
//! Two triggers: automatically when a viewer opens, no tools answer and the
//! agent runs (`guest_tools_auto_install`, one attempt per VM per
//! `guest_tools_auto_install_retry_secs`
//! so a guest without the agent or with a failing install is not hammered
//! on every viewer), and on request (`POST /api/vms/:id/console/guest-tools/install`).
use crate::vm::devices::qga_exec;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// PowerShell run by the guest agent (as SYSTEM) in a Windows guest. The
/// MSI path goes to msiexec unquoted (`E:\\windows\\horcrux-guest-tools-*.msi`
/// has no spaces): quotes embedded in the script did not survive the
/// guest agent's command line and msiexec answered 1619 (package not found).
pub const WINDOWS_INSTALL: &str = "$ErrorActionPreference='Stop'; \
$v=Get-Volume -FileSystemLabel HXTOOLS | Select-Object -First 1; \
if(-not $v -or -not $v.DriveLetter){throw 'the Horcrux guest tools CD (HXTOOLS) is not attached'}; \
$m=Get-ChildItem ($v.DriveLetter + ':\\\\windows\\\\horcrux-guest-tools-*-x64.msi') | Sort-Object Name | Select-Object -Last 1; \
$p=Start-Process msiexec.exe -ArgumentList @('/i',$m.FullName,'/qn','/norestart') -Wait -PassThru; \
exit $p.ExitCode";

/// POSIX sh run by the guest agent (as root) in a Linux guest. Exit codes:
/// 0 installed and started, 3 no tools CD, 4 no package for this
/// distribution, 5 mount failed, 6 the service did not start, other = the
/// package manager's own failure.
pub const LINUX_INSTALL: &str = r#"set -u
arch=$(uname -m)
dev=""
for d in /dev/disk/by-label/HXTOOLS /dev/sr* /dev/cdrom*; do
  [ -b "$d" ] || continue
  if [ "$d" = /dev/disk/by-label/HXTOOLS ] || blkid "$d" 2>/dev/null | grep -q 'LABEL="HXTOOLS"'; then dev=$d; break; fi
done
[ -n "$dev" ] || { echo "the Horcrux guest tools CD (HXTOOLS) is not attached" >&2; exit 3; }
mnt=$(mktemp -d)
mount -o ro "$dev" "$mnt" || exit 5
trap 'umount "$mnt" 2>/dev/null; rmdir "$mnt" 2>/dev/null' EXIT
pkg="$mnt/linux/$arch/horcrux-guest-tools"
rc=0
if command -v emerge >/dev/null 2>&1; then
  emerge --quiet --noreplace app-emulation/horcrux-guest-tools; rc=$?
elif command -v apk >/dev/null 2>&1 && [ -f "$pkg.apk" ]; then
  apk add --allow-untrusted "$pkg.apk"; rc=$?
elif command -v apt-get >/dev/null 2>&1 && [ -f "$pkg.deb" ]; then
  cp "$pkg.deb" /tmp/horcrux-guest-tools.deb
  DEBIAN_FRONTEND=noninteractive apt-get install -y /tmp/horcrux-guest-tools.deb; rc=$?
  rm -f /tmp/horcrux-guest-tools.deb
elif command -v dpkg >/dev/null 2>&1 && [ -f "$pkg.deb" ]; then
  dpkg -i "$pkg.deb"; rc=$?
elif command -v dnf >/dev/null 2>&1 && [ -f "$pkg.rpm" ]; then
  dnf install -y "$pkg.rpm"; rc=$?
elif command -v zypper >/dev/null 2>&1 && [ -f "$pkg.rpm" ]; then
  zypper --non-interactive install --allow-unsigned-rpm "$pkg.rpm"; rc=$?
elif command -v rpm >/dev/null 2>&1 && [ -f "$pkg.rpm" ]; then
  rpm -U --replacepkgs "$pkg.rpm"; rc=$?
else
  echo "no Horcrux guest tools package for this distribution ($arch); the CD's linux/ directory has the static agent and service files" >&2
  exit 4
fi
[ $rc -eq 0 ] || exit $rc
if [ -d /run/systemd/system ]; then
  systemctl daemon-reload
  systemctl enable horcrux-guest-agent.service || echo "systemctl enable failed; the agent runs until the next boot" >&2
  systemctl restart horcrux-guest-agent.service || exit 6
elif command -v rc-service >/dev/null 2>&1; then
  rc-update add horcrux-guest-agent default >/dev/null; rc-service horcrux-guest-agent restart || exit 6
fi
echo installed
"#;

/// Result of one install, for the API and the session's `ctl` channel.
#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    /// `guest-get-osinfo` id (mswindows, debian, alpine, ...).
    pub os: String,
    pub ok: bool,
    pub exit_code: Option<i64>,
    pub message: String,
    /// Tail of the installer's output.
    pub output: String,
}

/// The latest install per VM (automatic or requested), for
/// `GET /api/vms/:id/console/guest-tools`.
#[derive(Debug, Clone, Serialize)]
pub struct InstallState {
    /// `installing`, `installed` or `failed`.
    pub state: String,
    /// `auto` (a viewer opened) or the user who asked.
    pub by: String,
    pub started: chrono::DateTime<chrono::Utc>,
    pub finished: Option<chrono::DateTime<chrono::Utc>>,
    pub report: Option<InstallReport>,
    pub error: Option<String>,
}

static STATES: Mutex<Option<HashMap<String, InstallState>>> = Mutex::new(None);

/// The latest install of this VM's guest tools, if any ran since the node
/// started.
pub fn last_install(vm: &str) -> Option<InstallState> {
    STATES
        .lock()
        .ok()
        .and_then(|g| g.as_ref().and_then(|m| m.get(vm).cloned()))
}

fn set_state(vm: &str, st: InstallState) {
    if let Ok(mut g) = STATES.lock() {
        g.get_or_insert_with(HashMap::new)
            .insert(vm.to_string(), st);
    }
}

/// Run an install and record it (`by` = `auto` or the requesting user).
/// Refused while another install of the same VM runs.
pub async fn install_recorded(
    vm: &str,
    qga: &Path,
    by: &str,
    timeout_secs: u64,
) -> Result<InstallReport, String> {
    if last_install(vm)
        .map(|s| s.state == "installing")
        .unwrap_or(false)
    {
        return Err("an install of the guest tools is already running in this VM".into());
    }
    let started = chrono::Utc::now();
    set_state(
        vm,
        InstallState {
            state: "installing".into(),
            by: by.into(),
            started,
            finished: None,
            report: None,
            error: None,
        },
    );
    let r = install(qga, timeout_secs).await;
    let (state, report, error) = match &r {
        Ok(rep) if rep.ok => ("installed", Some(rep.clone()), None),
        Ok(rep) => ("failed", Some(rep.clone()), Some(rep.message.clone())),
        Err(e) => ("failed", None, Some(e.clone())),
    };
    set_state(
        vm,
        InstallState {
            state: state.into(),
            by: by.into(),
            started,
            finished: Some(chrono::Utc::now()),
            report,
            error,
        },
    );
    r
}

static LAST: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

fn due(vm: &str, retry: Duration) -> bool {
    let Ok(mut g) = LAST.lock() else { return false };
    let map = g.get_or_insert_with(HashMap::new);
    match map.get(vm) {
        Some(t) if t.elapsed() < retry => false,
        _ => {
            map.insert(vm.to_string(), Instant::now());
            true
        }
    }
}

/// Start an automatic install in the background. `None` when one ran
/// recently or the VM has no guest agent channel.
pub fn spawn_install(
    vm_id: String,
    qga: PathBuf,
    timeout_secs: u64,
    retry_secs: u64,
) -> Option<tokio::task::JoinHandle<Result<String, String>>> {
    if !qga.exists() || !due(&vm_id, Duration::from_secs(retry_secs)) {
        return None;
    }
    Some(tokio::spawn(async move {
        let r = install_recorded(&vm_id, &qga, "auto", timeout_secs).await;
        match &r {
            Ok(rep) if rep.ok => tracing::info!("console guest tools on {vm_id}: {}", rep.message),
            Ok(rep) => tracing::info!(
                "console guest tools on {vm_id} not installed: {} {}",
                rep.message,
                rep.output
            ),
            Err(e) => tracing::info!("console guest tools on {vm_id} not installed: {e}"),
        }
        match r {
            Ok(rep) if rep.ok => Ok(rep.message),
            Ok(rep) => Err(rep.message),
            Err(e) => Err(e),
        }
    }))
}

/// What to run in the guest for its OS (`guest-get-osinfo` id).
pub fn install_command(os_id: &str) -> (&'static str, Vec<String>) {
    if os_id == "mswindows" {
        (
            "powershell.exe",
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-Command".into(),
                WINDOWS_INSTALL.into(),
            ],
        )
    } else {
        ("/bin/sh", vec!["-c".into(), LINUX_INSTALL.into()])
    }
}

fn message(os: &str, code: i64) -> (bool, String) {
    let windows = os == "mswindows";
    match code {
        0 => (true, "installed".into()),
        3010 if windows => (true, "installed (Windows asks for a restart)".into()),
        3 if !windows => (
            false,
            "the Horcrux guest tools CD is not attached to this VM run (restart the VM to get it)"
                .into(),
        ),
        4 if !windows => (
            false,
            "no Horcrux guest tools package for this distribution".into(),
        ),
        6 if !windows => (false, "installed, but the service did not start".into()),
        c => (false, format!("installer exit code {c}")),
    }
}

fn tail(b64: Option<&Value>) -> String {
    use base64::Engine as _;
    let text = b64
        .and_then(Value::as_str)
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_default();
    let t = text.trim();
    let start = t.len().saturating_sub(2000);
    let start = (start..t.len())
        .find(|&i| t.is_char_boundary(i))
        .unwrap_or(t.len());
    t[start..].to_string()
}

/// Install the tools through the guest agent and wait for the installer.
pub async fn install(qga: &Path, timeout_secs: u64) -> Result<InstallReport, String> {
    let os = qga_exec(qga, "guest-get-osinfo", json!({}), 10)
        .await
        .map_err(|e| e.to_string())?;
    let id = os
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let (path, args) = install_command(&id);
    let r = qga_exec(
        qga,
        "guest-exec",
        json!({"path": path, "arg": args, "capture-output": true}),
        15,
    )
    .await
    .map_err(|e| e.to_string())?;
    let pid = r
        .get("pid")
        .and_then(Value::as_i64)
        .ok_or("guest-exec gave no pid")?;
    // The guest agent can stop answering for a while during an install
    // (Windows: an MSI that adds a driver); keep asking until the deadline.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut last_err = String::new();
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let st = match qga_exec(qga, "guest-exec-status", json!({"pid": pid}), 10).await {
            Ok(st) => st,
            Err(e) => {
                last_err = e.to_string();
                continue;
            }
        };
        if st.get("exited").and_then(Value::as_bool) == Some(true) {
            let code = st.get("exitcode").and_then(Value::as_i64).unwrap_or(-1);
            let (ok, message) = message(&id, code);
            let mut output = tail(st.get("out-data"));
            let err = tail(st.get("err-data"));
            if !err.is_empty() {
                output = format!("{output}\n{err}").trim().to_string();
            }
            return Ok(InstallReport {
                os: id,
                ok,
                exit_code: Some(code),
                message,
                output,
            });
        }
    }
    if last_err.is_empty() {
        Err(format!("installer still running after {timeout_secs} s"))
    } else {
        Err(format!(
            "no installer result within {timeout_secs} s (last guest agent error: {last_err})"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_installs_the_msi_from_the_cd() {
        let (p, a) = install_command("mswindows");
        assert_eq!(p, "powershell.exe");
        let s = a.last().unwrap();
        assert!(
            s.contains("FileSystemLabel HXTOOLS") && s.contains("msiexec.exe") && s.contains("/qn")
        );
    }

    #[test]
    fn linux_installs_the_distribution_package_and_starts_the_service() {
        for os in ["debian", "alpine", "fedora", "gentoo", "ubuntu"] {
            let (p, a) = install_command(os);
            assert_eq!(p, "/bin/sh");
            let s = &a[1];
            for want in [
                "linux/$arch/horcrux-guest-tools",
                ".deb",
                ".rpm",
                ".apk",
                "app-emulation/horcrux-guest-tools",
                "systemctl enable horcrux-guest-agent.service",
                "systemctl restart horcrux-guest-agent.service || exit 6",
                "rc-update add horcrux-guest-agent default",
                "LABEL=\"HXTOOLS\"",
            ] {
                assert!(s.contains(want), "{want}");
            }
        }
    }

    #[test]
    fn exit_codes_are_explained() {
        assert!(message("mswindows", 0).0);
        assert!(message("mswindows", 3010).0);
        assert!(!message("mswindows", 1603).0);
        assert!(message("debian", 3).1.contains("CD"));
        assert!(message("alpine", 4)
            .1
            .contains("no Horcrux guest tools package"));
        assert!(!message("debian", 6).0);
    }

    #[test]
    fn output_tail_is_decoded_and_bounded() {
        use base64::Engine as _;
        let long = "x".repeat(5000) + "end";
        let v = Value::String(base64::engine::general_purpose::STANDARD.encode(long));
        let t = tail(Some(&v));
        assert!(t.len() <= 2000 && t.ends_with("end"));
        assert_eq!(tail(None), "");
    }

    #[test]
    fn one_attempt_per_interval() {
        let r = Duration::from_secs(600);
        assert!(due("vm-a", r));
        assert!(!due("vm-a", r));
        assert!(due("vm-b", r));
        // a zero wait allows the next attempt at once
        assert!(due("vm-c", Duration::ZERO));
        assert!(due("vm-c", Duration::ZERO));
    }
}
