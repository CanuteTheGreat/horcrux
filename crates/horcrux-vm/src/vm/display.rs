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
    /// Started with `-display dbus,p2p=yes` for the Horcrux Console
    /// (set by the start path when the console is enabled and QEMU has
    /// the D-Bus display; see console::hxc).
    #[serde(default)]
    pub dbus_display: bool,
    /// Horcrux guest-tools virtio-serial port (name, host socket), when
    /// the console's guest tools are enabled for this run.
    #[serde(default)]
    pub guest_tools_port: Option<String>,
    #[serde(default)]
    pub guest_tools_socket: Option<PathBuf>,
    /// Guest tools CD attached to this run (virtio-scsi, read-only).
    #[serde(default)]
    pub guest_tools_iso: Option<PathBuf>,
    /// Horcrux frame ring for the same-host viewer: backing file of the
    /// ivshmem device (`[console] shm_frames*`) and its size in MiB.
    #[serde(default)]
    pub frame_ring: Option<PathBuf>,
    #[serde(default)]
    pub frame_ring_mib: u32,
    /// Guest audio goes to the Horcrux Console over QEMU's D-Bus audio
    /// (`-audiodev dbus`) instead of being discarded.
    #[serde(default)]
    pub dbus_audio: bool,
}

impl DisplayRuntime {
    pub fn state_path(disk: &Path) -> PathBuf {
        super::sidecar::anchor(disk).with_extension("display.json")
    }

    pub fn serial_socket(disk: &Path) -> PathBuf {
        super::sidecar::anchor(disk).with_extension("serial.sock")
    }

    pub fn serial_log(disk: &Path) -> PathBuf {
        super::sidecar::anchor(disk).with_extension("serial.log")
    }

    pub fn qmp_socket(disk: &Path) -> PathBuf {
        super::sidecar::anchor(disk).with_extension("qmp.sock")
    }

    /// Host end of the Horcrux guest tools' virtio-serial port.
    pub fn guest_tools_socket_path(disk: &Path) -> PathBuf {
        super::sidecar::anchor(disk).with_extension("hxgt.sock")
    }

    /// Host end of the guest agent's virtio-serial channel.
    pub fn qga_socket(disk: &Path) -> PathBuf {
        super::sidecar::anchor(disk).with_extension("qga.sock")
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
            dbus_display: false,
            guest_tools_port: None,
            guest_tools_socket: None,
            guest_tools_iso: None,
            frame_ring: None,
            frame_ring_mib: 0,
            dbus_audio: false,
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
        // The frame ring lives outside the VM's directory: its path is in
        // the state file.
        if let Ok(rt) = Self::load(disk).await {
            if let Some(ring) = rt.frame_ring {
                let _ = tokio::fs::remove_file(ring).await;
            }
        }
        for p in [
            Self::state_path(disk),
            Self::serial_socket(disk),
            Self::qmp_socket(disk),
            Self::qga_socket(disk),
            Self::guest_tools_socket_path(disk),
        ] {
            let _ = tokio::fs::remove_file(p).await;
        }
    }
}

/// Backing file of a VM's Horcrux frame ring.
pub fn frame_ring_path(dir: &str, vm_id: &str) -> PathBuf {
    let safe: String = vm_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Path::new(dir).join(format!("horcrux-frames-{safe}"))
}

/// Whether `dir`'s filesystem has room for a `len`-byte ring.
fn ring_fits(dir: &str, len: u64) -> std::io::Result<()> {
    let st = nix::sys::statvfs::statvfs(Path::new(dir)).map_err(std::io::Error::from)?;
    let avail = (st.blocks_available() as u64).saturating_mul(st.fragment_size() as u64);
    if avail < len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            format!(
                "{dir} has {} MiB free, the frame ring needs {} MiB",
                avail >> 20,
                len >> 20
            ),
        ));
    }
    Ok(())
}

