//! Headless reader for tests and latency runs: attaches to a frame ring
//! (a file, or the descriptor the node hands over), stamps the reader
//! clock like the window does, and records every frame it sees:
//! arrival time (CLOCK_MONOTONIC, comparable with other processes on the
//! host), the writer's capture-to-publish time, publish-to-seen measured on
//! this host's clock, the copy time and how many pixels changed.
use crate::conn::Mapped;
use horcrux_guest_proto::ring::FrameInfo;
use std::io::Write;
use std::time::{Duration, Instant};

pub fn now_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Pixels whose colour moved by more than `thr` in any channel (BGRX).
pub fn changed_pixels(a: &[u8], b: &[u8], thr: u8) -> u64 {
    if a.len() != b.len() {
        return (b.len() / 4) as u64;
    }
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > thr))
        .count() as u64
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Sample {
    pub seq: u64,
    pub seen_ns: u64,
    pub width: u32,
    pub height: u32,
    pub capture_to_publish_us: f64,
    /// Publish -> this reader saw it, on the host clock (0 when the writer
    /// had no reader clock of this run to copy: a frame left in the ring by
    /// an earlier reader).
    pub publish_to_seen_us: f64,
    pub copy_us: f64,
    pub changed: u64,
}

/// Poll the ring for `secs`, writing one JSON line per frame to `out` and
/// leaving the newest frame (header and pixels) in `keep`.
pub fn run(
    m: &Mapped,
    secs: f64,
    poll_us: u64,
    fps: u32,
    out: &mut dyn Write,
    keep: &mut Option<(FrameInfo, Vec<u8>)>,
) -> Vec<Sample> {
    let r = &m.ring;
    let end = Instant::now() + Duration::from_secs_f64(secs);
    // The first stamp of this run: a frame published under an older stamp
    // is left over from an earlier reader and has no publish-to-seen time.
    let started = now_ns();
    let mut last = 0u64;
    let mut buf = Vec::new();
    let mut prev: Vec<u8> = Vec::new();
    let mut samples = Vec::new();
    while Instant::now() < end {
        r.reader_tick(now_ns(), fps);
        let t0 = now_ns();
        if let Some(fi) = r.read_latest(last, &mut buf) {
            let t1 = now_ns();
            let s = sample(&fi, t0, t1, started, &prev, &buf);
            let _ = writeln!(out, "{}", serde_json::to_string(&s).unwrap_or_default());
            last = fi.seq;
            *keep = Some((fi, buf.clone()));
            std::mem::swap(&mut prev, &mut buf);
            samples.push(s);
        } else {
            std::thread::sleep(Duration::from_micros(poll_us));
        }
    }
    samples
}

fn sample(fi: &FrameInfo, t0: u64, t1: u64, started: u64, prev: &[u8], cur: &[u8]) -> Sample {
    Sample {
        seq: fi.seq,
        seen_ns: t0,
        width: fi.width,
        height: fi.height,
        capture_to_publish_us: fi.publish_ns.saturating_sub(fi.capture_ns) as f64 / 1000.0,
        publish_to_seen_us: if fi.reader_clock_at_publish >= started {
            t0.saturating_sub(fi.reader_clock_at_publish) as f64 / 1000.0
        } else {
            0.0
        },
        copy_us: (t1 - t0) as f64 / 1000.0,
        changed: changed_pixels(prev, cur, 64),
    }
}

/// Write a ring frame (BGRX rows of `stride` bytes) as a binary PPM (P6).
pub fn write_ppm(path: &str, fi: &FrameInfo, px: &[u8]) -> Result<(), String> {
    let (w, h, stride) = (fi.width as usize, fi.height as usize, fi.stride as usize);
    if stride < w * 4 || px.len() < stride * h.saturating_sub(1) + w * 4 {
        return Err(format!(
            "frame {w}x{h} stride {stride} does not match {} bytes",
            px.len()
        ));
    }
    let mut out = Vec::with_capacity(w * h * 3 + 32);
    out.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
    for y in 0..h {
        let row = &px[y * stride..y * stride + w * 4];
        for x in 0..w {
            out.extend_from_slice(&[row[x * 4 + 2], row[x * 4 + 1], row[x * 4]]);
        }
    }
    std::fs::write(path, out).map_err(|e| format!("{path}: {e}"))
}

