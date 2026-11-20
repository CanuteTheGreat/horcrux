//! `horcrux-viewer`: the Horcrux Console native viewer (docs/design/
//! horcrux-console.md sections 4.2 and 4.3).
//!
//! On the node that runs the VM it shows the guest's raw frames from the
//! VM's frame ring (no encode, no decode) and sends keyboard, mouse (raw
//! relative motion under pointer lock) and gamepads back through the
//! node's local socket. Signs in with a Horcrux account; the node checks
//! VM.Console / VM.Console.Input like for any console.
//!
//! ```text
//! horcrux-viewer --vm ID [--server URL] [--user NAME] [--profile P]
//!                [--fullscreen] [--grab] [--fps N]
//! horcrux-viewer measure (--ring FILE | --vm ID ...) [--secs S] [--json FILE]
//! horcrux-viewer ring-init FILE MIB SLOTS FPS
//! ```
//!
//! Every value is a setting: flags override `~/.config/horcrux/viewer.conf`
//! (`key = value` lines), which overrides the defaults. The password comes
//! from `HORCRUX_PASSWORD` or the `password_file` setting, never a flag.
mod conn;
mod measure;
mod pad;
mod view;

use std::collections::HashMap;

#[derive(Debug, Clone)]
struct Opts {
    server: String,
    user: String,
    vm: String,
    profile: Option<String>,
    insecure: bool,
    password_file: String,
    view: view::ViewOpts,
    ring: Option<String>,
    secs: f64,
    json: Option<String>,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            server: "https://localhost:8006".into(),
            user: std::env::var("USER").unwrap_or_else(|_| "admin".into()),
            vm: String::new(),
            profile: None,
            // the node's certificate is self-issued unless replaced; the
            // connection never leaves this host for the local path
            insecure: true,
            password_file: String::new(),
            view: view::ViewOpts {
                title: "Horcrux Console".into(),
                fullscreen: false,
                grab: false,
                grab_key: "KeyG".into(),
                fullscreen_key: "KeyF".into(),
                fps: 0,
                poll_us: 250,
                input_dir: "/dev/input".into(),
            },
            ring: None,
            secs: 10.0,
            json: None,
        }
    }
}

fn config_file() -> String {
    let base = std::env::var("XDG_CONFIG_HOME")
        .unwrap_or_else(|_| format!("{}/.config", std::env::var("HOME").unwrap_or_default()));
    format!("{base}/horcrux/viewer.conf")
}

fn set(o: &mut Opts, k: &str, v: &str) -> Result<(), String> {
    let b = |v: &str| matches!(v, "true" | "1" | "yes" | "on");
    let num = |v: &str| {
        v.parse::<u64>()
            .map_err(|_| format!("{k}: not a number: {v}"))
    };
    match k {
        "server" => o.server = v.trim_end_matches('/').into(),
        "user" => o.user = v.into(),
        "vm" => o.vm = v.into(),
        "profile" => o.profile = Some(v.into()),
        "insecure" => o.insecure = b(v),
        "password_file" => o.password_file = v.into(),
        "fullscreen" => o.view.fullscreen = b(v),
        "grab" => o.view.grab = b(v),
        "grab_key" => o.view.grab_key = v.into(),
        "fullscreen_key" => o.view.fullscreen_key = v.into(),
        "fps" => o.view.fps = num(v)?.min(1000) as u32,
        "poll_us" => o.view.poll_us = num(v)?.clamp(50, 100_000),
        "input_dir" => o.view.input_dir = v.into(),
        "title" => o.view.title = v.into(),
        "ring" => o.ring = Some(v.into()),
        "secs" => o.secs = v.parse().map_err(|_| format!("secs: {v}"))?,
        "json" => o.json = Some(v.into()),
        _ => return Err(format!("unknown setting {k}")),
    }
    Ok(())
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    if let Ok(text) = std::fs::read_to_string(config_file()) {
        let kv: HashMap<String, String> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| {
                (
                    k.trim().replace('-', "_"),
                    v.trim().trim_matches('"').to_string(),
                )
            })
            .collect();
        for (k, v) in kv {
            set(&mut o, &k, &v)?;
        }
    }
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let k = a
            .strip_prefix("--")
            .ok_or(format!("unexpected argument {a}"))?;
        let (k, v) = match k.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None if matches!(k, "fullscreen" | "grab" | "insecure") => {
                (k.to_string(), "true".into())
            }
            None => {
                i += 1;
                (
                    k.to_string(),
                    args.get(i).ok_or(format!("--{k} needs a value"))?.clone(),
                )
            }
        };
        set(&mut o, &k.replace('-', "_"), &v)?;
        i += 1;
    }
    Ok(o)
}

fn password(o: &Opts) -> Result<String, String> {
    if let Ok(p) = std::env::var("HORCRUX_PASSWORD") {
        return Ok(p);
    }
    if !o.password_file.is_empty() {
        return std::fs::read_to_string(&o.password_file)
            .map(|s| s.trim_end_matches(['\r', '\n']).to_string())
            .map_err(|e| format!("{}: {e}", o.password_file));
    }
    Err("set HORCRUX_PASSWORD or password_file".into())
}

