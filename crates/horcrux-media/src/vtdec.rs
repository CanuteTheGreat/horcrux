//! H.264 decoding on macOS through VideoToolbox (the Mac's hardware decoder
//! where it has one, Apple's software decoder otherwise). Plain C calls into
//! VideoToolbox, CoreMedia, CoreVideo and CoreFoundation; no extra crate.
//!
//! Annex-B access units are split into NAL units: SPS/PPS make the format
//! description (a new pair makes a new session), the rest go to the decoder
//! as one AVCC sample (4-byte lengths). Output is NV12
//! (`kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange`), copied to I420.
use crate::decoder::{nal_units, Picture, Planes};
use std::ffi::c_void;

type OSStatus = i32;
type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CMFormatDescriptionRef = *const c_void;
type CMBlockBufferRef = *const c_void;
type CMSampleBufferRef = *const c_void;
type CVImageBufferRef = *const c_void;
type VTDecompressionSessionRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

#[repr(C)]
struct Opaque([u8; 0]);

type OutputCallback = extern "C" fn(
    refcon: *mut c_void,
    frame_refcon: *mut c_void,
    status: OSStatus,
    info_flags: u32,
    image: CVImageBufferRef,
    pts: CMTime,
    duration: CMTime,
);

#[repr(C)]
struct OutputCallbackRecord {
    callback: OutputCallback,
    refcon: *mut c_void,
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: Opaque;
    static kCFTypeDictionaryValueCallBacks: Opaque;
    static kCFBooleanTrue: CFTypeRef;
    static kCFBooleanFalse: CFTypeRef;
    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        n: isize,
        key_callbacks: *const Opaque,
        value_callbacks: *const Opaque,
    ) -> CFDictionaryRef;
    fn CFNumberCreate(allocator: CFAllocatorRef, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFBooleanGetValue(b: CFTypeRef) -> u8;
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: CFAllocatorRef,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_length_size: i32,
        out: *mut CMFormatDescriptionRef,
    ) -> OSStatus;
    fn CMBlockBufferCreateWithMemoryBlock(
        allocator: CFAllocatorRef,
        memory: *mut c_void,
        block_length: usize,
        block_allocator: CFAllocatorRef,
        custom_source: *const c_void,
        offset: usize,
        data_length: usize,
        flags: u32,
        out: *mut CMBlockBufferRef,
    ) -> OSStatus;
    fn CMBlockBufferReplaceDataBytes(
        src: *const c_void,
        dst: CMBlockBufferRef,
        offset: usize,
        length: usize,
    ) -> OSStatus;
    fn CMSampleBufferCreateReady(
        allocator: CFAllocatorRef,
        data: CMBlockBufferRef,
        format: CMFormatDescriptionRef,
        num_samples: isize,
        num_timing: isize,
        timing: *const c_void,
        num_sizes: isize,
        sizes: *const usize,
        out: *mut CMSampleBufferRef,
    ) -> OSStatus;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    fn CVPixelBufferLockBaseAddress(buf: CVImageBufferRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buf: CVImageBufferRef, flags: u64) -> i32;
    fn CVPixelBufferGetPixelFormatType(buf: CVImageBufferRef) -> u32;
    fn CVPixelBufferGetWidth(buf: CVImageBufferRef) -> usize;
    fn CVPixelBufferGetHeight(buf: CVImageBufferRef) -> usize;
    fn CVPixelBufferGetBaseAddressOfPlane(buf: CVImageBufferRef, plane: usize) -> *const u8;
    fn CVPixelBufferGetBytesPerRowOfPlane(buf: CVImageBufferRef, plane: usize) -> usize;
    fn CVPixelBufferGetHeightOfPlane(buf: CVImageBufferRef, plane: usize) -> usize;
}

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    static kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder: CFStringRef;
    static kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder: CFStringRef;
    fn VTDecompressionSessionCreate(
        allocator: CFAllocatorRef,
        format: CMFormatDescriptionRef,
        decoder_spec: CFDictionaryRef,
        image_attrs: CFDictionaryRef,
        callback: *const OutputCallbackRecord,
        out: *mut VTDecompressionSessionRef,
    ) -> OSStatus;
    fn VTDecompressionSessionDecodeFrame(
        session: VTDecompressionSessionRef,
        sample: CMSampleBufferRef,
        flags: u32,
        frame_refcon: *mut c_void,
        info_out: *mut u32,
    ) -> OSStatus;
    fn VTDecompressionSessionWaitForAsynchronousFrames(
        session: VTDecompressionSessionRef,
    ) -> OSStatus;
    fn VTDecompressionSessionInvalidate(session: VTDecompressionSessionRef);
    fn VTSessionCopyProperty(
        session: VTDecompressionSessionRef,
        key: CFStringRef,
        allocator: CFAllocatorRef,
        out: *mut CFTypeRef,
    ) -> OSStatus;
}

