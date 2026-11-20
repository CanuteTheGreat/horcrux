//! H.264 decoding for the Horcrux desktop client, one backend per operating
//! system, each the system's own decoder (nothing is shipped):
//!
//! | backend | systems | what |
//! |---|---|---|
//! | `avcodec` | Linux, other Unix | the system FFmpeg (libavcodec), loaded at run time |
//! | `mediafoundation` | Windows | the Media Foundation H.264 decoder MFT; DXVA (GPU) through a D3D11 device when `hardware` is on and the GPU decodes H.264, its software path otherwise |
//! | `videotoolbox` | macOS | VideoToolbox (hardware where the Mac has it) |
//!
//! Every backend hands out the same planar 4:2:0 [`Picture`].

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

/// Which decoder to open, and its options (the desktop client's settings).
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// `auto` (the system's own: `avcodec` on Linux, `mediafoundation` on
    /// Windows, `videotoolbox` on macOS) or one of [`backends`].
    pub backend: String,
    /// Use the GPU's decoder where the backend can (DXVA, VideoToolbox
    /// hardware); off: the backend's software decoder.
    pub hardware: bool,
    /// `avcodec` backend: library names or paths tried in order.
    pub avcodec: Vec<String>,
    pub avutil: Vec<String>,
}

/// Backends compiled in on this system.
pub fn backends() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["mediafoundation"]
    }
    #[cfg(target_os = "macos")]
    {
        &["videotoolbox", "avcodec"]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        &["avcodec"]
    }
    #[cfg(not(any(unix, windows)))]
    {
        &[]
    }
}

/// The backend `auto` picks on this system.
pub fn native_backend() -> &'static str {
    backends().first().copied().unwrap_or("none")
}

enum Backend {
    #[cfg(unix)]
    Avcodec(crate::avcodec::Decoder),
    #[cfg(windows)]
    MediaFoundation(crate::mfdec::Decoder),
    #[cfg(target_os = "macos")]
    VideoToolbox(crate::vtdec::Decoder),
}

/// An H.264 decoder (low delay: every access unit is decoded and shown at
/// once, no reordering wait).
pub struct Decoder {
    backend: Backend,
}

impl Decoder {
    pub fn open(c: &Config) -> Result<Decoder, String> {
        let want = if c.backend == "auto" || c.backend.is_empty() {
            native_backend()
        } else {
            c.backend.as_str()
        };
        if !backends().contains(&want) {
            return Err(format!(
                "decoder {want:?} is not available on this system (available: {})",
                backends().join(", ")
            ));
        }
        let backend = match want {
            #[cfg(unix)]
            "avcodec" => Backend::Avcodec(crate::avcodec::Decoder::open(&c.avcodec, &c.avutil)?),
            #[cfg(windows)]
            "mediafoundation" => Backend::MediaFoundation(crate::mfdec::Decoder::open(c.hardware)?),
            #[cfg(target_os = "macos")]
            "videotoolbox" => Backend::VideoToolbox(crate::vtdec::Decoder::open(c.hardware)?),
            other => return Err(format!("decoder {other:?} is not available")),
        };
        Ok(Decoder { backend })
    }

    /// Decode one access unit (Annex-B). Calls `show` with the newest
    /// picture it produced, if any. Errors are per frame (a lost reference:
    /// ask the sender for a keyframe).
    pub fn decode(&mut self, au: &[u8], show: impl FnOnce(Picture)) -> Result<bool, String> {
        match &mut self.backend {
            #[cfg(unix)]
            Backend::Avcodec(d) => d.decode(au, show),
            #[cfg(windows)]
            Backend::MediaFoundation(d) => d.decode(au, show),
            #[cfg(target_os = "macos")]
            Backend::VideoToolbox(d) => d.decode(au, show),
        }
    }

    /// What decodes (backend, library or MFT, hardware or software), for
    /// reports and the window title.
    pub fn describe(&self) -> String {
        match &self.backend {
            #[cfg(unix)]
            Backend::Avcodec(d) => format!("avcodec ({} + {})", d.libraries.0, d.libraries.1),
            #[cfg(windows)]
            Backend::MediaFoundation(d) => d.describe(),
            #[cfg(target_os = "macos")]
            Backend::VideoToolbox(d) => d.describe(),
        }
    }
}

/// Owned I420 planes, filled from the semi-planar NV12 most system decoders
/// produce (`mediafoundation`, `videotoolbox`).
#[derive(Default)]
pub struct Planes {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Planes {
    /// Copy an NV12 picture: `y` rows of `y_stride` bytes, then `uv` rows of
    /// `uv_stride` bytes holding interleaved U,V pairs.
    #[allow(clippy::too_many_arguments)]
    pub fn from_nv12(
        &mut self,
        width: usize,
        height: usize,
        y: &[u8],
        y_stride: usize,
        uv: &[u8],
        uv_stride: usize,
    ) -> Result<(), String> {
        let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
        if width == 0
            || height == 0
            || y_stride < width
            || uv_stride < cw * 2
            || y.len() < y_stride * (height - 1) + width
            || uv.len() < uv_stride * (ch - 1) + cw * 2
        {
            return Err(format!(
                "NV12 picture {width}x{height} does not fit its planes ({} / {} bytes, strides {y_stride} / {uv_stride})",
                y.len(),
                uv.len()
            ));
        }
        self.width = width;
        self.height = height;
        self.y.resize(width * height, 0);
        self.u.resize(cw * ch, 0);
        self.v.resize(cw * ch, 0);
        for r in 0..height {
            self.y[r * width..(r + 1) * width]
                .copy_from_slice(&y[r * y_stride..r * y_stride + width]);
        }
        for r in 0..ch {
            let row = &uv[r * uv_stride..r * uv_stride + cw * 2];
            let (u, v) = (
                &mut self.u[r * cw..(r + 1) * cw],
                &mut self.v[r * cw..(r + 1) * cw],
            );
            for (i, pair) in row.as_chunks::<2>().0.iter().enumerate() {
                u[i] = pair[0];
                v[i] = pair[1];
            }
        }
        Ok(())
    }

