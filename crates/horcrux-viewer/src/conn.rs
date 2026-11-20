//! Connection to the node: API login, local console ticket, the local
//! socket (welcome + the frame ring's descriptor), input messages.
use horcrux_guest_proto::local::{self as lp, kind};
use horcrux_guest_proto::ring::Ring;
use std::io::Write;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

/// A shared mapping of a frame ring (unmapped on drop).
pub struct Mapped {
    base: *mut u8,
    len: usize,
    pub ring: Ring,
    _file: std::fs::File,
}
unsafe impl Send for Mapped {}
unsafe impl Sync for Mapped {}

impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: base/len from a successful mmap.
        unsafe {
            libc::munmap(self.base as *mut libc::c_void, self.len);
        }
    }
}

/// Map a frame ring file (or a descriptor received from the node).
pub fn map_ring(file: std::fs::File) -> Result<Mapped, String> {
    let len = file.metadata().map_err(|e| e.to_string())?.len() as usize;
    // SAFETY: shared mapping of a file we hold.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if p == libc::MAP_FAILED {
        return Err(format!("mmap: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: the mapping outlives the ring (both live in Mapped).
    let ring = match unsafe { Ring::attach(p as *mut u8, len) } {
        Ok(r) => r,
        Err(e) => {
            unsafe { libc::munmap(p, len) };
            return Err(e);
        }
    };
    Ok(Mapped {
        base: p as *mut u8,
        len,
        ring,
        _file: file,
    })
}

/// Create a frame ring file (tests and manual setups; the node does this
/// itself when it starts a VM).
pub fn ring_init(path: &str, mib: u32, slots: u32, fps: u32) -> Result<(), String> {
    let len = (mib as usize) << 20;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| format!("{path}: {e}"))?;
    f.set_len(len as u64).map_err(|e| e.to_string())?;
    // SAFETY: shared mapping of the sized file, unmapped below.
    unsafe {
        let p = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            f.as_raw_fd(),
            0,
        );
        if p == libc::MAP_FAILED {
            return Err(format!("mmap: {}", std::io::Error::last_os_error()));
        }
        let r = Ring::init(p as *mut u8, len, slots, fps).map(drop);
        libc::munmap(p, len);
        r
    }
}

/// Writer half of the local socket, shared by the window, the gamepad
/// threads and the ping timer.
#[derive(Clone)]
pub struct Input {
    sock: Arc<Mutex<UnixStream>>,
    pub allowed: bool,
}

impl Input {
    fn send(&self, k: u8, p: &[u8]) {
        if let Ok(mut s) = self.sock.lock() {
            let _ = s.write_all(&horcrux_guest_proto::frame(k, p));
        }
    }
    pub fn key(&self, code: &str, down: bool) {
        let m = serde_json::json!({"t": if down { "down" } else { "up" }, "code": code});
        self.send(kind::KEYS, m.to_string().as_bytes());
    }
    pub fn button(&self, b: u8, down: bool) {
        let m = serde_json::json!({"t": "btn", "b": b, "down": down});
        self.send(kind::KEYS, m.to_string().as_bytes());
    }
    pub fn wheel(&self, dx: i32, dy: i32) {
        let m = serde_json::json!({"t": "wheel", "dx": dx, "dy": dy});
        self.send(kind::KEYS, m.to_string().as_bytes());
    }
    /// kind 1 = absolute (guest pixels), 2 = relative.
    pub fn motion(&self, kind_: u8, seq: u32, x: i32, y: i32) {
        let mut b = vec![kind_];
        b.extend_from_slice(&seq.to_le_bytes());
        b.extend_from_slice(&x.to_le_bytes());
        b.extend_from_slice(&y.to_le_bytes());
        self.send(kind::MOTION, &b);
    }
    pub fn pad(&self, pkt: &[u8; 16]) {
        self.send(kind::PAD, pkt);
    }
}

pub struct Session {
    pub welcome: lp::Welcome,
    pub ring: Option<Mapped>,
    pub input: Input,
}

/// Receive one frame plus an optional descriptor (SCM_RIGHTS). The
/// descriptor rides on the frame's first bytes, so only the length is read
/// with recvmsg and the rest with plain reads (nothing of the next frame is
/// consumed).
fn recv_with_fd(sock: &UnixStream) -> Result<(u8, Vec<u8>, Option<RawFd>), String> {
    use nix::sys::socket::{recvmsg, ControlMessageOwned, MsgFlags};
    use std::io::Read;
    let mut len = [0u8; 4];
    let mut got = 0usize;
    let mut fd = None;
    while got < 4 {
        let (n, f) = {
            let mut iov = [std::io::IoSliceMut::new(&mut len[got..])];
            let mut space = nix::cmsg_space!([RawFd; 1]);
            let m = recvmsg::<()>(
                sock.as_raw_fd(),
                &mut iov,
                Some(&mut space),
                MsgFlags::empty(),
            )
            .map_err(|e| format!("local socket: {e}"))?;
            let f = m.cmsgs().ok().and_then(|mut c| {
                c.find_map(|c| match c {
                    ControlMessageOwned::ScmRights(v) => v.first().copied(),
                    _ => None,
                })
            });
            (m.bytes, f)
        };
        if f.is_some() {
            fd = f;
        }
        if n == 0 {
            return Err("node closed the local socket".into());
        }
        got += n;
    }
    let n = u32::from_le_bytes(len) as usize;
    if n == 0 || n > lp::MAX_LOCAL_FRAME {
        return Err("bad frame from the node".into());
    }
    let mut body = vec![0u8; n];
    let mut r = sock;
    r.read_exact(&mut body).map_err(|e| e.to_string())?;
    let k = body.remove(0);
    Ok((k, body, fd))
}

