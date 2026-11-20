//! H.264 decoding through the system's FFmpeg libraries (libavcodec,
//! libavutil; LGPL-2.1-or-later or GPL, compatible with Horcrux's GPL-3.0),
//! loaded at run time like [`crate::x264`]: the Horcrux desktop client
//! builds without FFmpeg headers and uses whatever FFmpeg the system has.
//!
//! Options are set by name (`av_opt_set`), so `AVCodecContext`'s layout
//! never matters. The leading fields of `AVPacket` (buf, pts, dts, data,
//! size) and `AVFrame` (data[8], linesize[8], extended_data, width, height,
//! nb_samples, format) are read directly: they have kept their places
//! across FFmpeg 4 to 8, and are checked against what the library fills in
//! (`av_new_packet` writes the size; a decoded frame must have sane planes).
use std::ffi::{c_char, c_int, c_void, CStr, CString};

type FindDecoder = unsafe extern "C" fn(*const c_char) -> *const c_void;
type AllocContext = unsafe extern "C" fn(*const c_void) -> *mut c_void;
type Open2 = unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> c_int;
type FreeContext = unsafe extern "C" fn(*mut *mut c_void);
type SendPacket = unsafe extern "C" fn(*mut c_void, *const c_void) -> c_int;
type ReceiveFrame = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;
type PacketAlloc = unsafe extern "C" fn() -> *mut c_void;
type PacketFree = unsafe extern "C" fn(*mut *mut c_void);
type NewPacket = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type PacketUnref = unsafe extern "C" fn(*mut c_void);
type FrameAlloc = unsafe extern "C" fn() -> *mut c_void;
type FrameFree = unsafe extern "C" fn(*mut *mut c_void);
type FrameUnref = unsafe extern "C" fn(*mut c_void);
type OptSet = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, c_int) -> c_int;

const AV_OPT_SEARCH_CHILDREN: c_int = 1;
const AVERROR_EAGAIN: c_int = -11;
/// AV_PIX_FMT_YUV420P and AV_PIX_FMT_YUVJ420P.
const YUV420P: c_int = 0;
const YUVJ420P: c_int = 12;

const PKT_DATA: usize = 24;
const PKT_SIZE: usize = 32;
const FRAME_LINESIZE: usize = 64;
const FRAME_WIDTH: usize = 104;
const FRAME_HEIGHT: usize = 108;
const FRAME_FORMAT: usize = 116;

struct Lib {
    _codec: *mut c_void,
    _util: *mut c_void,
    find_decoder: FindDecoder,
    alloc_context: AllocContext,
    open2: Open2,
    free_context: FreeContext,
    send_packet: SendPacket,
    receive_frame: ReceiveFrame,
    packet_alloc: PacketAlloc,
    packet_free: PacketFree,
    new_packet: NewPacket,
    packet_unref: PacketUnref,
    frame_alloc: FrameAlloc,
    frame_free: FrameFree,
    frame_unref: FrameUnref,
    opt_set: OptSet,
}

unsafe fn open_first(names: &[String]) -> Result<(*mut c_void, String), String> {
    let mut errs = Vec::new();
    for n in names {
        let c = CString::new(n.as_str()).map_err(|_| "NUL in a library name".to_string())?;
        let h = libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if !h.is_null() {
            return Ok((h, n.clone()));
        }
        let e = libc::dlerror();
        errs.push(if e.is_null() {
            format!("{n}: dlopen failed")
        } else {
            CStr::from_ptr(e).to_string_lossy().into_owned()
        });
    }
    Err(if errs.is_empty() {
        "no library names configured".into()
    } else {
        errs.join("; ")
    })
}

unsafe fn sym<T: Copy>(h: *mut c_void, name: &str) -> Result<T, String> {
    let c = CString::new(name).map_err(|e| e.to_string())?;
    let p = libc::dlsym(h, c.as_ptr());
    if p.is_null() {
        Err(format!("missing symbol {name}"))
    } else {
        Ok(std::mem::transmute_copy::<*mut c_void, T>(&p))
    }
}

/// A decoded picture in planar 4:2:0 (borrowed from the decoder until the
/// next call).
pub struct Picture<'a> {
    pub width: usize,
    pub height: usize,
    pub y: &'a [u8],
    pub y_stride: usize,
    pub u: &'a [u8],
    pub u_stride: usize,
    pub v: &'a [u8],
    pub v_stride: usize,
}

