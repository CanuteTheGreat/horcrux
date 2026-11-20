//! Platform-neutral agent loop: one reader thread turns the node's
//! messages into [`Platform`] calls, one writer thread owns the port's
//! write side; capture/encode/audio/clipboard threads send through the
//! same [`Out`] channel.
use crate::config::Config;
use horcrux_guest_proto::{
    self as proto, Clipboard, Hello, Input, Layout, Msg, StartAudio, StartVideo, VideoStatus,
};
use std::sync::mpsc;

/// Messages to the node. Bounded so a stalled port applies back-pressure
/// to the encoder (frames are dropped there, not queued without limit).
pub type Out = mpsc::SyncSender<Msg>;

/// What a guest OS implementation provides. Every method must return
/// quickly; long-running work (capture, encode) runs on its own threads
/// and sends through `out`.
pub trait Platform: Send {
    /// Capabilities found on this guest (probed, not assumed).
    fn hello(&self) -> Hello;
    /// Start or restart capture + encode with these parameters.
    fn start_video(&mut self, cfg: &StartVideo, out: Out) -> VideoStatus;
    fn stop_video(&mut self);
    /// Next frame must be a keyframe.
    fn keyframe(&mut self);
    fn set_clipboard(&mut self, c: &Clipboard);
    /// Start watching the guest clipboard; changes go to `out`.
    fn watch_clipboard(&mut self, out: Out, max_bytes: usize);
    /// Size/arrange the displays; returns the layout now in effect.
    fn set_layout(&mut self, l: &Layout) -> Option<Layout>;
    fn input(&mut self, i: &Input);
    fn start_audio(&mut self, a: &StartAudio, out: Out) -> Result<(), String>;
    fn stop_audio(&mut self);
}

/// Placeholder on operating systems without guest tools.
#[allow(dead_code)]
pub struct Unsupported;

impl Platform for Unsupported {
    fn hello(&self) -> Hello {
        Hello {
            v: proto::VERSION,
            os: std::env::consts::OS.into(),
            agent_version: env!("CARGO_PKG_VERSION").into(),
            ..Default::default()
        }
    }
    fn start_video(&mut self, _: &StartVideo, _: Out) -> VideoStatus {
        VideoStatus {
            error: Some("capture is not supported on this OS".into()),
            ..Default::default()
        }
    }
    fn stop_video(&mut self) {}
    fn keyframe(&mut self) {}
    fn set_clipboard(&mut self, _: &Clipboard) {}
    fn watch_clipboard(&mut self, _: Out, _: usize) {}
    fn set_layout(&mut self, _: &Layout) -> Option<Layout> {
        None
    }
    fn input(&mut self, _: &Input) {}
    fn start_audio(&mut self, _: &StartAudio, _: Out) -> Result<(), String> {
        Err("audio is not supported on this OS".into())
    }
    fn stop_audio(&mut self) {}
}

/// Open the transport: TCP (`--connect`) or the guest-tools port.
fn open(cfg: &Config) -> Result<crate::Port, String> {
    if let Some(addr) = &cfg.connect {
        let s = std::net::TcpStream::connect(addr).map_err(|e| format!("connect {addr}: {e}"))?;
        let _ = s.set_nodelay(true);
        let r = s.try_clone().map_err(|e| e.to_string())?;
        return Ok((Box::new(r), Box::new(s)));
    }
    crate::open_port(&cfg.port)
}

/// Serve one connection until the node closes it.
pub fn run(cfg: &Config, mut p: Box<dyn Platform>) -> Result<(), String> {
    let (mut r, mut w) = open(cfg)?;
    let (tx, rx) = mpsc::sync_channel::<Msg>(64);
    let writer = std::thread::spawn(move || {
        while let Ok(m) = rx.recv() {
            if proto::write_msg(&mut w, &m).is_err() {
                break;
            }
        }
    });
    let hello = p.hello();
    tx.send(Msg::Hello(hello)).map_err(|e| e.to_string())?;
    let mut clip_max = cfg.clipboard_max_bytes;
    let mut watching = false;
    let result = loop {
        let m = match proto::read_msg(&mut r) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break Ok(()),
            Err(e) => break Err(format!("port: {e}")),
        };
        match m {
            Msg::HostHello(h) => {
                if h.clipboard_max_bytes > 0 {
                    clip_max = clip_max.min(h.clipboard_max_bytes as usize);
                }
                if !watching {
                    watching = true;
                    p.watch_clipboard(tx.clone(), clip_max);
                }
            }
            Msg::StartVideo(s) => {
                let st = p.start_video(&s, tx.clone());
                let _ = tx.send(Msg::VideoStatus(st));
            }
            Msg::StopVideo => p.stop_video(),
            Msg::Keyframe => p.keyframe(),
            Msg::Clipboard(c) => {
                if c.text.len() <= clip_max {
                    p.set_clipboard(&c);
                }
            }
            Msg::SetLayout(l) => {
                if let Some(now) = p.set_layout(&l) {
                    let _ = tx.send(Msg::Layout(now));
                }
            }
            Msg::Input(i) => p.input(&i),
            Msg::StartAudio(a) => {
                if let Err(e) = p.start_audio(&a, tx.clone()) {
                    let _ = tx.send(Msg::Log(format!("audio: {e}")));
                }
            }
            Msg::StopAudio => p.stop_audio(),
            Msg::Ping(t) => {
                let _ = tx.send(Msg::Pong(t));
            }
            _ => {}
        }
    };
    p.stop_video();
    p.stop_audio();
    drop(tx);
    drop(p);
    let _ = writer.join();
    result
}

/// Monotonic microseconds since the agent started (capture timestamps).
pub fn now_us() -> u64 {
    use std::sync::OnceLock;
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    T0.get_or_init(std::time::Instant::now)
        .elapsed()
        .as_micros() as u64
}