/// Create (or reset) a VM's frame ring before QEMU starts: a root-only
/// file of `mib` MiB under `dir` (tmpfs or hugetlbfs) whose header the
/// guest tools and the viewer attach to. Pages are only used as frames are
/// written. The file is mapped to write the header because hugetlbfs has
/// no write(2).
pub fn create_frame_ring(
    dir: &str,
    vm_id: &str,
    mib: u32,
    slots: u32,
    want_fps: u32,
) -> std::io::Result<PathBuf> {
    use nix::libc;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let path = frame_ring_path(dir, vm_id);
    let len = (mib as usize) << 20;
    let _ = std::fs::remove_file(&path);
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    // QEMU maps the ring as guest RAM (shared) and reads all of it when the
    // VM is saved or live-migrated; a page the filesystem cannot back (a
    // ring larger than a small tmpfs such as docker's 64 MiB /dev/shm) is a
    // SIGBUS that kills QEMU mid-migration. So refuse a ring the directory
    // cannot hold; the caller then starts the VM without one.
    if let Err(e) = ring_fits(dir, len as u64) {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    f.set_len(len as u64)?;
    // SAFETY: shared mapping of the file just sized; unmapped below.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            f.as_raw_fd(),
            0,
        )
    };
    if p == libc::MAP_FAILED {
        let e = std::io::Error::last_os_error();
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    // SAFETY: p is a fresh len-byte shared mapping, page aligned.
    let r = unsafe { horcrux_guest_proto::ring::Ring::init(p as *mut u8, len, slots, want_fps) };
    // SAFETY: unmapping what was mapped above; `r` is dropped first.
    let res = r.map(drop);
    unsafe {
        libc::munmap(p, len);
    }
    if let Err(e) = res {
        let _ = std::fs::remove_file(&path);
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, e));
    }
    Ok(path)
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

