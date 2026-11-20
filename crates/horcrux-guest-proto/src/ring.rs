//! Same-host raw frame ring (docs/design/horcrux-console.md section 4.2).
//!
//! A Horcrux-managed ivshmem device (`ivshmem-plain` backed by a file under
//! `/dev/shm` or a hugetlbfs mount on the node, BAR 2 in the guest) carries
//! uncompressed frames from the guest tools to the native viewer on the same
//! host: no encode, no decode, one copy on each side.
//!
//! Designed from first principles for Horcrux (no code or layout taken from
//! any other shared-memory frame transport). Everything is little-endian
//! and naturally aligned; every multi-byte field that both sides touch
//! concurrently is a 32- or 64-bit atomic.
//!
//! ```text
//! offset 0          header (4096 bytes)
//! cursor_offset     cursor slot: 256-byte header + up to 256x256 BGRA
//! slot_offset + i*slot_stride, i < slot_count
//!                   frame slot: 256-byte header + pixels
//! ```
//!
//! Roles:
//! * the **node** creates the backing file and writes the header
//!   ([`Ring::init`]) before QEMU starts; it never touches the ring again;
//! * the **writer** (guest tools) attaches ([`Ring::attach`]), captures only
//!   while a reader is present ([`Ring::reader_present`]) and publishes
//!   frames with [`Ring::publish`];
//! * the **reader** (native viewer) attaches, stamps its clock
//!   ([`Ring::reader_tick`], doubling as its heartbeat) and copies the
//!   newest frame out with [`Ring::read_latest`].
//!
//! Consistency: each slot carries `seq_begin`/`seq_end`. The writer sets
//! `seq_begin = n`, writes pixels and fields, then sets `seq_end = n` and
//! finally `latest = n << 8 | slot`. A reader copies the slot and accepts
//! it only if `seq_begin == seq_end == n` before and after the copy, so a
//! frame overwritten mid-copy is never shown. The writer never picks the
//! slot named by `latest` or by the reader's `reader_slot`, so with three
//! or more slots a reader that keeps up never loses a frame to a retry.
//!
//! Trust: both sides treat every field written by the other as untrusted:
//! sizes are checked against the slot before any copy, so a hostile guest
//! can only produce wrong pixels, never an out-of-bounds read.
use std::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};

/// `HXFRAME1` read as a little-endian u64.
pub const MAGIC: u64 = u64::from_le_bytes(*b"HXFRAME1");
pub const VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 4096;
pub const SLOT_HEADER: usize = 256;
pub const CURSOR_MAX: u32 = 256;
/// Cursor slot: header + the largest shape, rounded to 4 KiB.
pub const CURSOR_AREA: usize = 4096 + (CURSOR_MAX * CURSOR_MAX * 4) as usize;
pub const MIN_SLOTS: u32 = 2;
pub const MAX_SLOTS: u32 = 8;
/// PCI ids of QEMU's ivshmem device (Red Hat / Qumranet vendor id).
pub const IVSHMEM_VENDOR: u16 = 0x1af4;
pub const IVSHMEM_DEVICE: u16 = 0x1110;

/// DRM fourcc of a pixel format.
pub const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    u32::from_le_bytes([a, b, c, d])
}
/// 32 bits per pixel, bytes B, G, R, X in memory (DRM XRGB8888).
pub const FORMAT_XRGB8888: u32 = fourcc(b'X', b'R', b'2', b'4');
/// 32 bits per pixel, bytes B, G, R, A in memory (DRM ARGB8888).
pub const FORMAT_ARGB8888: u32 = fourcc(b'A', b'R', b'2', b'4');

/// Bytes per pixel of a known format.
pub fn bytes_per_pixel(format: u32) -> Option<u32> {
    match format {
        FORMAT_XRGB8888 | FORMAT_ARGB8888 => Some(4),
        _ => None,
    }
}

// Header field offsets.
mod h {
    pub const MAGIC: usize = 0;
    pub const VERSION: usize = 8;
    pub const HEADER_SIZE: usize = 12;
    pub const REGION_SIZE: usize = 16;
    pub const SLOT_COUNT: usize = 24;
    pub const WANT_FPS: usize = 28;
    pub const SLOT_OFFSET: usize = 32;
    pub const SLOT_STRIDE: usize = 40;
    pub const CURSOR_OFFSET: usize = 48;
    /// `frame seq << 8 | slot`, 0 = nothing published yet.
    pub const LATEST: usize = 64;
    /// Reader's CLOCK_MONOTONIC (ns), updated while it watches.
    pub const READER_CLOCK: usize = 72;
    /// Slot the reader is copying (u32, `u32::MAX` = none).
    pub const READER_SLOT: usize = 80;
    /// Frame rate the reader would like (0 = writer's choice).
    pub const READER_FPS: usize = 84;
    /// Incremented by the writer about once a second while it runs.
    pub const WRITER_BEAT: usize = 88;
    /// Writer state: 0 idle, 1 capturing, 2 error.
    pub const WRITER_STATE: usize = 96;
    /// Writer's capture method/encoder name, NUL padded (64 bytes).
    pub const WRITER_INFO: usize = 128;
    pub const WRITER_INFO_LEN: usize = 64;
}

