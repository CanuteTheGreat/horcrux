//! Gamepads on the viewer's machine, read straight from evdev
//! (`/dev/input/event*` devices with gamepad buttons; the user's session
//! normally has access through logind/udev ACLs), mapped to the W3C
//! "standard gamepad" layout the Horcrux Console `pad` channel uses.
use std::io::Read;
use std::os::unix::io::AsRawFd;

const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const BTN_GAMEPAD: usize = 0x130;

const fn ioc_read(nr: u32, size: u32) -> libc::c_ulong {
    ((2u32 << 30) | (size << 16) | ((b'E' as u32) << 8) | nr) as libc::c_ulong
}

/// W3C standard button index for an evdev key code.
pub fn button_index(code: u16) -> Option<u32> {
    Some(match code {
        0x130 => 0,  // BTN_SOUTH (A)
        0x131 => 1,  // BTN_EAST (B)
        0x134 => 2,  // BTN_WEST (X)
        0x133 => 3,  // BTN_NORTH (Y)
        0x136 => 4,  // BTN_TL
        0x137 => 5,  // BTN_TR
        0x138 => 6,  // BTN_TL2 (digital trigger)
        0x139 => 7,  // BTN_TR2
        0x13a => 8,  // BTN_SELECT
        0x13b => 9,  // BTN_START
        0x13d => 10, // BTN_THUMBL
        0x13e => 11, // BTN_THUMBR
        0x220 => 12, // BTN_DPAD_UP
        0x221 => 13, // BTN_DPAD_DOWN
        0x222 => 14, // BTN_DPAD_LEFT
        0x223 => 15, // BTN_DPAD_RIGHT
        0x13c => 16, // BTN_MODE
        _ => return None,
    })
}

#[derive(Debug, Default, Clone, Copy)]
struct Abs {
    min: i32,
    max: i32,
}

impl Abs {
    fn to_i16(self, v: i32) -> i16 {
        if self.max <= self.min {
            return 0;
        }
        let t = (v - self.min) as f64 / (self.max - self.min) as f64;
        (t * 65535.0 - 32768.0).clamp(-32768.0, 32767.0) as i16
    }
    fn to_u8(self, v: i32) -> u8 {
        if self.max <= self.min {
            return 0;
        }
        (((v - self.min) as f64 / (self.max - self.min) as f64) * 255.0).clamp(0.0, 255.0) as u8
    }
}

/// One pad's state; `packet` is the `pad` channel encoding.
#[derive(Debug, Default, Clone)]
pub struct Pad {
    pub index: u8,
    pub buttons: u32,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
    pub lt: u8,
    pub rt: u8,
    abs: [Abs; 6],
}

