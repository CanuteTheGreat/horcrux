//! The active graphical session of the guest: found by scanning processes
//! for DISPLAY / WAYLAND_DISPLAY (the agent runs as a system service), so
//! clipboard and layout commands run as that user in that session.
use horcrux_guest_proto::{Layout, Monitor};
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct Session {
    pub uid: u32,
    pub gid: u32,
    pub display: Option<String>,
    pub wayland: Option<String>,
    pub env: Vec<(String, String)>,
}

pub fn which(prog: &str) -> bool {
    if prog.contains('/') {
        return std::path::Path::new(prog).exists();
    }
    std::env::var("PATH")
        .unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into())
        .split(':')
        .any(|d| std::path::Path::new(d).join(prog).exists())
}

/// The newest process that carries a display in its environment. Ours
/// (the agent's own environment) wins when it has one.
pub fn find() -> Option<Session> {
    let keep = [
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
    ];
    if std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok() {
        let env: Vec<(String, String)> = keep
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect();
        return Some(Session {
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            display: std::env::var("DISPLAY").ok(),
            wayland: std::env::var("WAYLAND_DISPLAY").ok(),
            env,
        });
    }
    let mut best: Option<(i64, Session)> = None;
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        let name = e.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(raw) = std::fs::read(e.path().join("environ")) else {
            continue;
        };
        let mut env = Vec::new();
        for kv in raw.split(|b| *b == 0) {
            let s = String::from_utf8_lossy(kv);
            if let Some((k, v)) = s.split_once('=') {
                if keep.contains(&k) {
                    env.push((k.to_string(), v.to_string()));
                }
            }
        }
        let get = |k: &str| env.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        let (display, wayland) = (get("DISPLAY"), get("WAYLAND_DISPLAY"));
        if display.is_none() && wayland.is_none() {
            continue;
        }
        let Ok(md) = std::fs::metadata(e.path()) else {
            continue;
        };
        let started = md.ctime();
        if best.as_ref().map(|(t, _)| started > *t).unwrap_or(true) {
            best = Some((
                started,
                Session {
                    uid: md.uid(),
                    gid: md.gid(),
                    display,
                    wayland,
                    env,
                },
            ));
        }
    }
    best.map(|(_, s)| s)
}

impl Session {
    /// Run as the session's user with its display environment.
    pub fn apply(&self, c: &mut Command) {
        for (k, v) in &self.env {
            c.env(k, v);
        }
        if unsafe { libc::geteuid() } == 0 && self.uid != 0 {
            c.uid(self.uid).gid(self.gid);
        }
    }