// Slot header field offsets.
mod s {
    pub const SEQ_BEGIN: usize = 0;
    pub const SEQ_END: usize = 8;
    pub const WIDTH: usize = 16;
    pub const HEIGHT: usize = 20;
    pub const STRIDE: usize = 24;
    pub const FORMAT: usize = 28;
    pub const DATA_LEN: usize = 32;
    pub const FLAGS: usize = 36;
    pub const CAPTURE_NS: usize = 40;
    pub const PUBLISH_NS: usize = 48;
    pub const READER_CLOCK_AT_PUBLISH: usize = 56;
}

// Cursor header field offsets (inside the cursor area).
mod c {
    pub const SEQ_BEGIN: usize = 0;
    pub const SEQ_END: usize = 8;
    pub const X: usize = 16;
    pub const Y: usize = 20;
    pub const VISIBLE: usize = 24;
    pub const SHAPE_SERIAL: usize = 28;
    pub const WIDTH: usize = 32;
    pub const HEIGHT: usize = 36;
    pub const HOT_X: usize = 40;
    pub const HOT_Y: usize = 44;
}

/// Where everything sits in a region of a given size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub region_size: u64,
    pub slot_count: u32,
    pub slot_offset: u64,
    pub slot_stride: u64,
    pub cursor_offset: u64,
}

impl Layout {
    /// Split `region_size` bytes into a header, the cursor slot and
    /// `slot_count` equal frame slots (4 KiB aligned).
    pub fn new(region_size: u64, slot_count: u32) -> Result<Layout, String> {
        if !(MIN_SLOTS..=MAX_SLOTS).contains(&slot_count) {
            return Err(format!(
                "slot count must be {MIN_SLOTS}..={MAX_SLOTS}, got {slot_count}"
            ));
        }
        let cursor_offset = HEADER_SIZE as u64;
        let slot_offset = cursor_offset + CURSOR_AREA as u64;
        let room = region_size
            .checked_sub(slot_offset)
            .ok_or("region too small for the frame ring header")?;
        let slot_stride = (room / slot_count as u64) & !4095;
        if slot_stride < (SLOT_HEADER as u64) + 64 * 64 * 4 {
            return Err(format!(
                "region of {region_size} bytes is too small for {slot_count} frame slots"
            ));
        }
        Ok(Layout {
            region_size,
            slot_count,
            slot_offset,
            slot_stride,
            cursor_offset,
        })
    }

    /// Largest frame (pixel bytes) one slot holds.
    pub fn max_frame_bytes(&self) -> u64 {
        self.slot_stride - SLOT_HEADER as u64
    }

    /// Does a `width` x `height` 32-bit frame fit?
    pub fn fits(&self, width: u32, height: u32) -> bool {
        (width as u64) * (height as u64) * 4 <= self.max_frame_bytes()
    }

    /// Region size (power of two, as ivshmem's BAR needs) for `slots`
    /// frames of `width` x `height` at 32 bits per pixel.
    pub fn region_for(width: u32, height: u32, slots: u32) -> u64 {
        let frame = (width as u64 * height as u64 * 4 + SLOT_HEADER as u64 + 4095) & !4095;
        let need = (HEADER_SIZE + CURSOR_AREA) as u64 + frame * slots as u64;
        need.next_power_of_two()
    }
}

/// Fields of a published frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameInfo {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes per row in the slot.
    pub stride: u32,
    pub format: u32,
    pub data_len: u32,
    pub flags: u32,
    /// Writer's monotonic clock when the frame was captured (ns).
    pub capture_ns: u64,
    /// Writer's monotonic clock when it was published (ns).
    pub publish_ns: u64,
    /// The reader's clock word as the writer saw it at publish time, so the
    /// reader can measure publish-to-display on its own clock (0 = no
    /// reader clock seen yet).
    pub reader_clock_at_publish: u64,
}

/// Cursor state carried in the cursor slot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CursorInfo {
    pub seq: u64,
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    /// Changes whenever the shape changes.
    pub shape_serial: u32,
    pub width: u32,
    pub height: u32,
    pub hot_x: u32,
    pub hot_y: u32,
}

/// A mapped frame ring. Cheap to share between threads.
pub struct Ring {
    base: *mut u8,
    len: usize,
    layout: Layout,
}

// The ring only hands out copies and uses atomics for shared fields.
unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