impl Pad {
    pub fn packet(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0] = self.index;
        b[1..5].copy_from_slice(&self.buttons.to_le_bytes());
        b[5..7].copy_from_slice(&self.lx.to_le_bytes());
        b[7..9].copy_from_slice(&self.ly.to_le_bytes());
        b[9..11].copy_from_slice(&self.rx.to_le_bytes());
        b[11..13].copy_from_slice(&self.ry.to_le_bytes());
        b[13] = self.lt;
        b[14] = self.rt;
        b
    }

    /// Apply one evdev event; true when the state changed.
    pub fn apply(&mut self, ty: u16, code: u16, value: i32) -> bool {
        let before = self.packet();
        match ty {
            EV_KEY => {
                if let Some(i) = button_index(code) {
                    if value != 0 {
                        self.buttons |= 1 << i;
                    } else {
                        self.buttons &= !(1 << i);
                    }
                }
            }
            EV_ABS => {
                let a = self.abs.get(code as usize).copied().unwrap_or_default();
                match code {
                    0x00 => self.lx = a.to_i16(value),
                    0x01 => self.ly = a.to_i16(value),
                    0x03 => self.rx = a.to_i16(value),
                    0x04 => self.ry = a.to_i16(value),
                    0x02 => self.lt = a.to_u8(value),
                    0x05 => self.rt = a.to_u8(value),
                    // hat: d-pad buttons 12..15
                    0x10 => {
                        self.buttons &= !((1 << 14) | (1 << 15));
                        if value < 0 {
                            self.buttons |= 1 << 14;
                        } else if value > 0 {
                            self.buttons |= 1 << 15;
                        }
                    }
                    0x11 => {
                        self.buttons &= !((1 << 12) | (1 << 13));
                        if value < 0 {
                            self.buttons |= 1 << 12;
                        } else if value > 0 {
                            self.buttons |= 1 << 13;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        before != self.packet()
    }
}

fn is_gamepad(f: &std::fs::File) -> bool {
    let mut bits = [0u8; 96];
    // SAFETY: EVIOCGBIT(EV_KEY) into a buffer of the size given.
    let r = unsafe {
        libc::ioctl(
            f.as_raw_fd(),
            ioc_read(0x20 + EV_KEY as u32, bits.len() as u32),
            bits.as_mut_ptr(),
        )
    };
    r > 0 && bits[BTN_GAMEPAD / 8] & (1 << (BTN_GAMEPAD % 8)) != 0
}

fn abs_info(f: &std::fs::File, code: u32) -> Abs {
    let mut info = [0i32; 6];
    // SAFETY: EVIOCGABS(code) fills a struct input_absinfo (6 x i32).
    let r = unsafe { libc::ioctl(f.as_raw_fd(), ioc_read(0x40 + code, 24), info.as_mut_ptr()) };
    if r < 0 {
        return Abs::default();
    }
    Abs {
        min: info[1],
        max: info[2],
    }
}

/// Start one reader thread per gamepad found (up to 4); every change calls
/// `send` with the 16-byte `pad` packet.
pub fn spawn(dir: &str, send: impl Fn(&[u8; 16]) + Send + Sync + Clone + 'static) -> usize {
    let mut n = 0u8;
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    paths.sort();
    for p in paths {
        if n >= 4
            || !p
                .file_name()
                .map(|f| f.to_string_lossy().starts_with("event"))
                .unwrap_or(false)
        {
            continue;
        }
        let Ok(mut f) = std::fs::File::open(&p) else {
            continue;
        };
        if !is_gamepad(&f) {
            continue;
        }
        let mut pad = Pad {
            index: n,
            ..Default::default()
        };
        for code in [0u32, 1, 2, 3, 4, 5] {
            pad.abs[code as usize] = abs_info(&f, code);
        }
        let send = send.clone();
        std::thread::spawn(move || {
            let mut ev = [0u8; 24];
            while f.read_exact(&mut ev).is_ok() {
                let ty = u16::from_ne_bytes([ev[16], ev[17]]);
                let code = u16::from_ne_bytes([ev[18], ev[19]]);
                let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
                if pad.apply(ty, code, value) {
                    send(&pad.packet());
                }
            }
        });
        n += 1;
    }
    n as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evdev_to_standard_pad() {
        let mut p = Pad::default();
        p.abs[0] = Abs {
            min: -32768,
            max: 32767,
        };
        p.abs[2] = Abs { min: 0, max: 1023 };
        assert!(p.apply(EV_KEY, 0x130, 1));
        assert_eq!(p.buttons, 1);
        assert!(!p.apply(EV_KEY, 0x130, 1), "no change");
        p.apply(EV_ABS, 0x00, 32767);
        assert_eq!(p.lx, 32767);
        p.apply(EV_ABS, 0x02, 1023);
        assert_eq!(p.lt, 255);
        p.apply(EV_ABS, 0x11, -1);
        assert_eq!(p.buttons & (1 << 12), 1 << 12);
        p.apply(EV_ABS, 0x11, 0);
        assert_eq!(p.buttons & (1 << 12), 0);
        let b = p.packet();
        assert_eq!(b.len(), 16);
        assert_eq!(u32::from_le_bytes(b[1..5].try_into().unwrap()), 1);
        assert_eq!(ioc_read(0x21, 96), 0x8060_4521);
    }
}