fn connect(o: &Opts) -> Result<conn::Session, String> {
    if o.vm.is_empty() {
        return Err("--vm is required".into());
    }
    let (ticket, socket) = conn::local_ticket(
        &o.server,
        &o.user,
        &password(o)?,
        &o.vm,
        o.profile.as_deref(),
        o.insecure,
    )?;
    conn::open_local(&socket, &ticket)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("ring-init") => {
            let a = |i: usize| args.get(i).cloned().unwrap_or_default();
            let n = |i: usize| a(i).parse::<u32>().unwrap_or(0);
            conn::ring_init(&a(1), n(2), n(3), n(4))
        }
        Some("measure") => parse(&args[1..]).and_then(|o| cmd_measure(&o)),
        Some("-h") | Some("--help") => Err(HELP.into()),
        _ => parse(&args).and_then(|o| cmd_view(&o)),
    };
    if let Err(e) = r {
        eprintln!("horcrux-viewer: {e}");
        std::process::exit(1);
    }
}

fn cmd_view(o: &Opts) -> Result<(), String> {
    let s = connect(o)?;
    let Some(ring) = s.ring else {
        return Err(
            "this VM has no frame ring on this node (turned off, or the VM started before it was \
             enabled): open the console in the Horcrux UI for the stream"
                .into(),
        );
    };
    let mut v = o.view.clone();
    v.title = format!("{} - {}", v.title, s.welcome.vm);
    if o.profile.as_deref() == Some("gaming") && !v.grab {
        v.grab = true;
    }
    view::run(v, ring, s.input, s.welcome.gamepad)
}

fn cmd_measure(o: &Opts) -> Result<(), String> {
    let (ring, _session) = match &o.ring {
        Some(p) => {
            let f = std::fs::File::options()
                .read(true)
                .write(true)
                .open(p)
                .map_err(|e| format!("{p}: {e}"))?;
            (conn::map_ring(f)?, None)
        }
        None => {
            let mut s = connect(o)?;
            let r = s.ring.take().ok_or("the node handed over no frame ring")?;
            eprintln!(
                "welcome: {}",
                serde_json::to_string(&s.welcome).unwrap_or_default()
            );
            (r, Some(s))
        }
    };
    let mut out: Box<dyn std::io::Write> = match &o.json {
        Some(p) => Box::new(std::fs::File::create(p).map_err(|e| format!("{p}: {e}"))?),
        None => Box::new(std::io::sink()),
    };
    let s = measure::run(&ring, o.secs, o.view.poll_us, o.view.fps, &mut *out);
    let (c2p, c2p95) =
        measure::med_p95(s.iter().map(|x| x.capture_to_publish_us / 1000.0).collect());
    let (p2s, p2s95) = measure::med_p95(
        s.iter()
            .filter(|x| x.publish_to_seen_us > 0.0)
            .map(|x| x.publish_to_seen_us / 1000.0)
            .collect(),
    );
    let (cp, cp95) = measure::med_p95(s.iter().map(|x| x.copy_us / 1000.0).collect());
    let total: Vec<f64> = s
        .iter()
        .filter(|x| x.publish_to_seen_us > 0.0)
        .map(|x| (x.capture_to_publish_us + x.publish_to_seen_us + x.copy_us) / 1000.0)
        .collect();
    let (t, t95) = measure::med_p95(total);
    let fps = if s.len() > 1 {
        (s.len() - 1) as f64 / ((s.last().unwrap().seen_ns - s[0].seen_ns) as f64 / 1e9)
    } else {
        0.0
    };
    println!(
        "{}",
        serde_json::json!({
            "frames": s.len(), "fps": fps,
            "size": s.last().map(|x| format!("{}x{}", x.width, x.height)),
            "capture_to_publish_ms": {"median": c2p, "p95": c2p95},
            "publish_to_seen_ms": {"median": p2s, "p95": p2s95},
            "copy_ms": {"median": cp, "p95": cp95},
            "frame_to_viewer_ms": {"median": t, "p95": t95},
            "writer": ring.ring.writer_info(),
        })
    );
    Ok(())
}

const HELP: &str = "usage: horcrux-viewer --vm ID [--server URL] [--user NAME] [--profile desktop|gaming|console] \
[--fullscreen] [--grab] [--fps N]\n       horcrux-viewer measure (--ring FILE | --vm ID ...) [--secs S] [--json FILE]\n       \
horcrux-viewer ring-init FILE MIB SLOTS FPS\nPassword: HORCRUX_PASSWORD or password_file in ~/.config/horcrux/viewer.conf";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_override_defaults() {
        let o = parse(&[
            "--vm".into(),
            "v1".into(),
            "--fps=240".into(),
            "--grab".into(),
        ])
        .unwrap();
        assert_eq!(o.vm, "v1");
        assert_eq!(o.view.fps, 240);
        assert!(o.view.grab);
        assert_eq!(o.server, "https://localhost:8006");
        assert!(parse(&["--nope=1".into()]).is_err());
        assert!(parse(&["stray".into()]).is_err());
    }
}
