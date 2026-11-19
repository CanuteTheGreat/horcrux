//! QEMU's D-Bus display as a Horcrux Console frame source.
//!
//! VMs started while the console is enabled run with
//! `-display dbus,p2p=yes`: QEMU exports its consoles, keyboard, mouse and
//! audio on peer-to-peer D-Bus connections that it accepts through QMP
//! (`getfd` + `add_client protocol=@dbus-display`). Per viewer session the
//! broker opens one such connection, registers a listener object on a
//! second socket pair (QEMU connects to it as the D-Bus client) and gets
//! the guest's framebuffer (`Scanout`/`Update`), cursor and audio pushed to
//! it. No guest tools are involved, so this works for every guest and
//! before any OS runs (firmware, boot menus, installers).
//!
//! Interfaces used (QEMU `ui/dbus-display1.xml`, QEMU 7.0+):
//! `org.qemu.Display1.Console` (RegisterListener, Width/Height),
//! `org.qemu.Display1.Keyboard` (Press/Release qnum),
//! `org.qemu.Display1.Mouse` (Press/Release/SetAbsPosition/RelMotion,
//! IsAbsolute), `org.qemu.Display1.Listener` (implemented here),
//! `org.qemu.Display1.Audio` + `org.qemu.Display1.AudioOutListener`.
//! Consumed over D-Bus only: nothing of QEMU is linked or copied.
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::Path;
use tokio::sync::mpsc;
use zbus::zvariant;

/// Something the guest's display did.
#[derive(Debug)]
pub enum DisplayEvent {
    /// Whole new framebuffer (size or format change, or first frame).
    Scanout {
        width: u32,
        height: u32,
        stride: u32,
        format: u32,
        data: Vec<u8>,
    },
    /// Damaged rectangle.
    Update {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        stride: u32,
        format: u32,
        data: Vec<u8>,
    },
    /// The console has no picture (guest switched the display off).
    Disable,
    CursorDefine {
        width: i32,
        height: i32,
        hot_x: i32,
        hot_y: i32,
        data: Vec<u8>,
    },
    MouseSet {
        x: i32,
        y: i32,
        on: i32,
    },
    AudioInit {
        id: u64,
        bits: u8,
        is_signed: bool,
        is_float: bool,
        freq: u32,
        channels: u8,
    },
    AudioFini {
        id: u64,
    },
    AudioEnabled {
        id: u64,
        enabled: bool,
    },
    AudioData {
        id: u64,
        data: Vec<u8>,
    },
}

/// Hands the far end of a socket pair to QEMU over QMP with SCM_RIGHTS,
/// then asks the D-Bus display to accept it as a client.
pub fn qmp_add_dbus_client(qmp_socket: &Path, fd: &OwnedFd, name: &str) -> std::io::Result<()> {
    use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags};
    use std::io::{BufRead, BufReader, Write};
    let err = std::io::Error::other::<String>;
    let mut s = StdUnixStream::connect(qmp_socket)?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    let mut r = BufReader::new(s.try_clone()?);
    let reply = |r: &mut BufReader<StdUnixStream>| -> std::io::Result<serde_json::Value> {
        loop {
            let mut line = String::new();
            if r.read_line(&mut line)? == 0 {
                return Err(err("QMP closed".into()));
            }
            let v: serde_json::Value =
                serde_json::from_str(&line).map_err(|e| err(format!("QMP: {e}")))?;
            if v.get("return").is_some() || v.get("QMP").is_some() {
                return Ok(v);
            }
            if let Some(e) = v.get("error") {
                return Err(err(format!("QMP error: {e}")));
            }
        }
    };
    reply(&mut r)?; // greeting
    s.write_all(b"{\"execute\":\"qmp_capabilities\"}\n")?;
    reply(&mut r)?;
    let getfd =
        serde_json::json!({"execute": "getfd", "arguments": {"fdname": name}}).to_string() + "\n";
    let iov = [std::io::IoSlice::new(getfd.as_bytes())];
    let fds = [fd.as_raw_fd()];
    let cmsg = [ControlMessage::ScmRights(&fds)];
    sendmsg::<()>(s.as_raw_fd(), &iov, &cmsg, MsgFlags::empty(), None)
        .map_err(|e| err(format!("sendmsg: {e}")))?;
    reply(&mut r)?;
    let add = serde_json::json!({"execute": "add_client",
        "arguments": {"protocol": "@dbus-display", "fdname": name}})
    .to_string()
        + "\n";
    s.write_all(add.as_bytes())?;
    reply(&mut r)?;
    Ok(())
}

