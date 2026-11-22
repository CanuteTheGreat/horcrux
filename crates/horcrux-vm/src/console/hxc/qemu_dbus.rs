//! QEMU's D-Bus display as a Horcrux Console frame source.
//!
//! VMs started while the console is enabled run with
//! `-display dbus,p2p=yes`: QEMU exports its consoles, keyboard, mouse and
//! audio on peer-to-peer D-Bus connections that it accepts through QMP
//! (`getfd` + `add_client protocol=@dbus-display`). QEMU serves its
//! objects on only the newest such connection, so all viewer sessions of a
//! VM share one (see `Control`); per viewer session the broker registers a
//! listener object on its own socket pair (QEMU connects to it as the
//! D-Bus client) and gets
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
use std::collections::HashMap;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
    /// `RefreshRate` offered to QEMU (millihertz, 0 = QEMU's default).
    refresh_mhz: u32,
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

    /// How often (millihertz) QEMU should poll display devices that do not
    /// report their own changes (VGA) while this listener is registered.
    /// QEMU builds with the listener refresh-rate support read it when the
    /// listener registers (GDBus loads every property then); others never
    /// ask and keep their 30 ms interval, so nothing depends on it.
    #[zbus(property)]
    async fn refresh_rate(&self) -> u32 {
        self.refresh_mhz
    }
}

/// One audio voice QEMU announced (replayed to viewers that join later).
#[derive(Clone, Copy)]
struct Voice {
    bits: u8,
    is_signed: bool,
    is_float: bool,
    freq: u32,
    channels: u8,
    enabled: bool,
}

/// QEMU accepts ONE audio-out listener per VM in p2p mode (it keys them
/// by sender, and every p2p sender is "p2p": audio/dbusaudio.c), so the
/// listener is shared and fans the voices out to every viewer session.
#[derive(Default)]
struct AudioFanState {
    subs: Vec<mpsc::Sender<DisplayEvent>>,
    voices: std::collections::HashMap<u64, Voice>,
}

struct AudioOutListener {
    state: Arc<Mutex<AudioFanState>>,
}

