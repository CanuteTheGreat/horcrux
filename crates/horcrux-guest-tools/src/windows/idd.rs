//! Horcrux virtual display (hxidd driver) control from the agent: find the
//! driver's device interface, read its state and set the virtual monitors;
//! plus the swap-chain capture source that reads the frames the driver
//! publishes as a shared D3D11 texture (`capture = idd`).
use super::capture::{Capturer, Grab};
use crate::hxdrv::{self, IddInfo, IddSetMonitors};
use windows::core::{Interface, GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::IO::DeviceIoControl;

pub fn interface_guid() -> GUID {
    let (a, b, c, d) = hxdrv::IDD_INTERFACE_GUID;
    GUID::from_values(a, b, c, d)
}

/// Present device interfaces of a class, as NUL-terminated wide paths.
pub fn interface_paths(guid: &GUID) -> Vec<Vec<u16>> {
    let mut len = 0u32;
    // SAFETY: plain size query with a valid GUID pointer.
    let cr = unsafe {
        CM_Get_Device_Interface_List_SizeW(
            &mut len,
            guid,
            PCWSTR::null(),
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        )
    };
    if cr != CR_SUCCESS || len <= 1 {
        return Vec::new();
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: buf holds `len` UTF-16 units as the size query reported.
    let cr = unsafe {
        CM_Get_Device_Interface_ListW(
            guid,
            PCWSTR::null(),
            &mut buf,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        )
    };
    if cr != CR_SUCCESS {
        return Vec::new();
    }
    buf.split(|c| *c == 0)
        .filter(|s| !s.is_empty())
        .map(|s| s.iter().copied().chain(std::iter::once(0)).collect())
        .collect()
}

pub struct Idd {
    h: HANDLE,
}

// SAFETY: a device handle may be used from any thread; IOCTLs are synchronous.
unsafe impl Send for Idd {}

impl Drop for Idd {
    fn drop(&mut self) {
        // SAFETY: owned handle, closed once.
        unsafe {
            let _ = CloseHandle(self.h);
        }
    }
}

impl Idd {
    /// Open the first Horcrux virtual display adapter.
    pub fn open() -> Result<Idd, String> {
        let path = interface_paths(&interface_guid())
            .into_iter()
            .next()
            .ok_or("the Horcrux virtual display driver is not installed (no hxidd device)")?;
        // SAFETY: path is NUL-terminated and outlives the call.
        let h = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|e| format!("open the virtual display device: {e}"))?;
        Ok(Idd { h })
    }

    pub fn info(&self) -> Result<IddInfo, String> {
        let mut info = IddInfo::default();
        let mut got = 0u32;
        // SAFETY: the output buffer is a valid IddInfo of the stated size.
        unsafe {
            DeviceIoControl(
                self.h,
                hxdrv::IOCTL_IDD_GET_INFO,
                None,
                0,
                Some(&mut info as *mut _ as *mut _),
                std::mem::size_of::<IddInfo>() as u32,
                Some(&mut got),
                None,
            )
        }
        .map_err(|e| format!("virtual display GET_INFO: {e}"))?;
        if info.abi != hxdrv::ABI_VERSION {
            return Err(format!(
                "virtual display driver speaks ABI {} (agent {})",
                info.abi,
                hxdrv::ABI_VERSION
            ));
        }
        Ok(info)
    }

    pub fn set(&self, s: &IddSetMonitors) -> Result<(), String> {
        let mut got = 0u32;
        // SAFETY: the input buffer is a valid IddSetMonitors of the stated size.
        unsafe {
            DeviceIoControl(
                self.h,
                hxdrv::IOCTL_IDD_SET_MONITORS,
                Some(s as *const _ as *const _),
                std::mem::size_of::<IddSetMonitors>() as u32,
                None,
                0,
                Some(&mut got),
                None,
            )
        }
        .map_err(|e| format!("virtual display SET_MONITORS: {e}"))
    }

    /// Wait until the OS has committed a mode on `n` monitors (or timeout).
    pub fn wait_active(&self, n: usize, timeout_ms: u64) -> Result<IddInfo, String> {
        let t0 = std::time::Instant::now();
        loop {
            let i = self.info()?;
            let active = i.active().iter().filter(|m| m.width > 0).count();
            if active >= n || t0.elapsed().as_millis() as u64 >= timeout_ms {
                return Ok(i);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// Is the Horcrux virtual display driver present?
pub fn present() -> bool {
    !interface_paths(&interface_guid()).is_empty()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Frames from the driver's shared texture (HXIDD_MON_SHARE_FRAMES): the
/// driver copies each swap-chain frame on the GPU into a named, keyed-mutex
/// texture and signals an event; this opens it on the same adapter.
pub struct IddShared {
    idd: Idd,
    id: u32,
    dev: ID3D11Device1,
    ctx: ID3D11DeviceContext,
    event: Option<HANDLE>,
    tex: Option<(u32, ID3D11Texture2D, IDXGIKeyedMutex, ID3D11Texture2D)>,
    w: u32,
    h: u32,
    /// The shared texture always holds the newest frame: the first grab
    /// reads it without waiting for a change (a static desktop sends none).
    fresh: bool,
}

impl Drop for IddShared {
    fn drop(&mut self) {
        if let Some(e) = self.event.take() {
            // SAFETY: owned event handle.
            unsafe {
                let _ = CloseHandle(e);
            }
        }
    }
}

impl IddShared {
    pub fn open(monitor: u32) -> Result<IddShared, String> {
        let idd = Idd::open()?;
        let info = idd.info()?;
        let mi = *info
            .active()
            .iter()
            .find(|m| m.id == monitor)
            .ok_or(format!("virtual monitor {monitor} does not exist"))?;
        if mi.flags & hxdrv::MON_SHARE_FRAMES == 0 {
            return Err(format!(
                "virtual monitor {monitor} does not publish frames (set capture = idd before the layout)"
            ));
        }
        let luid = LUID {
            LowPart: mi.render_luid_low,
            HighPart: mi.render_luid_high,
        };
        // SAFETY: COM/DXGI calls on objects created here; names are NUL-terminated.
        unsafe {
            let f: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))
                .map_err(|e| format!("DXGI: {e}"))?;
            let ad: IDXGIAdapter1 = f
                .EnumAdapterByLuid(luid)
                .map_err(|e| format!("render adapter of the virtual monitor: {e}"))?;
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
            let dev: ID3D11Device1 = dev
                .ok_or("no D3D11 device")?
                .cast()
                .map_err(|e| format!("ID3D11Device1: {e}"))?;
            Ok(IddShared {
                idd,
                id: monitor,
                dev,
                ctx: ctx.ok_or("no D3D11 context")?,
                event: None,
                tex: None,
                w: mi.width,
                h: mi.height,
                fresh: true,
            })
        }
    }

    fn reopen(&mut self) -> Result<(), String> {
        let info = self.idd.info()?;
        let mi = *info
            .active()
            .iter()
            .find(|m| m.id == self.id)
            .ok_or("virtual monitor removed")?;
        if let Some((g, ..)) = &self.tex {
            if *g == mi.share_generation {
                return Ok(());
            }
        }
        self.tex = None;
        if mi.share_generation == 0 {
            return Err("no frame published yet".into());
        }
        if self.event.is_none() {
            let name = wide(&format!(r"Global\HorcruxIdd{}Frame", self.id));
            // SAFETY: name is NUL-terminated.
            let e =
                unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(name.as_ptr())) }
                    .map_err(|e| format!("frame event: {e}"))?;
            self.event = Some(e);
        }
        self.fresh = true;
        let name = wide(&format!(
            r"Global\HorcruxIdd{}Frame{}",
            self.id, mi.share_generation
        ));
        // SAFETY: COM calls on objects created here.
        unsafe {
            let tex: ID3D11Texture2D = self
                .dev
                .OpenSharedResourceByName(
                    PCWSTR(name.as_ptr()),
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                )
                .map_err(|e| format!("open the shared frame: {e}"))?;
            let mutex: IDXGIKeyedMutex = tex.cast().map_err(|e| e.to_string())?;
            let mut d = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut d);
            let sd = D3D11_TEXTURE2D_DESC {
                Width: d.Width,
                Height: d.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: d.Format,
                SampleDesc: d.SampleDesc,
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            self.dev
                .CreateTexture2D(&sd, None, Some(&mut staging))
                .map_err(|e| format!("staging texture: {e}"))?;
            self.w = d.Width;
            self.h = d.Height;
            self.tex = Some((
                mi.share_generation,
                tex,
                mutex,
                staging.ok_or("no staging texture")?,
            ));
        }
        Ok(())
    }
}

impl Capturer for IddShared {
    fn name(&self) -> &'static str {
        "idd"
    }

    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    fn grab(&mut self, timeout_ms: u32, buf: &mut Vec<u8>) -> Grab {
        if (self.event.is_none() || self.tex.is_none()) && self.reopen().is_err() {
            std::thread::sleep(std::time::Duration::from_millis(timeout_ms.min(50) as u64));
            return Grab::Idle;
        }
        if !self.fresh {
            let ev = self.event.expect("opened");
            // SAFETY: waiting on an owned event handle.
            if unsafe { WaitForSingleObject(ev, timeout_ms) } == WAIT_TIMEOUT {
                return Grab::Idle;
            }
            // a new generation (mode change) replaces the texture
            if let Err(e) = self.reopen() {
                return Grab::Err(e);
            }
        }
        self.fresh = false;
        let (_, tex, mutex, staging) = self.tex.as_ref().expect("reopened");
        // SAFETY: COM calls on live objects; the mapped rows are read within bounds.
        unsafe {
            if mutex.AcquireSync(0, timeout_ms).is_err() {
                return Grab::Idle;
            }
            self.ctx.CopyResource(staging, tex);
            let _ = mutex.ReleaseSync(0);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(e) = self.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut m)) {
                return Grab::Err(format!("map: {e}"));
            }
            let row = self.w as usize * 4;
            buf.resize(row * self.h as usize, 0);
            for y in 0..self.h as usize {
                let src = (m.pData as *const u8).add(y * m.RowPitch as usize);
                std::ptr::copy_nonoverlapping(src, buf.as_mut_ptr().add(y * row), row);
            }
            self.ctx.Unmap(staging, 0);
        }
        Grab::New
    }
}
