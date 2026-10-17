//! VM display / console wiring for QEMU.
//!
//! QEMU used to be started with `-display none` and `-serial file:...`, so
//! the VNC port the console API handed out had nothing listening, SPICE was
//! never configured, and the serial WebSocket bridge connected to a Unix
//! socket QEMU never created. This module builds the real arguments and
//! records where each console actually is:
//!
//! * **Serial**: always a Unix socket next to the disk
//!   (`<disk>.serial.sock`), with QEMU's chardev `logfile=` still writing
//!   the boot log to `<disk>.serial.log` (the nightly realistic test reads
//!   it).
//! * **VNC**: `127.0.0.1:<port>` with `password=on`; the per-start password
//!   is set over QMP and handed to the ticketed noVNC page, so even local
//!   users on the host can't attach without it.
//! * **SPICE**: `<listen>:<port>` (listen defaults to 127.0.0.1, see
//!   `HORCRUX_SPICE_LISTEN`), ticketing on with a per-start password set
//!   over QMP, plus audio (`-audiodev spice`), the clipboard/agent channel
//!   (virtio-serial + spicevmc vdagent) and USB redirection (qemu-xhci +
//!   two usb-redir spicevmc channels).
//!
//! What was set up for a running VM is written to `<disk>.display.json`
//! (mode 0600) for the console API to read.
use horcrux_common::{
    DisplayProtocol, Result, SoundDevice, VideoDevice, VmArchitecture, VmDisplay,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Where the consoles of a running VM are.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisplayRuntime {
    pub protocol: DisplayProtocol,
    pub vnc_port: Option<u16>,
    pub spice_port: Option<u16>,
    pub spice_listen: Option<String>,
    /// VNC (8 chars, RFB limit) or SPICE password for this VM run.
    pub password: Option<String>,
    pub serial_socket: PathBuf,
    pub serial_log: PathBuf,
    pub qmp_socket: PathBuf,
}

impl DisplayRuntime {
    pub fn state_path(disk: &Path) -> PathBuf {
        disk.with_extension("display.json")
    }

    pub fn serial_socket(disk: &Path) -> PathBuf {
        disk.with_extension("serial.sock")
    }

    pub fn serial_log(disk: &Path) -> PathBuf {
        disk.with_extension("serial.log")
    }

    pub fn qmp_socket(disk: &Path) -> PathBuf {
        disk.with_extension("qmp.sock")
    }

    /// Plan the consoles for a VM about to start (allocates free ports).
    pub fn plan(disk: &Path, display: &VmDisplay) -> Result<Self> {
        let (vnc_port, spice_port, spice_listen) = match display.protocol {
            DisplayProtocol::None => (None, None, None),
            DisplayProtocol::Vnc => (Some(free_port(5900..6000, "127.0.0.1")?), None, None),
            DisplayProtocol::Spice => {
                let listen = spice_listen_addr();
                (None, Some(free_port(61000..61100, &listen)?), Some(listen))
            }
        };
        let password = match display.protocol {
            DisplayProtocol::None => None,
            // RFB VNC authentication only uses the first 8 characters.
            DisplayProtocol::Vnc => Some(random_password(8)),
            DisplayProtocol::Spice => Some(random_password(24)),
        };
        Ok(Self {
            protocol: display.protocol,
            vnc_port,
            spice_port,
            spice_listen,
            password,
            serial_socket: Self::serial_socket(disk),
            serial_log: Self::serial_log(disk),
            qmp_socket: Self::qmp_socket(disk),
        })
    }

    pub async fn save(&self, disk: &Path) -> Result<()> {
        let path = Self::state_path(disk);
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| horcrux_common::Error::System(format!("display state: {e}")))?;
        tokio::fs::write(&path, json).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).await?;
        }
        Ok(())
    }

    pub async fn load(disk: &Path) -> Result<Self> {
        let path = Self::state_path(disk);
        let bytes = tokio::fs::read(&path).await.map_err(|_| {
            horcrux_common::Error::InvalidConfig(
                "VM is not running (no console state); start it first".to_string(),
            )
        })?;
        serde_json::from_slice(&bytes)
            .map_err(|e| horcrux_common::Error::System(format!("bad display state: {e}")))
    }

    pub async fn remove(disk: &Path) {
        for p in [
            Self::state_path(disk),
            Self::serial_socket(disk),
            Self::qmp_socket(disk),
        ] {
            let _ = tokio::fs::remove_file(p).await;
        }
    }
}