#[zbus::proxy(
    interface = "org.qemu.Display1.Console",
    default_service = "org.qemu",
    default_path = "/org/qemu/Display1/Console_0"
)]
pub trait Console {
    fn register_listener(&self, listener: zvariant::Fd<'_>) -> zbus::Result<()>;
    #[zbus(property)]
    fn width(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn height(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn label(&self) -> zbus::Result<String>;
}

#[zbus::proxy(
    interface = "org.qemu.Display1.Keyboard",
    default_service = "org.qemu",
    default_path = "/org/qemu/Display1/Console_0"
)]
pub trait Keyboard {
    fn press(&self, keycode: u32) -> zbus::Result<()>;
    fn release(&self, keycode: u32) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.qemu.Display1.Mouse",
    default_service = "org.qemu",
    default_path = "/org/qemu/Display1/Console_0"
)]
pub trait Mouse {
    fn press(&self, button: u32) -> zbus::Result<()>;
    fn release(&self, button: u32) -> zbus::Result<()>;
    fn set_abs_position(&self, x: u32, y: u32) -> zbus::Result<()>;
    fn rel_motion(&self, dx: i32, dy: i32) -> zbus::Result<()>;
    #[zbus(property)]
    fn is_absolute(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.qemu.Display1.Audio",
    default_service = "org.qemu",
    default_path = "/org/qemu/Display1/Audio"
)]
pub trait Audio {
    fn register_out_listener(&self, listener: zvariant::Fd<'_>) -> zbus::Result<()>;
}

/// The listener QEMU pushes the framebuffer to.
struct Listener {
    tx: mpsc::Sender<DisplayEvent>,
}

// `spawn = false`: QEMU pushes Update calls without waiting for replies;
// zbus would run each in its own task and could apply an older damage
// rectangle after a newer one (stale stripes). In order, in the dispatch task.
#[zbus::interface(name = "org.qemu.Display1.Listener", spawn = false)]
impl Listener {
    async fn scanout(
        &self,
        width: u32,
        height: u32,
        stride: u32,
        pixman_format: u32,
        data: Vec<u8>,
    ) {
        let _ = self
            .tx
            .send(DisplayEvent::Scanout {
                width,
                height,
                stride,
                format: pixman_format,
                data,
            })
            .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn update(
        &self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        stride: u32,
        pixman_format: u32,
        data: Vec<u8>,
    ) {
        let _ = self
            .tx
            .send(DisplayEvent::Update {
                x,
                y,
                width,
                height,
                stride,
                format: pixman_format,
                data,
            })
            .await;
    }

    async fn disable(&self) {
        let _ = self.tx.send(DisplayEvent::Disable).await;
    }

    async fn mouse_set(&self, x: i32, y: i32, on: i32) {
        let _ = self.tx.send(DisplayEvent::MouseSet { x, y, on }).await;
    }

    async fn cursor_define(&self, width: i32, height: i32, hot_x: i32, hot_y: i32, data: Vec<u8>) {
        let _ = self
            .tx
            .send(DisplayEvent::CursorDefine {
                width,
                height,
                hot_x,
                hot_y,
                data,
            })
            .await;
    }

    /// Optional listener interfaces QEMU may use (shared-memory maps,
    /// DMA-BUF scanouts): none, so QEMU sends pixels inline.
    #[zbus(property)]
    async fn interfaces(&self) -> Vec<String> {
        Vec::new()
    }
}

struct AudioOutListener {
    tx: mpsc::Sender<DisplayEvent>,
}

#[zbus::interface(name = "org.qemu.Display1.AudioOutListener", spawn = false)]
impl AudioOutListener {
    #[allow(clippy::too_many_arguments)]
    async fn init(
        &self,
        id: u64,
        bits: u8,
        is_signed: bool,
        is_float: bool,
        freq: u32,
        nchannels: u8,
        _bytes_per_frame: u32,
        _bytes_per_second: u32,
        _be: bool,
    ) {
        let _ = self
            .tx
            .send(DisplayEvent::AudioInit {
                id,
                bits,
                is_signed,
                is_float,
                freq,
                channels: nchannels,
            })
            .await;
    }
    async fn fini(&self, id: u64) {
        let _ = self.tx.send(DisplayEvent::AudioFini { id }).await;
    }
    async fn set_enabled(&self, id: u64, enabled: bool) {
        let _ = self
            .tx
            .send(DisplayEvent::AudioEnabled { id, enabled })
            .await;
    }
    async fn set_volume(&self, _id: u64, _mute: bool, _volume: Vec<u8>) {}
    async fn write(&self, id: u64, data: Vec<u8>) {
        // Audio is best effort: never block QEMU's audio thread on a slow
        // viewer.
        let _ = self.tx.try_send(DisplayEvent::AudioData { id, data });
    }
    #[zbus(property)]
    async fn interfaces(&self) -> Vec<String> {
        Vec::new()
    }
}

/// One viewer's connection to a VM's D-Bus display.
pub struct DbusDisplay {
    pub conn: zbus::Connection,
    /// Kept alive: QEMU talks to our listener over it.
    _listener: zbus::Connection,
    _audio_listener: Option<zbus::Connection>,
    pub keyboard: KeyboardProxy<'static>,
    pub mouse: MouseProxy<'static>,
    pub absolute: bool,
}

fn pair() -> std::io::Result<(tokio::net::UnixStream, OwnedFd)> {
    let (a, b) = StdUnixStream::pair()?;
    a.set_nonblocking(true)?;
    Ok((tokio::net::UnixStream::from_std(a)?, OwnedFd::from(b)))
}

async fn serve_listener<I: zbus::object_server::Interface>(
    ours: tokio::net::UnixStream,
    path: &str,
    iface: I,
) -> zbus::Result<zbus::Connection> {
    // QEMU is the D-Bus *server* of a listener connection (it generates the
    // GUID and authenticates us, ui/dbus-console.c), and it answers
    // RegisterListener only after that handshake: we are the client.
    zbus::connection::Builder::unix_stream(ours)
        .p2p()
        .serve_at(path, iface)?
        .build()
        .await
}

impl DbusDisplay {
    /// Connect to the VM whose QMP socket is `qmp_socket`. Events (frames,
    /// cursor, audio when `audio`) arrive on `tx`.
    pub async fn connect(
        qmp_socket: &Path,
        session: &str,
        tx: mpsc::Sender<DisplayEvent>,
        audio: bool,
    ) -> Result<Self, String> {
        // A wedged QEMU (or monitor) must fail the request, not hang it.
        match tokio::time::timeout(
            std::time::Duration::from_secs(20),
            Self::connect_inner(qmp_socket, session, tx, audio),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err("QEMU's D-Bus display did not answer within 20 s".into()),
        }
    }

