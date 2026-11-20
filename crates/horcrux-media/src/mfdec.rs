//! H.264 decoding on Windows through Media Foundation: the system's H.264
//! decoder MFT (synchronous; on every Windows client edition, and on Windows
//! Server with the Media Foundation feature). With `hardware` on, the MFT
//! gets a D3D11 device manager and decodes through DXVA on the GPU where the
//! GPU has an H.264 decoder; the decoded NV12 surfaces are read back through
//! `IMF2DBuffer` (Media Foundation copies a GPU surface to system memory on
//! lock). Without a usable GPU the same MFT decodes in software.
use crate::decoder::{Picture, Planes};
use std::mem::ManuallyDrop;
use std::sync::OnceLock;
use windows::core::{Interface, GUID, PWSTR, VARIANT};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::IDXGIAdapter;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

static STARTUP: OnceLock<Result<(), String>> = OnceLock::new();

fn startup() -> Result<(), String> {
    STARTUP
        .get_or_init(|| {
            // SAFETY: process-wide Media Foundation start, once; never shut
            // down (the decoder lives as long as the process). COM may
            // already be initialised on this thread (e.g. as STA by the
            // window system): the MFT works in either apartment, so that
            // result is ignored.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| format!("MFStartup: {e}"))
            }
        })
        .clone()
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

/// Synchronous H.264 decoder MFTs (H.264 in, NV12 out), system order.
fn decoder_mfts() -> Vec<(IMFActivate, String)> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let mut arr: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut n = 0u32;
    let mut v = Vec::new();
    // SAFETY: the returned array is CoTaskMem memory of n interface pointers.
    unsafe {
        if MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut arr,
            &mut n,
        )
        .is_ok()
            && !arr.is_null()
        {
            for i in 0..n as usize {
                if let Some(a) = (*arr.add(i)).take() {
                    let name = string_attr(&a, &MFT_FRIENDLY_NAME_Attribute);
                    v.push((a, name));
                }
            }
            CoTaskMemFree(Some(arr as *const _));
        }
    }
    v
}

/// A D3D11 device (with video support) behind a DXGI device manager.
fn d3d_manager() -> Result<IMFDXGIDeviceManager, String> {
    // SAFETY: plain device creation; the manager keeps the device alive.
    unsafe {
        let mut dev: Option<ID3D11Device> = None;
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut dev),
            None,
            None,
        )
        .map_err(|e| format!("D3D11CreateDevice: {e}"))?;
        let dev = dev.ok_or("no D3D11 device")?;
        // the decoder uses the device from its own threads
        if let Ok(mt) = dev.cast::<ID3D11Multithread>() {
            let _ = mt.SetMultithreadProtected(true);
        }
        let mut token = 0u32;
        let mut mgr: Option<IMFDXGIDeviceManager> = None;
        MFCreateDXGIDeviceManager(&mut token, &mut mgr)
            .map_err(|e| format!("MFCreateDXGIDeviceManager: {e}"))?;
        let mgr = mgr.ok_or("no DXGI device manager")?;
        mgr.ResetDevice(&dev, token)
            .map_err(|e| format!("ResetDevice: {e}"))?;
        Ok(mgr)
    }
}

pub struct Decoder {
    mft: IMFTransform,
    name: String,
    /// The MFT took the D3D manager (DXVA requested).
    dxva: bool,
    /// The last output was a GPU surface.
    gpu_output: bool,
    provides: bool,
    out_size: u32,
    /// Coded size of the output type, and the visible part of it.
    coded: (usize, usize),
    visible: (usize, usize),
    stride: usize,
    pts: i64,
    planes: Planes,
    _manager: Option<IMFDXGIDeviceManager>,
    pub notes: Vec<String>,
}

// SAFETY: the decoder is used from one thread at a time (moved, never
// shared); the MFT is free-threaded COM.
unsafe impl Send for Decoder {}

fn pack_hi(v: u64) -> usize {
    (v >> 32) as usize
}
fn pack_lo(v: u64) -> usize {
    (v & 0xffff_ffff) as usize
}

impl Decoder {
    /// The first H.264 decoder MFT that opens; with `hardware`, DXVA is
    /// tried first and the software path used when the GPU (or the MFT)
    /// cannot.
    pub fn open(hardware: bool) -> Result<Decoder, String> {
        startup()?;
        let mfts = decoder_mfts();
        if mfts.is_empty() {
            return Err("Media Foundation has no H.264 decoder (on Windows Server, install the Media Foundation feature)".into());
        }
        let mut errs = Vec::new();
        for (act, name) in &mfts {
            for dxva in if hardware {
                &[true, false][..]
            } else {
                &[false][..]
            } {
                match Self::open_one(act, name, *dxva) {
                    Ok(mut d) => {
                        d.notes.append(&mut errs);
                        return Ok(d);
                    }
                    Err(e) => errs.push(format!(
                        "{name} ({}): {e}",
                        if *dxva { "dxva" } else { "software" }
                    )),
                }
            }
        }
        Err(errs.join("; "))
    }