    fn run(&self, prog: &str, args: &[&str], input: Option<&[u8]>) -> Result<String, String> {
        let mut c = Command::new(prog);
        c.args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(if input.is_some() {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stderr(Stdio::piped());
        self.apply(&mut c);
        let mut child = c.spawn().map_err(|e| format!("{prog}: {e}"))?;
        if let Some(data) = input {
            if let Some(mut si) = child.stdin.take() {
                si.write_all(data).map_err(|e| e.to_string())?;
            }
            // xclip/wl-copy stay in the background to own the selection
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(String::new());
        }
        let o = child.wait_with_output().map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(format!(
                "{prog}: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    }

    pub fn get_clipboard(&self) -> Result<String, String> {
        if self.wayland.is_some() {
            self.run("wl-paste", &["--no-newline", "--type", "text/plain"], None)
        } else {
            self.run("xclip", &["-selection", "clipboard", "-o"], None)
        }
    }

    pub fn set_clipboard(&self, text: &str) -> Result<(), String> {
        if self.wayland.is_some() {
            self.run("wl-copy", &["--type", "text/plain"], Some(text.as_bytes()))
                .map(|_| ())
        } else {
            self.run(
                "xclip",
                &["-selection", "clipboard", "-i"],
                Some(text.as_bytes()),
            )
            .map(|_| ())
        }
    }

    pub fn layout(&self) -> Result<Layout, String> {
        Ok(parse_xrandr(&self.run("xrandr", &["--query"], None)?).0)
    }

    /// Give each connected output the requested size (adds the mode with
    /// a CVT timing when the output does not list it, e.g. virtio-gpu and
    /// QXL accept any mode).
    pub fn apply_layout(&self, l: &Layout) -> Result<(), String> {
        let text = self.run("xrandr", &["--query"], None)?;
        let (_, outputs) = parse_xrandr(&text);
        for (i, m) in l.monitors.iter().enumerate() {
            let Some(out) = outputs.get(i) else { break };
            let hz = if m.refresh_hz == 0 { 60 } else { m.refresh_hz };
            let name = format!("{}x{}_{}", m.width, m.height, hz);
            let have = self.run(
                "xrandr",
                &[
                    "--output",
                    out,
                    "--mode",
                    &format!("{}x{}", m.width, m.height),
                ],
                None,
            );
            if have.is_ok() {
                continue;
            }
            let ml = cvt_modeline(m.width, m.height, hz);
            let mut args = vec!["--newmode".to_string(), name.clone()];
            args.extend(ml);
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            let _ = self.run("xrandr", &a, None);
            let _ = self.run("xrandr", &["--addmode", out, &name], None);
            let pos = format!("{}x{}", m.x.max(0), m.y.max(0));
            self.run(
                "xrandr",
                &["--output", out, "--mode", &name, "--pos", &pos],
                None,
            )?;
        }
        Ok(())
    }
}

/// Monitors in use and the names of connected outputs, from `xrandr --query`.
pub fn parse_xrandr(text: &str) -> (Layout, Vec<String>) {
    let mut l = Layout::default();
    let mut outs = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 || parts[1] != "connected" {
            continue;
        }
        outs.push(parts[0].to_string());
        let primary = parts.contains(&"primary");
        // geometry WxH+X+Y
        let geo = parts.iter().find(|p| p.contains('x') && p.contains('+'));
        let Some(geo) = geo else { continue };
        let mut it = geo.split(['x', '+']);
        let w = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let h = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let x = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let y = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut hz = 0;
        while let Some(next) = lines.peek() {
            if !next.starts_with(' ') {
                break;
            }
            let n = lines.next().unwrap_or_default();
            if let Some(cur) = n.split_whitespace().find(|t| t.contains('*')) {
                hz = cur
                    .trim_end_matches(['*', '+'])
                    .parse::<f64>()
                    .map(|f| f.round() as u32)
                    .unwrap_or(0);
            }
        }
        l.monitors.push(Monitor {
            x,
            y,
            width: w,
            height: h,
            refresh_hz: hz,
            primary,
        });
    }
    (l, outs)
}

/// VESA CVT reduced-blanking modeline numbers (clock MHz, h timings, v
/// timings, flags) for xrandr --newmode.
pub fn cvt_modeline(w: u32, h: u32, hz: u32) -> Vec<String> {
    // CVT-RB: fixed 160 px horizontal blanking, >= 460 us vertical blanking
    let hblank = 160u32;
    let htotal = w + hblank;
    let vsync = 6u32;
    let min_vblank_us = 460.0;
    let line_us = (1_000_000.0 / hz as f64 - min_vblank_us) / h as f64;
    let vblank = ((min_vblank_us / line_us).ceil() as u32).max(vsync + 3 + 6);
    let vtotal = h + vblank;
    let clock = (htotal as f64 * vtotal as f64 * hz as f64 / 1_000_000.0 / 0.25).floor() * 0.25;
    let v = [
        format!("{clock:.2}"),
        w.to_string(),
        (w + 48).to_string(),
        (w + 80).to_string(),
        htotal.to_string(),
        h.to_string(),
        (h + 3).to_string(),
        (h + 3 + vsync).to_string(),
        vtotal.to_string(),
        "+hsync".into(),
        "-vsync".into(),
    ];
    v.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const XR: &str = "Screen 0: minimum 320 x 200, current 2560 x 1440, maximum 8192 x 8192
Virtual-1 connected primary 2560x1440+0+0 (normal left inverted right x axis y axis) 0mm x 0mm
   1024x768      60.00 +
   2560x1440    239.97*
Virtual-2 connected 1920x1080+2560+0 (normal) 0mm x 0mm
   1920x1080     60.00*+
Virtual-3 disconnected (normal left inverted right x axis y axis)
";

    #[test]
    fn xrandr_layout() {
        let (l, outs) = parse_xrandr(XR);
        assert_eq!(outs, vec!["Virtual-1", "Virtual-2"]);
        assert_eq!(l.monitors.len(), 2);
        assert_eq!(
            l.monitors[0],
            Monitor {
                x: 0,
                y: 0,
                width: 2560,
                height: 1440,
                refresh_hz: 240,
                primary: true
            }
        );
        assert_eq!(l.monitors[1].x, 2560);
        assert_eq!(l.monitors[1].refresh_hz, 60);
    }

    #[test]
    fn cvt_rb_is_sane() {
        let m = cvt_modeline(1920, 1080, 60);
        // CVT-RB 1920x1080@60 is 138.50 MHz, htotal 2080, vtotal 1111
        assert_eq!(m[0], "138.50");
        assert_eq!(m[4], "2080");
        assert_eq!(m[8], "1111");
        let hi = cvt_modeline(2560, 1440, 240);
        let clock: f64 = hi[0].parse().unwrap();
        assert!(clock > 900.0 && clock < 1100.0, "{clock}");
    }
}