impl Ring {
    /// Write a fresh header into `base[..len]` (the node, before the guest
    /// runs). `len` must be the whole region.
    ///
    /// # Safety
    /// `base` must point to `len` writable bytes, 8-byte aligned, valid for
    /// as long as the returned ring lives.
    pub unsafe fn init(
        base: *mut u8,
        len: usize,
        slots: u32,
        want_fps: u32,
    ) -> Result<Ring, String> {
        if !(base as usize).is_multiple_of(8) {
            return Err("frame ring must be 8-byte aligned".into());
        }
        let layout = Layout::new(len as u64, slots)?;
        std::ptr::write_bytes(base, 0, HEADER_SIZE.min(len));
        let r = Ring { base, len, layout };
        r.u32_at(h::VERSION).store(VERSION, Ordering::Relaxed);
        r.u32_at(h::HEADER_SIZE)
            .store(HEADER_SIZE as u32, Ordering::Relaxed);
        r.u64_at(h::REGION_SIZE)
            .store(len as u64, Ordering::Relaxed);
        r.u32_at(h::SLOT_COUNT).store(slots, Ordering::Relaxed);
        r.u32_at(h::WANT_FPS).store(want_fps, Ordering::Relaxed);
        r.u64_at(h::SLOT_OFFSET)
            .store(layout.slot_offset, Ordering::Relaxed);
        r.u64_at(h::SLOT_STRIDE)
            .store(layout.slot_stride, Ordering::Relaxed);
        r.u64_at(h::CURSOR_OFFSET)
            .store(layout.cursor_offset, Ordering::Relaxed);
        r.u32_at(h::READER_SLOT).store(u32::MAX, Ordering::Relaxed);
        for i in 0..slots {
            let o = r.slot_base(i);
            r.u64_at(o + s::SEQ_BEGIN).store(0, Ordering::Relaxed);
            r.u64_at(o + s::SEQ_END).store(0, Ordering::Relaxed);
        }
        let co = layout.cursor_offset as usize;
        r.u64_at(co + c::SEQ_BEGIN).store(0, Ordering::Relaxed);
        r.u64_at(co + c::SEQ_END).store(0, Ordering::Relaxed);
        // Magic last: a writer that attaches early sees no ring until the
        // header is complete.
        r.u64_at(h::MAGIC).store(MAGIC, Ordering::Release);
        Ok(r)
    }

    /// A header for a region of `len` bytes, as bytes (the node writes it
    /// to the start of the backing file without mapping it).
    pub fn header_bytes(len: u64, slots: u32, want_fps: u32) -> Result<Vec<u8>, String> {
        let layout = Layout::new(len, slots)?;
        let mut v = vec![0u64; (layout.slot_offset as usize + 64) / 8];
        let need = layout.slot_offset as usize;
        let base = v.as_mut_ptr() as *mut u8;
        // SAFETY: the buffer covers the header and the cursor header; init
        // only touches those (and slot headers, skipped below by limiting
        // the region to what the buffer holds for the slot loop).
        unsafe {
            std::ptr::write_bytes(base, 0, need);
            let r = Ring {
                base,
                len: need + 64,
                layout,
            };
            r.u32_at(h::VERSION).store(VERSION, Ordering::Relaxed);
            r.u32_at(h::HEADER_SIZE)
                .store(HEADER_SIZE as u32, Ordering::Relaxed);
            r.u64_at(h::REGION_SIZE).store(len, Ordering::Relaxed);
            r.u32_at(h::SLOT_COUNT).store(slots, Ordering::Relaxed);
            r.u32_at(h::WANT_FPS).store(want_fps, Ordering::Relaxed);
            r.u64_at(h::SLOT_OFFSET)
                .store(layout.slot_offset, Ordering::Relaxed);
            r.u64_at(h::SLOT_STRIDE)
                .store(layout.slot_stride, Ordering::Relaxed);
            r.u64_at(h::CURSOR_OFFSET)
                .store(layout.cursor_offset, Ordering::Relaxed);
            r.u32_at(h::READER_SLOT).store(u32::MAX, Ordering::Relaxed);
            r.u64_at(h::MAGIC).store(MAGIC, Ordering::Relaxed);
        }
        let bytes: Vec<u8> = v.iter().flat_map(|w| w.to_ne_bytes()).collect();
        Ok(bytes[..need].to_vec())
    }

