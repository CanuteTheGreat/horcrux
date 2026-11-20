//! Virtual input devices through /dev/uinput: a keyboard + relative mouse
//! (raw motion for games), an absolute pointer (desktop) and gamepads
//! (evdev gamepad layout, so SDL/Steam/Proton see a normal controller).
use horcrux_guest_proto::{qnum_to_evdev, Gamepad, Input};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const ABS_Z: u16 = 2;
const ABS_RX: u16 = 3;
const ABS_RY: u16 = 4;
const ABS_RZ: u16 = 5;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;
const BTN_LEFT: u16 = 0x110;
const BTN_TOUCH: u16 = 0x14a;
const ABS_MAX: i32 = 65535;

// ioctl numbers (asm-generic _IOW('U', n, T))
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_SET_RELBIT: libc::c_ulong = 0x4004_5566;
const UI_SET_ABSBIT: libc::c_ulong = 0x4004_5567;
const UI_SET_PROPBIT: libc::c_ulong = 0x4004_556e;
const UI_DEV_SETUP: libc::c_ulong = 0x405c_5503;
const UI_ABS_SETUP: libc::c_ulong = 0x401c_5504;
const INPUT_PROP_DIRECT: libc::c_int = 1;

/// W3C standard gamepad button index -> evdev code (index 12-15 are the
/// d-pad, sent as the hat axes instead).
const PAD_BUTTONS: [(u32, u16); 13] = [
    (0, 0x130),  // A / south
    (1, 0x131),  // B / east
    (2, 0x134),  // X / west
    (3, 0x133),  // Y / north
    (4, 0x136),  // LB
    (5, 0x137),  // RB
    (6, 0x138),  // LT (digital)
    (7, 0x139),  // RT (digital)
    (8, 0x13a),  // back / select
    (9, 0x13b),  // start
    (10, 0x13d), // left stick
    (11, 0x13e), // right stick
    (16, 0x13c), // home / mode
];

#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[repr(C)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

#[repr(C)]
struct UinputAbsSetup {
    code: u16,
    absinfo: AbsInfo,
}

#[repr(C)]
struct InputEvent {
    time: libc::timeval,
    kind: u16,
    code: u16,
    value: i32,
}

struct Dev {
    f: File,
}

