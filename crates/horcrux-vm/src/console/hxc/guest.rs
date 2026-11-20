//! The node's end of the Horcrux guest tools port (virtio-serial socket,
//! wire format in `horcrux-guest-proto`).
//!
//! A session connects when it starts, sends `HostHello` and waits up to
//! `guest_tools_wait_ms` for the tools' `Hello`. Without an answer the
//! session uses the node's own capture (QEMU D-Bus display + host
//! encode); nothing depends on the tools being installed.
use horcrux_guest_proto::{Decoder, Hello, HostHello, Msg, VERSION};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

pub struct GuestLink {
    pub hello: Hello,
    /// Messages to the guest tools.
    pub tx: mpsc::Sender<Msg>,
    /// Messages from the guest tools (after Hello).
    pub rx: mpsc::Receiver<Msg>,
}

impl GuestLink {
    pub fn can_capture(&self) -> bool {
        self.hello.caps.codecs.iter().any(|c| c == "h264") && !self.hello.caps.capture.is_empty()
    }
}

/// Connect and handshake. `Ok(None)`: the port exists but no guest tools
/// answered in time (or another session holds the port).
pub async fn connect(
    socket: &Path,
    wait: Duration,
    clipboard_max_bytes: u32,
) -> Result<Option<GuestLink>, String> {
    let stream = match UnixStream::connect(socket).await {
        Ok(s) => s,
        Err(e) => return Err(format!("{}: {e}", socket.display())),
    };
    let (mut rd, mut wr) = stream.into_split();
    let (to_guest, mut from_node) = mpsc::channel::<Msg>(256);
    let (to_node, mut from_guest) = mpsc::channel::<Msg>(256);
    tokio::spawn(async move {
        while let Some(m) = from_node.recv().await {
            let (k, p) = m.encode();
            if wr
                .write_all(&horcrux_guest_proto::frame(k, &p))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    tokio::spawn(async move {
        let mut dec = Decoder::default();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = match rd.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let Ok(msgs) = dec.push(&buf[..n]) else {
                tracing::warn!("console guest tools: malformed frame, closing the port");
                break;
            };
            for m in msgs {
                if to_node.send(m).await.is_err() {
                    return;
                }
            }
        }
    });
    let hh = Msg::HostHello(HostHello {
        v: VERSION,
        clipboard_max_bytes,
    });
    to_guest.send(hh).await.map_err(|e| e.to_string())?;
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, from_guest.recv()).await {
            Ok(Some(Msg::Hello(hello))) => {
                // Drain a duplicate Hello (one sent when the tools opened
                // the port, one in answer to HostHello) without blocking.
                return Ok(Some(GuestLink {
                    hello,
                    tx: to_guest,
                    rx: from_guest,
                }));
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => return Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_guest_proto::{read_msg, write_msg, Caps};

    #[tokio::test]
    async fn handshake_and_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("g.sock");
        let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        // a fake guest: answers HostHello with Hello, then echoes a Pong
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let m = read_msg(&mut s).unwrap();
            assert!(matches!(
                m,
                Msg::HostHello(HostHello {
                    clipboard_max_bytes: 77,
                    ..
                })
            ));
            let hello = Hello {
                v: 1,
                os: "linux".into(),
                caps: Caps {
                    codecs: vec!["h264".into()],
                    capture: vec!["kmsgrab".into()],
                    ..Default::default()
                },
                ..Default::default()
            };
            write_msg(&mut s, &Msg::Hello(hello)).unwrap();
            let m = read_msg(&mut s).unwrap();
            assert_eq!(m, Msg::Ping(5));
            write_msg(&mut s, &Msg::Pong(5)).unwrap();
            // keep the port open but silent, like a guest without tools
            let l2 = l;
            let (_s2, _) = l2.accept().unwrap();
            std::thread::sleep(Duration::from_millis(600));
        });
        let mut g = connect(&sock, Duration::from_secs(2), 77)
            .await
            .unwrap()
            .expect("tools answered");
        assert!(g.can_capture());
        g.tx.send(Msg::Ping(5)).await.unwrap();
        assert_eq!(g.rx.recv().await, Some(Msg::Pong(5)));
        drop(g);
        let none = connect(&sock, Duration::from_millis(300), 1).await.unwrap();
        assert!(none.is_none(), "no Hello -> no tools");
        t.join().unwrap();
        assert!(
            connect(&dir.path().join("missing"), Duration::from_millis(10), 1)
                .await
                .is_err()
        );
    }
}