    /// Attach to a region whose header the node wrote. Every header field
    /// is checked against `len`, so a corrupt header is refused instead of
    /// trusted.
    ///
    /// # Safety
    /// `base` must point to `len` readable and writable bytes, 8-byte
    /// aligned, mapped shared, valid for as long as the ring lives.
    pub unsafe fn attach(base: *mut u8, len: usize) -> Result<Ring, String> {
        if !(base as usize).is_multiple_of(8) {
            return Err("frame ring must be 8-byte aligned".into());
        }
        if len < HEADER_SIZE + CURSOR_AREA {
            return Err(format!("region of {len} bytes is too small"));
        }
        let probe = Ring {
            base,
            len,
            layout: Layout {
                region_size: len as u64,
                slot_count: 0,
                slot_offset: 0,
                slot_stride: 0,
                cursor_offset: 0,
            },
        };
        if probe.u64_at(h::MAGIC).load(Ordering::Acquire) != MAGIC {
            return Err("no Horcrux frame ring here (magic missing)".into());
        }
        let v = probe.u32_at(h::VERSION).load(Ordering::Relaxed);
        if v != VERSION {
            return Err(format!(
                "frame ring version {v}, this side speaks {VERSION}"
            ));
        }
        let region = probe.u64_at(h::REGION_SIZE).load(Ordering::Relaxed);
        if region > len as u64 {
            return Err(format!("frame ring says {region} bytes, only {len} mapped"));
        }
        let slots = probe.u32_at(h::SLOT_COUNT).load(Ordering::Relaxed);
        let expect = Layout::new(region, slots)?;
        let got = Layout {
            region_size: region,
            slot_count: slots,
            slot_offset: probe.u64_at(h::SLOT_OFFSET).load(Ordering::Relaxed),
            slot_stride: probe.u64_at(h::SLOT_STRIDE).load(Ordering::Relaxed),
            cursor_offset: probe.u64_at(h::CURSOR_OFFSET).load(Ordering::Relaxed),
        };
        if got != expect {
            return Err(format!(
                "frame ring layout {got:?} does not match {expect:?}"
            ));
        }
        Ok(Ring {
            base,
            len,
            layout: got,
        })
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Frame rate the node asked for (profile default).
    pub fn want_fps(&self) -> u32 {
        let r = self.u32_at(h::READER_FPS).load(Ordering::Relaxed);
        if r > 0 {
            return r.min(1000);
        }
        self.u32_at(h::WANT_FPS).load(Ordering::Relaxed).min(1000)
    }

    // ---- reader side ----

    /// Stamp the reader's clock (ns, any monotonic clock); this is also the
    /// reader's heartbeat that makes the writer capture.
    pub fn reader_tick(&self, now_ns: u64, want_fps: u32) {
        self.u32_at(h::READER_FPS)
            .store(want_fps, Ordering::Relaxed);
        self.u64_at(h::READER_CLOCK)
            .store(now_ns.max(1), Ordering::Release);
    }

    /// The reader's last clock stamp (0 = never).
    pub fn reader_clock(&self) -> u64 {
        self.u64_at(h::READER_CLOCK).load(Ordering::Acquire)
    }

    /// Sequence number of the newest frame (0 = none yet).
    pub fn latest_seq(&self) -> u64 {
        self.u64_at(h::LATEST).load(Ordering::Acquire) >> 8
    }

    /// Copy the newest frame into `out` if it is newer than `after`.
    /// Returns `None` when there is nothing new or the writer kept
    /// overwriting the slot (try again on the next tick).
    pub fn read_latest(&self, after: u64, out: &mut Vec<u8>) -> Option<FrameInfo> {
        for _ in 0..4 {
            let latest = self.u64_at(h::LATEST).load(Ordering::Acquire);
            let seq = latest >> 8;
            let slot = (latest & 0xff) as u32;
            if seq == 0 || seq <= after || slot >= self.layout.slot_count {
                return None;
            }
            self.u32_at(h::READER_SLOT).store(slot, Ordering::SeqCst);
            fence(Ordering::SeqCst);
            // The writer may have moved on while we marked the slot; it
            // only avoids slots it can see marked.
            if self.u64_at(h::LATEST).load(Ordering::Acquire) != latest {
                continue;
            }
            let r = self.copy_slot(slot, seq, out);
            self.u32_at(h::READER_SLOT)
                .store(u32::MAX, Ordering::Release);
            if r.is_some() {
                return r;
            }
        }
        None
    }

    fn copy_slot(&self, slot: u32, seq: u64, out: &mut Vec<u8>) -> Option<FrameInfo> {
        let o = self.slot_base(slot);
        let b = self.u64_at(o + s::SEQ_BEGIN).load(Ordering::Acquire);
        let e = self.u64_at(o + s::SEQ_END).load(Ordering::Acquire);
        if b != seq || e != seq {
            return None;
        }
        let info = FrameInfo {
            seq,
            width: self.u32_at(o + s::WIDTH).load(Ordering::Relaxed),
            height: self.u32_at(o + s::HEIGHT).load(Ordering::Relaxed),
            stride: self.u32_at(o + s::STRIDE).load(Ordering::Relaxed),
            format: self.u32_at(o + s::FORMAT).load(Ordering::Relaxed),
            data_len: self.u32_at(o + s::DATA_LEN).load(Ordering::Relaxed),
            flags: self.u32_at(o + s::FLAGS).load(Ordering::Relaxed),
            capture_ns: self.u64_at(o + s::CAPTURE_NS).load(Ordering::Relaxed),
            publish_ns: self.u64_at(o + s::PUBLISH_NS).load(Ordering::Relaxed),
            reader_clock_at_publish: self
                .u64_at(o + s::READER_CLOCK_AT_PUBLISH)
                .load(Ordering::Relaxed),
        };
        if !Self::frame_ok(&info, self.layout.max_frame_bytes()) {
            return None;
        }
        out.resize(info.data_len as usize, 0);
        // SAFETY: data_len <= max_frame_bytes, so the copy stays inside the
        // slot, which lies inside the mapped region (Layout::new).
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.base.add(o + SLOT_HEADER),
                out.as_mut_ptr(),
                info.data_len as usize,
            );
        }
        fence(Ordering::Acquire);
        let b2 = self.u64_at(o + s::SEQ_BEGIN).load(Ordering::Acquire);
        if b2 != seq {
            return None;
        }
        Some(info)
    }

    fn frame_ok(i: &FrameInfo, max: u64) -> bool {
        let Some(bpp) = bytes_per_pixel(i.format) else {
            return false;
        };
        i.width > 0
            && i.height > 0
            && (i.stride as u64) >= i.width as u64 * bpp as u64
            && (i.data_len as u64) <= max
            && (i.data_len as u64)
                >= i.stride as u64 * (i.height as u64 - 1) + i.width as u64 * bpp as u64
    }

    /// Copy the cursor state (and its shape into `shape` when its serial
    /// differs from `known_serial`).
    pub fn read_cursor(&self, known_serial: u32, shape: &mut Vec<u8>) -> Option<CursorInfo> {
        let co = self.layout.cursor_offset as usize;
        for _ in 0..4 {
            let b = self.u64_at(co + c::SEQ_BEGIN).load(Ordering::Acquire);
            let e = self.u64_at(co + c::SEQ_END).load(Ordering::Acquire);
            if b == 0 || b != e {
                continue;
            }
            let ci = CursorInfo {
                seq: b,
                x: self.u32_at(co + c::X).load(Ordering::Relaxed) as i32,
                y: self.u32_at(co + c::Y).load(Ordering::Relaxed) as i32,
                visible: self.u32_at(co + c::VISIBLE).load(Ordering::Relaxed) != 0,
                shape_serial: self.u32_at(co + c::SHAPE_SERIAL).load(Ordering::Relaxed),
                width: self
                    .u32_at(co + c::WIDTH)
                    .load(Ordering::Relaxed)
                    .min(CURSOR_MAX),
                height: self
                    .u32_at(co + c::HEIGHT)
                    .load(Ordering::Relaxed)
                    .min(CURSOR_MAX),
                hot_x: self.u32_at(co + c::HOT_X).load(Ordering::Relaxed),
                hot_y: self.u32_at(co + c::HOT_Y).load(Ordering::Relaxed),
            };
            if ci.shape_serial != known_serial {
                let n = (ci.width * ci.height * 4) as usize;
                shape.resize(n, 0);
                // SAFETY: width/height are clamped to CURSOR_MAX, so the
                // shape lies inside CURSOR_AREA.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        self.base.add(co + SLOT_HEADER),
                        shape.as_mut_ptr(),
                        n,
                    );
                }
            }
            fence(Ordering::Acquire);
            if self.u64_at(co + c::SEQ_BEGIN).load(Ordering::Acquire) == b {
                return Some(ci);
            }
        }
        None
    }

    /// Writer heartbeat counter (changes about once a second while the
    /// writer runs) and its state/info.
    pub fn writer_beat(&self) -> u64 {
        self.u64_at(h::WRITER_BEAT).load(Ordering::Acquire)
    }
    pub fn writer_state(&self) -> u32 {
        self.u32_at(h::WRITER_STATE).load(Ordering::Acquire)
    }
    pub fn writer_info(&self) -> String {
        let mut v = vec![0u8; h::WRITER_INFO_LEN];
        // SAFETY: inside the header.
        unsafe {
            std::ptr::copy_nonoverlapping(self.base.add(h::WRITER_INFO), v.as_mut_ptr(), v.len());
        }
        let end = v.iter().position(|b| *b == 0).unwrap_or(v.len());
        String::from_utf8_lossy(&v[..end]).into_owned()
    }

    // ---- writer side ----

    /// Starting point for [`Ring::reader_present`]: whatever clock stamp
    /// is in the ring now counts as old, so a reader that went away (its
    /// last stamp stays behind) is not taken for a live one. A live reader
    /// stamps again within its poll interval.
    pub fn reader_watch_start(&self) -> (u64, std::time::Instant) {
        let now = std::time::Instant::now();
        (
            self.reader_clock(),
            now.checked_sub(std::time::Duration::from_secs(3600))
                .unwrap_or(now),
        )
    }

    /// Is a reader watching? True when its clock stamp changed within
    /// `window` calls of this function spaced by the caller (the writer
    /// passes the previous stamp it saw and how long ago that changed).
    pub fn reader_present(
        &self,
        last_seen: &mut (u64, std::time::Instant),
        timeout: std::time::Duration,
    ) -> bool {
        let now = self.reader_clock();
        if now != 0 && now != last_seen.0 {
            *last_seen = (now, std::time::Instant::now());
            return true;
        }
        last_seen.0 != 0 && last_seen.1.elapsed() < timeout
    }

    pub fn set_writer_state(&self, state: u32, info: &str) {
        let mut v = [0u8; h::WRITER_INFO_LEN];
        let b = info.as_bytes();
        let n = b.len().min(h::WRITER_INFO_LEN - 1);
        v[..n].copy_from_slice(&b[..n]);
        // SAFETY: inside the header.
        unsafe {
            std::ptr::copy_nonoverlapping(v.as_ptr(), self.base.add(h::WRITER_INFO), v.len());
        }
        self.u32_at(h::WRITER_STATE).store(state, Ordering::Release);
    }

    pub fn writer_heartbeat(&self) {
        self.u64_at(h::WRITER_BEAT).fetch_add(1, Ordering::AcqRel);
    }

    /// Pick a slot for the next frame and mark it being written. The
    /// caller fills [`Ring::slot_pixels`] and then calls [`Ring::publish`].
    pub fn begin(&self) -> FrameSlot {
        let latest = self.u64_at(h::LATEST).load(Ordering::Acquire);
        let seq = (latest >> 8) + 1;
        let cur = if latest == 0 {
            u32::MAX
        } else {
            (latest & 0xff) as u32
        };
        let reading = self.u32_at(h::READER_SLOT).load(Ordering::SeqCst);
        let n = self.layout.slot_count;
        let start = if cur == u32::MAX { 0 } else { (cur + 1) % n };
        let mut slot = start;
        for k in 0..n {
            let cand = (start + k) % n;
            if cand != cur && cand != reading {
                slot = cand;
                break;
            }
        }
        let o = self.slot_base(slot);
        self.u64_at(o + s::SEQ_BEGIN).store(seq, Ordering::SeqCst);
        fence(Ordering::SeqCst);
        FrameSlot { slot, seq }
    }

    /// Writable pixel area of a slot being written.
    ///
    /// # Safety
    /// Only the single writer may call this, between `begin` and `publish`
    /// of the same slot; the returned slice must not outlive that.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn slot_pixels(&self, f: &FrameSlot) -> &mut [u8] {
        let o = self.slot_base(f.slot) + SLOT_HEADER;
        std::slice::from_raw_parts_mut(self.base.add(o), self.layout.max_frame_bytes() as usize)
    }

    /// Finish a frame written into `f`'s pixels and make it the newest.
    pub fn publish(&self, f: FrameSlot, info: &FrameInfo, publish_ns: u64) -> Result<u64, String> {
        let mut i = *info;
        i.seq = f.seq;
        if !Self::frame_ok(&i, self.layout.max_frame_bytes()) {
            return Err(format!(
                "frame {}x{} stride {} ({} bytes) does not fit a {}-byte slot",
                i.width,
                i.height,
                i.stride,
                i.data_len,
                self.layout.max_frame_bytes()
            ));
        }
        let o = self.slot_base(f.slot);
        self.u32_at(o + s::WIDTH).store(i.width, Ordering::Relaxed);
        self.u32_at(o + s::HEIGHT)
            .store(i.height, Ordering::Relaxed);
        self.u32_at(o + s::STRIDE)
            .store(i.stride, Ordering::Relaxed);
        self.u32_at(o + s::FORMAT)
            .store(i.format, Ordering::Relaxed);
        self.u32_at(o + s::DATA_LEN)
            .store(i.data_len, Ordering::Relaxed);
        self.u32_at(o + s::FLAGS).store(i.flags, Ordering::Relaxed);
        self.u64_at(o + s::CAPTURE_NS)
            .store(i.capture_ns, Ordering::Relaxed);
        self.u64_at(o + s::PUBLISH_NS)
            .store(publish_ns, Ordering::Relaxed);
        self.u64_at(o + s::READER_CLOCK_AT_PUBLISH)
            .store(self.reader_clock(), Ordering::Relaxed);
        self.u64_at(o + s::SEQ_END).store(f.seq, Ordering::Release);
        self.u64_at(h::LATEST)
            .store(f.seq << 8 | f.slot as u64, Ordering::Release);
        Ok(f.seq)
    }

    /// Convenience: copy a whole frame and publish it.
    pub fn write_frame(
        &self,
        info: &FrameInfo,
        data: &[u8],
        publish_ns: u64,
    ) -> Result<u64, String> {
        if data.len() as u64 > self.layout.max_frame_bytes() {
            return Err(format!(
                "frame of {} bytes does not fit a {}-byte slot",
                data.len(),
                self.layout.max_frame_bytes()
            ));
        }
        let f = self.begin();
        // SAFETY: single writer, between begin and publish.
        unsafe {
            self.slot_pixels(&f)[..data.len()].copy_from_slice(data);
        }
        let mut i = *info;
        i.data_len = data.len() as u32;
        self.publish(f, &i, publish_ns)
    }

    /// Update the cursor (shape only when `shape` is given).
    pub fn write_cursor(&self, ci: &CursorInfo, shape: Option<&[u8]>) {
        let co = self.layout.cursor_offset as usize;
        let seq = self.u64_at(co + c::SEQ_END).load(Ordering::Relaxed) + 1;
        self.u64_at(co + c::SEQ_BEGIN).store(seq, Ordering::SeqCst);
        fence(Ordering::SeqCst);
        self.u32_at(co + c::X).store(ci.x as u32, Ordering::Relaxed);
        self.u32_at(co + c::Y).store(ci.y as u32, Ordering::Relaxed);
        self.u32_at(co + c::VISIBLE)
            .store(ci.visible as u32, Ordering::Relaxed);
        if let Some(px) = shape {
            let w = ci.width.min(CURSOR_MAX);
            let hgt = ci.height.min(CURSOR_MAX);
            let n = ((w * hgt * 4) as usize).min(px.len());
            // SAFETY: n <= CURSOR_MAX^2*4, inside the cursor area.
            unsafe {
                std::ptr::copy_nonoverlapping(px.as_ptr(), self.base.add(co + SLOT_HEADER), n);
            }
            self.u32_at(co + c::WIDTH).store(w, Ordering::Relaxed);
            self.u32_at(co + c::HEIGHT).store(hgt, Ordering::Relaxed);
            self.u32_at(co + c::HOT_X)
                .store(ci.hot_x, Ordering::Relaxed);
            self.u32_at(co + c::HOT_Y)
                .store(ci.hot_y, Ordering::Relaxed);
            self.u32_at(co + c::SHAPE_SERIAL)
                .store(ci.shape_serial, Ordering::Relaxed);
        }
        self.u64_at(co + c::SEQ_END).store(seq, Ordering::Release);
    }

    // ---- helpers ----

    fn slot_base(&self, i: u32) -> usize {
        (self.layout.slot_offset + i as u64 * self.layout.slot_stride) as usize
    }

    fn u64_at(&self, off: usize) -> &AtomicU64 {
        assert!(off + 8 <= self.len && off.is_multiple_of(8));
        // SAFETY: in bounds, aligned (base is 8-aligned, offsets are
        // multiples of 8), and the region outlives `self`.
        unsafe { &*(self.base.add(off) as *const AtomicU64) }
    }

    fn u32_at(&self, off: usize) -> &AtomicU32 {
        assert!(off + 4 <= self.len && off.is_multiple_of(4));
        // SAFETY: as above.
        unsafe { &*(self.base.add(off) as *const AtomicU32) }
    }
}

