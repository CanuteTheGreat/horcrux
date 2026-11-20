//! Display enumeration and mode setting (EnumDisplayDevices /
//! EnumDisplaySettings / ChangeDisplaySettingsEx).
use horcrux_guest_proto::{Layout, Monitor};
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::*;

/// One attached display and the GDI device it is.
#[derive(Debug, Clone)]
pub struct Mon {
    pub device: Vec<u16>,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub hz: u32,
    pub primary: bool,
}

impl Mon {
    pub fn to_proto(&self) -> Monitor {
        Monitor {
            x: self.x,
            y: self.y,
            width: self.w,
            height: self.h,
            refresh_hz: self.hz,
            primary: self.primary,
        }
    }
}

fn devmode() -> DEVMODEW {
    DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    }
}

/// Attached displays, in GDI order.
pub fn monitors() -> Vec<Mon> {
    let mut v = Vec::new();
    let mut i = 0;
    loop {
        let mut dd = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: dd is a valid, initialised DISPLAY_DEVICEW with cb set.
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut dd, 0) }.as_bool() {
            break;
        }
        i += 1;
        if dd.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
            continue;
        }
        let mut dm = devmode();
        // SAFETY: DeviceName is a NUL-terminated wide string; dm is valid.
        if !unsafe {
            EnumDisplaySettingsW(PCWSTR(dd.DeviceName.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm)
        }
        .as_bool()
        {
            continue;
        }
        // SAFETY: reading the position member of the display variant of the union.
        let pos = unsafe { dm.Anonymous1.Anonymous2.dmPosition };
        let end = dd.DeviceName.iter().position(|c| *c == 0).unwrap_or(dd.DeviceName.len());
        let mut device = dd.DeviceName[..end].to_vec();
        device.push(0);
        v.push(Mon {
            device,
            x: pos.x,
            y: pos.y,
            w: dm.dmPelsWidth,
            h: dm.dmPelsHeight,
            hz: dm.dmDisplayFrequency,
            primary: dd.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0,
        });
    }
    v
}

pub fn layout() -> Layout {
    Layout {
        monitors: monitors().iter().map(Mon::to_proto).collect(),
    }
}

/// Find a mode of `dev` with this size (and the closest refresh rate).
fn find_mode(dev: &[u16], w: u32, h: u32, hz: u32) -> Option<DEVMODEW> {
    let mut best: Option<DEVMODEW> = None;
    let mut i = 0;
    loop {
        let mut dm = devmode();
        // SAFETY: dev is NUL-terminated; dm is valid.
        if !unsafe { EnumDisplaySettingsW(PCWSTR(dev.as_ptr()), ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm) }
            .as_bool()
        {
            break;
        }
        i += 1;
        if dm.dmPelsWidth != w || dm.dmPelsHeight != h || dm.dmBitsPerPel < 32 {
            continue;
        }
        let better = match &best {
            None => true,
            Some(b) => {
                hz != 0 && dm.dmDisplayFrequency.abs_diff(hz) < b.dmDisplayFrequency.abs_diff(hz)
            }
        };
        if better {
            best = Some(dm);
        }
    }
    best
}

/// Apply the requested size/refresh/position to each existing display
/// whose mode exists. Returns a log line per change that could not be made.
pub fn apply(l: &Layout, settle_ms: u64) -> Vec<String> {
    let mut notes = Vec::new();
    let cur = monitors();
    for (i, want) in l.monitors.iter().enumerate() {
        let Some(m) = cur.get(i) else {
            notes.push(format!("monitor {i}: not present (no virtual display driver)"));
            continue;
        };
        let size_change = want.width != 0
            && want.height != 0
            && (want.width != m.w || want.height != m.h || (want.refresh_hz != 0 && want.refresh_hz != m.hz));
        let pos_change = !m.primary && (want.x != m.x || want.y != m.y);
        if !size_change && !pos_change {
            continue;
        }
        let mut dm = if size_change {
            match find_mode(&m.device, want.width, want.height, want.refresh_hz) {
                Some(dm) => dm,
                None => {
                    notes.push(format!("monitor {i}: no {}x{} mode", want.width, want.height));
                    continue;
                }
            }
        } else {
            let mut dm = devmode();
            // SAFETY: m.device is NUL-terminated.
            let _ = unsafe { EnumDisplaySettingsW(PCWSTR(m.device.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm) };
            dm
        };
        if pos_change {
            dm.dmFields |= DM_POSITION;
            dm.Anonymous1.Anonymous2.dmPosition.x = want.x;
            dm.Anonymous1.Anonymous2.dmPosition.y = want.y;
        }
        // SAFETY: device name and dm are valid for the call.
        let r = unsafe { ChangeDisplaySettingsExW(PCWSTR(m.device.as_ptr()), Some(&dm), None, CDS_TYPE(0), None) };
        if r != DISP_CHANGE_SUCCESSFUL {
            notes.push(format!("monitor {i}: ChangeDisplaySettingsEx returned {}", r.0));
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(settle_ms));
    notes
}
