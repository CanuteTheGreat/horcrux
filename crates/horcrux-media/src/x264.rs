//! Software H.264 through the system's libx264, loaded at run time.
//!
//! libx264 is GPL-2.0-or-later (compatible with Horcrux's GPL-3.0); it is
//! not linked at build time: the node's packaged library is opened with
//! `dlopen` from a configurable list of names, so a node without it keeps
//! the FFmpeg program encoder and builds need no x264 headers.
//!
//! Only the stable parts of x264's ABI are touched directly: the leading
//! fields of `x264_picture_t` (type, pts, image planes), of `x264_param_t`
//! (picture size, which has no parameter name) and `x264_nal_t`. Every
//! other encoder parameter is set by name through `x264_param_parse` on an
//! opaque, generously sized parameter block, so the rest of
//! `x264_param_t` (which changes between x264 builds) never matters. The
//! layout assumptions are checked against what the library itself fills in
//! (`x264_picture_alloc`, the NAL sizes adding up to the returned frame
//! size) and the encoder refuses to run when they don't hold.
//!
//! Why in-process: link adaptation changes the target bitrate often. The
//! FFmpeg program can't change it in-stream, so each change cost a restart
//! and a keyframe; `x264_encoder_reconfig` changes rate and VBV between two
//! frames, and an IDR can be asked for on any frame.
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::Arc;

type ParamDefaultPreset = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
type ParamApplyProfile = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
type ParamParse = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
type ParamCleanup = unsafe extern "C" fn(*mut c_void);
type EncoderOpen = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type EncoderReconfig = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;
type EncoderEncode =
    unsafe extern "C" fn(*mut c_void, *mut *mut Nal, *mut c_int, *mut c_void, *mut c_void) -> c_int;
type EncoderClose = unsafe extern "C" fn(*mut c_void);
type PictureAlloc = unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int) -> c_int;
type PictureClean = unsafe extern "C" fn(*mut c_void);
type PictureInit = unsafe extern "C" fn(*mut c_void);

/// `x264_nal_t` (unchanged since x264 build 76).
#[repr(C)]
pub struct Nal {
    i_ref_idc: c_int,
    i_type: c_int,
    b_long_startcode: c_int,
    i_first_mb: c_int,
    i_last_mb: c_int,
    i_payload: c_int,
    p_payload: *mut u8,
    i_padding: c_int,
}

const X264_CSP_I420: c_int = 0x0002;
const X264_TYPE_AUTO: c_int = 0x0000;
const X264_TYPE_IDR: c_int = 0x0001;
/// Offsets inside `x264_picture_t`: i_type 0, i_pts 16, img 40 (i_csp,
/// i_plane, i_stride[4], plane[4]).
const PIC_PTS: usize = 16;
const PIC_IMG: usize = 40;
/// Opaque buffers, far larger than any x264 build's structs (x264_param_t
/// is about 1 KiB, x264_picture_t a few hundred bytes).
const PARAM_WORDS: usize = 4096;
const PICTURE_WORDS: usize = 512;

/// The loaded library.
pub struct Library {
    _handle: *mut c_void,
    pub name: String,
    pub build: u32,
    default_preset: ParamDefaultPreset,
    apply_profile: ParamApplyProfile,
    parse: ParamParse,
    cleanup: Option<ParamCleanup>,
    open: EncoderOpen,
    reconfig: EncoderReconfig,
    encode: EncoderEncode,
    close: EncoderClose,
    pic_alloc: PictureAlloc,
    pic_clean: PictureClean,
    pic_init: PictureInit,
}

// The handle and function pointers are process-global and thread-safe to
// call from any thread (each encoder is used by one thread at a time).
unsafe impl Send for Library {}
unsafe impl Sync for Library {}

unsafe fn sym<T: Copy>(h: *mut c_void, name: &str) -> Option<T> {
    let c = CString::new(name).ok()?;
    let p = libc::dlsym(h, c.as_ptr());
    if p.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy::<*mut c_void, T>(&p))
    }
}