/// Address SPICE listens on: `HORCRUX_SPICE_LISTEN`, default 127.0.0.1.
/// SPICE is always password-protected (new password per VM start), so
/// binding it to a LAN address for remote-viewer is an explicit opt-in.
pub fn spice_listen_addr() -> String {
    std::env::var("HORCRUX_SPICE_LISTEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

fn free_port(range: std::ops::Range<u16>, host: &str) -> Result<u16> {
    for port in range.clone() {
        if std::net::TcpListener::bind((host, port)).is_ok() {
            return Ok(port);
        }
    }
    Err(horcrux_common::Error::System(format!(
        "no free console port in {}..{} on {}",
        range.start, range.end, host
    )))
}

fn random_password(len: usize) -> String {
    let mut s = String::new();
    while s.len() < len {
        s.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    s.truncate(len);
    s
}

/// QEMU arguments for the serial console, display, video and sound.
/// `pc` is false for non-PC machine types (aarch64/riscv64 `virt`, ppc64).
pub fn qemu_args(display: &VmDisplay, arch: &VmArchitecture, rt: &DisplayRuntime) -> Vec<String> {
    let pc = *arch == VmArchitecture::X86_64;
    let mut a: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));

    // Serial on a Unix socket; logfile keeps the boot log next to the disk.
    let serial = format!(
        "socket,id=serial0,path={},server=on,wait=off,logfile={},logappend=on",
        rt.serial_socket.display(),
        rt.serial_log.display()
    );
    push(&["-chardev", &serial, "-serial", "chardev:serial0"]);
    let qmp = format!("unix:{},server=on,wait=off", rt.qmp_socket.display());
    push(&["-qmp", &qmp]);

    // No local window, ever: graphical consoles are network protocols.
    push(&["-display", "none"]);

    // Video card.
    match (display.video, pc) {
        (VideoDevice::None, true) => push(&["-vga", "none"]),
        (VideoDevice::None, false) => {}
        (VideoDevice::Std, true) => push(&["-vga", "std"]),
        (VideoDevice::Virtio, true) => push(&["-vga", "virtio"]),
        (VideoDevice::Qxl, true) => push(&["-vga", "qxl"]),
        // Non-PC machines have no VGA: virtio-gpu is the framebuffer.
        (_, false) => push(&["-device", "virtio-gpu-pci"]),
    }

    match display.protocol {
        DisplayProtocol::None => {}
        DisplayProtocol::Vnc => {
            let port = rt.vnc_port.unwrap_or(5900);
            let spec = format!("127.0.0.1:{},password=on", port.saturating_sub(5900));
            push(&["-vnc", &spec]);
        }
        DisplayProtocol::Spice => {
            // Ticketing stays on (no disable-ticketing) and no password is
            // on the command line: until QMP set_password runs nobody can
            // connect, and the password never shows up in `ps`.
            let spec = format!(
                "port={},addr={}",
                rt.spice_port.unwrap_or(61000),
                rt.spice_listen.as_deref().unwrap_or("127.0.0.1")
            );
            push(&["-spice", &spec]);
            // Clipboard / resolution agent channel (spice-vdagent in the guest).
            push(&[
                "-device",
                "virtio-serial-pci,id=spice-vs",
                "-chardev",
                "spicevmc,id=vdagent,name=vdagent",
                "-device",
                "virtserialport,bus=spice-vs.0,chardev=vdagent,name=com.redhat.spice.0",
            ]);
            // USB redirection from the viewer (two devices).
            push(&["-device", "qemu-xhci,id=spice-xhci"]);
            for i in 0..2 {
                let chr = format!("spicevmc,name=usbredir,id=usbredirchr{i}");
                let dev =
                    format!("usb-redir,bus=spice-xhci.0,chardev=usbredirchr{i},id=usbredir{i}");
                push(&["-chardev", &chr, "-device", &dev]);
            }
            // Audio over the SPICE playback/record channels.
            if display.sound != SoundDevice::None {
                push(&["-audiodev", "spice,id=snd0"]);
                match display.sound {
                    SoundDevice::IntelHda => push(&[
                        "-device",
                        "intel-hda,id=sound0",
                        "-device",
                        "hda-duplex,bus=sound0.0,audiodev=snd0",
                    ]),
                    SoundDevice::Ich9IntelHda => push(&[
                        "-device",
                        "ich9-intel-hda,id=sound0",
                        "-device",
                        "hda-duplex,bus=sound0.0,audiodev=snd0",
                    ]),
                    SoundDevice::Ac97 => push(&["-device", "AC97,audiodev=snd0"]),
                    SoundDevice::None => {}
                }
            }
        }
    }
    a
}

/// Send one QMP command (after the capabilities handshake).
pub async fn qmp_command(socket: &Path, command: serde_json::Value) -> Result<serde_json::Value> {
    let fail = |e: String| horcrux_common::Error::System(format!("QMP {}: {e}", socket.display()));
    let stream = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match tokio::net::UnixStream::connect(socket).await {
                Ok(s) => return Ok(s),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await
                }
                Err(e) => return Err(e),
            }
        }
    })
    .await
    .map_err(|_| fail("timed out connecting".into()))?
    .map_err(|e| fail(e.to_string()))?;
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    qmp_reply(&mut lines).await.map_err(fail)?; // greeting
    w.write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
        .await
        .map_err(|e| fail(e.to_string()))?;
    qmp_reply(&mut lines).await.map_err(fail)?;
    w.write_all(format!("{command}\n").as_bytes())
        .await
        .map_err(|e| fail(e.to_string()))?;
    let reply = qmp_reply(&mut lines).await.map_err(fail)?;
    if let Some(err) = reply.get("error") {
        return Err(fail(err.to_string()));
    }
    Ok(reply["return"].clone())
}

