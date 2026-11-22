//! Same-host native viewer endpoint (docs/design/horcrux-console.md
//! section 4.3, wire format in `horcrux_guest_proto::local`).
//!
//! The node listens on `[console] local_socket`. A viewer on this host
//! redeems a local console ticket (issued by the API to an authenticated
//! user with VM.Console, bound to the viewer's Unix uid, single use, short
//! lifetime) and receives the VM's frame ring as a file descriptor, so the
//! ring file itself stays root-only. Input comes back over the same socket
//! and goes to the guest exactly like WebRTC input: keyboard, buttons and
//! the absolute pointer through QEMU's D-Bus input, raw relative motion and
//! gamepads through the guest tools when they answer.
use super::guest;
use super::input_route::{InputRouter, Routed};
use super::qemu_dbus::{DbusDisplay, DisplayEvent};
use super::session::{self, parse_keys, parse_motion, parse_pad, InputEvent, SessionStats};
use super::{Broker, HxcTicket};
use crate::vm::display::DisplayRuntime;
use base64::Engine as _;
use horcrux_common::console::ConsoleSettings;
use horcrux_guest_proto as gproto;
use horcrux_guest_proto::local::{self as lp, kind};
use std::future::Future;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

/// VM id -> the disk path its runtime state is keyed by.
pub type Resolve = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<PathBuf, String>> + Send>> + Send + Sync,
>;

/// What happened, for the API's audit log.
#[derive(Debug, Clone)]
pub enum LocalEvent {
    Started {
        user: String,
        vm: String,
        session: String,
        details: String,
    },
    Ended {
        user: String,
        vm: String,
        session: String,
        details: String,
    },
    Refused {
        reason: String,
        uid: u32,
    },
}

pub type OnEvent = Arc<dyn Fn(LocalEvent) + Send + Sync>;

/// Bind the socket (replacing a stale one) and serve viewers until the
/// process ends. Mode 0666: every local user may connect, but only a valid
/// ticket for their own uid gets anything.
pub async fn serve(
    path: PathBuf,
    broker: &'static Broker,
    settings: impl Fn() -> ConsoleSettings + Send + Sync + 'static,
    resolve: Resolve,
    on_event: OnEvent,
) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let _ = tokio::fs::remove_file(&path).await;
    let l = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
    }
    let settings = Arc::new(settings);
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            let (resolve, on_event, settings) =
                (resolve.clone(), on_event.clone(), settings.clone());
            tokio::spawn(async move {
                if let Err((uid, e)) =
                    handle(s, broker, settings(), resolve, on_event.clone()).await
                {
                    on_event(LocalEvent::Refused { reason: e, uid });
                }
            });
        }
    });
    Ok(())
}

async fn read_frame(s: &mut UnixStream) -> std::io::Result<(u8, Vec<u8>)> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).await?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > lp::MAX_LOCAL_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad frame length",
        ));
    }
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf).await?;
    let k = buf.remove(0);
    Ok((k, buf))
}

async fn send_error(s: &mut UnixStream, msg: &str) {
    let body = serde_json::json!({ "message": msg }).to_string();
    let _ = s
        .write_all(&gproto::frame(kind::ERROR, body.as_bytes()))
        .await;
}