fn ioctl_int(f: &File, req: libc::c_ulong, v: libc::c_int) -> Result<(), String> {
    let r = unsafe { libc::ioctl(f.as_raw_fd(), req as _, v) };
    if r < 0 {
        return Err(format!(
            "uinput ioctl {req:#x}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

impl Dev {
    fn create(
        name: &str,
        product: u16,
        setup: impl Fn(&File) -> Result<(), String>,
    ) -> Result<Self, String> {
        let f = OpenOptions::new()
            .write(true)
            .custom_flags_nonblock()
            .open("/dev/uinput")
            .map_err(|e| format!("/dev/uinput: {e}"))?;
        setup(&f)?;
        let mut s = UinputSetup {
            // BUS_VIRTUAL
            id: InputId {
                bustype: 0x06,
                vendor: 0x1d6b,
                product,
                version: 1,
            },
            name: [0; 80],
            ff_effects_max: 0,
        };
        for (i, b) in name.bytes().take(79).enumerate() {
            s.name[i] = b;
        }
        let r = unsafe { libc::ioctl(f.as_raw_fd(), UI_DEV_SETUP as _, &s as *const UinputSetup) };
        if r < 0 {
            return Err(format!("UI_DEV_SETUP: {}", std::io::Error::last_os_error()));
        }
        let r = unsafe { libc::ioctl(f.as_raw_fd(), UI_DEV_CREATE as _) };
        if r < 0 {
            return Err(format!(
                "UI_DEV_CREATE: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self { f })
    }

    fn abs(f: &File, code: u16, min: i32, max: i32) -> Result<(), String> {
        ioctl_int(f, UI_SET_ABSBIT, code as libc::c_int)?;
        let s = UinputAbsSetup {
            code,
            absinfo: AbsInfo {
                value: 0,
                minimum: min,
                maximum: max,
                fuzz: 0,
                flat: 0,
                resolution: 0,
            },
        };
        let r = unsafe {
            libc::ioctl(
                f.as_raw_fd(),
                UI_ABS_SETUP as _,
                &s as *const UinputAbsSetup,
            )
        };
        if r < 0 {
            return Err(format!("UI_ABS_SETUP: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn emit(&mut self, evs: &[(u16, u16, i32)]) -> Result<(), String> {
        let mut buf = Vec::with_capacity((evs.len() + 1) * std::mem::size_of::<InputEvent>());
        for &(kind, code, value) in evs.iter().chain(std::iter::once(&(EV_SYN, 0, 0))) {
            let e = InputEvent {
                time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                kind,
                code,
                value,
            };
            let p = &e as *const InputEvent as *const u8;
            buf.extend_from_slice(unsafe {
                std::slice::from_raw_parts(p, std::mem::size_of::<InputEvent>())
            });
        }
        self.f.write_all(&buf).map_err(|e| e.to_string())
    }
}

trait NonBlock {
    fn custom_flags_nonblock(&mut self) -> &mut Self;
}
impl NonBlock for OpenOptions {
    fn custom_flags_nonblock(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.custom_flags(libc::O_NONBLOCK)
    }
}

pub struct Devices {
    name: String,
    kbd_mouse: Dev,
    tablet: Dev,
    pads: Vec<Option<(Dev, Gamepad)>>,
}

impl Devices {
    pub fn create(name: &str) -> Result<Self, String> {
        let kbd_mouse = Dev::create(name, 1, |f| {
            ioctl_int(f, UI_SET_EVBIT, EV_KEY as _)?;
            ioctl_int(f, UI_SET_EVBIT, EV_REL as _)?;
            for k in 1..=248 {
                ioctl_int(f, UI_SET_KEYBIT, k)?;
            }
            for b in BTN_LEFT..=BTN_LEFT + 4 {
                ioctl_int(f, UI_SET_KEYBIT, b as _)?;
            }
            for r in [REL_X, REL_Y, REL_WHEEL, REL_HWHEEL] {
                ioctl_int(f, UI_SET_RELBIT, r as _)?;
            }
            Ok(())
        })?;
        let tablet = Dev::create(&format!("{name} pointer"), 2, |f| {
            ioctl_int(f, UI_SET_EVBIT, EV_KEY as _)?;
            ioctl_int(f, UI_SET_EVBIT, EV_ABS as _)?;
            ioctl_int(f, UI_SET_PROPBIT, INPUT_PROP_DIRECT)?;
            for b in BTN_LEFT..=BTN_LEFT + 4 {
                ioctl_int(f, UI_SET_KEYBIT, b as _)?;
            }
            ioctl_int(f, UI_SET_KEYBIT, BTN_TOUCH as _)?;
            Dev::abs(f, ABS_X, 0, ABS_MAX)?;
            Dev::abs(f, ABS_Y, 0, ABS_MAX)
        })?;
        Ok(Self {
            name: name.to_string(),
            kbd_mouse,
            tablet,
            pads: Vec::new(),
        })
    }

    fn pad(&mut self, index: u8) -> Result<&mut (Dev, Gamepad), String> {
        let i = index as usize;
        if i >= 4 {
            return Err("at most 4 gamepads".into());
        }
        while self.pads.len() <= i {
            self.pads.push(None);
        }
        if self.pads[i].is_none() {
            let d = Dev::create(
                &format!("{} gamepad {}", self.name, i + 1),
                0x10 + index as u16,
                |f| {
                    ioctl_int(f, UI_SET_EVBIT, EV_KEY as _)?;
                    ioctl_int(f, UI_SET_EVBIT, EV_ABS as _)?;
                    for (_, code) in PAD_BUTTONS {
                        ioctl_int(f, UI_SET_KEYBIT, code as _)?;
                    }
                    for a in [ABS_X, ABS_Y, ABS_RX, ABS_RY] {
                        Dev::abs(f, a, -32768, 32767)?;
                    }
                    for a in [ABS_Z, ABS_RZ] {
                        Dev::abs(f, a, 0, 255)?;
                    }
                    for a in [ABS_HAT0X, ABS_HAT0Y] {
                        Dev::abs(f, a, -1, 1)?;
                    }
                    Ok(())
                },
            )?;
            self.pads[i] = Some((d, Gamepad::default()));
        }
        Ok(self.pads[i].as_mut().unwrap())
    }

    /// `size` is the captured monitor's size, for absolute positions.
    pub fn inject(&mut self, i: &Input, size: (u32, u32)) -> Result<(), String> {
        match *i {
            Input::Key { qnum, down } => match qnum_to_evdev(qnum) {
                Some(k) => self.kbd_mouse.emit(&[(EV_KEY, k, down as i32)]),
                None => Ok(()),
            },
            Input::Button { button, down } => {
                let code = match button {
                    0 => BTN_LEFT,
                    1 => BTN_LEFT + 2,
                    2 => BTN_LEFT + 1,
                    3 => BTN_LEFT + 3,
                    4 => BTN_LEFT + 4,
                    _ => return Ok(()),
                };
                self.kbd_mouse.emit(&[(EV_KEY, code, down as i32)])
            }
            Input::Rel { dx, dy } => self
                .kbd_mouse
                .emit(&[(EV_REL, REL_X, dx), (EV_REL, REL_Y, dy)]),
            Input::Wheel { dx, dy } => {
                let mut ev = Vec::new();
                if dy != 0 {
                    ev.push((EV_REL, REL_WHEEL, -dy.signum()));
                }
                if dx != 0 {
                    ev.push((EV_REL, REL_HWHEEL, dx.signum()));
                }
                self.kbd_mouse.emit(&ev)
            }
            Input::Abs { x, y, .. } => {
                let (w, h) = (size.0.max(1), size.1.max(1));
                let sx = ((x.min(w - 1) as u64 * ABS_MAX as u64) / (w - 1).max(1) as u64) as i32;
                let sy = ((y.min(h - 1) as u64 * ABS_MAX as u64) / (h - 1).max(1) as u64) as i32;
                self.tablet
                    .emit(&[(EV_ABS, ABS_X, sx), (EV_ABS, ABS_Y, sy)])
            }
            Input::Gamepad(g) => {
                let (dev, last) = self.pad(g.index)?;
                let mut ev = Vec::new();
                for (bit, code) in PAD_BUTTONS {
                    let now = g.buttons >> bit & 1;
                    if now != last.buttons >> bit & 1 {
                        ev.push((EV_KEY, code, now as i32));
                    }
                }
                let hat = |neg: u32, pos: u32| {
                    (g.buttons >> pos & 1) as i32 - (g.buttons >> neg & 1) as i32
                };
                ev.extend([
                    (EV_ABS, ABS_X, g.lx as i32),
                    (EV_ABS, ABS_Y, g.ly as i32),
                    (EV_ABS, ABS_RX, g.rx as i32),
                    (EV_ABS, ABS_RY, g.ry as i32),
                    (EV_ABS, ABS_Z, g.lt as i32),
                    (EV_ABS, ABS_RZ, g.rt as i32),
                    (EV_ABS, ABS_HAT0X, hat(14, 15)),
                    (EV_ABS, ABS_HAT0Y, hat(12, 13)),
                ]);
                *last = g;
                dev.emit(&ev)
            }
        }
    }
}
