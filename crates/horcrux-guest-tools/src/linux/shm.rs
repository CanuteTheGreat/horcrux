//! Same-host raw frames (docs/design/horcrux-console.md section 4.2): find
//! the Horcrux frame ring on the ivshmem PCI device (BAR 2 through sysfs,
//! no driver needed), and while a viewer on the host watches, capture the
//! display with FFmpeg as raw BGRX and write every frame straight into a
//! ring slot. No encoder runs.
use super::video::{self, Capture};
use crate::config::Config;
use horcrux_guest_proto::ring::{self, FrameInfo, Ring};
use horcrux_guest_proto::StartVideo;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A mapped BAR (unmapped on drop).
pub struct Mapping {
    base: *mut u8,
    len: usize,
}
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: base/len came from a successful mmap.
        unsafe {
            libc::munmap(self.base as *mut libc::c_void, self.len);
        }
    }
}

fn read_hex(p: &Path) -> Option<u16> {
    let s = std::fs::read_to_string(p).ok()?;
    u16::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
}

/// ivshmem devices in the PCI sysfs directory.
pub fn devices(pci_dir: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(pci_dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.retain(|d| {
        read_hex(&d.join("vendor")) == Some(ring::IVSHMEM_VENDOR)
            && read_hex(&d.join("device")) == Some(ring::IVSHMEM_DEVICE)
    });
    v.sort();
    v
}

/// Map BAR 2 of an ivshmem device and attach to the ring in it.
pub fn open_device(dev: &Path, write_combine: bool) -> Result<(Mapping, Ring), String> {
    // Memory decoding off means BAR accesses go nowhere.
    let _ = std::fs::write(dev.join("enable"), "1");
    // ivshmem's BAR 2 is prefetchable, so the kernel also offers a
    // write-combined mapping of it.
    let wc = dev.join("resource2_wc");
    let res = if write_combine && wc.exists() {
        wc
    } else {
        dev.join("resource2")
    };
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&res)
        .map_err(|e| format!("{}: {e}", res.display()))?;
    let len = f.metadata().map_err(|e| e.to_string())?.len() as usize;
    open_fd(&f, len).map_err(|e| format!("{}: {e}", res.display()))
}

/// Map `len` bytes of a file shared and attach to the ring in it.
pub fn open_fd(f: &std::fs::File, len: usize) -> Result<(Mapping, Ring), String> {
    use std::os::unix::io::AsRawFd;
    if len < ring::HEADER_SIZE {
        return Err(format!("{len} bytes is no frame ring"));
    }
    // SAFETY: plain shared mapping of a file we hold open.
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
        return Err(format!("mmap: {}", std::io::Error::last_os_error()));
    }
    let m = Mapping {
        base: p as *mut u8,
        len,
    };
    // SAFETY: the mapping lives as long as the returned pair is kept
    // together (callers keep both).
    let r = unsafe { Ring::attach(m.base, m.len)? };
    Ok((m, r))
}

/// The first ivshmem device that carries a Horcrux frame ring.
pub fn find(cfg: &Config) -> Option<(Mapping, Ring)> {
    for d in devices(&cfg.shm_pci_dir) {
        match open_device(&d, cfg.shm_write_combine) {
            Ok(x) => return Some(x),
            Err(e) => eprintln!("frame ring: {e}"),
        }
    }
    None
}

/// Is the frame ring there (capability report)?
pub fn available(cfg: &Config) -> bool {
    cfg.shm_frames && find(cfg).is_some()
}

