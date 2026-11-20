//! Horcrux virtual gamepad (hxpad driver) feeding: the agent writes each
//! viewer gamepad state as an output report to the pad's vendor control
//! collection; the driver turns it into the Game Pad input report games read.
use super::idd::interface_paths;
use crate::hxdrv;
use horcrux_guest_proto::Gamepad;
use windows::core::PCWSTR;
use windows::Win32::Devices::HumanInterfaceDevice::*;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;

struct Pad {
    index: u8,
    h: HANDLE,
}

impl Drop for Pad {
    fn drop(&mut self) {
        // SAFETY: owned handle.
        unsafe {
            let _ = CloseHandle(self.h);
        }
    }
}

/// The pads present, indexed by the number at the end of their device
/// instance id (`SWD\HorcruxEnum\HxPad<N>`), which the driver reports as
/// the HID serial number.
pub struct Pads {
    pads: Vec<Pad>,
}

// SAFETY: HID handles may be used from any thread; writes are synchronous.
unsafe impl Send for Pads {}

fn pad_index(serial: &str) -> Option<u8> {
    let tail: String = serial
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if !serial.to_ascii_lowercase().contains("hxpad") {
        return None;
    }
    tail.parse().ok()
}

impl Pads {
    pub fn open() -> Pads {
        let mut pads = Vec::new();
        // SAFETY: plain GUID query.
        let guid = unsafe { HidD_GetHidGuid() };
        for path in interface_paths(&guid) {
            // SAFETY: path is NUL-terminated; handles are closed by Pad/explicitly.
            unsafe {
                let Ok(h) = CreateFileW(
                    PCWSTR(path.as_ptr()),
                    GENERIC_WRITE.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                ) else {
                    continue;
                };
                let mut pp = PHIDP_PREPARSED_DATA::default();
                let mut keep = None;
                if HidD_GetPreparsedData(h, &mut pp).as_bool() {
                    let mut caps = HIDP_CAPS::default();
                    if HidP_GetCaps(pp, &mut caps).is_ok()
                        && caps.UsagePage == hxdrv::PAD_CONTROL_USAGE_PAGE
                    {
                        let mut ser = [0u16; 200];
                        if HidD_GetSerialNumberString(
                            h,
                            ser.as_mut_ptr() as *mut _,
                            (ser.len() * 2) as u32,
                        )
                        .as_bool()
                        {
                            let end = ser.iter().position(|c| *c == 0).unwrap_or(ser.len());
                            keep = pad_index(&String::from_utf16_lossy(&ser[..end]));
                        }
                    }
                    let _ = HidD_FreePreparsedData(pp);
                }
                match keep {
                    Some(index) => pads.push(Pad { index, h }),
                    None => {
                        let _ = CloseHandle(h);
                    }
                }
            }
        }
        pads.sort_by_key(|p| p.index);
        Pads { pads }
    }

    pub fn count(&self) -> usize {
        self.pads.len()
    }

    /// Feed one viewer gamepad; pads the guest does not have are ignored.
    pub fn write(&self, g: &Gamepad) -> Result<(), String> {
        let Some(p) = self.pads.iter().find(|p| p.index == g.index) else {
            return Ok(());
        };
        let r = hxdrv::pad_report(g);
        // SAFETY: r is a valid buffer of the stated length.
        if unsafe { HidD_SetOutputReport(p.h, r.as_ptr() as *const _, r.len() as u32) }.as_bool() {
            Ok(())
        } else {
            Err(format!(
                "virtual gamepad {}: {}",
                g.index,
                std::io::Error::last_os_error()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pad_index_from_serial() {
        assert_eq!(super::pad_index(r"SWD\HorcruxEnum\HxPad0"), Some(0));
        assert_eq!(super::pad_index(r"SWD\HORCRUXENUM\HXPAD3"), Some(3));
        assert_eq!(super::pad_index("SomeOtherPad1"), None);
    }
}