/// A slot reserved by [`Ring::begin`].
#[derive(Debug)]
pub struct FrameSlot {
    pub slot: u32,
    pub seq: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Region(Vec<u64>);
    impl Region {
        fn new(bytes: usize) -> Self {
            Region(vec![0u64; bytes / 8])
        }
        fn ptr(&mut self) -> *mut u8 {
            self.0.as_mut_ptr() as *mut u8
        }
        fn len(&self) -> usize {
            self.0.len() * 8
        }
    }

    fn info(w: u32, h: u32) -> FrameInfo {
        FrameInfo {
            width: w,
            height: h,
            stride: w * 4,
            format: FORMAT_XRGB8888,
            data_len: w * h * 4,
            capture_ns: 5,
            ..Default::default()
        }
    }

    #[test]
    fn layout_sizes() {
        let l = Layout::new(64 << 20, 3).unwrap();
        assert_eq!(l.slot_offset as usize, HEADER_SIZE + CURSOR_AREA);
        assert_eq!(l.slot_stride % 4096, 0);
        assert!(l.slot_offset + 3 * l.slot_stride <= 64 << 20);
        assert!(l.fits(2560, 1440));
        assert!(!l.fits(3840, 2160));
        assert!(Layout::new(256 << 10, 3).is_err());
        assert!(Layout::new(64 << 20, 1).is_err());
        assert!(Layout::new(64 << 20, 9).is_err());
        let r = Layout::region_for(3840, 2160, 3);
        assert!(r.is_power_of_two());
        assert!(Layout::new(r, 3).unwrap().fits(3840, 2160));
        assert_eq!(Layout::region_for(3840, 2160, 3), 128 << 20);
    }

