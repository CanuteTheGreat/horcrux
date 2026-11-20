//! Delivering the guest tools into a VM.
//!
//! Horcrux attaches the guest tools CD (`[console] guest_tools_iso`,
//! label `HXTOOLS`) to every console VM run. When a viewer opens, the
//! tools do not answer and the VM runs the QEMU guest agent, the node
//! installs them through the agent: Windows guests get the MSI from the CD
//! (Windows Installer, so it is a managed, removable package). Linux guests
//! install the distribution package (`app-emulation/horcrux-guest-tools`
//! on Gentoo); the CD carries the static agent and service files for
//! distributions without a package yet, installed by the guest's admin.
//!
//! One attempt per VM per `RETRY` interval, so a guest without the agent
//! or with a failing install is not hammered on every viewer.
use crate::vm::devices::qga_exec;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const RETRY: Duration = Duration::from_secs(600);

/// PowerShell run by the guest agent (as SYSTEM) in a Windows guest.
pub const WINDOWS_INSTALL: &str = "$ErrorActionPreference='Stop'; \
$v=Get-Volume -FileSystemLabel HXTOOLS | Select-Object -First 1; \
if(-not $v -or -not $v.DriveLetter){throw 'the Horcrux guest tools CD (HXTOOLS) is not attached'}; \
$m=Get-ChildItem ($v.DriveLetter + ':\\windows\\horcrux-guest-tools-*-x64.msi') | Sort-Object Name | Select-Object -Last 1; \
$p=Start-Process msiexec.exe -ArgumentList @('/i',('\"'+$m.FullName+'\"'),'/qn','/norestart') -Wait -PassThru; \
exit $p.ExitCode";

static LAST: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

fn due(vm: &str) -> bool {
    let Ok(mut g) = LAST.lock() else { return false };
    let map = g.get_or_insert_with(HashMap::new);
    match map.get(vm) {
        Some(t) if t.elapsed() < RETRY => false,
        _ => {
            map.insert(vm.to_string(), Instant::now());
            true
        }
    }
}

/// Start an install in the background (no-op when one ran recently).
pub fn spawn_install(vm_id: String, qga: PathBuf) {
    if !qga.exists() || !due(&vm_id) {
        return;
    }
    tokio::spawn(async move {
        match install(&qga).await {
            Ok(msg) => tracing::info!("console guest tools on {vm_id}: {msg}"),
            Err(e) => tracing::info!("console guest tools on {vm_id} not installed: {e}"),
        }
    });
}

/// What to run in the guest for its OS (`guest-get-osinfo` id).
pub fn install_command(os_id: &str) -> Option<(&'static str, Vec<String>)> {
    if os_id == "mswindows" {
        Some((
            "powershell.exe",
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-Command".into(),
                WINDOWS_INSTALL.into(),
            ],
        ))
    } else {
        None
    }
}

pub async fn install(qga: &Path) -> Result<String, String> {
    let os = qga_exec(qga, "guest-get-osinfo", json!({}), 10)
        .await
        .map_err(|e| e.to_string())?;
    let id = os.get("id").and_then(Value::as_str).unwrap_or("");
    let Some((path, args)) = install_command(id) else {
        return Err(format!(
            "automatic install covers Windows guests; this guest ({id}) installs the \
             horcrux-guest-tools package"
        ));
    };
    let r = qga_exec(
        qga,
        "guest-exec",
        json!({"path": path, "arg": args, "capture-output": true}),
        15,
    )
    .await
    .map_err(|e| e.to_string())?;
    let pid = r.get("pid").and_then(Value::as_i64).ok_or("guest-exec gave no pid")?;
    for _ in 0..300 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let st = qga_exec(qga, "guest-exec-status", json!({"pid": pid}), 10)
            .await
            .map_err(|e| e.to_string())?;
        if st.get("exited").and_then(Value::as_bool) == Some(true) {
            let code = st.get("exitcode").and_then(Value::as_i64).unwrap_or(-1);
            return if code == 0 || code == 3010 {
                Ok(format!("installed (msiexec exit {code})"))
            } else {
                Err(format!("installer exit code {code}"))
            };
        }
    }
    Err("installer still running after 300 s".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_installs_the_msi_from_the_cd() {
        let (p, a) = install_command("mswindows").unwrap();
        assert_eq!(p, "powershell.exe");
        let s = a.last().unwrap();
        assert!(s.contains("FileSystemLabel HXTOOLS") && s.contains("msiexec.exe") && s.contains("/qn"));
        assert!(install_command("debian").is_none());
    }

    #[test]
    fn one_attempt_per_interval() {
        assert!(due("vm-a"));
        assert!(!due("vm-a"));
        assert!(due("vm-b"));
    }
}