/// `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange` ('420v').
const NV12_VIDEO: u32 = u32::from_be_bytes(*b"420v");
/// `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange` ('420f').
const NV12_FULL: u32 = u32::from_be_bytes(*b"420f");
const CF_NUMBER_SINT32: isize = 3;
const LOCK_READ_ONLY: u64 = 1;

/// Written by the output callback.
#[derive(Default)]
struct Shared {
    planes: Planes,
    got: bool,
    err: Option<String>,
}

extern "C" fn on_output(
    refcon: *mut c_void,
    _frame: *mut c_void,
    status: OSStatus,
    _flags: u32,
    image: CVImageBufferRef,
    _pts: CMTime,
    _dur: CMTime,
) {
    // SAFETY: refcon is the decoder's boxed `Shared`, alive for the session.
    let sh = unsafe { &mut *(refcon as *mut Shared) };
    if status != 0 || image.is_null() {
        sh.err = Some(format!("VideoToolbox decode status {status}"));
        return;
    }
    // SAFETY: the pixel buffer is locked while its planes are read.
    unsafe {
        let fmt = CVPixelBufferGetPixelFormatType(image);
        if fmt != NV12_VIDEO && fmt != NV12_FULL {
            sh.err = Some(format!("unexpected pixel format {fmt:#x}"));
            return;
        }
        if CVPixelBufferLockBaseAddress(image, LOCK_READ_ONLY) != 0 {
            sh.err = Some("cannot lock the decoded picture".into());
            return;
        }
        let (w, h) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
        let (yp, ys, yh) = (
            CVPixelBufferGetBaseAddressOfPlane(image, 0),
            CVPixelBufferGetBytesPerRowOfPlane(image, 0),
            CVPixelBufferGetHeightOfPlane(image, 0),
        );
        let (cp, cs, chh) = (
            CVPixelBufferGetBaseAddressOfPlane(image, 1),
            CVPixelBufferGetBytesPerRowOfPlane(image, 1),
            CVPixelBufferGetHeightOfPlane(image, 1),
        );
        let r = if yp.is_null() || cp.is_null() {
            Err("decoded picture has no planes".to_string())
        } else {
            sh.planes.from_nv12(
                w,
                h,
                std::slice::from_raw_parts(yp, ys * yh),
                ys,
                std::slice::from_raw_parts(cp, cs * chh),
                cs,
            )
        };
        CVPixelBufferUnlockBaseAddress(image, LOCK_READ_ONLY);
        match r {
            Ok(()) => sh.got = true,
            Err(e) => sh.err = Some(e),
        }
    }
}

pub struct Decoder {
    hardware: bool,
    format: CMFormatDescriptionRef,
    session: VTDecompressionSessionRef,
    params: (Vec<u8>, Vec<u8>),
    shared: Box<Shared>,
    record: Box<OutputCallbackRecord>,
    using_hardware: Option<bool>,
}