impl AudioOutListener {
    fn subs(&self) -> Vec<mpsc::Sender<DisplayEvent>> {
        let mut st = lock(&self.state);
        st.subs.retain(|s| !s.is_closed());
        st.subs.clone()
    }
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
        lock(&self.state).voices.insert(
            id,
            Voice {
                bits,
                is_signed,
                is_float,
                freq,
                channels: nchannels,
                enabled: false,
            },
        );
        for tx in self.subs() {
            let _ = tx
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
    }
    async fn fini(&self, id: u64) {
        lock(&self.state).voices.remove(&id);
        for tx in self.subs() {
            let _ = tx.send(DisplayEvent::AudioFini { id }).await;
        }
    }
    async fn set_enabled(&self, id: u64, enabled: bool) {
        if let Some(v) = lock(&self.state).voices.get_mut(&id) {
            v.enabled = enabled;
        }
        for tx in self.subs() {
            let _ = tx.send(DisplayEvent::AudioEnabled { id, enabled }).await;
        }
    }
    async fn set_volume(&self, _id: u64, _mute: bool, _volume: Vec<u8>) {}
    async fn write(&self, id: u64, data: Vec<u8>) {
        // Audio is best effort: never block QEMU's audio thread on a slow
        // viewer.
        for tx in self.subs() {
            let _ = tx.try_send(DisplayEvent::AudioData {
                id,
                data: data.clone(),
            });
        }
    }
    #[zbus(property)]
    async fn interfaces(&self) -> Vec<String> {
        Vec::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The shared audio-out listener of one VM; dropped (QEMU unregisters it)
/// when the last viewer session holding it ends.
struct AudioFan {
    state: Arc<Mutex<AudioFanState>>,
    _listener: zbus::Connection,
}

impl AudioFan {
    /// Adds a viewer: it gets the voices QEMU already announced first.
    fn subscribe(&self, tx: mpsc::Sender<DisplayEvent>) {
        subscribe(&self.state, tx)
    }
}

fn subscribe(state: &Mutex<AudioFanState>, tx: mpsc::Sender<DisplayEvent>) {
    let mut st = lock(state);
    for (id, v) in &st.voices {
        let _ = tx.try_send(DisplayEvent::AudioInit {
            id: *id,
            bits: v.bits,
            is_signed: v.is_signed,
            is_float: v.is_float,
            freq: v.freq,
            channels: v.channels,
        });
        if v.enabled {
            let _ = tx.try_send(DisplayEvent::AudioEnabled {
                id: *id,
                enabled: true,
            });
        }
    }
    st.subs.push(tx);
}

/// QEMU's p2p D-Bus display serves its objects (consoles, keyboard,
/// mouse, audio) on ONE client connection: every QMP `add_client` moves
/// them to the new connection and closes the previous one (ui/dbus.c,
/// `g_dbus_object_manager_server_set_connection`). So all viewer sessions
/// of a VM share one control connection; each still registers its own
/// display listener (QEMU keeps any number of those, each on its own
/// socket, and they outlive a control connection being replaced).
struct Control {
    conn: zbus::Connection,
    console: ConsoleProxy<'static>,
    keyboard: KeyboardProxy<'static>,
    mouse: MouseProxy<'static>,
}

impl Control {
    async fn open(qmp_socket: &Path, session: &str) -> Result<Control, String> {
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
        let keyboard = KeyboardProxy::new(&conn)
            .await
            .map_err(|e| format!("keyboard proxy: {e}"))?;
        let mouse = MouseProxy::new(&conn)
            .await
            .map_err(|e| format!("mouse proxy: {e}"))?;
        Ok(Control {
            conn,
            console,
            keyboard,
            mouse,
        })
    }

    /// Still QEMU's current client? (GDBus answers org.freedesktop.DBus.Peer
    /// on every connection; a replaced or dead one fails at once.)
    async fn alive(&self) -> bool {
        let ping = self.conn.call_method(
            None::<&str>,
            "/org/qemu/Display1",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        );
        matches!(
            tokio::time::timeout(std::time::Duration::from_secs(3), ping).await,
            Ok(Ok(_))
        )
    }

    async fn send(&self, input: GuestInput) -> zbus::Result<()> {
        match input {
            GuestInput::KeyPress(q) => self.keyboard.press(q).await,
            GuestInput::KeyRelease(q) => self.keyboard.release(q).await,
            GuestInput::ButtonPress(b) => self.mouse.press(b).await,
            GuestInput::ButtonRelease(b) => self.mouse.release(b).await,
            GuestInput::Abs(x, y) => self.mouse.set_abs_position(x, y).await,
            GuestInput::Rel(dx, dy) => self.mouse.rel_motion(dx, dy).await,
        }
    }
}

/// Per VM (QMP socket): the shared control connection and audio listener.
#[derive(Default)]
struct Shared {
    control: std::sync::Weak<Control>,
    audio: std::sync::Weak<AudioFan>,
}

fn registry() -> &'static tokio::sync::Mutex<HashMap<PathBuf, Shared>> {
    static R: std::sync::OnceLock<tokio::sync::Mutex<HashMap<PathBuf, Shared>>> =
        std::sync::OnceLock::new();
    R.get_or_init(Default::default)
}

/// The VM's control connection: the current one when it still answers
/// (and is not `stale`), else a new one that every session then uses.
async fn control_for(
    qmp_socket: &Path,
    session: &str,
    stale: Option<&Arc<Control>>,
) -> Result<Arc<Control>, String> {
    let mut reg = registry().lock().await;
    reg.retain(|_, s| s.control.strong_count() > 0 || s.audio.strong_count() > 0);
    let entry = reg.entry(qmp_socket.to_path_buf()).or_default();
    if let Some(c) = entry.control.upgrade() {
        let replaced = stale.is_some_and(|s| !Arc::ptr_eq(s, &c));
        if replaced || (stale.is_none() && c.alive().await) {
            return Ok(c);
        }
    }
    let c = Arc::new(Control::open(qmp_socket, session).await?);
    entry.control = Arc::downgrade(&c);
    Ok(c)
}

/// One input event for the guest.
#[derive(Debug, Clone, Copy)]
pub enum GuestInput {
    KeyPress(u32),
    KeyRelease(u32),
    ButtonPress(u32),
    ButtonRelease(u32),
    Abs(u32, u32),
    Rel(i32, i32),
}

/// One viewer's connection to a VM's D-Bus display.
pub struct DbusDisplay {
    qmp_socket: PathBuf,
    session: String,
    control: Mutex<Arc<Control>>,
    /// Kept alive: QEMU talks to our listener over it.
    _listener: zbus::Connection,
    _audio: Option<Arc<AudioFan>>,
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

/// Register for QEMU's audio output (`-audiodev dbus`). Best effort and
/// bounded: a VM without D-Bus audio answers RegisterOutListener with an
/// error and never connects to our listener, so the listener side must not
/// be awaited unconditionally (that hung the whole console connect).
async fn audio_out(conn: &zbus::Connection, session: &str) -> Option<AudioFan> {
    let proxy = AudioProxy::new(conn).await.ok()?;
    let (ours, theirs) = pair().ok()?;
    let state = Arc::new(Mutex::new(AudioFanState::default()));
    let serve = tokio::spawn(serve_listener(
        ours,
        "/org/qemu/Display1/AudioOutListener",
        AudioOutListener {
            state: state.clone(),
        },
    ));
    let wait = std::time::Duration::from_secs(5);
    let reg = tokio::time::timeout(
        wait,
        proxy.register_out_listener(zvariant::Fd::from(&theirs)),
    )
    .await;
    drop(theirs);
    match reg {
        Ok(Ok(())) => match tokio::time::timeout(wait, serve).await {
            Ok(Ok(Ok(l))) => Some(AudioFan {
                state,
                _listener: l,
            }),
            Ok(Ok(Err(e))) => {
                tracing::debug!("audio listener for {session}: {e}");
                None
            }
            _ => {
                tracing::debug!("audio listener for {session}: QEMU did not connect");
                None
            }
        },
        Ok(Err(e)) => {
            serve.abort();
            tracing::debug!("no D-Bus audio for {session}: {e}");
            None
        }
        Err(_) => {
            serve.abort();
            tracing::debug!("no D-Bus audio for {session}: RegisterOutListener timed out");
            None
        }
    }
}

/// The VM's shared audio listener, registered on first use.
async fn audio_for(qmp_socket: &Path, control: &Control, session: &str) -> Option<Arc<AudioFan>> {
    let mut reg = registry().lock().await;
    let entry = reg.entry(qmp_socket.to_path_buf()).or_default();
    if let Some(a) = entry.audio.upgrade() {
        return Some(a);
    }
    let a = Arc::new(audio_out(&control.conn, session).await?);
    entry.audio = Arc::downgrade(&a);
    Some(a)
}

impl DbusDisplay {
    /// Connect to the VM whose QMP socket is `qmp_socket`. Events (frames,
    /// cursor, audio when `audio`) arrive on `tx`. `refresh_mhz` is the
    /// display poll rate offered to QEMU (`ProfileSettings::display_poll_mhz`).
    pub async fn connect(
        qmp_socket: &Path,
        session: &str,
        tx: mpsc::Sender<DisplayEvent>,
        audio: bool,
        refresh_mhz: u32,
    ) -> Result<Self, String> {
        // A wedged QEMU (or monitor) must fail the request, not hang it.
        match tokio::time::timeout(
            std::time::Duration::from_secs(20),
            Self::connect_inner(qmp_socket, session, tx, audio, refresh_mhz),
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
        refresh_mhz: u32,
    ) -> Result<Self, String> {
        let control = control_for(qmp_socket, session, None).await?;

        // QEMU connects to our listener as a client; serve it while the
        // registration call is in flight.
        let (ours, theirs) = pair().map_err(|e| e.to_string())?;
        let serve = serve_listener(
            ours,
            "/org/qemu/Display1/Listener",
            Listener {
                tx: tx.clone(),
                refresh_mhz,
            },
        );
        let register = control
            .console
            .register_listener(zvariant::Fd::from(&theirs));
        let (listener, reg) = tokio::join!(serve, register);
        reg.map_err(|e| format!("RegisterListener: {e}"))?;
        tracing::debug!("hxc {session}: display listener registered");
        let listener = listener.map_err(|e| format!("listener connection: {e}"))?;
        drop(theirs);

        let mut audio_fan = None;
        if audio {
            audio_fan = audio_for(qmp_socket, &control, session).await;
            if let Some(a) = &audio_fan {
                a.subscribe(tx);
            }
        }

        let absolute = control.mouse.is_absolute().await.unwrap_or(false);
        Ok(Self {
            qmp_socket: qmp_socket.to_path_buf(),
            session: session.to_string(),
            control: Mutex::new(control),
            _listener: listener,
            _audio: audio_fan,
            absolute,
        })
    }

    /// Send one input event. When the shared control connection was lost
    /// (QEMU accepted another D-Bus client, e.g. a tool outside Horcrux),
    /// reconnect once and resend.
    pub async fn input(&self, input: GuestInput) -> zbus::Result<()> {
        let c = lock(&self.control).clone();
        match c.send(input).await {
            Err(zbus::Error::InputOutput(e)) => {
                tracing::debug!(
                    "hxc {}: D-Bus control connection lost ({e}), reconnecting",
                    self.session
                );
                let fresh = control_for(&self.qmp_socket, &self.session, Some(&c))
                    .await
                    .map_err(zbus::Error::Failure)?;
                *lock(&self.control) = fresh.clone();
                fresh.send(input).await
            }
            r => r,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_late_viewer_gets_the_voices_already_announced() {
        let state = Arc::new(Mutex::new(AudioFanState::default()));
        let l = AudioOutListener {
            state: state.clone(),
        };
        let (tx1, mut rx1) = mpsc::channel(8);
        subscribe(&state, tx1);
        l.init(7, 16, true, false, 48000, 2, 4, 192000, false).await;
        l.set_enabled(7, true).await;
        l.write(7, vec![1, 2]).await;
        assert!(matches!(
            rx1.recv().await,
            Some(DisplayEvent::AudioInit {
                id: 7,
                freq: 48000,
                ..
            })
        ));
        assert!(matches!(
            rx1.recv().await,
            Some(DisplayEvent::AudioEnabled {
                id: 7,
                enabled: true
            })
        ));
        assert!(matches!(
            rx1.recv().await,
            Some(DisplayEvent::AudioData { id: 7, .. })
        ));

        // joins after Init/SetEnabled: replayed, then both get the data
        let (tx2, mut rx2) = mpsc::channel(8);
        subscribe(&state, tx2);
        assert!(matches!(
            rx2.recv().await,
            Some(DisplayEvent::AudioInit {
                id: 7,
                channels: 2,
                ..
            })
        ));
        assert!(matches!(
            rx2.recv().await,
            Some(DisplayEvent::AudioEnabled {
                id: 7,
                enabled: true
            })
        ));
        l.write(7, vec![3]).await;
        assert!(matches!(
            rx1.recv().await,
            Some(DisplayEvent::AudioData { id: 7, .. })
        ));
        assert!(matches!(
            rx2.recv().await,
            Some(DisplayEvent::AudioData { id: 7, .. })
        ));

        // a viewer that left is dropped; Fini reaches the rest
        drop(rx1);
        l.fini(7).await;
        assert!(matches!(
            rx2.recv().await,
            Some(DisplayEvent::AudioFini { id: 7 })
        ));
        assert_eq!(lock(&state).subs.len(), 1);
        assert!(lock(&state).voices.is_empty());
    }
}