    pub fn picture(&self) -> Picture<'_> {
        let cw = self.width.div_ceil(2);
        Picture {
            width: self.width,
            height: self.height,
            y: &self.y,
            y_stride: self.width,
            u: &self.u,
            u_stride: cw,
            v: &self.v,
            v_stride: cw,
        }
    }
}

/// NAL units of an Annex-B access unit (start codes stripped).
pub fn nal_units(au: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 2 < au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut v = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map(|&n| n - 3).unwrap_or(au.len());
        // a 4-byte start code leaves one zero byte on the previous unit
        while end > s && au[end - 1] == 0 && starts.get(k + 1).is_some() {
            end -= 1;
        }
        if end > s {
            v.push(&au[s..end]);
        }
    }
    v
}

/// `avcodec` backend: library names tried by default (newest FFmpeg first;
/// the same on every system, so the settings file is too).
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

    #[test]
    fn nv12_to_i420() {
        // 4x2 picture, strides with padding
        let y: Vec<u8> = (0..16).collect(); // stride 8, 2 rows
        let uv = vec![10, 20, 11, 21, 0, 0, 0, 0]; // one row: (10,20),(11,21)
        let mut p = Planes::default();
        p.from_nv12(4, 2, &y, 8, &uv, 8).unwrap();
        assert_eq!(p.y, vec![0, 1, 2, 3, 8, 9, 10, 11]);
        assert_eq!(p.u, vec![10, 11]);
        assert_eq!(p.v, vec![20, 21]);
        let pic = p.picture();
        assert_eq!(
            (pic.width, pic.height, pic.y_stride, pic.u_stride),
            (4, 2, 4, 2)
        );
        // planes that are too small are refused, not read past
        assert!(p.from_nv12(4, 4, &y, 8, &uv, 8).is_err());
        assert!(p.from_nv12(9, 2, &y, 8, &uv, 8).is_err());
    }

    #[test]
    fn splits_annex_b() {
        let au = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9,
        ];
        let n = nal_units(&au);
        assert_eq!(
            n,
            vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 9, 9][..]]
        );
        assert!(nal_units(&[1, 2, 3]).is_empty());
    }

    /// Access units of the test stream (split at access unit delimiters):
    /// two 128x96 dark grey frames (IDR, P) then two light grey ones.
    pub(crate) fn test_stream() -> Vec<Vec<u8>> {
        let d = include_bytes!("../tests/data/gray-128x96.h264");
        let mut aus: Vec<Vec<u8>> = Vec::new();
        let mut i = 0;
        let mut start = 0;
        while i + 3 < d.len() {
            if d[i] == 0 && d[i + 1] == 0 && d[i + 2] == 1 && d[i + 3] & 0x1f == 9 {
                let s = if i > 0 && d[i - 1] == 0 { i - 1 } else { i };
                if s > start {
                    aus.push(d[start..s].to_vec());
                }
                start = s;
                i += 3;
            } else {
                i += 1;
            }
        }
        aus.push(d[start..].to_vec());
        aus
    }

    /// The system decoder decodes the test stream (set
    /// HX_REQUIRE_DECODER=1 where it must exist, e.g. the Windows CI job).
    #[test]
    fn decodes_the_test_stream() {
        let c = Config {
            backend: "auto".into(),
            hardware: true,
            avcodec: default_avcodec(),
            avutil: default_avutil(),
        };
        let mut dec = match Decoder::open(&c) {
            Ok(d) => d,
            Err(e) if std::env::var_os("HX_REQUIRE_DECODER").is_none() => {
                eprintln!("skip: no system H.264 decoder ({e})");
                return;
            }
            Err(e) => panic!("no system H.264 decoder: {e}"),
        };
        let aus = test_stream();
        assert_eq!(aus.len(), 4);
        let mut luma = Vec::new();
        for au in &aus {
            let mut got = None;
            dec.decode(au, |p| {
                got = Some((p.width, p.height, p.y[p.y_stride * 40 + 60]))
            })
            .unwrap_or_else(|e| panic!("{}: {e}", dec.describe()));
            if let Some((w, h, y)) = got {
                assert_eq!((w, h), (128, 96), "{}", dec.describe());
                luma.push(y);
            }
        }
        eprintln!("{}: luma {luma:?}", dec.describe());
        assert!(
            luma.len() >= 3,
            "{}: {} pictures",
            dec.describe(),
            luma.len()
        );
        assert!(luma[0] < 100, "{luma:?}");
        assert!(*luma.last().unwrap() > 150, "{luma:?}");
        // garbage is an error or nothing, never a crash
        let _ = dec.decode(&[0, 0, 0, 1, 0x65, 0xff, 0x00], |_| {});
    }

    #[test]
    fn unknown_backend_is_an_error() {
        let c = Config {
            backend: "nope".into(),
            hardware: false,
            avcodec: vec![],
            avutil: vec![],
        };
        let e = Decoder::open(&c).err().unwrap();
        assert!(e.contains("not available"), "{e}");
        assert!(!backends().is_empty());
    }
}
