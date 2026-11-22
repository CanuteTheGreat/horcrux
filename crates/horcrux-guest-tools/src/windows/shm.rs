//! Same-host raw frames on Windows (docs/design/horcrux-console.md section
//! 4.2): the ivshmem PCI device is served by the IVSHMEM driver from the
//! virtio-win driver set (BSD-3-Clause, installed with the other virtio
//! drivers); the guest tools open its device interface, ask it to map BAR 2
//! into this process (`IOCTL_IVSHMEM_REQUEST_MMAP`, the driver's documented
//! interface) and write Desktop Duplication frames straight into the
//! Horcrux frame ring while a same-host viewer watches.
use super::capture::{Capturer, Grab};
use crate::config::Config;
use horcrux_guest_proto::ring::{self, FrameInfo, Ring};
use horcrux_guest_proto::StartVideo;
use std::time::{Duration, Instant};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::DeviceIoControl;

/// Device interface class registered by the virtio-win IVSHMEM driver.
const GUID_DEVINTERFACE_IVSHMEM: GUID = GUID::from_u128(0xdf576976_569d_4672_95a0_f57e4ea0b210);

const fn ctl_code(function: u32) -> u32 {
    // CTL_CODE(FILE_DEVICE_UNKNOWN, function, METHOD_BUFFERED, FILE_ANY_ACCESS)
    (0x22 << 16) | (function << 2)
}
const IOCTL_IVSHMEM_REQUEST_SIZE: u32 = ctl_code(0x801);
const IOCTL_IVSHMEM_REQUEST_MMAP: u32 = ctl_code(0x802);
const IOCTL_IVSHMEM_RELEASE_MMAP: u32 = ctl_code(0x803);
/// Cache modes the driver offers (its `IVSHMEM_CACHE_MODE`): write-combined
/// suits a write-mostly frame buffer.
const CACHE_NONCACHED: u8 = 0;
const CACHE_WRITECOMBINED: u8 = 2;

#[repr(C)]
#[derive(Default)]
struct MmapReply {
    peer_id: u16,
    size: u64,
    ptr: usize,
    vectors: u16,
}

