//! The Horcrux Windows guest drivers' control ABI (mirror of
//! `drivers/windows/include/hxdrv.h`) and the pure mapping logic around it
//! (layout -> virtual monitors, viewer gamepad -> HID report, mode lists).
//! Platform-neutral so the layouts and mappings are unit-tested on every
//! host; the Windows I/O lives in `windows/idd.rs` and `windows/pad.rs`.
#![cfg_attr(not(windows), allow(dead_code))]

use horcrux_guest_proto::{Gamepad, Monitor};

pub const ABI_VERSION: u32 = 1;
pub const IDD_ABI_MONITORS: usize = 16;
pub const IDD_ABI_MODES: usize = 32;

const fn ctl(func: u32, access: u32) -> u32 {
    (0x22 << 16) | (access << 14) | (func << 2)
}
pub const IOCTL_IDD_GET_INFO: u32 = ctl(0x900, 1);
pub const IOCTL_IDD_SET_MONITORS: u32 = ctl(0x901, 2);

/// {6c3b1f4e-2a7d-4c59-9e0b-8d41f2a7c613}
pub const IDD_INTERFACE_GUID: (u32, u16, u16, [u8; 8]) = (
    0x6c3b1f4e,
    0x2a7d,
    0x4c59,
    [0x9e, 0x0b, 0x8d, 0x41, 0xf2, 0xa7, 0xc6, 0x13],
);