    #[test]
    fn header_bytes_match_init() {
        let len = 8usize << 20;
        let hb = Ring::header_bytes(len as u64, 3, 120).unwrap();
        let mut reg = Region::new(len);
        // SAFETY: test region.
        unsafe {
            std::ptr::copy_nonoverlapping(hb.as_ptr(), reg.ptr(), hb.len());
            let r = Ring::attach(reg.ptr(), reg.len()).unwrap();
            assert_eq!(r.layout(), Layout::new(len as u64, 3).unwrap());
            assert_eq!(r.want_fps(), 120);
            assert_eq!(r.latest_seq(), 0);
        }
    }

    #[test]
    fn write_read_round_trip_and_seq() {
        let mut reg = Region::new(8 << 20);
        let (p, l) = (reg.ptr(), reg.len());
        // SAFETY: test region outlives the rings.
        let (_node, w, r) = unsafe {
            (
                Ring::init(p, l, 3, 60).unwrap(),
                Ring::attach(p, l).unwrap(),
                Ring::attach(p, l).unwrap(),
            )
        };
        let mut out = Vec::new();
        assert!(r.read_latest(0, &mut out).is_none());
        r.reader_tick(1000, 0);
        let px: Vec<u8> = (0..64 * 64 * 4).map(|i| i as u8).collect();
        let s1 = w.write_frame(&info(64, 64), &px, 7).unwrap();
        assert_eq!(s1, 1);
        let fi = r.read_latest(0, &mut out).unwrap();
        assert_eq!(fi.seq, 1);
        assert_eq!((fi.width, fi.height), (64, 64));
        assert_eq!(fi.reader_clock_at_publish, 1000);
        assert_eq!(fi.publish_ns, 7);
        assert_eq!(out, px);
        assert!(r.read_latest(1, &mut out).is_none(), "nothing newer");
        let s2 = w.write_frame(&info(64, 64), &px, 8).unwrap();
        assert_eq!(s2, 2);
        assert_eq!(r.read_latest(1, &mut out).unwrap().seq, 2);
    }

