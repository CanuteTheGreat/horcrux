//! Optional XInput backend: feeds the viewer's gamepad to the ViGEm bus
//! driver (ViGEmBus, BSD-3-Clause, production-signed by its author), which
//! presents an Xbox 360 class controller that XInput-only games can read.
//!
//! Why this exists: XInput lists only Xbox-class devices, and a device
//! becomes one only through a kernel driver that Microsoft signed. Horcrux
//! has no Microsoft signature (see docs/design/horcrux-console.md, "Windows
//! driver signing without a paid certificate"), so it does not ship such a
//! driver of its own; it uses the already-signed one, if the guest's owner
//! opted in (`--drivers install-xinput`). The wire protocol here is the
//! bus's public IOCTL interface (ViGEmClient `km/BusShared.h`, MIT).
//!
//! The bus unplugs every controller a handle created when the handle is
//! closed, so nothing outlives the agent.
use super::idd::interface_paths;
use crate::hxdrv;
use horcrux_guest_proto::Gamepad;
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::DeviceIoControl;

/// {96E42B22-F5E9-42F8-B043-ED0F932F014F}
const BUS_GUID: GUID = GUID::from_u128(0x96E42B22_F5E9_42F8_B043_ED0F932F014F);
use crate::hxdrv::vigem::*;

/// Is the ViGEm bus driver installed and running?
pub fn bus_present() -> bool {
    !interface_paths(&BUS_GUID).is_empty()
}

pub struct Bus {
    h: HANDLE,
    /// Serial numbers of the controllers plugged in (index = pad number).
    plugged: Vec<u32>,
}

// SAFETY: the handle may be used from any thread; calls are synchronous.
unsafe impl Send for Bus {}

impl Drop for Bus {
    fn drop(&mut self) {
        for serial in self.plugged.drain(..) {
            let _ = ioctl(self.h, IOCTL_UNPLUG_TARGET, &u32s(&[8, serial]));
        }
        // SAFETY: owned handle; closing it would unplug any controller left.
        unsafe {
            let _ = CloseHandle(self.h);
        }
    }
}

fn ioctl(h: HANDLE, code: u32, input: &[u8]) -> Result<(), String> {
    let mut got = 0u32;
    // SAFETY: input is a valid buffer of the stated length; no output buffer.
    unsafe {
        DeviceIoControl(
            h,
            code,
            Some(input.as_ptr() as *const _),
            input.len() as u32,
            None,
            0,
            Some(&mut got),
            None,
        )
    }
    .map_err(|e| format!("ViGEm bus ioctl 0x{code:08x}: {e}"))
}

impl Bus {
    /// Open the bus and check that it speaks the protocol version we know.
    pub fn open() -> Result<Bus, String> {
        let paths = interface_paths(&BUS_GUID);
        let path = paths
            .first()
            .ok_or("the ViGEm bus driver is not installed (--drivers install-xinput)")?;
        // SAFETY: path is NUL-terminated.
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
        .map_err(|e| format!("open the ViGEm bus: {e}"))?;
        let bus = Bus {
            h,
            plugged: Vec::new(),
        };
        ioctl(bus.h, IOCTL_CHECK_VERSION, &u32s(&[8, COMMON_VERSION]))
            .map_err(|e| format!("{e} (a ViGEm bus with another protocol version?)"))?;
        Ok(bus)
    }

    /// Plug in controller `index` (serial = index + 1) when it is not there yet.
    fn ensure(&mut self, index: u8) -> Result<u32, String> {
        let serial = index as u32 + 1;
        if self.plugged.contains(&serial) {
            return Ok(serial);
        }
        ioctl(self.h, IOCTL_PLUGIN_TARGET, &plugin_target(serial))?;
        // returns when the controller's device node is ready for reports
        ioctl(self.h, IOCTL_WAIT_DEVICE_READY, &u32s(&[8, serial]))?;
        self.plugged.push(serial);
        Ok(serial)
    }

    /// Feed one viewer gamepad (the controller is plugged in on first use).
    pub fn write(&mut self, g: &Gamepad, max_pads: u32) -> Result<(), String> {
        if g.index as u32 >= max_pads {
            return Ok(());
        }
        let serial = self.ensure(g.index)?;
        ioctl(
            self.h,
            IOCTL_XUSB_SUBMIT_REPORT,
            &submit_report(serial, &hxdrv::xusb_report(g)),
        )
    }
}