impl Library {
    /// Open the first library of `names` (sonames or paths) that is a usable
    /// libx264.
    pub fn load(names: &[String]) -> Result<Arc<Library>, String> {
        let mut errors = Vec::new();
        for n in names {
            match Self::load_one(n) {
                Ok(l) => return Ok(Arc::new(l)),
                Err(e) => errors.push(format!("{n}: {e}")),
            }
        }
        if errors.is_empty() {
            Err("no libx264 names configured".into())
        } else {
            Err(errors.join("; "))
        }
    }

    fn load_one(name: &str) -> Result<Library, String> {
        let c = CString::new(name).map_err(|_| "name contains NUL".to_string())?;
        unsafe {
            let h = libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            if h.is_null() {
                let e = libc::dlerror();
                let msg = if e.is_null() {
                    "dlopen failed".to_string()
                } else {
                    CStr::from_ptr(e).to_string_lossy().into_owned()
                };
                return Err(msg);
            }
            // x264_encoder_open is exported with the API build number
            // appended (x264_encoder_open_164): it names the build.
            let mut open = None;
            let mut build = 0;
            for b in (100..=400u32).rev() {
                if let Some(f) = sym::<EncoderOpen>(h, &format!("x264_encoder_open_{b}")) {
                    open = Some(f);
                    build = b;
                    break;
                }
            }
            let missing = |what: &str| format!("{name} has no {what} (not libx264?)");
            let lib = Library {
                _handle: h,
                name: name.to_string(),
                build,
                open: open.ok_or_else(|| missing("x264_encoder_open_<build>"))?,
                default_preset: sym(h, "x264_param_default_preset")
                    .ok_or_else(|| missing("x264_param_default_preset"))?,
                apply_profile: sym(h, "x264_param_apply_profile")
                    .ok_or_else(|| missing("x264_param_apply_profile"))?,
                parse: sym(h, "x264_param_parse").ok_or_else(|| missing("x264_param_parse"))?,
                cleanup: sym(h, "x264_param_cleanup"),
                reconfig: sym(h, "x264_encoder_reconfig")
                    .ok_or_else(|| missing("x264_encoder_reconfig"))?,
                encode: sym(h, "x264_encoder_encode")
                    .ok_or_else(|| missing("x264_encoder_encode"))?,
                close: sym(h, "x264_encoder_close").ok_or_else(|| missing("x264_encoder_close"))?,
                pic_alloc: sym(h, "x264_picture_alloc")
                    .ok_or_else(|| missing("x264_picture_alloc"))?,
                pic_clean: sym(h, "x264_picture_clean")
                    .ok_or_else(|| missing("x264_picture_clean"))?,
                pic_init: sym(h, "x264_picture_init")
                    .ok_or_else(|| missing("x264_picture_init"))?,
            };
            // Builds before 148 predate the parameter names used here.
            if lib.build < 148 {
                return Err(format!(
                    "{name}: x264 build {} is too old (need 148+)",
                    lib.build
                ));
            }
            Ok(lib)
        }
    }
}

/// What an encoder is opened with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub width: u32,
    pub height: u32,
    /// Frame rate the rate control budgets for.
    pub fps: u32,
    pub keyint: u32,
    pub preset: String,
    pub tune: String,
    pub profile: String,
    pub rate: Rate,
}

/// Rate control (kbit/s and kbit): ABR with a VBV, as low-delay streaming
/// needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    pub bitrate_kbps: u32,
    pub maxrate_kbps: u32,
    pub bufsize_kbit: u32,
}

/// An open encoder; one thread uses it at a time.
pub struct Encoder {
    lib: Arc<Library>,
    enc: *mut c_void,
    param: Box<[u64; PARAM_WORDS]>,
    pic: Box<[u64; PICTURE_WORDS]>,
    out: Box<[u64; PICTURE_WORDS]>,
    pub config: Config,
    frames: i64,
    planes: [(*mut u8, usize); 3],
}

unsafe impl Send for Encoder {}

fn cstr(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| format!("{s:?} contains NUL"))
}