/// The sound card plays into QEMU's D-Bus audio (read by the Horcrux
/// Console) rather than SPICE or nowhere.
fn dbus_audio(display: &VmDisplay, rt: &DisplayRuntime) -> bool {
    display.sound != SoundDevice::None
        && display.protocol != DisplayProtocol::Spice
        && rt.dbus_display
        && rt.dbus_audio
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
    // The Horcrux Console reads the display over peer-to-peer D-Bus
    // (connections are handed to QEMU through QMP add_client).
    if rt.dbus_display {
        // QEMU exports the D-Bus Audio interface only for the audiodev the
        // display names (`-display dbus,audiodev=`); a bare `-audiodev dbus`
        // plays into nothing a client can reach.
        if dbus_audio(display, rt) {
            push(&["-display", "dbus,p2p=yes,audiodev=snd0"]);
        } else {
            push(&["-display", "dbus,p2p=yes"]);
        }
    } else {
        push(&["-display", "none"]);
    }
    // Horcrux guest tools: their own virtio-serial port; the console
    // broker connects to the socket while a viewer is open.
    if let (Some(name), Some(sock)) = (&rt.guest_tools_port, &rt.guest_tools_socket) {
        let chr = format!("socket,id=hxgt0,path={},server=on,wait=off", sock.display());
        let port = format!("virtserialport,bus=hxgt-vs.0,chardev=hxgt0,name={name}");
        push(&[
            "-device",
            "virtio-serial-pci,id=hxgt-vs",
            "-chardev",
            &chr,
            "-device",
            &port,
        ]);
        // The tools CD on its own virtio-scsi controller (no collision with
        // the VM's IDE/SATA CD drives; the guest needs virtio drivers for
        // the port anyway).
        if let Some(iso) = &rt.guest_tools_iso {
            let drive = format!(
                "file={},if=none,id=hxgtcd,media=cdrom,readonly=on",
                iso.display()
            );
            push(&[
                "-device",
                "virtio-scsi-pci,id=hxgtscsi",
                "-drive",
                &drive,
                "-device",
                "scsi-cd,bus=hxgtscsi.0,drive=hxgtcd",
            ]);
        }
    }

    // Horcrux frame ring: raw frames for the same-host native viewer
    // (guest tools write, the viewer reads; see horcrux-guest-proto ring).
    if let Some(ring) = &rt.frame_ring {
        let mem = format!(
            "memory-backend-file,id=hxfbmem,size={}M,mem-path={},share=on",
            rt.frame_ring_mib,
            ring.display()
        );
        push(&[
            "-object",
            &mem,
            "-device",
            // master=on: without it QEMU marks the device "peer mode" and
            // refuses every live migration of the VM ("Migration is disabled
            // when using feature 'peer mode' in device 'ivshmem'").
            "ivshmem-plain,memdev=hxfbmem,id=hxframes,master=on",
        ]);
    }

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
        }
    }
    // Sound card: audio goes over SPICE's channels when the display is SPICE;
    // otherwise the card is present but its host-side output is discarded
    // (guest-side remote access such as RDP can still carry audio).
    if display.sound != SoundDevice::None {
        let backend = if display.protocol == DisplayProtocol::Spice {
            "spice,id=snd0"
        } else if dbus_audio(display, rt) {
            // the Horcrux Console reads it over the D-Bus display
            "dbus,id=snd0"
        } else {
            "none,id=snd0"
        };
        push(&["-audiodev", backend]);
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
            dbus_display: false,
            guest_tools_port: None,
            guest_tools_socket: None,
            guest_tools_iso: None,
            frame_ring: None,
            frame_ring_mib: 0,
            dbus_audio: false,
        }
    }

    #[test]
    fn frame_ring_file_has_a_header_the_guest_can_attach_to() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        let p = create_frame_ring(d, "vm/1", 8, 3, 120).unwrap();
        assert_eq!(p, dir.path().join("horcrux-frames-vm_1"));
        use std::os::unix::fs::PermissionsExt;
        let md = std::fs::metadata(&p).unwrap();
        assert_eq!(md.len(), 8 << 20);
        assert_eq!(md.permissions().mode() & 0o777, 0o600);
        let bytes = std::fs::read(&p).unwrap();
        let mut aligned = vec![0u64; bytes.len() / 8];
        // SAFETY: copying into an aligned buffer of the same size.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                aligned.as_mut_ptr() as *mut u8,
                bytes.len(),
            );
            let r = horcrux_guest_proto::ring::Ring::attach(
                aligned.as_mut_ptr() as *mut u8,
                bytes.len(),
            )
            .unwrap();
            assert_eq!(r.want_fps(), 120);
            assert_eq!(r.layout().slot_count, 3);
        }
        // a stale ring from an earlier run is replaced
        create_frame_ring(d, "vm/1", 8, 2, 60).unwrap();
        assert!(
            create_frame_ring(d, "vm/1", 8, 9, 60).is_err(),
            "too many slots"
        );
        assert!(!dir.path().join("horcrux-frames-vm_1").exists());
    }

    #[test]
    fn frame_ring_larger_than_the_filesystem_is_refused() {
        // A ring the tmpfs cannot back would SIGBUS QEMU when the VM is saved
        // or migrated (docker's 64 MiB /dev/shm vs the 128 MiB default).
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        let e = create_frame_ring(d, "vm/2", u32::MAX, 3, 60).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::StorageFull, "{e}");
        assert!(!dir.path().join("horcrux-frames-vm_2").exists());
    }

    #[test]
    fn frame_ring_device_and_dbus_audio() {
        let mut r = rt(DisplayProtocol::Vnc);
        let a = qemu_args(&VmDisplay::default(), &VmArchitecture::X86_64, &r).join(" ");
        assert!(
            !a.contains("hxframes") && !a.contains("audiodev dbus"),
            "{a}"
        );
        r.frame_ring = Some(PathBuf::from("/dev/shm/horcrux-frames-vm1"));
        r.frame_ring_mib = 128;
        r.dbus_display = true;
        r.dbus_audio = true;
        let d = VmDisplay {
            sound: SoundDevice::IntelHda,
            ..VmDisplay::default()
        };
        let a = qemu_args(&d, &VmArchitecture::X86_64, &r).join(" ");
        assert!(
            a.contains("-object memory-backend-file,id=hxfbmem,size=128M,mem-path=/dev/shm/horcrux-frames-vm1,share=on -device ivshmem-plain,memdev=hxfbmem,id=hxframes"),
            "{a}"
        );
        // regression: a peer-mode ivshmem blocks live migration
        assert!(a.contains("id=hxframes,master=on"), "{a}");
        assert!(a.contains("-audiodev dbus,id=snd0"), "{a}");
        // the display must name the audiodev, or QEMU exports no Audio object
        assert!(a.contains("-display dbus,p2p=yes,audiodev=snd0"), "{a}");
        // without the D-Bus display there is no D-Bus audio
        r.dbus_display = false;
        let a = qemu_args(&d, &VmArchitecture::X86_64, &r).join(" ");
        assert!(a.contains("-audiodev none,id=snd0"), "{a}");
    }

    fn joined(d: VmDisplay, p: DisplayProtocol) -> String {
        qemu_args(&d, &VmArchitecture::X86_64, &rt(p)).join(" ")
    }

    #[test]
    fn guest_tools_port_when_enabled() {
        let mut r = rt(DisplayProtocol::Vnc);
        let a = qemu_args(&VmDisplay::default(), &VmArchitecture::X86_64, &r).join(" ");
        assert!(!a.contains("hxgt"), "{a}");
        r.guest_tools_port = Some("org.horcrux.console.0".into());
        r.guest_tools_socket = Some(PathBuf::from("/vms/a.hxgt.sock"));
        let a = qemu_args(&VmDisplay::default(), &VmArchitecture::X86_64, &r).join(" ");
        assert!(
            a.contains("-device virtio-serial-pci,id=hxgt-vs -chardev socket,id=hxgt0,path=/vms/a.hxgt.sock,server=on,wait=off -device virtserialport,bus=hxgt-vs.0,chardev=hxgt0,name=org.horcrux.console.0"),
            "{a}"
        );
        assert!(!a.contains("hxgtcd"), "{a}");
        r.guest_tools_iso = Some(PathBuf::from("/usr/share/hx/t.iso"));
        let a = qemu_args(&VmDisplay::default(), &VmArchitecture::X86_64, &r).join(" ");
        assert!(
            a.contains("-device virtio-scsi-pci,id=hxgtscsi -drive file=/usr/share/hx/t.iso,if=none,id=hxgtcd,media=cdrom,readonly=on -device scsi-cd,bus=hxgtscsi.0,drive=hxgtcd"),
            "{a}"
        );
    }

    #[test]
    fn dbus_display_for_the_horcrux_console() {
        let mut r = rt(DisplayProtocol::Vnc);
        let a = qemu_args(&VmDisplay::default(), &VmArchitecture::X86_64, &r).join(" ");
        assert!(a.contains("-display none") && !a.contains("dbus"), "{a}");
        r.dbus_display = true;
        let a = qemu_args(&VmDisplay::default(), &VmArchitecture::X86_64, &r).join(" ");
        assert!(
            a.contains("-display dbus,p2p=yes") && !a.contains("-display none"),
            "{a}"
        );
        // the classic console keeps working next to it
        assert!(a.contains("-vnc 127.0.0.1:3,password=on"), "{a}");
        // old state files without the field still load
        let old = r#"{"protocol":"vnc","vnc_port":5900,"spice_port":null,"spice_listen":null,
            "password":null,"serial_socket":"/a","serial_log":"/b","qmp_socket":"/c"}"#;
        let back: DisplayRuntime = serde_json::from_str(old).unwrap();
        assert!(!back.dbus_display);
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
            relative_mouse: false,
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
            relative_mouse: false,
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
        // Sound is allowed with VNC (guest-side RDP/SPICE can carry it).
        assert!(sound_vnc.validate(&x86).is_ok());
        let args = qemu_args(&sound_vnc, &x86, &rt(DisplayProtocol::Vnc)).join(" ");
        assert!(args.contains("-audiodev none,id=snd0") && args.contains("intel-hda,id=sound0"));
        let spice_snd = VmDisplay {
            protocol: DisplayProtocol::Spice,
            sound: SoundDevice::Ac97,
            ..Default::default()
        };
        let args = qemu_args(&spice_snd, &x86, &rt(DisplayProtocol::Spice)).join(" ");
        assert!(args.contains("-audiodev spice,id=snd0"));
        let non_x86_sound = VmDisplay {
            sound: SoundDevice::Ac97,
            ..Default::default()
        };
        assert!(non_x86_sound.validate(&VmArchitecture::Aarch64).is_err());
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