fn now_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// FFmpeg arguments for raw BGRX capture to stdout.
pub fn raw_args(cfg: &Config, capture: Capture, fps: u32, display: Option<&str>) -> Vec<String> {
    let fps = fps.clamp(1, 360).to_string();
    let mut a: Vec<String> = [
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-fflags",
        "nobuffer",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut push = |v: &[&str]| a.extend(v.iter().map(|s| s.to_string()));
    match capture {
        Capture::Kms => push(&[
            "-device",
            &cfg.drm_card,
            "-framerate",
            &fps,
            "-f",
            "kmsgrab",
            "-i",
            "-",
            "-vf",
            "hwdownload,format=bgr0",
        ]),
        Capture::X11 => push(&[
            "-draw_mouse",
            "0",
            "-framerate",
            &fps,
            "-f",
            "x11grab",
            "-i",
            display.unwrap_or(":0"),
        ]),
    }
    // kmsgrab does not hold its `-framerate` once the frame copy-out gets
    // faster than the frame period (measured: 376 frames/s asked for 120,
    // which starved a 2-vCPU guest); cap the output rate explicitly.
    push(&["-fpsmax", &fps]);
    push(&["-pix_fmt", "bgr0", "-f", "rawvideo", "pipe:1"]);
    a
}

fn pick_capture(cfg: &Config) -> Option<Capture> {
    let caps = video::capture_methods(cfg);
    match cfg.capture.as_str() {
        "kmsgrab" => Some(Capture::Kms),
        "x11grab" => Some(Capture::X11),
        _ if caps.iter().any(|c| c == "kmsgrab") => Some(Capture::Kms),
        _ if caps.iter().any(|c| c == "x11grab") => Some(Capture::X11),
        _ => None,
    }
}

struct Capturing {
    child: Child,
    size: (u32, u32),
    fps: u32,
}

impl Drop for Capturing {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(cfg: &Config, fps: u32) -> Result<Capturing, String> {
    let capture =
        pick_capture(cfg).ok_or("no capture method (needs root + KMS, or an X session)")?;
    let size = video::mode_size(capture, &StartVideo::default());
    if size.0 == 0 || size.1 == 0 {
        return Err("display size unknown".into());
    }
    let sess = super::session::find();
    let display = sess.as_ref().and_then(|s| s.display.clone());
    let mut c = Command::new(&cfg.ffmpeg);
    c.args(raw_args(cfg, capture, fps, display.as_deref()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if capture == Capture::X11 {
        if let Some(s) = &sess {
            s.apply(&mut c);
        }
    }
    let child = c.spawn().map_err(|e| format!("{}: {e}", cfg.ffmpeg))?;
    Ok(Capturing { child, size, fps })
}

/// Run the frame-ring writer forever (its own thread).
pub fn spawn(cfg: Config) {
    if !cfg.shm_frames {
        return;
    }
    std::thread::spawn(move || writer(cfg));
}

fn writer(cfg: Config) {
    let poll = Duration::from_millis(cfg.shm_poll_ms);
    let timeout = Duration::from_millis(cfg.shm_reader_timeout_ms);
    let mut last_probe = Instant::now() - Duration::from_secs(3600);
    let mut ringm: Option<(Mapping, Ring)> = None;
    loop {
        if ringm.is_none() {
            if last_probe.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(poll);
                continue;
            }
            last_probe = Instant::now();
            ringm = find(&cfg);
            if ringm.is_none() {
                continue;
            }
        }
        let Some((_m, r)) = ringm.as_ref() else {
            continue;
        };
        let mut seen = r.reader_watch_start();
        let mut beat = Instant::now();
        r.set_writer_state(0, "idle");
        // wait for a reader
        while !r.reader_present(&mut seen, timeout) {
            if beat.elapsed() >= Duration::from_secs(1) {
                r.writer_heartbeat();
                beat = Instant::now();
            }
            std::thread::sleep(poll);
        }
        let fps = r.want_fps().max(1);
        let mut cap = match start(&cfg, fps) {
            Ok(c) => c,
            Err(e) => {
                r.set_writer_state(2, &e);
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        let (w, h) = cap.size;
        let frame = (w * h * 4) as usize;
        if !r.layout().fits(w, h) {
            r.set_writer_state(2, &format!("{w}x{h} does not fit the frame ring"));
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        r.set_writer_state(1, &format!("raw {w}x{h}@{fps}"));
        let Some(mut out) = cap.child.stdout.take() else {
            continue;
        };
        loop {
            let slot = r.begin();
            // SAFETY: single writer between begin and publish.
            let px = unsafe { &mut r.slot_pixels(&slot)[..frame] };
            // The first bytes of a frame arrive right after FFmpeg finished
            // capturing (and downloading) it.
            let mut got = match out.read(px) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let t_capture = now_ns();
            let mut ok = true;
            while got < frame {
                match out.read(&mut px[got..]) {
                    Ok(0) | Err(_) => {
                        ok = false;
                        break;
                    }
                    Ok(n) => got += n,
                }
            }
            if !ok {
                break;
            }
            let info = FrameInfo {
                width: w,
                height: h,
                stride: w * 4,
                format: ring::FORMAT_XRGB8888,
                data_len: frame as u32,
                capture_ns: t_capture,
                ..Default::default()
            };
            let _ = r.publish(slot, &info, now_ns());
            if beat.elapsed() >= Duration::from_secs(1) {
                r.writer_heartbeat();
                beat = Instant::now();
                if !r.reader_present(&mut seen, timeout) {
                    break;
                }
                // mode or frame-rate change: restart the capture
                let want = r.want_fps().max(1);
                let now = video::mode_size(
                    pick_capture(&cfg).unwrap_or(Capture::Kms),
                    &StartVideo::default(),
                );
                if want != cap.fps || (now != (0, 0) && now != cap.size) {
                    break;
                }
            }
        }
        drop(cap);
        r.set_writer_state(0, "idle");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_ivshmem_in_sysfs_and_maps_the_ring() {
        let dir = std::env::temp_dir().join(format!("hxshm-{}", std::process::id()));
        let dev = dir.join("0000:00:05.0");
        let other = dir.join("0000:00:04.0");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(dev.join("vendor"), "0x1af4\n").unwrap();
        std::fs::write(dev.join("device"), "0x1110\n").unwrap();
        std::fs::write(other.join("vendor"), "0x1af4\n").unwrap();
        std::fs::write(other.join("device"), "0x1000\n").unwrap();
        let len = 8u64 << 20;
        let hdr = Ring::header_bytes(len, 3, 90).unwrap();
        let f = std::fs::File::create(dev.join("resource2")).unwrap();
        f.set_len(len).unwrap();
        std::fs::write(dev.join("resource2"), &hdr).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(dev.join("resource2"))
            .unwrap()
            .set_len(len)
            .unwrap();
        assert_eq!(devices(dir.to_str().unwrap()), vec![dev.clone()]);
        let cfg = Config {
            shm_pci_dir: dir.display().to_string(),
            ..Default::default()
        };
        let (_m, r) = find(&cfg).expect("ring");
        assert_eq!(r.want_fps(), 90);
        let px = vec![7u8; 64 * 64 * 4];
        r.write_frame(
            &FrameInfo {
                width: 64,
                height: 64,
                stride: 256,
                format: ring::FORMAT_XRGB8888,
                ..Default::default()
            },
            &px,
            1,
        )
        .unwrap();
        // a second mapping of the same file (the viewer's) sees the frame
        let f2 = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dev.join("resource2"))
            .unwrap();
        let (_m2, r2) = open_fd(&f2, len as usize).unwrap();
        let mut out = Vec::new();
        assert_eq!(r2.read_latest(0, &mut out).unwrap().seq, 1);
        assert_eq!(out, px);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn raw_capture_args() {
        let a = raw_args(&Config::default(), Capture::Kms, 240, None).join(" ");
        assert!(
            a.contains("-framerate 240 -f kmsgrab -i - -vf hwdownload,format=bgr0"),
            "{a}"
        );
        assert!(
            a.ends_with("-fpsmax 240 -pix_fmt bgr0 -f rawvideo pipe:1"),
            "{a}"
        );
        assert!(!a.contains("libx264"));
    }
}