impl Encoder {
    pub fn open(lib: Arc<Library>, config: Config) -> Result<Encoder, String> {
        if config.width < 16
            || config.height < 16
            || config.width % 2 == 1
            || config.height % 2 == 1
        {
            return Err(format!("bad size {}x{}", config.width, config.height));
        }
        let mut param = Box::new([0u64; PARAM_WORDS]);
        let pp = param.as_mut_ptr() as *mut c_void;
        let preset = cstr(&config.preset)?;
        let tune = cstr(&config.tune)?;
        unsafe {
            if (lib.default_preset)(
                pp,
                preset.as_ptr(),
                if config.tune.is_empty() {
                    std::ptr::null()
                } else {
                    tune.as_ptr()
                },
            ) != 0
            {
                return Err(format!(
                    "x264 refused preset {:?} tune {:?}",
                    config.preset, config.tune
                ));
            }
        }
        let mut e = Encoder {
            lib: lib.clone(),
            enc: std::ptr::null_mut(),
            param,
            pic: Box::new([0u64; PICTURE_WORDS]),
            out: Box::new([0u64; PICTURE_WORDS]),
            config: config.clone(),
            frames: 0,
            planes: [(std::ptr::null_mut(), 0); 3],
        };
        let fps = config.fps.max(1).to_string();
        let keyint = config.keyint.max(1).to_string();
        e.set_picture(config.width, config.height)?;
        for (k, v) in [
            ("fps", fps.as_str()),
            ("force-cfr", "1"),
            ("keyint", keyint.as_str()),
            ("bframes", "0"),
            ("annexb", "1"),
            ("repeat-headers", "1"),
            ("aud", "1"),
        ] {
            e.set(k, v)?;
        }
        // quiet (x264 logs to stderr); not fatal on builds without the name
        let _ = e.set("log", "-1");
        e.set_rate_params(config.rate)?;
        let profile = cstr(&config.profile)?;
        unsafe {
            if (lib.apply_profile)(e.param.as_mut_ptr() as *mut c_void, profile.as_ptr()) != 0 {
                return Err(format!("x264 refused profile {:?}", config.profile));
            }
            e.enc = (lib.open)(e.param.as_mut_ptr() as *mut c_void);
            if e.enc.is_null() {
                return Err("x264_encoder_open failed".into());
            }
            let pic = e.pic.as_mut_ptr() as *mut c_void;
            (lib.pic_init)(pic);
            if (lib.pic_alloc)(
                pic,
                X264_CSP_I420,
                config.width as c_int,
                config.height as c_int,
            ) != 0
            {
                return Err("x264_picture_alloc failed".into());
            }
            // Check the layout this module assumes against what x264 filled in.
            let base = pic as *const u8;
            let csp = *(base.add(PIC_IMG) as *const c_int);
            let nplanes = *(base.add(PIC_IMG + 4) as *const c_int);
            let strides = base.add(PIC_IMG + 8) as *const c_int;
            let ptrs = base.add(PIC_IMG + 24) as *const *mut u8;
            if csp != X264_CSP_I420 || nplanes != 3 {
                (lib.pic_clean)(pic);
                return Err(format!(
                    "unexpected x264_picture_t layout (csp {csp:#x}, planes {nplanes})"
                ));
            }
            // validate all three before keeping any: Drop cleans the picture
            // once planes[0] is set, so an error here must not have set it
            let mut planes = [(std::ptr::null_mut(), 0); 3];
            for (i, plane) in planes.iter_mut().enumerate() {
                let s = *strides.add(i) as usize;
                let p = *ptrs.add(i);
                let w = if i == 0 {
                    config.width
                } else {
                    config.width / 2
                } as usize;
                if p.is_null() || s < w {
                    (lib.pic_clean)(pic);
                    return Err(format!("unexpected x264 plane {i} (stride {s})"));
                }
                *plane = (p, s);
            }
            e.planes = planes;
        }
        Ok(e)
    }

    fn word(&self, off: usize) -> c_int {
        unsafe { *((self.param.as_ptr() as *const u8).add(off) as *const c_int) }
    }

