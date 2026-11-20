//! Gamepads: read through evdev on Linux (no libudev needed), mapped to the
//! `pad` channel's 16-byte state (the web client's W3C standard layout).
//! Other systems: none yet (the window still works; see the design doc).

/// W3C "standard" gamepad button bits.
pub mod bit {
    pub const A: u32 = 1 << 0;
    pub const B: u32 = 1 << 1;
    pub const X: u32 = 1 << 2;
    pub const Y: u32 = 1 << 3;
    pub const LB: u32 = 1 << 4;
    pub const RB: u32 = 1 << 5;
    pub const BACK: u32 = 1 << 8;
    pub const START: u32 = 1 << 9;
    pub const LS: u32 = 1 << 10;
    pub const RS: u32 = 1 << 11;
    pub const UP: u32 = 1 << 12;
    pub const DOWN: u32 = 1 << 13;
    pub const LEFT: u32 = 1 << 14;
    pub const RIGHT: u32 = 1 << 15;
    pub const GUIDE: u32 = 1 << 16;
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pad {
    pub index: u8,
    pub buttons: u32,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
    pub lt: u8,
    pub rt: u8,
}

impl Pad {
    pub fn bytes(&self) -> [u8; 16] {
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
}

/// Scale a device axis value in `min..=max` to i16.
pub fn axis(v: i32, min: i32, max: i32) -> i16 {
    if max <= min {
        return 0;
    }
    let span = (max - min) as i64;
    let c = (v.clamp(min, max) - min) as i64;
    ((c * 65535 / span) - 32768).clamp(-32768, 32767) as i16
}

pub fn trigger(v: i32, min: i32, max: i32) -> u8 {
    if max <= min {
        return 0;
    }
    (((v.clamp(min, max) - min) as i64 * 255) / (max - min) as i64) as u8
}

#[cfg(target_os = "linux")]
pub fn spawn(send: impl Fn(Pad) + Send + Clone + 'static) -> usize {
    use evdev::{AbsoluteAxisType as A, InputEventKind, Key};
    let mut n = 0;
    for (_, dev) in evdev::enumerate() {
        let is_pad = dev
            .supported_keys()
            .map(|k| k.contains(Key::BTN_SOUTH))
            .unwrap_or(false)
            && dev
                .supported_absolute_axes()
                .map(|a| a.contains(A::ABS_X))
                .unwrap_or(false);
        if !is_pad || n >= 4 {
            continue;
        }
        let index = n as u8;
        n += 1;
        let send = send.clone();
        std::thread::spawn(move || {
            let mut dev = dev;
            let info = dev.get_abs_state().ok();
            let range = |a: A| {
                info.as_ref()
                    .map(|s| (s[a.0 as usize].minimum, s[a.0 as usize].maximum))
                    .unwrap_or((-32768, 32767))
            };
            let mut p = Pad {
                index,
                ..Default::default()
            };
            loop {
                let Ok(evs) = dev.fetch_events() else { return };
                let mut changed = false;
                for e in evs {
                    match e.kind() {
                        InputEventKind::Key(k) => {
                            let b = match k {
                                Key::BTN_SOUTH => bit::A,
                                Key::BTN_EAST => bit::B,
                                Key::BTN_NORTH => bit::Y,
                                Key::BTN_WEST => bit::X,
                                Key::BTN_TL => bit::LB,
                                Key::BTN_TR => bit::RB,
                                Key::BTN_SELECT => bit::BACK,
                                Key::BTN_START => bit::START,
                                Key::BTN_MODE => bit::GUIDE,
                                Key::BTN_THUMBL => bit::LS,
                                Key::BTN_THUMBR => bit::RS,
                                Key::BTN_DPAD_UP => bit::UP,
                                Key::BTN_DPAD_DOWN => bit::DOWN,
                                Key::BTN_DPAD_LEFT => bit::LEFT,
                                Key::BTN_DPAD_RIGHT => bit::RIGHT,
                                _ => 0,
                            };
                            if e.value() != 0 {
                                p.buttons |= b;
                            } else {
                                p.buttons &= !b;
                            }
                            changed = true;
                        }
                        InputEventKind::AbsAxis(a) => {
                            let (lo, hi) = range(a);
                            let v = e.value();
                            match a {
                                A::ABS_X => p.lx = axis(v, lo, hi),
                                A::ABS_Y => p.ly = axis(v, lo, hi),
                                A::ABS_RX => p.rx = axis(v, lo, hi),
                                A::ABS_RY => p.ry = axis(v, lo, hi),
                                A::ABS_Z => p.lt = trigger(v, lo, hi),
                                A::ABS_RZ => p.rt = trigger(v, lo, hi),
                                A::ABS_HAT0X => {
                                    p.buttons &= !(bit::LEFT | bit::RIGHT);
                                    p.buttons |= if v < 0 {
                                        bit::LEFT
                                    } else if v > 0 {
                                        bit::RIGHT
                                    } else {
                                        0
                                    };
                                }
                                A::ABS_HAT0Y => {
                                    p.buttons &= !(bit::UP | bit::DOWN);
                                    p.buttons |= if v < 0 {
                                        bit::UP
                                    } else if v > 0 {
                                        bit::DOWN
                                    } else {
                                        0
                                    };
                                }
                                _ => {}
                            }
                            changed = true;
                        }
                        _ => {}
                    }
                }
                if changed {
                    send(p);
                }
            }
        });
    }
    n
}

#[cfg(not(target_os = "linux"))]
pub fn spawn(_send: impl Fn(Pad) + Send + Clone + 'static) -> usize {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_scaling() {
        let p = Pad {
            index: 1,
            buttons: bit::A | bit::START,
            lx: -32768,
            ly: 32767,
            rx: 1,
            ry: -1,
            lt: 255,
            rt: 7,
        };
        let b = p.bytes();
        assert_eq!(b[0], 1);
        assert_eq!(u32::from_le_bytes(b[1..5].try_into().unwrap()), 0x201);
        assert_eq!(i16::from_le_bytes([b[5], b[6]]), -32768);
        assert_eq!((b[13], b[14], b[15]), (255, 7, 0));
        assert_eq!(axis(0, 0, 255), -32768);
        assert_eq!(axis(255, 0, 255), 32767);
        assert_eq!(axis(-32768, -32768, 32767), -32768);
        assert_eq!(trigger(1023, 0, 1023), 255);
        assert_eq!(trigger(0, 0, 0), 0);
    }
}