/// An H.264 decoder (low delay: one thread, no frame reordering wait).
pub struct Decoder {
    lib: Lib,
    ctx: *mut c_void,
    pkt: *mut c_void,
    frame: *mut c_void,
    pub libraries: (String, String),
}

unsafe impl Send for Decoder {}

impl Decoder {
    /// `avcodec` / `avutil`: library names or paths tried in order.
    pub fn open(avcodec: &[String], avutil: &[String]) -> Result<Decoder, String> {
        unsafe {
            let (util, un) = open_first(avutil)?;
            let (codec, cn) = open_first(avcodec)?;
            let lib = Lib {
                _codec: codec,
                _util: util,
                find_decoder: sym(codec, "avcodec_find_decoder_by_name")?,
                alloc_context: sym(codec, "avcodec_alloc_context3")?,
                open2: sym(codec, "avcodec_open2")?,
                free_context: sym(codec, "avcodec_free_context")?,
                send_packet: sym(codec, "avcodec_send_packet")?,
                receive_frame: sym(codec, "avcodec_receive_frame")?,
                packet_alloc: sym(codec, "av_packet_alloc")?,
                packet_free: sym(codec, "av_packet_free")?,
                new_packet: sym(codec, "av_new_packet")?,
                packet_unref: sym(codec, "av_packet_unref")?,
                frame_alloc: sym(util, "av_frame_alloc")?,
                frame_free: sym(util, "av_frame_free")?,
                frame_unref: sym(util, "av_frame_unref")?,
                opt_set: sym(util, "av_opt_set")?,
            };
            let name = CString::new("h264").expect("static");
            let codec_p = (lib.find_decoder)(name.as_ptr());
            if codec_p.is_null() {
                return Err(format!("{cn} has no h264 decoder"));
            }
            let ctx = (lib.alloc_context)(codec_p);
            if ctx.is_null() {
                return Err("avcodec_alloc_context3 failed".into());
            }
            let mut d = Decoder {
                lib,
                ctx,
                pkt: std::ptr::null_mut(),
                frame: std::ptr::null_mut(),
                libraries: (cn, un),
            };
            for (k, v) in [
                ("threads", "1"),
                ("flags", "+low_delay"),
                ("flags2", "+fast"),
            ] {
                let (ck, cv) = (
                    CString::new(k).expect("static"),
                    CString::new(v).expect("static"),
                );
                (d.lib.opt_set)(d.ctx, ck.as_ptr(), cv.as_ptr(), AV_OPT_SEARCH_CHILDREN);
            }
            if (d.lib.open2)(d.ctx, codec_p, std::ptr::null_mut()) < 0 {
                return Err("avcodec_open2 failed".into());
            }
            d.pkt = (d.lib.packet_alloc)();
            d.frame = (d.lib.frame_alloc)();
            if d.pkt.is_null() || d.frame.is_null() {
                return Err("out of memory".into());
            }
            Ok(d)
        }
    }