    /// Picture size and colour space have no `x264_param_parse` names: they
    /// are the leading `i_width`, `i_height`, `i_csp` fields of
    /// `x264_param_t` (after cpu, i_threads, i_lookahead_threads,
    /// b_sliced_threads, b_deterministic, b_cpu_independent,
    /// i_sync_lookahead). The offsets are proven first with fields that
    /// can be set by name (threads, sync-lookahead) and the defaults x264
    /// wrote (csp I420, size 0).
    fn set_picture(&mut self, width: u32, height: u32) -> Result<(), String> {
        self.set("threads", "7")?;
        self.set("sync-lookahead", "5")?;
        let ok = self.word(4) == 7
            && self.word(24) == 5
            && self.word(28) == 0
            && self.word(32) == 0
            && self.word(36) == X264_CSP_I420;
        self.set("threads", "auto")?;
        self.set("sync-lookahead", "0")?;
        if !ok {
            return Err(format!(
                "unexpected x264_param_t layout (build {}): not using the library",
                self.lib.build
            ));
        }
        unsafe {
            let base = self.param.as_mut_ptr() as *mut u8;
            *(base.add(28) as *mut c_int) = width as c_int;
            *(base.add(32) as *mut c_int) = height as c_int;
        }
        Ok(())
    }

    fn set(&mut self, k: &str, v: &str) -> Result<(), String> {
        let (ck, cv) = (cstr(k)?, cstr(v)?);
        let r = unsafe {
            (self.lib.parse)(
                self.param.as_mut_ptr() as *mut c_void,
                ck.as_ptr(),
                cv.as_ptr(),
            )
        };
        if r != 0 {
            return Err(format!("x264 refused {k}={v} ({r})"));
        }
        Ok(())
    }

    fn set_rate_params(&mut self, r: Rate) -> Result<(), String> {
        self.set("bitrate", &r.bitrate_kbps.max(1).to_string())?;
        self.set("vbv-maxrate", &r.maxrate_kbps.max(1).to_string())?;
        self.set("vbv-bufsize", &r.bufsize_kbit.max(1).to_string())
    }

    /// Change rate control of the running encoder; applies from the next
    /// frame, no keyframe.
    pub fn set_rate(&mut self, r: Rate) -> Result<(), String> {
        if r == self.config.rate {
            return Ok(());
        }
        self.set_rate_params(r)?;
        let rc = unsafe { (self.lib.reconfig)(self.enc, self.param.as_mut_ptr() as *mut c_void) };
        if rc < 0 {
            return Err(format!("x264_encoder_reconfig failed ({rc})"));
        }
        self.config.rate = r;
        Ok(())
    }

