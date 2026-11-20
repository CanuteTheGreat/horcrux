//! Media Foundation H.264 encoder: hardware MFTs (NVENC / AMF / QSV, which
//! are asynchronous) first, the Microsoft software MFT as the fallback.
use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};
use windows::core::{Interface, GUID, PWSTR, VARIANT};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

/// An encoder MFT found by enumeration.
pub struct Candidate {
    act: IMFActivate,
    pub name: String,
    pub hardware: bool,
    /// `nvenc`, `amf`, `qsv`, `mediafoundation` (other hardware) or
    /// `mediafoundation-software`.
    pub kind: String,
}

pub fn startup() -> Result<(), String> {
    // SAFETY: process-wide initialisation, balanced by `release` below.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| format!("MFStartup: {e}"))
    }
}

/// Balances a successful `startup`.
pub fn release() {
    // SAFETY: see above.
    unsafe {
        let _ = MFShutdown();
    }
}

fn string_attr(a: &IMFActivate, key: &GUID) -> String {
    let mut p = PWSTR::null();
    let mut n = 0u32;
    // SAFETY: GetAllocatedString allocates with CoTaskMemAlloc; freed below.
    unsafe {
        if a.GetAllocatedString(key, &mut p, &mut n).is_err() || p.is_null() {
            return String::new();
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p.0, n as usize));
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn enum_flags(flags: MFT_ENUM_FLAG) -> Vec<IMFActivate> {
    let info = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut arr: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut n = 0u32;
    let mut v = Vec::new();
    // SAFETY: the returned array is CoTaskMem memory of n interface pointers.
    unsafe {
        if MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, None, Some(&info), &mut arr, &mut n).is_ok() && !arr.is_null() {
            for i in 0..n as usize {
                if let Some(a) = (*arr.add(i)).take() {
                    v.push(a);
                }
            }
            CoTaskMemFree(Some(arr as *const _));
        }
    }
    v
}

/// H.264 encoder MFTs, hardware first (vendor-sorted by the system).
pub fn enumerate() -> Vec<Candidate> {
    let mut out = Vec::new();
    for (flags, hardware) in [
        (MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER, true),
        (MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER, false),
    ] {
        for act in enum_flags(flags) {
            let name = string_attr(&act, &MFT_FRIENDLY_NAME_Attribute);
            let vendor = string_attr(&act, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).to_uppercase();
            let lname = name.to_lowercase();
            let kind = if !hardware {
                "mediafoundation-software"
            } else if vendor.contains("VEN_10DE") || lname.contains("nvidia") {
                "nvenc"
            } else if vendor.contains("VEN_1002") || lname.contains("amd") {
                "amf"
            } else if vendor.contains("VEN_8086") || lname.contains("intel") || lname.contains("quick sync") {
                "qsv"
            } else {
                "mediafoundation"
            };
            out.push(Candidate {
                act,
                name,
                hardware,
                kind: kind.into(),
            });
        }
    }
    out
}

/// Encoder parameters from StartVideo (and settings).
pub struct Params {
    pub w: u32,
    pub h: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub max_bitrate_kbps: u32,
    pub gop: u32,
    pub low_delay: bool,
    pub profile: u32,
    pub output_wait_ms: u64,
}

pub struct Encoder {
    mft: IMFTransform,
    codec: Option<ICodecAPI>,
    events: Option<IMFMediaEventGenerator>,
    pub name: String,
    pub kind: String,
    p: Params,
    provides: bool,
    out_size: u32,
    need_input: u32,
    ready: Vec<Vec<u8>>,
    seq_header: Vec<u8>,
    force_key: bool,
    pub notes: Vec<String>,
}

fn var_u32(v: u32) -> VARIANT {
    VARIANT::from(v)
}

fn var_bool(v: bool) -> VARIANT {
    VARIANT::from(v)
}

const NAL_SPS: u8 = 7;
const NAL_IDR: u8 = 5;