/// Redeem a local ticket on the node's socket.
pub fn open_local(socket: &str, ticket: &str) -> Result<Session, String> {
    let mut s = UnixStream::connect(socket).map_err(|e| format!("{socket}: {e}"))?;
    let hello = lp::Hello {
        v: 1,
        ticket: ticket.into(),
    };
    s.write_all(&horcrux_guest_proto::frame(
        kind::HELLO,
        &serde_json::to_vec(&hello).unwrap_or_default(),
    ))
    .map_err(|e| e.to_string())?;
    let (k, p, fd) = recv_with_fd(&s)?;
    if k == kind::ERROR {
        let v: serde_json::Value = serde_json::from_slice(&p).unwrap_or_default();
        return Err(v["message"].as_str().unwrap_or("refused").to_string());
    }
    if k != kind::WELCOME {
        return Err(format!("unexpected message {k:#x} from the node"));
    }
    let welcome: lp::Welcome = serde_json::from_slice(&p).map_err(|e| e.to_string())?;
    let ring = match fd {
        // SAFETY: descriptor received just now; we own it.
        Some(fd) => Some(map_ring(unsafe { std::fs::File::from_raw_fd(fd) })?),
        None => None,
    };
    Ok(Session {
        input: Input {
            sock: Arc::new(Mutex::new(s)),
            allowed: welcome.input,
        },
        welcome,
        ring,
    })
}

/// API login and a local ticket for `vm` (the viewer's own uid).
pub fn local_ticket(
    server: &str,
    user: &str,
    password: &str,
    vm: &str,
    profile: Option<&str>,
    insecure: bool,
) -> Result<(String, String), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        let c = reqwest::Client::builder()
            .danger_accept_invalid_certs(insecure)
            .build()
            .map_err(|e| e.to_string())?;
        let r = c
            .post(format!("{server}/api/auth/login"))
            .json(&serde_json::json!({"username": user, "password": password}))
            .send()
            .await
            .map_err(|e| format!("login: {e}"))?;
        if !r.status().is_success() {
            return Err(format!("login: HTTP {}", r.status()));
        }
        let j: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
        let tok = j["ticket"]
            .as_str()
            .or(j["token"].as_str())
            .ok_or("login answered without a token")?
            .to_string();
        // SAFETY: getuid never fails.
        let uid = unsafe { libc::getuid() };
        let mut body = serde_json::json!({"uid": uid});
        if let Some(p) = profile {
            body["profile"] = serde_json::Value::String(p.into());
        }
        let r = c
            .post(format!("{server}/api/console/{vm}/hxc/local-ticket"))
            .bearer_auth(&tok)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("local ticket: {e}"))?;
        let st = r.status();
        let j: serde_json::Value = r.json().await.unwrap_or_default();
        if !st.is_success() {
            return Err(format!(
                "local ticket: HTTP {st} {}",
                j["error"].as_str().or(j["message"].as_str()).unwrap_or("")
            ));
        }
        Ok((
            j["ticket"].as_str().unwrap_or_default().to_string(),
            j["socket"].as_str().unwrap_or_default().to_string(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_init_then_map() {
        let p = std::env::temp_dir().join(format!("hxv-ring-{}", std::process::id()));
        ring_init(p.to_str().unwrap(), 8, 3, 144).unwrap();
        let m = map_ring(
            std::fs::File::options()
                .read(true)
                .write(true)
                .open(&p)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(m.ring.want_fps(), 144);
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn input_messages_are_framed_like_the_data_channels() {
        let (a, b) = UnixStream::pair().unwrap();
        let i = Input {
            sock: Arc::new(Mutex::new(a)),
            allowed: true,
        };
        i.key("KeyA", true);
        i.motion(2, 7, -3, 4);
        let mut r = b;
        let (k, p) = lp::read_frame(&mut r).unwrap();
        assert_eq!(k, kind::KEYS);
        let v: serde_json::Value = serde_json::from_slice(&p).unwrap();
        assert_eq!(
            (v["t"].as_str(), v["code"].as_str()),
            (Some("down"), Some("KeyA"))
        );
        let (k, p) = lp::read_frame(&mut r).unwrap();
        assert_eq!(k, kind::MOTION);
        assert_eq!(p.len(), 13);
        assert_eq!(p[0], 2);
        assert_eq!(i32::from_le_bytes(p[5..9].try_into().unwrap()), -3);
    }
}