    /// Decode one access unit (Annex-B). Calls `show` with the newest
    /// picture it produced, if any. Errors are per frame (a lost reference:
    /// ask the sender for a keyframe).
    pub fn decode(&mut self, au: &[u8], show: impl FnOnce(Picture)) -> Result<bool, String> {
        unsafe {
            if (self.lib.new_packet)(self.pkt, au.len() as c_int) < 0 {
                return Err("av_new_packet failed".into());
            }
            let base = self.pkt as *mut u8;
            let data = *(base.add(PKT_DATA) as *const *mut u8);
            let size = *(base.add(PKT_SIZE) as *const c_int);
            if data.is_null() || size as usize != au.len() {
                (self.lib.packet_unref)(self.pkt);
                return Err("unexpected AVPacket layout".into());
            }
            std::ptr::copy_nonoverlapping(au.as_ptr(), data, au.len());
            let r = (self.lib.send_packet)(self.ctx, self.pkt);
            (self.lib.packet_unref)(self.pkt);
            if r < 0 && r != AVERROR_EAGAIN {
                return Err(format!("decode error {r}"));
            }
            let mut shown = false;
            let mut show = Some(show);
            loop {
                let r = (self.lib.receive_frame)(self.ctx, self.frame);
                if r < 0 {
                    break;
                }
                let f = self.frame as *const u8;
                let w = *(f.add(FRAME_WIDTH) as *const c_int);
                let h = *(f.add(FRAME_HEIGHT) as *const c_int);
                let fmt = *(f.add(FRAME_FORMAT) as *const c_int);
                let planes = f as *const *const u8;
                let ls = f.add(FRAME_LINESIZE) as *const c_int;
                let ok = w > 0
                    && h > 0
                    && w <= 16384
                    && h <= 16384
                    && (fmt == YUV420P || fmt == YUVJ420P)
                    && (0..3).all(|i| !(*planes.add(i)).is_null() && *ls.add(i) > 0)
                    && *ls >= w;
                if !ok {
                    (self.lib.frame_unref)(self.frame);
                    return Err(format!("unexpected decoded frame ({w}x{h}, format {fmt})"));
                }
                if let Some(show) = show.take() {
                    let (w, h) = (w as usize, h as usize);
                    let (ys, us, vs) = (*ls as usize, *ls.add(1) as usize, *ls.add(2) as usize);
                    let ch = h.div_ceil(2);
                    show(Picture {
                        width: w,
                        height: h,
                        y: std::slice::from_raw_parts(*planes, ys * h),
                        y_stride: ys,
                        u: std::slice::from_raw_parts(*planes.add(1), us * ch),
                        u_stride: us,
                        v: std::slice::from_raw_parts(*planes.add(2), vs * ch),
                        v_stride: vs,
                    });
                    shown = true;
                }
                (self.lib.frame_unref)(self.frame);
            }
            Ok(shown)
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            if !self.frame.is_null() {
                (self.lib.frame_free)(&mut self.frame);
            }
            if !self.pkt.is_null() {
                (self.lib.packet_free)(&mut self.pkt);
            }
            if !self.ctx.is_null() {
                (self.lib.free_context)(&mut self.ctx);
            }
        }
    }
}

/// Library names tried by default (newest FFmpeg first).
pub fn default_avcodec() -> Vec<String> {
    [
        "libavcodec.so.62",
        "libavcodec.so.61",
        "libavcodec.so.60",
        "libavcodec.so.59",
        "libavcodec.so.58",
        "libavcodec.so",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
pub fn default_avutil() -> Vec<String> {
    [
        "libavutil.so.60",
        "libavutil.so.59",
        "libavutil.so.58",
        "libavutil.so.57",
        "libavutil.so.56",
        "libavutil.so",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode with libx264, decode with libavcodec, compare (where both
    /// are installed; CI's Test image has them).
    #[test]
    fn decodes_what_x264_encodes() {
        let Ok(lib) = crate::x264::Library::load(&[
            "libx264.so.165".into(),
            "libx264.so.164".into(),
            "libx264.so".into(),
        ]) else {
            eprintln!("skip: no libx264");
            return;
        };
        let mut dec = match Decoder::open(&default_avcodec(), &default_avutil()) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("skip: no libavcodec ({e})");
                return;
            }
        };
        let (w, h) = (64u32, 48u32);
        let mut enc = crate::x264::Encoder::open(
            lib,
            crate::x264::Config {
                width: w,
                height: h,
                fps: 30,
                keyint: 30,
                preset: "ultrafast".into(),
                tune: "zerolatency".into(),
                profile: "baseline".into(),
                rate: crate::x264::Rate {
                    bitrate_kbps: 2000,
                    maxrate_kbps: 2000,
                    bufsize_kbit: 400,
                },
            },
        )
        .unwrap();
        for shade in [40u8, 200] {
            let au = enc
                .encode(
                    |y, ys, u, us, v, vs| {
                        for r in 0..h as usize {
                            y[r * ys..r * ys + w as usize].fill(shade);
                        }
                        for r in 0..(h / 2) as usize {
                            u[r * us..r * us + (w / 2) as usize].fill(128);
                            v[r * vs..r * vs + (w / 2) as usize].fill(128);
                        }
                        true
                    },
                    false,
                )
                .unwrap();
            let mut got = None;
            assert!(dec
                .decode(&au, |p| got =
                    Some((p.width, p.height, p.y[p.y_stride * 10 + 10])))
                .unwrap());
            let (gw, gh, px) = got.unwrap();
            assert_eq!((gw, gh), (64, 48));
            assert!((px as i32 - shade as i32).abs() <= 3, "{px} vs {shade}");
        }
        // garbage is an error or nothing, never a crash
        let _ = dec.decode(&[0, 0, 0, 1, 0x65, 0xff, 0x00], |_| {});
    }

    #[test]
    fn missing_libraries_are_errors() {
        assert!(Decoder::open(&["libnope-hx.so".into()], &["libnope-hx.so".into()]).is_err());
    }
}