/// The driver's mapping of BAR 2 into this process (released on drop).
pub struct Mapping {
    dev: HANDLE,
    base: *mut u8,
    len: usize,
}
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: handle opened by `open`.
        unsafe {
            let _ = DeviceIoControl(
                self.dev,
                IOCTL_IVSHMEM_RELEASE_MMAP,
                None,
                0,
                None,
                0,
                None,
                None,
            );
            let _ = CloseHandle(self.dev);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Device interface paths of the IVSHMEM driver.
fn interface_paths() -> Vec<String> {
    let mut v = Vec::new();
    // SAFETY: SetupAPI enumeration with buffers sized by the API.
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(
            Some(&GUID_DEVINTERFACE_IVSHMEM),
            PCWSTR::null(),
            HWND::default(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        ) else {
            return v;
        };
        let mut i = 0;
        loop {
            let mut ifd = SP_DEVICE_INTERFACE_DATA {
                cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(set, None, &GUID_DEVINTERFACE_IVSHMEM, i, &mut ifd)
                .is_err()
            {
                break;
            }
            i += 1;
            let mut need = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(set, &ifd, None, 0, Some(&mut need), None);
            if need < 8 {
                continue;
            }
            let mut buf = vec![0u64; (need as usize).div_ceil(8)];
            let det = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*det).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &ifd, Some(det), need, None, None).is_ok() {
                let p = (*det).DevicePath.as_ptr();
                let mut n = 0;
                while *p.add(n) != 0 {
                    n += 1;
                }
                v.push(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    v
}

fn open(path: &str, write_combine: bool) -> Result<(Mapping, Ring), String> {
    let w = wide(path);
    // SAFETY: CreateFileW/DeviceIoControl with correctly sized buffers.
    unsafe {
        let dev = CreateFileW(
            PCWSTR(w.as_ptr()),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            HANDLE::default(),
        )
        .map_err(|e| format!("{path}: {e}"))?;
        let mut size = 0u64;
        let mut got = 0u32;
        if let Err(e) = DeviceIoControl(
            dev,
            IOCTL_IVSHMEM_REQUEST_SIZE,
            None,
            0,
            Some(&mut size as *mut u64 as *mut _),
            8,
            Some(&mut got),
            None,
        ) {
            let _ = CloseHandle(dev);
            return Err(format!("IVSHMEM size: {e}"));
        }
        let mode = if write_combine {
            CACHE_WRITECOMBINED
        } else {
            CACHE_NONCACHED
        };
        let mut reply = MmapReply::default();
        if let Err(e) = DeviceIoControl(
            dev,
            IOCTL_IVSHMEM_REQUEST_MMAP,
            Some(&mode as *const u8 as *const _),
            1,
            Some(&mut reply as *mut MmapReply as *mut _),
            std::mem::size_of::<MmapReply>() as u32,
            Some(&mut got),
            None,
        ) {
            let _ = CloseHandle(dev);
            return Err(format!("IVSHMEM map: {e}"));
        }
        let m = Mapping {
            dev,
            base: reply.ptr as *mut u8,
            len: reply.size.min(size.max(reply.size)) as usize,
        };
        if m.base.is_null() || m.len == 0 {
            return Err("IVSHMEM driver returned an empty mapping".into());
        }
        let r = Ring::attach(m.base, m.len)?;
        Ok((m, r))
    }
}

/// The first IVSHMEM device that carries a Horcrux frame ring.
pub fn find(cfg: &Config) -> Option<(Mapping, Ring)> {
    let paths = if cfg.shm_device.is_empty() {
        interface_paths()
    } else {
        vec![cfg.shm_device.clone()]
    };
    for p in paths {
        match open(&p, cfg.shm_write_combine) {
            Ok(x) => return Some(x),
            Err(e) => eprintln!("frame ring: {e}"),
        }
    }
    None
}

pub fn available(cfg: &Config) -> bool {
    cfg.shm_frames && find(cfg).is_some()
}

fn now_ns(t0: Instant) -> u64 {
    t0.elapsed().as_nanos() as u64 + 1
}

/// Run the frame-ring writer forever (its own thread).
pub fn spawn(cfg: Config) {
    if !cfg.shm_frames {
        return;
    }
    std::thread::spawn(move || writer(cfg));
}

fn writer(cfg: Config) {
    let t0 = Instant::now();
    let poll = Duration::from_millis(cfg.shm_poll_ms);
    let timeout = Duration::from_millis(cfg.shm_reader_timeout_ms);
    loop {
        let Some((_m, r)) = find(&cfg) else {
            std::thread::sleep(Duration::from_secs(10));
            continue;
        };
        loop {
            let mut seen = r.reader_watch_start();
            let mut beat = Instant::now();
            r.set_writer_state(0, "idle");
            while !r.reader_present(&mut seen, timeout) {
                if beat.elapsed() >= Duration::from_secs(1) {
                    r.writer_heartbeat();
                    beat = Instant::now();
                }
                std::thread::sleep(poll);
            }
            let mut cap: Box<dyn Capturer> =
                match super::video::open_capture(&cfg, &StartVideo::default()) {
                    Ok(c) => c,
                    Err(e) => {
                        r.set_writer_state(2, &e);
                        std::thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                };
            let (w, h) = cap.size();
            if !r.layout().fits(w, h) {
                r.set_writer_state(2, &format!("{w}x{h} does not fit the frame ring"));
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            r.set_writer_state(1, &format!("raw {w}x{h} {}", cap.name()));
            let mut buf = Vec::new();
            let mut stats = Stats::new(cfg.shm_stats_secs);
            let wait = (1000 / r.want_fps().max(1)).max(1);
            loop {
                let g0 = Instant::now();
                match cap.grab(wait.max(cfg.idle_poll_ms as u32), &mut buf) {
                    Grab::New => {
                        let grabbed = g0.elapsed();
                        // The picture counts from when Windows presented it,
                        // not from when this thread got to look at it.
                        let t = now_ns(t0).saturating_sub(cap.frame_age_ns()).max(1);
                        let (cw, ch) = cap.size();
                        let info = FrameInfo {
                            width: cw,
                            height: ch,
                            stride: cw * 4,
                            format: ring::FORMAT_XRGB8888,
                            capture_ns: t,
                            ..Default::default()
                        };
                        let w0 = Instant::now();
                        if r.write_frame(&info, &buf, now_ns(t0)).is_err() {
                            break;
                        }
                        stats.frame(grabbed, w0.elapsed());
                    }
                    Grab::Idle => {}
                    Grab::Lost | Grab::Err(_) => break,
                }
                if beat.elapsed() >= Duration::from_secs(1) {
                    r.writer_heartbeat();
                    beat = Instant::now();
                    if !r.reader_present(&mut seen, timeout) {
                        break;
                    }
                }
            }
        }
    }
}

/// Optional writer statistics (`shm_stats_secs`).
struct Stats {
    every: Duration,
    since: Instant,
    frames: u32,
    grab: Duration,
    write: Duration,
}

impl Stats {
    fn new(secs: u64) -> Stats {
        Stats {
            every: Duration::from_secs(secs),
            since: Instant::now(),
            frames: 0,
            grab: Duration::ZERO,
            write: Duration::ZERO,
        }
    }

    fn frame(&mut self, grab: Duration, write: Duration) {
        if self.every.is_zero() {
            return;
        }
        self.frames += 1;
        self.grab += grab;
        self.write += write;
        let el = self.since.elapsed();
        if el >= self.every {
            let n = self.frames.max(1) as f64;
            eprintln!(
                "frame ring: {:.1} fps, grab {:.2} ms (includes the wait for a change), write {:.2} ms per frame",
                self.frames as f64 / el.as_secs_f64(),
                self.grab.as_secs_f64() * 1000.0 / n,
                self.write.as_secs_f64() * 1000.0 / n
            );
            *self = Stats::new(self.every.as_secs());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_off_by_default_and_reset_after_a_report() {
        let mut s = Stats::new(0);
        s.frame(Duration::from_millis(1), Duration::from_millis(1));
        assert_eq!(s.frames, 0);
        let mut s = Stats::new(3600);
        s.frame(Duration::from_millis(1), Duration::from_millis(2));
        assert_eq!(s.frames, 1);
        assert_eq!(s.write, Duration::from_millis(2));
    }

    #[test]
    fn ioctl_codes() {
        assert_eq!(IOCTL_IVSHMEM_REQUEST_SIZE, 0x0022_2004);
        assert_eq!(IOCTL_IVSHMEM_REQUEST_MMAP, 0x0022_2008);
        assert_eq!(IOCTL_IVSHMEM_RELEASE_MMAP, 0x0022_200c);
        assert_eq!(std::mem::size_of::<MmapReply>(), 32);
    }
}