/// Next greeting/reply line from QMP, skipping asynchronous events.
async fn qmp_reply(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
) -> std::result::Result<serde_json::Value, String> {
    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "QMP closed".to_string())?;
        let v: serde_json::Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if v.get("return").is_some() || v.get("error").is_some() || v.get("QMP").is_some() {
            return Ok(v);
        }
    }
}

/// Install this run's VNC/SPICE password in QEMU.
pub async fn apply_password(rt: &DisplayRuntime) -> Result<()> {
    let protocol = match rt.protocol {
        DisplayProtocol::None => return Ok(()),
        DisplayProtocol::Vnc => "vnc",
        DisplayProtocol::Spice => "spice",
    };
    if let Some(password) = &rt.password {
        qmp_command(
            &rt.qmp_socket,
            serde_json::json!({"execute": "set_password",
                "arguments": {"protocol": protocol, "password": password}}),
        )
        .await?;
    }
    Ok(())
}

/// A `.vv` file for virt-viewer/remote-viewer. `host` is what the viewer
/// should dial (the SPICE listen address, or the API host when SPICE
/// listens on all interfaces).
pub fn virt_viewer_file(title: &str, host: &str, rt: &DisplayRuntime) -> Option<String> {
    let port = rt.spice_port?;
    let password = rt.password.as_deref()?;
    Some(format!(
        "[virt-viewer]\ntype=spice\nhost={host}\nport={port}\npassword={password}\n\
         title={title}\ndelete-this-file=1\nenable-usbredir=1\nenable-smartcard=0\n\
         toggle-fullscreen=shift+f11\nrelease-cursor=shift+f12\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(protocol: DisplayProtocol) -> DisplayRuntime {
        DisplayRuntime {
            protocol,
            vnc_port: Some(5903),
            spice_port: Some(61002),
            spice_listen: Some("127.0.0.1".into()),
            password: Some("pw".into()),
            serial_socket: "/vms/a.serial.sock".into(),
            serial_log: "/vms/a.serial.log".into(),
            qmp_socket: "/vms/a.qmp.sock".into(),
        }
    }

    fn joined(d: VmDisplay, p: DisplayProtocol) -> String {
        qemu_args(&d, &VmArchitecture::X86_64, &rt(p)).join(" ")
    }

    #[test]
    fn serial_is_a_socket_that_still_logs() {
        let a = joined(VmDisplay::default(), DisplayProtocol::Vnc);
        assert!(a.contains("-chardev socket,id=serial0,path=/vms/a.serial.sock,server=on,wait=off,logfile=/vms/a.serial.log"));
        assert!(a.contains("-serial chardev:serial0"));
        assert!(a.contains("-qmp unix:/vms/a.qmp.sock,server=on,wait=off"));
    }

    #[test]
    fn vnc_binds_localhost_with_password() {
        let a = joined(VmDisplay::default(), DisplayProtocol::Vnc);
        assert!(a.contains("-vnc 127.0.0.1:3,password=on"), "{a}");
        assert!(a.contains("-vga std"));
        assert!(!a.contains("-spice"));
    }

    #[test]
    fn spice_has_audio_agent_and_usbredir_and_no_password_on_cmdline() {
        let d = VmDisplay {
            protocol: DisplayProtocol::Spice,
            video: VideoDevice::Qxl,
            sound: SoundDevice::Ich9IntelHda,
        };
        let a = joined(d, DisplayProtocol::Spice);
        assert!(a.contains("-spice port=61002,addr=127.0.0.1"), "{a}");
        assert!(
            !a.contains("password") && !a.contains("disable-ticketing"),
            "{a}"
        );
        assert!(a.contains("-vga qxl"));
        assert!(a.contains("-audiodev spice,id=snd0"));
        assert!(a.contains(
            "-device ich9-intel-hda,id=sound0 -device hda-duplex,bus=sound0.0,audiodev=snd0"
        ));
        assert!(a.contains("name=com.redhat.spice.0"));
        assert_eq!(a.matches("usb-redir,").count(), 2);
        let ac97 = joined(
            VmDisplay {
                sound: SoundDevice::Ac97,
                ..d
            },
            DisplayProtocol::Spice,
        );
        assert!(ac97.contains("-device AC97,audiodev=snd0"));
    }

    #[test]
    fn headless_and_non_pc() {
        let d = VmDisplay {
            protocol: DisplayProtocol::None,
            video: VideoDevice::None,
            sound: SoundDevice::None,
        };
        let a = joined(d, DisplayProtocol::None);
        assert!(a.contains("-vga none") && !a.contains("-vnc") && !a.contains("-spice"));
        let arm = qemu_args(
            &VmDisplay::default(),
            &VmArchitecture::Aarch64,
            &rt(DisplayProtocol::Vnc),
        )
        .join(" ");
        assert!(arm.contains("-device virtio-gpu-pci") && !arm.contains("-vga"));
    }

    #[test]
    fn validation() {
        let x86 = VmArchitecture::X86_64;
        let sound_vnc = VmDisplay {
            sound: SoundDevice::IntelHda,
            ..Default::default()
        };
        assert!(sound_vnc.validate(&x86).is_err());
        let qxl_arm = VmDisplay {
            video: VideoDevice::Qxl,
            ..Default::default()
        };
        assert!(qxl_arm.validate(&VmArchitecture::Aarch64).is_err());
        assert!(VmDisplay::default().validate(&x86).is_ok());
    }

    #[test]
    fn vv_file() {
        let f = virt_viewer_file("vm1", "10.0.0.5", &rt(DisplayProtocol::Spice)).unwrap();
        assert!(f.contains("type=spice") && f.contains("port=61002") && f.contains("password=pw"));
        assert!(f.contains("host=10.0.0.5"));
    }
}