/// Median and 95th percentile.
pub fn med_p95(mut v: Vec<f64>) -> (f64, f64) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    v.sort_by(|a, b| a.total_cmp(b));
    (
        v[v.len() / 2],
        v[((v.len() as f64 * 0.95) as usize).min(v.len() - 1)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppm_dump_swaps_channels_and_honours_stride() {
        let fi = FrameInfo {
            width: 2,
            height: 2,
            stride: 12,
            ..Default::default()
        };
        // BGRX pixels, 4 bytes of row padding
        let px = [
            1, 2, 3, 0, 4, 5, 6, 0, 9, 9, 9, 9, //
            7, 8, 9, 0, 10, 11, 12, 0, 9, 9, 9, 9,
        ];
        let p = std::env::temp_dir().join(format!("hxv-ppm-{}", std::process::id()));
        write_ppm(p.to_str().unwrap(), &fi, &px).unwrap();
        let got = std::fs::read(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        assert_eq!(&got[..11], b"P6\n2 2\n255\n");
        assert_eq!(&got[11..], &[3, 2, 1, 6, 5, 4, 9, 8, 7, 12, 11, 10]);
        assert!(write_ppm("/nonexistent/x", &fi, &px[..10]).is_err());
    }

    #[test]
    fn change_counter() {
        let a = vec![0u8; 16];
        let mut b = a.clone();
        b[4] = 200;
        b[9] = 30;
        assert_eq!(changed_pixels(&a, &b, 64), 1);
        assert_eq!(changed_pixels(&a, &b[..8], 64), 2);
        assert_eq!(med_p95(vec![3.0, 1.0, 2.0]), (2.0, 3.0));
    }

    #[test]
    fn measures_a_writer_through_a_ring_file() {
        let p = std::env::temp_dir().join(format!("hxv-measure-{}", std::process::id()));
        crate::conn::ring_init(p.to_str().unwrap(), 8, 3, 60).unwrap();
        let open = || {
            crate::conn::map_ring(
                std::fs::File::options()
                    .read(true)
                    .write(true)
                    .open(&p)
                    .unwrap(),
            )
            .unwrap()
        };
        let w = open();
        let rd = open();
        let writer = std::thread::spawn(move || {
            // wait for the reader's clock, like the guest tools do
            let mut seen = (0u64, Instant::now());
            while !w.ring.reader_present(&mut seen, Duration::from_secs(2)) {
                std::thread::sleep(Duration::from_millis(1));
            }
            for i in 0..20u8 {
                let px = vec![if i.is_multiple_of(2) { 0u8 } else { 255u8 }; 64 * 64 * 4];
                let t = now_ns();
                let info = FrameInfo {
                    width: 64,
                    height: 64,
                    stride: 256,
                    format: horcrux_guest_proto::ring::FORMAT_XRGB8888,
                    capture_ns: t,
                    ..Default::default()
                };
                w.ring.write_frame(&info, &px, now_ns()).unwrap();
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let mut sink = Vec::new();
        let s = run(&rd, 0.6, 200, 60, &mut sink, &mut None);
        writer.join().unwrap();
        assert!(s.len() >= 15, "{} frames", s.len());
        // frames alternate black/white: every frame seen is either all changed
        // (the reader saw consecutive frames) or unchanged (it skipped one)
        assert!(s
            .iter()
            .skip(1)
            .all(|x| x.changed == 0 || x.changed == 64 * 64));
        assert!(s.iter().skip(1).any(|x| x.changed == 64 * 64));
        assert!(s
            .iter()
            .all(|x| x.publish_to_seen_us > 0.0 && x.publish_to_seen_us < 50_000.0));
        std::fs::remove_file(p).ok();
    }
}