    /// Fill the input picture's planes (Y, U, V as (pointer, stride)) and
    /// encode it. Returns the Annex-B access unit (may be empty when the
    /// encoder produced nothing for this frame).
    pub fn encode(
        &mut self,
        fill: impl FnOnce(&mut [u8], usize, &mut [u8], usize, &mut [u8], usize) -> bool,
        keyframe: bool,
    ) -> Result<Vec<u8>, String> {
        let (w, h) = (self.config.width as usize, self.config.height as usize);
        let ok = unsafe {
            let [(py, sy), (pu, su), (pv, sv)] = self.planes;
            let y = std::slice::from_raw_parts_mut(py, sy * h);
            let u = std::slice::from_raw_parts_mut(pu, su * (h / 2));
            let v = std::slice::from_raw_parts_mut(pv, sv * (h / 2));
            let _ = w;
            fill(y, sy, u, su, v, sv)
        };
        if !ok {
            return Err("frame conversion failed".into());
        }
        unsafe {
            let base = self.pic.as_mut_ptr() as *mut u8;
            *(base as *mut c_int) = if keyframe {
                X264_TYPE_IDR
            } else {
                X264_TYPE_AUTO
            };
            *(base.add(PIC_PTS) as *mut i64) = self.frames;
            self.frames += 1;
            let mut nals: *mut Nal = std::ptr::null_mut();
            let mut n: c_int = 0;
            let size = (self.lib.encode)(
                self.enc,
                &mut nals,
                &mut n,
                self.pic.as_mut_ptr() as *mut c_void,
                self.out.as_mut_ptr() as *mut c_void,
            );
            *(base as *mut c_int) = X264_TYPE_AUTO;
            if size < 0 {
                return Err(format!("x264_encoder_encode failed ({size})"));
            }
            if size == 0 || n <= 0 || nals.is_null() {
                return Ok(Vec::new());
            }
            // x264 guarantees the NAL payloads are sequential in memory.
            let list = std::slice::from_raw_parts(nals, n as usize);
            let total: i64 = list.iter().map(|x| x.i_payload as i64).sum();
            if total != size as i64 || list[0].p_payload.is_null() {
                return Err(format!(
                    "unexpected x264_nal_t layout ({total} bytes in NALs, frame {size})"
                ));
            }
            Ok(std::slice::from_raw_parts(list[0].p_payload, size as usize).to_vec())
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            if !self.planes[0].0.is_null() {
                (self.lib.pic_clean)(self.pic.as_mut_ptr() as *mut c_void);
            }
            if !self.enc.is_null() {
                (self.lib.close)(self.enc);
            }
            if let Some(c) = self.lib.cleanup {
                c(self.param.as_mut_ptr() as *mut c_void);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        [
            "libx264.so.165",
            "libx264.so.164",
            "libx264.so.163",
            "libx264.so",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn missing_library_is_an_error_not_a_crash() {
        let e = Library::load(&["libdoes-not-exist-hx.so".into()])
            .err()
            .unwrap();
        assert!(e.contains("libdoes-not-exist-hx.so"), "{e}");
        assert!(Library::load(&[]).is_err());
        // a real library that is not x264
        let e = Library::load(&["libc.so.6".into()]).err().unwrap();
        assert!(e.contains("not libx264"), "{e}");
    }

    /// Runs where libx264 is installed (CI's Test image has it with
    /// FFmpeg); prints a skip otherwise.
    #[test]
    fn encodes_reconfigures_and_forces_idr() {
        let lib = match Library::load(&names()) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("skip: no libx264 ({e})");
                return;
            }
        };
        let (w, h) = (320u32, 240u32);
        let cfg = Config {
            width: w,
            height: h,
            fps: 30,
            keyint: 300,
            preset: "ultrafast".into(),
            tune: "zerolatency".into(),
            profile: "baseline".into(),
            rate: Rate {
                bitrate_kbps: 1000,
                maxrate_kbps: 1000,
                bufsize_kbit: 200,
            },
        };
        let mut e = Encoder::open(lib, cfg).expect("open");
        let mut sizes = Vec::new();
        let mut idr = Vec::new();
        for i in 0..40u32 {
            if i == 20 {
                e.set_rate(Rate {
                    bitrate_kbps: 200,
                    maxrate_kbps: 200,
                    bufsize_kbit: 40,
                })
                .expect("reconfig");
            }
            let shade = (i * 5) as u8;
            let out = e
                .encode(
                    |y, sy, u, su, v, sv| {
                        for r in 0..h as usize {
                            for c in 0..w as usize {
                                y[r * sy + c] = shade.wrapping_add((c ^ r) as u8);
                            }
                        }
                        for r in 0..(h / 2) as usize {
                            u[r * su..r * su + (w / 2) as usize].fill(128);
                            v[r * sv..r * sv + (w / 2) as usize].fill(128);
                        }
                        true
                    },
                    i == 30,
                )
                .expect("encode");
            assert!(!out.is_empty(), "zerolatency: one access unit per frame");
            // Debug aid: HX_X264_DUMP=<file> appends the stream so its SPS/VUI can be read with
            // `ffmpeg -i <file> -c copy -bsf:v trace_headers -f null -`.
            if let Some(path) = std::env::var_os("HX_X264_DUMP") {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .unwrap();
                f.write_all(&out).unwrap();
            }
            assert_eq!(&out[..5], &[0, 0, 0, 1, 0x09], "AUD first");
            // NAL types in the access unit
            let mut types = Vec::new();
            let mut k = 0;
            while k + 3 < out.len() {
                if out[k] == 0 && out[k + 1] == 0 && out[k + 2] == 1 {
                    types.push(out[k + 3] & 0x1f);
                    k += 3;
                } else {
                    k += 1;
                }
            }
            idr.push(types.contains(&5));
            if i == 0 {
                assert!(
                    types.contains(&7) && types.contains(&8),
                    "SPS/PPS with the first IDR"
                );
            }
            sizes.push(out.len());
        }
        assert!(idr[0], "first frame is IDR");
        assert!(
            !idr[1..30].iter().any(|&b| b),
            "no IDR from the rate change: {idr:?}"
        );
        assert!(idr[30], "IDR on request");
        let before: usize = sizes[10..20].iter().sum();
        let after: usize = sizes[21..30].iter().sum::<usize>() * 10 / 9;
        assert!(
            after < before,
            "lower rate -> smaller frames ({before} vs {after})"
        );
    }
}