    #[test]
    fn writer_avoids_latest_and_reader_slot() {
        let mut reg = Region::new(8 << 20);
        let (p, l) = (reg.ptr(), reg.len());
        let w = unsafe { Ring::init(p, l, 3, 60).unwrap() };
        let px = vec![1u8; 64 * 64 * 4];
        w.write_frame(&info(64, 64), &px, 1).unwrap(); // slot 0
                                                       // reader marks slot 0 (latest)
        w.u32_at(h::READER_SLOT).store(0, Ordering::SeqCst);
        let f = w.begin();
        assert_ne!(f.slot, 0);
        w.publish(f, &info(64, 64), 2).unwrap();
        let f = w.begin();
        assert_ne!(f.slot, 0, "reader still on 0");
        let latest_slot = (w.u64_at(h::LATEST).load(Ordering::Relaxed) & 0xff) as u32;
        assert_ne!(f.slot, latest_slot);
    }

    #[test]
    fn torn_slot_is_refused() {
        let mut reg = Region::new(8 << 20);
        let (p, l) = (reg.ptr(), reg.len());
        let w = unsafe { Ring::init(p, l, 3, 60).unwrap() };
        let px = vec![1u8; 64 * 64 * 4];
        w.write_frame(&info(64, 64), &px, 1).unwrap();
        // writer starts overwriting the latest slot (as if it lost a race)
        let o = w.slot_base(0);
        w.u64_at(o + s::SEQ_BEGIN).store(9, Ordering::SeqCst);
        let mut out = Vec::new();
        assert!(w.read_latest(0, &mut out).is_none());
    }