// SAFETY: used from one thread at a time; VideoToolbox sessions may be
// driven from any thread.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn open(hardware: bool) -> Result<Decoder, String> {
        let mut shared = Box::<Shared>::default();
        let record = Box::new(OutputCallbackRecord {
            callback: on_output,
            refcon: &mut *shared as *mut Shared as *mut c_void,
        });
        Ok(Decoder {
            hardware,
            format: std::ptr::null(),
            session: std::ptr::null(),
            params: (vec![], vec![]),
            shared,
            record,
            using_hardware: None,
        })
    }

    pub fn describe(&self) -> String {
        match self.using_hardware {
            Some(true) => "videotoolbox (hardware)".into(),
            Some(false) => "videotoolbox (software)".into(),
            None => "videotoolbox".into(),
        }
    }

    fn close_session(&mut self) {
        // SAFETY: releasing what open_session created.
        unsafe {
            if !self.session.is_null() {
                VTDecompressionSessionInvalidate(self.session);
                CFRelease(self.session);
                self.session = std::ptr::null();
            }
            if !self.format.is_null() {
                CFRelease(self.format);
                self.format = std::ptr::null();
            }
        }
    }

    fn open_session(&mut self, sps: &[u8], pps: &[u8]) -> Result<(), String> {
        self.close_session();
        // SAFETY: CoreFoundation/VideoToolbox object creation; every created
        // object is released here or in close_session.
        unsafe {
            let ptrs = [sps.as_ptr(), pps.as_ptr()];
            let sizes = [sps.len(), pps.len()];
            let mut fmt: CMFormatDescriptionRef = std::ptr::null();
            let st = CMVideoFormatDescriptionCreateFromH264ParameterSets(
                std::ptr::null(),
                2,
                ptrs.as_ptr(),
                sizes.as_ptr(),
                4,
                &mut fmt,
            );
            if st != 0 {
                return Err(format!("format from SPS/PPS: status {st}"));
            }
            self.format = fmt;
            let hw = if self.hardware {
                kCFBooleanTrue
            } else {
                kCFBooleanFalse
            };
            let spec_k = [kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder];
            let spec_v = [hw];
            let spec = CFDictionaryCreate(
                std::ptr::null(),
                spec_k.as_ptr(),
                spec_v.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            let pf = NV12_VIDEO as i32;
            let num = CFNumberCreate(
                std::ptr::null(),
                CF_NUMBER_SINT32,
                &pf as *const i32 as *const c_void,
            );
            let attr_k = [kCVPixelBufferPixelFormatTypeKey];
            let attr_v = [num];
            let attrs = CFDictionaryCreate(
                std::ptr::null(),
                attr_k.as_ptr(),
                attr_v.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            let mut session: VTDecompressionSessionRef = std::ptr::null();
            let st = VTDecompressionSessionCreate(
                std::ptr::null(),
                fmt,
                spec,
                attrs,
                &*self.record,
                &mut session,
            );
            CFRelease(num);
            CFRelease(spec);
            CFRelease(attrs);
            if st != 0 {
                return Err(format!("VTDecompressionSessionCreate: status {st}"));
            }
            self.session = session;
            let mut v: CFTypeRef = std::ptr::null();
            if VTSessionCopyProperty(
                session,
                kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder,
                std::ptr::null(),
                &mut v,
            ) == 0
                && !v.is_null()
            {
                self.using_hardware = Some(CFBooleanGetValue(v) != 0);
                CFRelease(v);
            }
        }
        self.params = (sps.to_vec(), pps.to_vec());
        Ok(())
    }

    pub fn decode(&mut self, au: &[u8], show: impl FnOnce(Picture)) -> Result<bool, String> {
        let mut sps = None;
        let mut pps = None;
        let mut avcc = Vec::with_capacity(au.len() + 16);
        for n in nal_units(au) {
            match n[0] & 0x1f {
                7 => sps = Some(n),
                8 => pps = Some(n),
                9 => {} // access unit delimiter
                _ => {
                    avcc.extend_from_slice(&(n.len() as u32).to_be_bytes());
                    avcc.extend_from_slice(n);
                }
            }
        }
        if let (Some(s), Some(p)) = (sps, pps) {
            if self.session.is_null() || self.params.0 != s || self.params.1 != p {
                self.open_session(s, p)?;
            }
        }
        if self.session.is_null() {
            return Err("waiting for a keyframe (no SPS/PPS yet)".into());
        }
        if avcc.is_empty() {
            return Ok(false);
        }
        self.shared.got = false;
        self.shared.err = None;
        // SAFETY: a block buffer that owns a copy of the bytes, a sample over
        // it, a synchronous decode; both are released after.
        unsafe {
            let mut block: CMBlockBufferRef = std::ptr::null();
            let st = CMBlockBufferCreateWithMemoryBlock(
                std::ptr::null(),
                std::ptr::null_mut(),
                avcc.len(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                avcc.len(),
                0,
                &mut block,
            );
            if st != 0 {
                return Err(format!("CMBlockBufferCreate: status {st}"));
            }
            let st =
                CMBlockBufferReplaceDataBytes(avcc.as_ptr() as *const c_void, block, 0, avcc.len());
            if st != 0 {
                CFRelease(block);
                return Err(format!("CMBlockBufferReplaceDataBytes: status {st}"));
            }
            let size = avcc.len();
            let mut sample: CMSampleBufferRef = std::ptr::null();
            let st = CMSampleBufferCreateReady(
                std::ptr::null(),
                block,
                self.format,
                1,
                0,
                std::ptr::null(),
                1,
                &size,
                &mut sample,
            );
            CFRelease(block);
            if st != 0 {
                return Err(format!("CMSampleBufferCreateReady: status {st}"));
            }
            let mut info = 0u32;
            let st = VTDecompressionSessionDecodeFrame(
                self.session,
                sample,
                0,
                std::ptr::null_mut(),
                &mut info,
            );
            VTDecompressionSessionWaitForAsynchronousFrames(self.session);
            CFRelease(sample);
            if st != 0 {
                return Err(format!("VTDecompressionSessionDecodeFrame: status {st}"));
            }
        }
        if let Some(e) = self.shared.err.take() {
            return Err(e);
        }
        if self.shared.got {
            show(self.shared.planes.picture());
        }
        Ok(self.shared.got)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.close_session();
    }
}
