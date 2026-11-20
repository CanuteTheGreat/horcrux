//! The virtio-serial port as a Win32 overlapped handle.
//!
//! A synchronous handle serialises ReadFile and WriteFile per handle (a
//! blocking read stalls every write), so the port is opened with
//! FILE_FLAG_OVERLAPPED and each direction waits on its own event.
//!
//! virtio-serial (vioser) hands one WriteFile to the host as one virtqueue
//! request: a write spanning more pages than the ring has descriptors (a
//! video key frame of a few hundred KB) or a write while the ring is full
//! fails with ERROR_NO_SYSTEM_RESOURCES ("Insufficient system resources").
//! Writes are therefore split into `port_write_max` byte pieces and a full
//! ring is retried after `port_busy_wait_ms` (settings in config.rs).
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Threading::CreateEventW;
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

static WRITE_MAX: AtomicU64 = AtomicU64::new(65536);
static BUSY_WAIT_MS: AtomicU64 = AtomicU64::new(5);
static BUSY_TRIES: AtomicU64 = AtomicU64::new(400);

/// Applies the port settings (config.rs `port_write_max`,
/// `port_busy_wait_ms`, `port_busy_tries`).
pub fn configure(write_max: u64, busy_wait_ms: u64, busy_tries: u64) {
    WRITE_MAX.store(write_max.max(512), Ordering::Relaxed);
    BUSY_WAIT_MS.store(busy_wait_ms.max(1), Ordering::Relaxed);
    BUSY_TRIES.store(busy_tries.max(1), Ordering::Relaxed);
}

/// Length of the next write piece.
pub fn piece(len: usize) -> usize {
    len.min(WRITE_MAX.load(Ordering::Relaxed) as usize)
}

struct Owned(HANDLE);
// SAFETY: a Win32 file/event handle may be used from any thread.
unsafe impl Send for Owned {}
// SAFETY: as above; overlapped calls on distinct OVERLAPPED are independent.
unsafe impl Sync for Owned {}
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// One direction of the port.
pub struct Half {
    file: Arc<Owned>,
    event: Owned,
    write: bool,
}

pub fn open(path: &str) -> Result<crate::Port, String> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: wide is NUL-terminated and outlives the call.
    let h = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            None,
        )
    }
    .map_err(|e| {
        format!("{path}: {e} (is the virtio-serial driver installed and the port enabled?)")
    })?;
    let file = Arc::new(Owned(h));
    let half = |write| -> Result<Half, String> {
        // SAFETY: plain event creation.
        let ev = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map_err(|e| e.to_string())?;
        Ok(Half {
            file: file.clone(),
            event: Owned(ev),
            write,
        })
    };
    Ok((Box::new(half(false)?), Box::new(half(true)?)))
}

impl Half {
    fn wait(&self, ov: &mut OVERLAPPED, started: windows::core::Result<()>) -> io::Result<usize> {
        if let Err(e) = started {
            if e.code() != HRESULT::from_win32(ERROR_IO_PENDING.0) {
                return Err(map_err(e));
            }
        }
        let mut n = 0u32;
        // SAFETY: ov is the OVERLAPPED the operation was started with and
        // stays alive until the wait completes.
        unsafe { GetOverlappedResult(self.file.0, ov, &mut n, true) }.map_err(map_err)?;
        Ok(n as usize)
    }
}

const BUSY: &str = "port ring full";

fn is_busy(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::WouldBlock && e.to_string().contains(BUSY)
}

fn map_err(e: windows::core::Error) -> io::Error {
    let code = e.code();
    if code == HRESULT::from_win32(ERROR_BROKEN_PIPE.0)
        || code == HRESULT::from_win32(ERROR_HANDLE_EOF.0)
    {
        return io::Error::new(io::ErrorKind::UnexpectedEof, e.message());
    }
    if code == HRESULT::from_win32(ERROR_NO_SYSTEM_RESOURCES.0) {
        return io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("{BUSY}: {}", e.message()),
        );
    }
    io::Error::other(e.message())
}

impl Read for Half {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut ov = OVERLAPPED {
            hEvent: self.event.0,
            ..Default::default()
        };
        // SAFETY: buf and ov outlive the wait inside `wait`.
        let r = unsafe { ReadFile(self.file.0, Some(buf), None, Some(&mut ov)) };
        let n = self.wait(&mut ov, r)?;
        if n == 0 && !self.write {
            // A zero-length completion on a message port is not EOF; the
            // caller's read_exact would spin, so treat it as one.
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "port closed"));
        }
        Ok(n)
    }
}

impl Write for Half {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let buf = &buf[..piece(buf.len())];
        let tries = BUSY_TRIES.load(Ordering::Relaxed);
        let mut n = 0;
        loop {
            let mut ov = OVERLAPPED {
                hEvent: self.event.0,
                ..Default::default()
            };
            // SAFETY: buf and ov outlive the wait inside `wait`.
            let r = unsafe { WriteFile(self.file.0, Some(buf), None, Some(&mut ov)) };
            match self.wait(&mut ov, r) {
                Err(e) if is_busy(&e) && n + 1 < tries => {
                    n += 1;
                    std::thread::sleep(std::time::Duration::from_millis(
                        BUSY_WAIT_MS.load(Ordering::Relaxed),
                    ));
                }
                other => return other,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