/// NAL unit types in an Annex B access unit.
pub fn nal_types(au: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    let mut i = 0;
    while i + 3 < au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            v.push(au[i + 3] & 0x1f);
            i += 3;
        } else {
            i += 1;
        }
    }
    v
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

impl Encoder {
    pub fn new(c: &Candidate, p: Params) -> Result<Encoder, String> {
        let mut notes = Vec::new();
        // SAFETY: COM calls on interfaces created here.
        unsafe {
            let mft: IMFTransform = c.act.ActivateObject().map_err(|e| format!("{}: activate: {e}", c.name))?;
            let mut asynchronous = false;
            if let Ok(attrs) = mft.GetAttributes() {
                if attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) == 1 {
                    asynchronous = true;
                    attrs
                        .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                        .map_err(|e| format!("{}: async unlock: {e}", c.name))?;
                }
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, p.low_delay as u32);
            }
            let events = if asynchronous {
                Some(mft.cast::<IMFMediaEventGenerator>().map_err(|e| e.to_string())?)
            } else {
                None
            };
            let codec = mft.cast::<ICodecAPI>().ok();

            let bps = p.bitrate_kbps.max(1) * 1000;
            let out_t = MFCreateMediaType().map_err(|e| e.to_string())?;
            out_t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ok();
            out_t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).ok();
            out_t.SetUINT32(&MF_MT_AVG_BITRATE, bps).ok();
            out_t.SetUINT64(&MF_MT_FRAME_SIZE, pack(p.w, p.h)).ok();
            out_t.SetUINT64(&MF_MT_FRAME_RATE, pack(p.fps, 1)).ok();
            out_t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).ok();
            out_t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).ok();
            out_t.SetUINT32(&MF_MT_MPEG2_PROFILE, p.profile).ok();
            if let Some(api) = &codec {
                let mut set = |k: &GUID, v: VARIANT, what: &str| {
                    if let Err(e) = api.SetValue(k, &v) {
                        notes.push(format!("{what}: {e}"));
                    }
                };
                if p.low_delay {
                    set(&CODECAPI_AVLowLatencyMode, var_bool(true), "low latency mode");
                }
                set(
                    &CODECAPI_AVEncCommonRateControlMode,
                    var_u32(eAVEncCommonRateControlMode_CBR.0 as u32),
                    "CBR",
                );
                set(&CODECAPI_AVEncCommonMeanBitRate, var_u32(bps), "bitrate");
                if p.max_bitrate_kbps > 0 {
                    set(&CODECAPI_AVEncCommonMaxBitRate, var_u32(p.max_bitrate_kbps * 1000), "max bitrate");
                }
                set(&CODECAPI_AVEncMPVDefaultBPictureCount, var_u32(0), "no B-frames");
                if p.gop > 0 {
                    set(&CODECAPI_AVEncMPVGOPSize, var_u32(p.gop), "GOP size");
                }
            }
            mft.SetOutputType(0, &out_t, 0)
                .map_err(|e| format!("{}: SetOutputType: {e}", c.name))?;

            let in_t = MFCreateMediaType().map_err(|e| e.to_string())?;
            in_t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ok();
            in_t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).ok();
            in_t.SetUINT64(&MF_MT_FRAME_SIZE, pack(p.w, p.h)).ok();
            in_t.SetUINT64(&MF_MT_FRAME_RATE, pack(p.fps, 1)).ok();
            in_t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).ok();
            in_t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).ok();
            in_t.SetUINT32(&MF_MT_DEFAULT_STRIDE, p.w).ok();
            in_t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32).ok();
            in_t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32).ok();
            in_t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32).ok();
            in_t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32).ok();
            if let Err(e) = mft.SetInputType(0, &in_t, 0) {
                // Some MFTs only accept one of their own advertised types.
                let mut ok = false;
                for i in 0.. {
                    let Ok(t) = mft.GetInputAvailableType(0, i) else { break };
                    if t.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12) {
                        t.SetUINT64(&MF_MT_FRAME_SIZE, pack(p.w, p.h)).ok();
                        t.SetUINT64(&MF_MT_FRAME_RATE, pack(p.fps, 1)).ok();
                        if mft.SetInputType(0, &t, 0).is_ok() {
                            ok = true;
                            break;
                        }
                    }
                }
                if !ok {
                    return Err(format!("{}: SetInputType NV12 {}x{}: {e}", c.name, p.w, p.h));
                }
            }
            let info = mft
                .GetOutputStreamInfo(0)
                .map_err(|e| format!("GetOutputStreamInfo: {e}"))?;
            let provides = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                != 0;
            let mut enc = Encoder {
                mft,
                codec,
                events,
                name: c.name.clone(),
                kind: c.kind.clone(),
                provides,
                out_size: info.cbSize.max(p.w * p.h),
                p,
                need_input: 0,
                ready: Vec::new(),
                seq_header: Vec::new(),
                force_key: false,
                notes,
            };
            enc.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).ok();
            enc.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).ok();
            enc.refresh_header();
            Ok(enc)
        }
    }

    fn refresh_header(&mut self) {
        // SAFETY: blob size/read on a live media type.
        unsafe {
            if let Ok(t) = self.mft.GetOutputCurrentType(0) {
                if let Ok(n) = t.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) {
                    let mut b = vec![0u8; n as usize];
                    if t.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut b, None).is_ok() {
                        self.seq_header = b;
                    }
                }
            }
        }
    }

    /// The next frame will be an IDR.
    pub fn request_keyframe(&mut self) {
        self.force_key = true;
    }

    fn make_sample(nv12: &[u8], pts: i64, dur: i64) -> Result<IMFSample, String> {
        // SAFETY: the buffer is locked only while the NV12 bytes are copied.
        unsafe {
            let buf = MFCreateMemoryBuffer(nv12.len() as u32).map_err(|e| e.to_string())?;
            let mut p = std::ptr::null_mut();
            buf.Lock(&mut p, None, None).map_err(|e| e.to_string())?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), p, nv12.len());
            buf.Unlock().ok();
            buf.SetCurrentLength(nv12.len() as u32).ok();
            let s = MFCreateSample().map_err(|e| e.to_string())?;
            s.AddBuffer(&buf).map_err(|e| e.to_string())?;
            s.SetSampleTime(pts).ok();
            s.SetSampleDuration(dur).ok();
            Ok(s)
        }
    }

    fn read_sample(s: &IMFSample) -> Result<Vec<u8>, String> {
        // SAFETY: the contiguous buffer is locked while copied out.
        unsafe {
            let b = s.ConvertToContiguousBuffer().map_err(|e| e.to_string())?;
            let mut p = std::ptr::null_mut();
            let mut len = 0u32;
            b.Lock(&mut p, None, Some(&mut len)).map_err(|e| e.to_string())?;
            let v = std::slice::from_raw_parts(p, len as usize).to_vec();
            b.Unlock().ok();
            Ok(v)
        }
    }

    /// Fetch one output if the MFT has one. Ok(false) = needs more input.
    fn take_output(&mut self) -> Result<bool, String> {
        // SAFETY: ProcessOutput with a buffer descriptor we own; the sample
        // fields are moved out exactly once.
        unsafe {
            let sample = if self.provides {
                None
            } else {
                let buf = MFCreateMemoryBuffer(self.out_size).map_err(|e| e.to_string())?;
                let s = MFCreateSample().map_err(|e| e.to_string())?;
                s.AddBuffer(&buf).map_err(|e| e.to_string())?;
                Some(s)
            };
            let mut data = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let r = self.mft.ProcessOutput(0, &mut data, &mut status);
            let s = ManuallyDrop::take(&mut data[0].pSample);
            let _ = ManuallyDrop::take(&mut data[0].pEvents);
            match r {
                Ok(()) => {
                    if let Some(s) = s {
                        let au = Self::read_sample(&s)?;
                        if !au.is_empty() {
                            self.ready.push(au);
                        }
                    }
                    Ok(true)
                }
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(false),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    if let Ok(t) = self.mft.GetOutputAvailableType(0, 0) {
                        self.mft.SetOutputType(0, &t, 0).ok();
                    }
                    self.refresh_header();
                    Ok(true)
                }
                Err(e) => Err(format!("ProcessOutput: {e}")),
            }
        }
    }

    /// Handle queued events of an asynchronous MFT for up to `wait`.
    fn pump(&mut self, wait: Duration) -> Result<(), String> {
        let Some(ev) = self.events.clone() else {
            return Ok(());
        };
        let deadline = Instant::now() + wait;
        loop {
            // SAFETY: event queue calls on a live generator.
            let e = unsafe { ev.GetEvent(MF_EVENT_FLAG_NO_WAIT) };
            match e {
                Ok(e) => {
                    // SAFETY: plain getter.
                    let t = unsafe { e.GetType() }.map_err(|e| e.to_string())?;
                    if t == METransformNeedInput.0 as u32 {
                        self.need_input += 1;
                    } else if t == METransformHaveOutput.0 as u32 {
                        self.take_output()?;
                    }
                }
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    if Instant::now() >= deadline {
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_micros(300));
                }
                Err(e) => return Err(format!("GetEvent: {e}")),
            }
        }
    }

    /// Encode one NV12 frame; returns the access units that are ready
    /// (usually one) as `(annex_b, keyframe)`.
    pub fn encode(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<Vec<(Vec<u8>, bool)>, String> {
        let dur = 10_000_000 / self.p.fps.max(1) as i64;
        if self.force_key {
            self.force_key = false;
            if let Some(api) = &self.codec {
                // SAFETY: plain property set.
                if let Err(e) = unsafe { api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &var_u32(1)) } {
                    self.notes.push(format!("force keyframe: {e}"));
                }
            }
        }
        let sample = Self::make_sample(nv12, pts_100ns, dur)?;
        if self.events.is_some() {
            let t0 = Instant::now();
            while self.need_input == 0 {
                self.pump(Duration::from_millis(5))?;
                if t0.elapsed() > Duration::from_secs(2) {
                    return Err("hardware encoder never asked for input".into());
                }
            }
            // SAFETY: the MFT asked for input.
            unsafe { self.mft.ProcessInput(0, &sample, 0) }.map_err(|e| format!("ProcessInput: {e}"))?;
            self.need_input -= 1;
            let t0 = Instant::now();
            while self.ready.is_empty() && t0.elapsed() < Duration::from_millis(self.p.output_wait_ms) {
                self.pump(Duration::from_millis(2))?;
            }
        } else {
            // SAFETY: synchronous MFT input.
            let r = unsafe { self.mft.ProcessInput(0, &sample, 0) };
            if let Err(e) = r {
                if e.code() != MF_E_NOTACCEPTING {
                    return Err(format!("ProcessInput: {e}"));
                }
                while self.take_output()? {}
                // SAFETY: as above.
                unsafe { self.mft.ProcessInput(0, &sample, 0) }.map_err(|e| format!("ProcessInput: {e}"))?;
            }
            while self.take_output()? {}
        }
        let mut v = Vec::new();
        for mut au in std::mem::take(&mut self.ready) {
            let types = nal_types(&au);
            let key = types.contains(&NAL_IDR);
            if key && !types.contains(&NAL_SPS) && !self.seq_header.is_empty() {
                let mut with = self.seq_header.clone();
                with.extend_from_slice(&au);
                au = with;
            }
            v.push((au, key));
        }
        Ok(v)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: orderly end of the stream; the MFT is released after.
        unsafe {
            self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0).ok();
            self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0).ok();
        }
    }
}

impl Candidate {
    /// Release the MFT the activate object created.
    pub fn close(&self) {
        // SAFETY: plain call on a live activate object.
        unsafe {
            let _ = self.act.ShutdownObject();
        }
    }
}