    fn open_one(act: &IMFActivate, name: &str, dxva: bool) -> Result<Decoder, String> {
        // SAFETY: COM calls on interfaces created here.
        unsafe {
            // a fresh MFT per attempt
            let _ = act.ShutdownObject();
            let mft: IMFTransform = act.ActivateObject().map_err(|e| format!("activate: {e}"))?;
            let mut notes = Vec::new();
            if let Ok(attrs) = mft.GetAttributes() {
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            if let Ok(api) = mft.cast::<ICodecAPI>() {
                if let Err(e) = api.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(true)) {
                    notes.push(format!("low latency mode: {e}"));
                }
            }
            let mut manager = None;
            if dxva {
                let aware = mft
                    .GetAttributes()
                    .ok()
                    .and_then(|a| a.GetUINT32(&MF_SA_D3D11_AWARE).ok())
                    .unwrap_or(0)
                    == 1;
                if !aware {
                    return Err("not D3D11 aware".into());
                }
                let mgr = d3d_manager()?;
                mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, mgr.as_raw() as usize)
                    .map_err(|e| format!("set D3D manager: {e}"))?;
                manager = Some(mgr);
            }
            let in_t = MFCreateMediaType().map_err(|e| e.to_string())?;
            in_t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| e.to_string())?;
            in_t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(|e| e.to_string())?;
            in_t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .ok();
            mft.SetInputType(0, &in_t, 0)
                .map_err(|e| format!("SetInputType H.264: {e}"))?;
            let mut d = Decoder {
                mft,
                name: name.to_string(),
                dxva,
                gpu_output: false,
                provides: false,
                out_size: 0,
                coded: (0, 0),
                visible: (0, 0),
                stride: 0,
                pts: 0,
                planes: Planes::default(),
                _manager: manager,
                notes,
            };
            d.set_output_type()?;
            d.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .ok();
            d.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .ok();
            Ok(d)
        }
    }

    /// Pick NV12 among the MFT's output types and read its geometry.
    fn set_output_type(&mut self) -> Result<(), String> {
        // SAFETY: COM calls on the live MFT and its media types.
        unsafe {
            let mut chosen = None;
            for i in 0.. {
                let Ok(t) = self.mft.GetOutputAvailableType(0, i) else {
                    break;
                };
                if t.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12) {
                    chosen = Some(t);
                    break;
                }
            }
            let t = chosen.ok_or("the decoder offers no NV12 output")?;
            self.mft
                .SetOutputType(0, &t, 0)
                .map_err(|e| format!("SetOutputType NV12: {e}"))?;
            if let Ok(fs) = t.GetUINT64(&MF_MT_FRAME_SIZE) {
                self.coded = (pack_hi(fs), pack_lo(fs));
            }
            self.visible = self.coded;
            let mut area = [0u8; 16];
            let mut got = 0u32;
            if t.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut area, Some(&mut got))
                .is_ok()
                && got as usize == area.len()
            {
                let cx = i32::from_le_bytes(area[8..12].try_into().expect("4 bytes"));
                let cy = i32::from_le_bytes(area[12..16].try_into().expect("4 bytes"));
                if cx > 0 && cy > 0 && cx as usize <= self.coded.0 && cy as usize <= self.coded.1 {
                    self.visible = (cx as usize, cy as usize);
                }
            }
            self.stride = t
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .ok()
                .map(|s| (s as i32).unsigned_abs() as usize)
                .filter(|s| *s >= self.coded.0)
                .unwrap_or(self.coded.0);
            let info = self
                .mft
                .GetOutputStreamInfo(0)
                .map_err(|e| format!("GetOutputStreamInfo: {e}"))?;
            self.provides = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                    | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                != 0;
            self.out_size = info
                .cbSize
                .max((self.stride * self.coded.1.max(16) * 3 / 2) as u32);
            Ok(())
        }
    }

    /// What decodes, e.g. `mediafoundation (Microsoft H264 Video Decoder
    /// MFT, dxva)`.
    pub fn describe(&self) -> String {
        let how = if self.gpu_output {
            "dxva"
        } else if self.dxva {
            "dxva requested, software output"
        } else {
            "software"
        };
        format!("mediafoundation ({}, {how})", self.name)
    }

    pub fn decode(&mut self, au: &[u8], show: impl FnOnce(Picture)) -> Result<bool, String> {
        let sample = make_sample(au, self.pts)?;
        self.pts += 166_667;
        // SAFETY: synchronous MFT input; outputs are drained when it is full.
        unsafe {
            if let Err(e) = self.mft.ProcessInput(0, &sample, 0) {
                if e.code() != MF_E_NOTACCEPTING {
                    return Err(format!("ProcessInput: {e}"));
                }
                while self.take_output()? != Out::NeedInput {}
                self.mft
                    .ProcessInput(0, &sample, 0)
                    .map_err(|e| format!("ProcessInput: {e}"))?;
            }
        }
        let mut got = false;
        loop {
            match self.take_output()? {
                Out::Frame => got = true,
                Out::Again => {}
                Out::NeedInput => break,
            }
        }
        if got {
            show(self.planes.picture());
        }
        Ok(got)
    }

    fn take_output(&mut self) -> Result<Out, String> {
        // SAFETY: ProcessOutput with a buffer descriptor we own; the sample
        // and event fields are moved out exactly once.
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
                Ok(()) => match s {
                    Some(s) => {
                        self.read(&s)?;
                        Ok(Out::Frame)
                    }
                    None => Ok(Out::Again),
                },
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(Out::NeedInput),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.set_output_type()?;
                    Ok(Out::Again)
                }
                Err(e) => Err(format!("ProcessOutput: {e}")),
            }
        }
    }

    /// Copy a decoded NV12 sample into the I420 planes.
    fn read(&mut self, s: &IMFSample) -> Result<(), String> {
        let (w, h) = self.visible;
        // SAFETY: each buffer is locked only while it is copied out.
        unsafe {
            let buf = s.GetBufferByIndex(0).map_err(|e| e.to_string())?;
            self.gpu_output = buf.cast::<IMFDXGIBuffer>().is_ok();
            if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
                let len = b2.GetContiguousLength().map_err(|e| e.to_string())? as usize;
                let mut p = std::ptr::null_mut();
                let mut pitch = 0i32;
                b2.Lock2D(&mut p, &mut pitch)
                    .map_err(|e| format!("Lock2D: {e}"))?;
                let r = if pitch <= 0 || p.is_null() {
                    Err(format!("unexpected NV12 pitch {pitch}"))
                } else {
                    let pitch = pitch as usize;
                    // rows of the luma plane (the surface can be taller than
                    // the coded picture)
                    let rows = (len / pitch * 2 / 3).max(self.coded.1);
                    let all = std::slice::from_raw_parts(p, (pitch * rows * 3 / 2).max(len));
                    let (y, uv) = all.split_at(pitch * rows);
                    self.planes.from_nv12(w, h, y, pitch, uv, pitch)
                };
                let _ = b2.Unlock2D();
                return r;
            }
            let b = s.ConvertToContiguousBuffer().map_err(|e| e.to_string())?;
            let mut p = std::ptr::null_mut();
            let mut len = 0u32;
            b.Lock(&mut p, None, Some(&mut len))
                .map_err(|e| e.to_string())?;
            let all = std::slice::from_raw_parts(p, len as usize);
            let stride = self.stride.max(1);
            let rows = self.coded.1;
            let r = if all.len() < stride * rows {
                Err(format!("decoded sample too small ({} bytes)", all.len()))
            } else {
                let (y, uv) = all.split_at(stride * rows);
                self.planes.from_nv12(w, h, y, stride, uv, stride)
            };
            let _ = b.Unlock();
            r
        }
    }
}

#[derive(PartialEq)]
enum Out {
    Frame,
    Again,
    NeedInput,
}

fn make_sample(au: &[u8], pts: i64) -> Result<IMFSample, String> {
    // SAFETY: the buffer is locked only while the bytes are copied.
    unsafe {
        let buf = MFCreateMemoryBuffer(au.len().max(1) as u32).map_err(|e| e.to_string())?;
        let mut p = std::ptr::null_mut();
        buf.Lock(&mut p, None, None).map_err(|e| e.to_string())?;
        std::ptr::copy_nonoverlapping(au.as_ptr(), p, au.len());
        buf.Unlock().ok();
        buf.SetCurrentLength(au.len() as u32).ok();
        let s = MFCreateSample().map_err(|e| e.to_string())?;
        s.AddBuffer(&buf).map_err(|e| e.to_string())?;
        s.SetSampleTime(pts).ok();
        s.SetSampleDuration(166_667).ok();
        Ok(s)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: orderly end of the stream on the live MFT.
        unsafe {
            self.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
                .ok();
            self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0).ok();
        }
    }
}