/// Send one frame with a file descriptor attached (SCM_RIGHTS).
async fn send_with_fd(s: &UnixStream, frame: &[u8], fd: Option<i32>) -> std::io::Result<()> {
    use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags};
    let mut off = 0;
    let mut first = true;
    while off < frame.len() {
        s.writable().await?;
        let r = s.try_io(tokio::io::Interest::WRITABLE, || {
            let iov = [std::io::IoSlice::new(&frame[off..])];
            let fds;
            let cmsg: Vec<ControlMessage> = match (first, fd) {
                (true, Some(f)) => {
                    fds = [f];
                    vec![ControlMessage::ScmRights(&fds)]
                }
                _ => vec![],
            };
            sendmsg::<()>(s.as_raw_fd(), &iov, &cmsg, MsgFlags::empty(), None)
                .map_err(std::io::Error::from)
        });
        match r {
            Ok(n) => {
                off += n;
                first = false;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

async fn handle(
    mut s: UnixStream,
    broker: &'static Broker,
    settings: ConsoleSettings,
    resolve: Resolve,
    on_event: OnEvent,
) -> Result<(), (u32, String)> {
    let uid = s
        .peer_cred()
        .map(|c| c.uid())
        .map_err(|e| (u32::MAX, e.to_string()))?;
    let fail = |e: String| (uid, e);
    let hello = match tokio::time::timeout(Duration::from_secs(10), read_frame(&mut s)).await {
        Ok(Ok((kind::HELLO, p))) => {
            serde_json::from_slice::<lp::Hello>(&p).map_err(|e| fail(e.to_string()))?
        }
        _ => return Err(fail("no hello".into())),
    };
    let t: HxcTicket = match broker.redeem_local(&hello.ticket, uid).await {
        Ok(t) => t,
        Err(e) => {
            send_error(&mut s, &e.to_string()).await;
            return Err(fail(e.to_string()));
        }
    };
    if broker.live_sessions_for(&t.vm_id).await >= settings.max_sessions_per_guest as usize {
        let m = "this VM has max_sessions_per_guest console sessions already";
        send_error(&mut s, m).await;
        return Err(fail(m.into()));
    }
    let disk = match resolve(t.vm_id.clone()).await {
        Ok(d) => d,
        Err(e) => {
            send_error(&mut s, &e).await;
            return Err(fail(e));
        }
    };
    let rt = match DisplayRuntime::load(&disk).await {
        Ok(rt) if rt.dbus_display => rt,
        Ok(_) => {
            let m = "This VM is not running with the D-Bus display; restart it";
            send_error(&mut s, m).await;
            return Err(fail(m.into()));
        }
        Err(e) => {
            send_error(&mut s, &e.to_string()).await;
            return Err(fail(e.to_string()));
        }
    };
    let session_id = uuid::Uuid::new_v4().simple().to_string();
    let (disp_tx, mut disp_rx) = mpsc::channel::<DisplayEvent>(64);
    let poll_mhz = settings.profile(t.profile).display_poll_mhz();
    let display =
        match DbusDisplay::connect(&rt.qmp_socket, &session_id, disp_tx, false, poll_mhz).await {
            Ok(d) => d,
            Err(e) => {
                send_error(&mut s, &e).await;
                return Err(fail(e));
            }
        };
    let ring = rt.frame_ring.as_ref().and_then(|p| open_ring(p).ok());
    // Guest tools for raw relative motion and gamepads (the port serves
    // one session at a time; without them those inputs are unavailable).
    let link = match &rt.guest_tools_socket {
        Some(sock) if settings.guest_tools && sock.exists() => guest::connect(
            sock,
            Duration::from_millis(settings.guest_tools_wait_ms as u64),
            settings.clipboard_max_bytes,
        )
        .await
        .ok()
        .flatten(),
        _ => None,
    };
    let caps = link
        .as_ref()
        .map(|l| l.hello.caps.clone())
        .unwrap_or_default();
    let welcome = lp::Welcome {
        v: 1,
        session: session_id.clone(),
        vm: t.vm_id.clone(),
        input: t.allow_input,
        gamepad: t.allow_input && caps.gamepad,
        relative: caps.relative_mouse,
        frame_ring_bytes: ring.as_ref().map(|(_, n)| *n).unwrap_or(0),
        absolute: display.absolute,
    };
    let body = serde_json::to_vec(&welcome).unwrap_or_default();
    let frame = gproto::frame(kind::WELCOME, &body);
    if let Err(e) = send_with_fd(&s, &frame, ring.as_ref().map(|(f, _)| f.as_raw_fd())).await {
        return Err(fail(e.to_string()));
    }
    drop(ring); // the viewer holds its own descriptor now

    let stats = SessionStats {
        connected: true,
        video_source: if welcome.frame_ring_bytes > 0 {
            "shm"
        } else {
            "none"
        }
        .into(),
        guest_tools: link.as_ref().map(|l| {
            format!(
                "{} {} (agent {})",
                l.hello.os, l.hello.os_version, l.hello.agent_version
            )
        }),
        ..Default::default()
    };
    let (handle, mut stop_rx, done_tx) = session::SessionHandle::new_local(
        session_id.clone(),
        t.vm_id.clone(),
        t.username.clone(),
        t.profile,
        stats,
    );
    let handle = broker.add_session(handle).await;
    let stats = handle.stats.clone();
    on_event(LocalEvent::Started {
        user: t.username.clone(),
        vm: t.vm_id.clone(),
        session: session_id.clone(),
        details: format!(
            "path=local uid={uid} input={} frame_ring={} guest_tools={}",
            t.allow_input,
            welcome.frame_ring_bytes > 0,
            link.is_some()
        ),
    });

    let (input_tx, input_rx) = mpsc::channel::<InputEvent>(256);
    tokio::spawn(session::input_task(display, input_rx, stats.clone()));
    let guest_tx = link.map(|l| {
        // drain the tools' messages; nothing else is asked of them here
        let mut rx = l.rx;
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        l.tx
    });
    // keys, buttons, wheel and absolute pointer: the guest tools' devices
    // while they offer them, QEMU's input otherwise (input_route.rs)
    let mut router = InputRouter::default();
    let tools_caps = (settings.input_via_guest_tools && guest_tx.is_some()).then_some(&caps);
    let (mut rd, mut wr) = s.into_split();
    // read_exact is not cancel-safe: frames are read in their own task
    let (in_tx, mut in_rx) = mpsc::channel::<(u8, Vec<u8>)>(256);
    let reader = tokio::spawn(async move {
        while let Ok(f) = read_frame_half(&mut rd).await {
            if in_tx.send(f).await.is_err() {
                break;
            }
        }
    });
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
    let writer = tokio::spawn(async move {
        while let Some(f) = out_rx.recv().await {
            if wr.write_all(&f).await.is_err() {
                break;
            }
        }
    });
    let reason = loop {
        tokio::select! {
            _ = stop_rx.changed() => break "stopped".to_string(),
            ev = disp_rx.recv() => {
                let Some(ev) = ev else { break "display closed".into() };
                let msg = match ev {
                    DisplayEvent::CursorDefine { width, height, hot_x, hot_y, data }
                        if width > 0 && height > 0 && width <= 256 && height <= 256 => Some(serde_json::json!({
                            "t": "shape", "w": width, "h": height, "hx": hot_x, "hy": hot_y,
                            "bgra": base64::engine::general_purpose::STANDARD.encode(&data)})),
                    DisplayEvent::MouseSet { x, y, on } => Some(serde_json::json!({"t": "pos", "x": x, "y": y, "on": on != 0})),
                    DisplayEvent::Scanout { width, height, .. } => {
                        if let Ok(mut st) = stats.lock() { st.guest_width = width; st.guest_height = height; }
                        let m = serde_json::json!({"t": "layout", "width": width, "height": height});
                        let _ = out_tx.try_send(gproto::frame(kind::STATUS, m.to_string().as_bytes()));
                        None
                    }
                    _ => None,
                };
                if let Some(m) = msg {
                    let _ = out_tx.try_send(gproto::frame(kind::CURSOR, m.to_string().as_bytes()));
                }
            }
            f = in_rx.recv() => {
                let Some((k, p)) = f else { break "viewer closed".into() };
                match k {
                    kind::CTL => {
                        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&p) {
                            if v.get("t").and_then(|t| t.as_str()) == Some("ping") {
                                let m = serde_json::json!({"t": "pong", "ts": v.get("ts")});
                                let _ = out_tx.try_send(gproto::frame(kind::STATUS, m.to_string().as_bytes()));
                            }
                        }
                        continue;
                    }
                    _ if !t.allow_input => {
                        if let Ok(mut st) = stats.lock() { st.input_denied += 1; }
                        continue;
                    }
                    kind::KEYS => {
                        if let Some(ev) = std::str::from_utf8(&p).ok().and_then(parse_keys) {
                            if let Ok(mut st) = stats.lock() {
                                match ev { InputEvent::Key { .. } => st.keys_in += 1, _ => st.pointer_in += 1 }
                            }
                            route(&mut router, ev, tools_caps, &guest_tx, &input_tx, &stats);
                        }
                    }
                    kind::MOTION => {
                        if let Some(ev) = parse_motion(&p) {
                            if let Ok(mut st) = stats.lock() { st.pointer_in += 1; }
                            match (&ev, &guest_tx) {
                                (InputEvent::Rel { dx, dy }, Some(g)) if caps.relative_mouse => {
                                    let _ = g.try_send(gproto::Msg::Input(gproto::Input::Rel { dx: *dx, dy: *dy }));
                                }
                                _ => route(&mut router, ev, tools_caps, &guest_tx, &input_tx, &stats),
                            }
                        }
                    }
                    kind::PAD => {
                        if let (Some(pad), Some(g)) = (parse_pad(&p), &guest_tx) {
                            if g.try_send(gproto::Msg::Input(gproto::Input::Gamepad(pad))).is_ok() {
                                if let Ok(mut st) = stats.lock() { st.gamepad_in += 1; }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    };
    writer.abort();
    reader.abort();
    let s2 = stats.lock().map(|x| x.clone()).unwrap_or_default();
    if let Ok(mut st) = stats.lock() {
        st.connected = false;
        st.end_reason = Some(reason.clone());
    }
    let _ = done_tx.send(true);
    on_event(LocalEvent::Ended {
        user: t.username,
        vm: t.vm_id,
        session: session_id,
        details: format!(
            "reason={reason} keys={} pointer={} gamepad={} input_denied={}",
            s2.keys_in, s2.pointer_in, s2.gamepad_in, s2.input_denied
        ),
    });
    Ok(())
}

async fn read_frame_half(
    s: &mut tokio::net::unix::OwnedReadHalf,
) -> std::io::Result<(u8, Vec<u8>)> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).await?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > lp::MAX_LOCAL_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad frame length",
        ));
    }
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf).await?;
    let k = buf.remove(0);
    Ok((k, buf))
}

/// Open the ring file read-write (the viewer stamps its clock into it).
fn open_ring(p: &Path) -> std::io::Result<(std::fs::File, u64)> {
    let f = std::fs::OpenOptions::new().read(true).write(true).open(p)?;
    let n = f.metadata()?.len();
    Ok((f, n))
}

/// Hand one event to the guest tools or QEMU (see `input_route.rs`).
fn route(
    router: &mut InputRouter,
    ev: InputEvent,
    tools: Option<&gproto::Caps>,
    guest_tx: &Option<mpsc::Sender<gproto::Msg>>,
    input_tx: &mpsc::Sender<InputEvent>,
    stats: &std::sync::Mutex<SessionStats>,
) {
    let mut is_key = false;
    let routed = router.route(ev, tools, |i| {
        is_key = matches!(i, gproto::Input::Key { .. } | gproto::Input::Button { .. });
        guest_tx
            .as_ref()
            .map(|g| g.try_send(gproto::Msg::Input(i)).is_ok())
            .unwrap_or(false)
    });
    match routed {
        Routed::Tools => {
            if let Ok(mut st) = stats.lock() {
                if is_key {
                    st.keys_via_tools += 1
                } else {
                    st.pointer_via_tools += 1
                }
            }
        }
        Routed::Qemu(ev) => {
            let _ = input_tx.try_send(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fd_passing_over_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let ring =
            crate::vm::display::create_frame_ring(dir.path().to_str().unwrap(), "v", 8, 3, 60)
                .unwrap();
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        a.set_nonblocking(true).unwrap();
        let a = UnixStream::from_std(a).unwrap();
        let (f, n) = open_ring(&ring).unwrap();
        let body = br#"{"v":1}"#;
        send_with_fd(&a, &gproto::frame(kind::WELCOME, body), Some(f.as_raw_fd()))
            .await
            .unwrap();
        drop(f);
        std::fs::remove_file(&ring).unwrap(); // the descriptor keeps it alive
        let got = tokio::task::spawn_blocking(move || {
            use nix::sys::socket::{recvmsg, ControlMessageOwned, MsgFlags};
            let mut buf = vec![0u8; 64];
            let mut iov = [std::io::IoSliceMut::new(&mut buf)];
            let mut space = nix::cmsg_space!([std::os::unix::io::RawFd; 1]);
            let m = recvmsg::<()>(b.as_raw_fd(), &mut iov, Some(&mut space), MsgFlags::empty())
                .unwrap();
            let fd = m
                .cmsgs()
                .unwrap()
                .find_map(|c| match c {
                    ControlMessageOwned::ScmRights(v) => v.first().copied(),
                    _ => None,
                })
                .unwrap();
            let n = m.bytes;
            (fd, n)
        })
        .await
        .unwrap();
        assert_eq!(got.1, 5 + body.len());
        use std::os::unix::io::FromRawFd;
        // SAFETY: fd received just now, owned here.
        let file = unsafe { std::fs::File::from_raw_fd(got.0) };
        assert_eq!(file.metadata().unwrap().len(), n);
    }
}