    async fn connect_inner(
        qmp_socket: &Path,
        session: &str,
        tx: mpsc::Sender<DisplayEvent>,
        audio: bool,
    ) -> Result<Self, String> {
        let (ours, theirs) = pair().map_err(|e| e.to_string())?;
        let qmp = qmp_socket.to_path_buf();
        let name = format!("hxc-{session}");
        tokio::task::spawn_blocking(move || qmp_add_dbus_client(&qmp, &theirs, &name))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("QEMU refused the D-Bus display client: {e}"))?;
        tracing::debug!("hxc {session}: QEMU accepted the D-Bus client");
        let conn = zbus::connection::Builder::unix_stream(ours)
            .p2p()
            .build()
            .await
            .map_err(|e| format!("D-Bus handshake with QEMU: {e}"))?;
        let console = ConsoleProxy::new(&conn)
            .await
            .map_err(|e| format!("console proxy: {e}"))?;

        // QEMU connects to our listener as a client; serve it while the
        // registration call is in flight.
        let (ours, theirs) = pair().map_err(|e| e.to_string())?;
        let serve = serve_listener(
            ours,
            "/org/qemu/Display1/Listener",
            Listener { tx: tx.clone() },
        );
        let register = console.register_listener(zvariant::Fd::from(&theirs));
        let (listener, reg) = tokio::join!(serve, register);
        reg.map_err(|e| format!("RegisterListener: {e}"))?;
        tracing::debug!("hxc {session}: display listener registered");
        let listener = listener.map_err(|e| format!("listener connection: {e}"))?;
        drop(theirs);

        let mut audio_listener = None;
        if audio {
            if let Ok(proxy) = AudioProxy::new(&conn).await {
                let (ours, theirs) = pair().map_err(|e| e.to_string())?;
                let serve = serve_listener(
                    ours,
                    "/org/qemu/Display1/AudioOutListener",
                    AudioOutListener { tx },
                );
                let register = proxy.register_out_listener(zvariant::Fd::from(&theirs));
                let (l, reg) = tokio::join!(serve, register);
                match (l, reg) {
                    (Ok(l), Ok(())) => audio_listener = Some(l),
                    (_, Err(e)) => tracing::debug!("no D-Bus audio for {session}: {e}"),
                    (Err(e), _) => tracing::debug!("audio listener for {session}: {e}"),
                }
            }
        }

        let keyboard = KeyboardProxy::new(&conn)
            .await
            .map_err(|e| format!("keyboard proxy: {e}"))?;
        let mouse = MouseProxy::new(&conn)
            .await
            .map_err(|e| format!("mouse proxy: {e}"))?;
        let absolute = mouse.is_absolute().await.unwrap_or(false);
        Ok(Self {
            conn,
            _listener: listener,
            _audio_listener: audio_listener,
            keyboard,
            mouse,
            absolute,
        })
    }
}

/// Does `binary` have the D-Bus display (`-display help` lists `dbus`)?
/// Distributions ship it as a loadable module (Debian/Ubuntu:
/// qemu-system-modules-opengl; Gentoo: USE=opengl + dbus support).
pub async fn qemu_has_dbus_display(binary: &str) -> bool {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, bool>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().ok().and_then(|c| c.get(binary).copied()) {
        return v;
    }
    let out = tokio::process::Command::new(binary)
        .args(["-display", "help"])
        .stdin(std::process::Stdio::null())
        .output()
        .await;
    let v = match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .any(|l| l.trim() == "dbus"),
        Err(_) => false,
    };
    if let Ok(mut c) = cache.lock() {
        c.insert(binary.to_string(), v);
    }
    v
}