    #[test]
    fn hostile_sizes_are_refused() {
        let mut reg = Region::new(8 << 20);
        let (p, l) = (reg.ptr(), reg.len());
        let w = unsafe { Ring::init(p, l, 3, 60).unwrap() };
        let max = w.layout().max_frame_bytes() as u32;
        let mut bad = info(64, 64);
        bad.data_len = max + 4096;
        let f = w.begin();
        assert!(w.publish(f, &bad, 1).is_err());
        let mut bad = info(64, 64);
        bad.stride = 8;
        let f = w.begin();
        assert!(w.publish(f, &bad, 1).is_err());
        let mut bad = info(64, 64);
        bad.format = 7;
        let f = w.begin();
        assert!(w.publish(f, &bad, 1).is_err());
        // a guest that writes a lying slot header directly is caught by the reader
        let f = w.begin();
        w.publish(f, &info(64, 64), 1).unwrap();
        let latest = w.u64_at(h::LATEST).load(Ordering::Relaxed);
        let o = w.slot_base((latest & 0xff) as u32);
        w.u32_at(o + s::DATA_LEN).store(u32::MAX, Ordering::SeqCst);
        let mut out = Vec::new();
        assert!(w.read_latest(0, &mut out).is_none());
    }

    #[test]
    fn corrupt_header_is_refused() {
        let mut reg = Region::new(8 << 20);
        let (p, l) = (reg.ptr(), reg.len());
        assert!(unsafe { Ring::attach(p, l) }.is_err(), "no magic");
        let w = unsafe { Ring::init(p, l, 3, 60).unwrap() };
        w.u64_at(h::SLOT_STRIDE).store(1 << 40, Ordering::SeqCst);
        assert!(unsafe { Ring::attach(p, l) }.is_err());
        let w = unsafe { Ring::init(p, l, 3, 60).unwrap() };
        w.u64_at(h::REGION_SIZE).store(1 << 40, Ordering::SeqCst);
        assert!(unsafe { Ring::attach(p, l) }.is_err());
    }

    #[test]
    fn cursor_round_trip() {
        let mut reg = Region::new(8 << 20);
        let (p, l) = (reg.ptr(), reg.len());
        let w = unsafe { Ring::init(p, l, 3, 60).unwrap() };
        let mut shape = Vec::new();
        assert!(w.read_cursor(0, &mut shape).is_none());
        let ci = CursorInfo {
            x: 10,
            y: -3,
            visible: true,
            shape_serial: 1,
            width: 2,
            height: 2,
            hot_x: 1,
            hot_y: 0,
            ..Default::default()
        };
        w.write_cursor(&ci, Some(&[9u8; 16]));
        let got = w.read_cursor(0, &mut shape).unwrap();
        assert_eq!((got.x, got.y, got.visible, got.width), (10, -3, true, 2));
        assert_eq!(shape, vec![9u8; 16]);
        w.write_cursor(
            &CursorInfo {
                x: 11,
                ..ci.clone()
            },
            None,
        );
        let got = w.read_cursor(1, &mut shape).unwrap();
        assert_eq!((got.x, got.shape_serial), (11, 1));
    }

    #[test]
    fn concurrent_writer_reader_never_tears() {
        let mut reg = Region::new(4 << 20);
        let (p, l) = (reg.ptr() as usize, reg.len());
        let w = unsafe { Ring::init(p as *mut u8, l, 3, 60).unwrap() };
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let st = stop.clone();
        let reader = std::thread::spawn(move || {
            let r = unsafe { Ring::attach(p as *mut u8, l).unwrap() };
            let mut out = Vec::new();
            let mut last = 0;
            let mut n = 0;
            while !st.load(Ordering::Relaxed) {
                if let Some(fi) = r.read_latest(last, &mut out) {
                    assert!(fi.seq > last);
                    last = fi.seq;
                    let v = (fi.seq & 0xff) as u8;
                    assert!(out.iter().all(|b| *b == v), "torn frame {}", fi.seq);
                    n += 1;
                }
            }
            n
        });
        let mut buf = vec![0u8; 128 * 128 * 4];
        for i in 1..=3000u64 {
            buf.fill((i & 0xff) as u8);
            w.write_frame(&info(128, 128), &buf, i).unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        let n = reader.join().unwrap();
        assert!(n > 0);
        drop(reg);
    }
    #[test]
    fn a_stale_reader_stamp_is_not_a_reader() {
        let len = 4 << 20;
        let mut mem = vec![0u64; len / 8];
        let r = unsafe { Ring::init(mem.as_mut_ptr() as *mut u8, len, 2, 60) }.unwrap();
        let t = std::time::Duration::from_millis(500);
        r.reader_tick(12345, 60);
        // the reader left; a writer (re)starting now must not see it
        let mut seen = r.reader_watch_start();
        assert!(!r.reader_present(&mut seen, t));
        // it comes back
        r.reader_tick(12346, 60);
        assert!(r.reader_present(&mut seen, t));
        assert!(r.reader_present(&mut seen, t), "within the timeout");
    }
}
