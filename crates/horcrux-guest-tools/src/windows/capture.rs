//! Screen capture: DXGI Desktop Duplication, with GDI BitBlt as a fallback
//! for display adapters/sessions where duplication is unavailable.
use windows::core::Interface;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;

/// What one `grab` produced.
pub enum Grab {
    /// `buf` holds a new frame.
    New,
    /// Nothing changed within the timeout.
    Idle,
    /// The duplication was lost (mode change, secure desktop): reopen.
    Lost,
    Err(String),
}

pub trait Capturer {
    fn name(&self) -> &'static str;
    fn size(&self) -> (u32, u32);
    /// Wait up to `timeout_ms` for a new frame; tight BGRA rows into `buf`.
    fn grab(&mut self, timeout_ms: u32, buf: &mut Vec<u8>) -> Grab;
}

/// Desktop rectangle (left, top) of the output to capture.
pub type Origin = (i32, i32);

pub struct Dup {
    _dev: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    w: u32,
    h: u32,
    got_first: bool,
}

impl Dup {
    pub fn open(origin: Origin, index: usize) -> Result<Dup, String> {
        // SAFETY: COM calls on interfaces created here.
        unsafe {
            let factory: IDXGIFactory1 =
                CreateDXGIFactory1().map_err(|e| format!("CreateDXGIFactory1: {e}"))?;
            let mut found: Option<(IDXGIAdapter1, IDXGIOutput)> = None;
            let mut flat = Vec::new();
            let mut a = 0;
            while let Ok(ad) = factory.EnumAdapters1(a) {
                a += 1;
                let mut o = 0;
                while let Ok(out) = ad.EnumOutputs(o) {
                    o += 1;
                    flat.push((ad.clone(), out));
                }
            }
            for (ad, out) in &flat {
                if let Ok(d) = out.GetDesc() {
                    if (d.DesktopCoordinates.left, d.DesktopCoordinates.top) == origin {
                        found = Some((ad.clone(), out.clone()));
                        break;
                    }
                }
            }
            if found.is_none() {
                found = flat.get(index).cloned();
            }
            let (ad, out) = found.ok_or("no DXGI output")?;
            let mut dev = None;
            let mut ctx = None;
            D3D11CreateDevice(
                &ad,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut dev),
                None,
                Some(&mut ctx),
            )
            .map_err(|e| format!("D3D11CreateDevice: {e}"))?;
            let (dev, ctx) = (dev.ok_or("no device")?, ctx.ok_or("no context")?);
            let out1: IDXGIOutput1 = out.cast().map_err(|e| e.to_string())?;
            let dup = out1
                .DuplicateOutput(&dev)
                .map_err(|e| format!("DuplicateOutput: {e}"))?;
            let d = dup.GetDesc();
            let (w, h) = (d.ModeDesc.Width, d.ModeDesc.Height);
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            dev.CreateTexture2D(&desc, None, Some(&mut staging))
                .map_err(|e| format!("CreateTexture2D: {e}"))?;
            Ok(Dup {
                _dev: dev,
                ctx,
                dup,
                staging: staging.ok_or("no staging texture")?,
                w,
                h,
                got_first: false,
            })
        }
    }
}

impl Capturer for Dup {
    fn name(&self) -> &'static str {
        "dxgi-dup"
    }
    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }
    fn grab(&mut self, timeout_ms: u32, buf: &mut Vec<u8>) -> Grab {
        // SAFETY: COM calls on interfaces owned by self; the mapped memory
        // is only read between Map and Unmap.
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            if let Err(e) = self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) {
                return match e.code() {
                    DXGI_ERROR_WAIT_TIMEOUT => Grab::Idle,
                    DXGI_ERROR_ACCESS_LOST
                    | DXGI_ERROR_ACCESS_DENIED
                    | DXGI_ERROR_DEVICE_REMOVED => Grab::Lost,
                    _ => Grab::Err(format!("AcquireNextFrame: {e}")),
                };
            }
            // Pointer-only updates carry no new picture.
            if info.LastPresentTime == 0 && self.got_first {
                let _ = self.dup.ReleaseFrame();
                return Grab::Idle;
            }
            let Some(tex) = res.and_then(|r| r.cast::<ID3D11Texture2D>().ok()) else {
                let _ = self.dup.ReleaseFrame();
                return Grab::Err("frame is not a texture".into());
            };
            self.ctx.CopyResource(&self.staging, &tex);
            let _ = self.dup.ReleaseFrame();
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(e) = self
                .ctx
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))
            {
                return Grab::Err(format!("Map: {e}"));
            }
            let (w, h) = (self.w as usize, self.h as usize);
            buf.resize(w * h * 4, 0);
            let src = m.pData as *const u8;
            for y in 0..h {
                std::ptr::copy_nonoverlapping(
                    src.add(y * m.RowPitch as usize),
                    buf.as_mut_ptr().add(y * w * 4),
                    w * 4,
                );
            }
            self.ctx.Unmap(&self.staging, 0);
            self.got_first = true;
            Grab::New
        }
    }
}

pub struct Gdi {
    origin: Origin,
    w: u32,
    h: u32,
    screen: HDC,
    mem: HDC,
    bmp: HBITMAP,
    bits: *mut u8,
    prev: Vec<u8>,
    poll_ms: u32,
}

impl Gdi {
    pub fn open(origin: Origin, size: (u32, u32), poll_ms: u32) -> Result<Gdi, String> {
        let (w, h) = size;
        if w == 0 || h == 0 {
            return Err("no display".into());
        }
        // SAFETY: GDI objects are created here and released in Drop.
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(screen);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
                .map_err(|e| format!("CreateDIBSection: {e}"))?;
            SelectObject(mem, bmp);
            Ok(Gdi {
                origin,
                w,
                h,
                screen,
                mem,
                bmp,
                bits: bits as *mut u8,
                prev: Vec::new(),
                poll_ms: poll_ms.max(1),
            })
        }
    }
}

impl Drop for Gdi {
    fn drop(&mut self) {
        // SAFETY: the objects were created in `open`.
        unsafe {
            let _ = DeleteObject(self.bmp);
            let _ = DeleteDC(self.mem);
            ReleaseDC(None, self.screen);
        }
    }
}

impl Capturer for Gdi {
    fn name(&self) -> &'static str {
        "gdi"
    }
    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }
    fn grab(&mut self, timeout_ms: u32, buf: &mut Vec<u8>) -> Grab {
        let t0 = std::time::Instant::now();
        let n = (self.w * self.h * 4) as usize;
        loop {
            // SAFETY: bits points at the DIB section of n bytes.
            unsafe {
                if BitBlt(
                    self.mem,
                    0,
                    0,
                    self.w as i32,
                    self.h as i32,
                    self.screen,
                    self.origin.0,
                    self.origin.1,
                    SRCCOPY | CAPTUREBLT,
                )
                .is_err()
                {
                    return Grab::Lost;
                }
                let cur = std::slice::from_raw_parts(self.bits, n);
                if self.prev.as_slice() != cur {
                    self.prev.clear();
                    self.prev.extend_from_slice(cur);
                    buf.clear();
                    buf.extend_from_slice(cur);
                    return Grab::New;
                }
            }
            if t0.elapsed().as_millis() as u32 >= timeout_ms {
                return Grab::Idle;
            }
            std::thread::sleep(std::time::Duration::from_millis(self.poll_ms as u64));
        }
    }
}