pub const MON_SHARE_FRAMES: u32 = 0x1;
pub const SET_PERSIST: u32 = 0x1;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IddMode {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IddMonitor {
    pub id: u32,
    pub flags: u32,
    pub mode_count: u32,
    pub modes: [IddMode; IDD_ABI_MODES],
}

impl Default for IddMonitor {
    fn default() -> Self {
        Self {
            id: 0,
            flags: 0,
            mode_count: 0,
            modes: [IddMode::default(); IDD_ABI_MODES],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IddSetMonitors {
    pub abi: u32,
    pub flags: u32,
    pub count: u32,
    pub reserved: u32,
    pub monitors: [IddMonitor; IDD_ABI_MONITORS],
}

impl Default for IddSetMonitors {
    fn default() -> Self {
        Self {
            abi: ABI_VERSION,
            flags: 0,
            count: 0,
            reserved: 0,
            monitors: [IddMonitor::default(); IDD_ABI_MONITORS],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IddMonitorInfo {
    pub id: u32,
    pub flags: u32,
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    pub os_target_id: u32,
    pub has_swapchain: u32,
    pub share_generation: u32,
    pub frames: u64,
    pub render_luid_low: u32,
    pub render_luid_high: i32,
    pub last_error: u32,
    /// Preferred mode (the first mode of the request).
    pub pref_width: u32,
    pub pref_height: u32,
    pub pref_refresh_mhz: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IddInfo {
    pub abi: u32,
    pub driver_version: u32,
    pub iddcx_version: u32,
    pub max_monitors: u32,
    pub adapter_ready: u32,
    pub count: u32,
    pub adapter_luid_low: u32,
    pub adapter_luid_high: i32,
    pub monitors: [IddMonitorInfo; IDD_ABI_MONITORS],
}

impl Default for IddInfo {
    fn default() -> Self {
        Self {
            abi: 0,
            driver_version: 0,
            iddcx_version: 0,
            max_monitors: 0,
            adapter_ready: 0,
            count: 0,
            adapter_luid_low: 0,
            adapter_luid_high: 0,
            monitors: [IddMonitorInfo::default(); IDD_ABI_MONITORS],
        }
    }
}

impl IddInfo {
    pub fn active(&self) -> &[IddMonitorInfo] {
        &self.monitors[..(self.count as usize).min(IDD_ABI_MONITORS)]
    }
}

/// Input report id of the Game Pad collection (what games read); the agent
/// never writes it, the driver produces it from the control reports.
#[allow(dead_code)]
pub const PAD_REPORT_GAMEPAD: u8 = 1;
pub const PAD_REPORT_CONTROL: u8 = 2;
/// HID usage page of the gamepad's control collection.
pub const PAD_CONTROL_USAGE_PAGE: u16 = 0xFF48;
pub const PAD_CONTROL_REPORT_LEN: usize = 16;

/// One virtual display mode: `WIDTHxHEIGHT@HZ` (HZ may be fractional,
/// e.g. `59.94`).
pub fn parse_mode(s: &str) -> Option<IddMode> {
    let (size, hz) = s.trim().split_once('@')?;
    let (w, h) = size.split_once('x')?;
    let hz: f64 = hz.trim().parse().ok()?;
    let m = IddMode {
        width: w.trim().parse().ok()?,
        height: h.trim().parse().ok()?,
        refresh_mhz: (hz * 1000.0).round() as u32,
    };
    mode_ok(&m).then_some(m)
}

/// Comma-separated list of modes; invalid entries are skipped.
pub fn parse_modes(s: &str) -> Vec<IddMode> {
    s.split(',')
        .filter(|p| !p.trim().is_empty())
        .filter_map(parse_mode)
        .collect()
}

/// Same bounds the driver enforces (HxModeValid).
pub fn mode_ok(m: &IddMode) -> bool {
    (320..=16384).contains(&m.width)
        && (200..=16384).contains(&m.height)
        && (1000..=1_000_000).contains(&m.refresh_mhz)
}

/// Build the driver request for a layout: one virtual monitor per layout
/// monitor (up to `max`), each with the requested mode first (the
/// preferred mode) followed by `extra` modes so Windows' display settings
/// offer common sizes too. Monitors with no refresh rate get `default_hz`.
pub fn idd_request(
    layout: &[Monitor],
    extra: &[IddMode],
    default_hz: u32,
    max: usize,
    flags: u32,
    share: bool,
) -> Result<IddSetMonitors, String> {
    let mut set = IddSetMonitors {
        flags,
        ..Default::default()
    };
    let n = layout.len().min(max).min(IDD_ABI_MONITORS);
    for (i, m) in layout.iter().take(n).enumerate() {
        let hz = if m.refresh_hz > 0 {
            m.refresh_hz
        } else {
            default_hz
        };
        let want = IddMode {
            width: m.width,
            height: m.height,
            refresh_mhz: hz.saturating_mul(1000),
        };
        if !mode_ok(&want) {
            return Err(format!(
                "monitor {i}: {}x{}@{hz} is outside what a virtual display offers",
                m.width, m.height
            ));
        }
        let mon = &mut set.monitors[i];
        mon.id = i as u32;
        mon.flags = if share { MON_SHARE_FRAMES } else { 0 };
        mon.modes[0] = want;
        let mut c = 1;
        for e in extra {
            if c >= IDD_ABI_MODES {
                break;
            }
            if *e != want && mode_ok(e) && !mon.modes[..c].contains(e) {
                mon.modes[c] = *e;
                c += 1;
            }
        }
        mon.mode_count = c as u32;
    }
    set.count = n as u32;
    Ok(set)
}

/// HID control report for the virtual gamepad from the viewer's gamepad
/// state (W3C "standard" mapping). Button order on the HID side follows
/// the common Xbox-style layout: 1 A, 2 B, 3 X, 4 Y, 5 LB, 6 RB, 7 Back,
/// 8 Start, 9 left stick, 10 right stick, 11 Guide; the d-pad is the hat;
/// triggers are analog (Z/Rz). Stick Y grows downwards in both the W3C
/// mapping (what the protocol carries) and HID, so axes pass through.
pub fn pad_report(g: &Gamepad) -> [u8; PAD_CONTROL_REPORT_LEN] {
    let b = |i: u32| g.buttons & (1 << i) != 0;
    let map: [(u32, u16); 11] = [
        (0, 1 << 0),
        (1, 1 << 1),
        (2, 1 << 2),
        (3, 1 << 3),
        (4, 1 << 4),
        (5, 1 << 5),
        (8, 1 << 6),
        (9, 1 << 7),
        (10, 1 << 8),
        (11, 1 << 9),
        (16, 1 << 10),
    ];
    let mut buttons = 0u16;
    for (w3c, hid) in map {
        if b(w3c) {
            buttons |= hid;
        }
    }
    // d-pad 12 up, 13 down, 14 left, 15 right -> hat 0..7 clockwise from up, 8 = centred
    let (up, down, left, right) = (b(12), b(13), b(14), b(15));
    let hat: u8 = match (up && !down, down && !up, left && !right, right && !left) {
        (true, _, false, false) => 0,
        (true, _, false, true) => 1,
        (false, false, false, true) => 2,
        (_, true, false, true) => 3,
        (_, true, false, false) => 4,
        (_, true, true, false) => 5,
        (false, false, true, false) => 6,
        (true, _, true, false) => 7,
        _ => 8,
    };
    let clamp = |v: i16| v.max(-32767);
    let mut r = [0u8; PAD_CONTROL_REPORT_LEN];
    r[0] = PAD_REPORT_CONTROL;
    r[1] = ABI_VERSION as u8;
    r[2..4].copy_from_slice(&buttons.to_le_bytes());
    r[4] = hat;
    r[5..7].copy_from_slice(&clamp(g.lx).to_le_bytes());
    r[7..9].copy_from_slice(&clamp(g.ly).to_le_bytes());
    r[9..11].copy_from_slice(&clamp(g.rx).to_le_bytes());
    r[11..13].copy_from_slice(&clamp(g.ry).to_le_bytes());
    r[13] = g.lt;
    r[14] = g.rt;
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn abi_sizes_match_the_c_header() {
        // hxdrv.h: 3*u32; 3*u32 + 32 modes; 4*u32 + 16 monitors; ...
        assert_eq!(size_of::<IddMode>(), 12);
        assert_eq!(size_of::<IddMonitor>(), 12 + 32 * 12);
        assert_eq!(size_of::<IddSetMonitors>(), 16 + 16 * (12 + 32 * 12));
        assert_eq!(size_of::<IddMonitorInfo>(), 64);
        assert_eq!(size_of::<IddInfo>(), 32 + 16 * 64);
        assert_eq!(IOCTL_IDD_GET_INFO, 0x0022_6400);
        assert_eq!(IOCTL_IDD_SET_MONITORS, 0x0022_a404);
    }

    #[test]
    fn modes_parse_and_bound() {
        assert_eq!(
            parse_mode("2560x1440@240"),
            Some(IddMode {
                width: 2560,
                height: 1440,
                refresh_mhz: 240_000
            })
        );
        assert_eq!(parse_mode("1920x1080@59.94").unwrap().refresh_mhz, 59_940);
        assert_eq!(parse_mode("100x100@60"), None);
        assert_eq!(parse_mode("1920x1080@0"), None);
        assert_eq!(parse_mode("garbage"), None);
        assert_eq!(parse_modes("3840x2160@60, bad ,1280x720@144").len(), 2);
    }

    #[test]
    fn request_puts_the_asked_mode_first_and_dedups() {
        let l = [Monitor {
            width: 2560,
            height: 1440,
            refresh_hz: 240,
            primary: true,
            ..Default::default()
        }];
        let extra = parse_modes("2560x1440@240,1920x1080@60,1920x1080@60,3840x2160@60");
        let s = idd_request(&l, &extra, 60, 4, SET_PERSIST, true).unwrap();
        assert_eq!(s.count, 1);
        assert_eq!(s.flags, SET_PERSIST);
        let m = &s.monitors[0];
        assert_eq!(m.flags, MON_SHARE_FRAMES);
        assert_eq!(
            m.modes[0],
            IddMode {
                width: 2560,
                height: 1440,
                refresh_mhz: 240_000
            }
        );
        assert_eq!(m.mode_count, 3);
        assert_eq!(m.modes[1].width, 1920);
        assert_eq!(m.modes[2].width, 3840);
    }

    #[test]
    fn request_caps_count_and_fills_default_refresh() {
        let one = Monitor {
            width: 1280,
            height: 720,
            ..Default::default()
        };
        let l = vec![one.clone(); 6];
        let s = idd_request(&l, &[], 75, 4, 0, false).unwrap();
        assert_eq!(s.count, 4);
        assert_eq!(s.monitors[3].id, 3);
        assert_eq!(s.monitors[0].modes[0].refresh_mhz, 75_000);
        assert!(idd_request(
            &[Monitor {
                width: 99999,
                height: 10,
                ..Default::default()
            }],
            &[],
            60,
            4,
            0,
            false
        )
        .is_err());
    }

    #[test]
    fn pad_report_maps_buttons_hat_and_axes() {
        let g = Gamepad {
            index: 0,
            buttons: (1 << 0) | (1 << 9) | (1 << 16) | (1 << 12) | (1 << 15),
            lx: -32768,
            ly: 1000,
            rx: 32767,
            ry: -5,
            lt: 7,
            rt: 255,
        };
        let r = pad_report(&g);
        assert_eq!(r[0], PAD_REPORT_CONTROL);
        assert_eq!(r[1], 1);
        let buttons = u16::from_le_bytes([r[2], r[3]]);
        assert_eq!(buttons, (1 << 0) | (1 << 7) | (1 << 10));
        assert_eq!(r[4], 1, "up+right = north-east");
        assert_eq!(
            i16::from_le_bytes([r[5], r[6]]),
            -32767,
            "clamped to the HID logical minimum"
        );
        assert_eq!(i16::from_le_bytes([r[7], r[8]]), 1000);
        assert_eq!(i16::from_le_bytes([r[9], r[10]]), 32767);
        assert_eq!(r[13], 7);
        assert_eq!(r[14], 255);
        let idle = pad_report(&Gamepad::default());
        assert_eq!(idle[4], 8, "no d-pad = centred hat");
        let opposite = pad_report(&Gamepad {
            buttons: (1 << 12) | (1 << 13),
            ..Default::default()
        });
        assert_eq!(opposite[4], 8, "up+down cancels");
        let sw = pad_report(&Gamepad {
            buttons: (1 << 13) | (1 << 14),
            ..Default::default()
        });
        assert_eq!(sw[4], 5);
    }
}
