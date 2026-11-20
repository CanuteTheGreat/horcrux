//! Gamepads, mapped to the `pad` channel's 16-byte state (the web client's
//! W3C standard layout):
//! * Linux: evdev events (no libudev needed);
//! * Windows: XInput, polled (`xinput1_4.dll`, else `xinput9_1_0.dll`,
//!   loaded at run time; the Guide button through XInputGetStateEx where
//!   the DLL has it);
//! * macOS: GameController.framework (extended gamepads), polled.

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
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn axis(v: i32, min: i32, max: i32) -> i16 {
    if max <= min {
        return 0;
    }
    let span = (max - min) as i64;
    let c = (v.clamp(min, max) - min) as i64;
    ((c * 65535 / span) - 32768).clamp(-32768, 32767) as i16
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn trigger(v: i32, min: i32, max: i32) -> u8 {
    if max <= min {
        return 0;
    }
    (((v.clamp(min, max) - min) as i64 * 255) / (max - min) as i64) as u8
}

#[cfg(target_os = "linux")]
pub fn spawn(send: impl Fn(Pad) + Send + Clone + 'static, _poll_hz: u32) -> usize {
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

/// XInput's `XINPUT_GAMEPAD`.
#[cfg_attr(not(windows), allow(dead_code))]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct XGamepad {
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
}

/// XInput stick Y grows upwards, the W3C layout (and evdev) downwards.
#[cfg_attr(not(windows), allow(dead_code))]
fn flip(v: i16) -> i16 {
    (-(v as i32)).clamp(-32768, 32767) as i16
}

/// An XInput state in the pad layout. `guide`: XInputGetStateEx's 0x0400.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn from_xinput(index: u8, g: &XGamepad) -> Pad {
    const MAP: [(u16, u32); 15] = [
        (0x0001, bit::UP),
        (0x0002, bit::DOWN),
        (0x0004, bit::LEFT),
        (0x0008, bit::RIGHT),
        (0x0010, bit::START),
        (0x0020, bit::BACK),
        (0x0040, bit::LS),
        (0x0080, bit::RS),
        (0x0100, bit::LB),
        (0x0200, bit::RB),
        (0x0400, bit::GUIDE),
        (0x1000, bit::A),
        (0x2000, bit::B),
        (0x4000, bit::X),
        (0x8000, bit::Y),
    ];
    Pad {
        index,
        buttons: MAP
            .iter()
            .filter(|(x, _)| g.buttons & x != 0)
            .fold(0, |b, (_, w)| b | w),
        lx: g.thumb_lx,
        ly: flip(g.thumb_ly),
        rx: g.thumb_rx,
        ry: flip(g.thumb_ry),
        lt: g.left_trigger,
        rt: g.right_trigger,
    }
}

/// A float axis (-1..1, up positive when `invert`) as i16.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn axis_f32(v: f32, invert: bool) -> i16 {
    let v = if invert { -v } else { v };
    (v.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn trigger_f32(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(windows)]
pub fn spawn(send: impl Fn(Pad) + Send + Clone + 'static, poll_hz: u32) -> usize {
    use windows::core::{s, w, PCSTR};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    #[repr(C)]
    #[derive(Default)]
    struct XState {
        packet: u32,
        pad: XGamepad,
    }
    type GetState = unsafe extern "system" fn(u32, *mut XState) -> u32;
    // SAFETY: loading a system DLL and looking up its exported functions.
    let get: Option<GetState> = unsafe {
        let lib =
            LoadLibraryW(w!("xinput1_4.dll")).or_else(|_| LoadLibraryW(w!("xinput9_1_0.dll")));
        lib.ok().and_then(|h| {
            // ordinal 100: XInputGetStateEx (adds the Guide button)
            GetProcAddress(h, PCSTR(100 as *const u8))
                .or_else(|| GetProcAddress(h, s!("XInputGetState")))
                .map(|f| std::mem::transmute::<_, GetState>(f))
        })
    };
    let Some(get) = get else {
        tracing::warn!("no XInput DLL: gamepads are not read");
        return 0;
    };
    let period = std::time::Duration::from_micros(1_000_000 / poll_hz.max(1) as u64);
    std::thread::spawn(move || {
        let mut last: [Option<u32>; 4] = [None; 4];
        loop {
            for i in 0..4u32 {
                let mut st = XState::default();
                // SAFETY: XInputGetState(Ex) fills the state it is given.
                let r = unsafe { get(i, &mut st) };
                if r == 0 {
                    if last[i as usize] != Some(st.packet) {
                        last[i as usize] = Some(st.packet);
                        send(from_xinput(i as u8, &st.pad));
                    }
                } else if last[i as usize].take().is_some() {
                    // unplugged: release everything it held
                    send(Pad {
                        index: i as u8,
                        ..Default::default()
                    });
                }
            }
            std::thread::sleep(period);
        }
    });
    // XInput slots are read whether or not a pad is plugged in now
    4
}

#[cfg(target_os = "macos")]
pub fn spawn(send: impl Fn(Pad) + Send + Clone + 'static, poll_hz: u32) -> usize {
    use objc2_game_controller::GCController;
    let period = std::time::Duration::from_micros(1_000_000 / poll_hz.max(1) as u64);
    std::thread::spawn(move || {
        let mut last: Vec<Option<Pad>> = vec![None; 4];
        loop {
            // SAFETY: GameController reads of connected controllers.
            unsafe {
                let ctrls = GCController::controllers();
                let mut seen = [false; 4];
                for i in 0..ctrls.count().min(4) {
                    let c = ctrls.objectAtIndex(i);
                    let Some(g) = c.extendedGamepad() else {
                        continue;
                    };
                    seen[i] = true;
                    let mut b = 0u32;
                    let mut set = |on: bool, bit: u32| {
                        if on {
                            b |= bit
                        }
                    };
                    set(g.buttonA().isPressed(), bit::A);
                    set(g.buttonB().isPressed(), bit::B);
                    set(g.buttonX().isPressed(), bit::X);
                    set(g.buttonY().isPressed(), bit::Y);
                    set(g.leftShoulder().isPressed(), bit::LB);
                    set(g.rightShoulder().isPressed(), bit::RB);
                    set(g.buttonMenu().isPressed(), bit::START);
                    if let Some(o) = g.buttonOptions() {
                        set(o.isPressed(), bit::BACK);
                    }
                    if let Some(h) = g.buttonHome() {
                        set(h.isPressed(), bit::GUIDE);
                    }
                    if let Some(l) = g.leftThumbstickButton() {
                        set(l.isPressed(), bit::LS);
                    }
                    if let Some(r) = g.rightThumbstickButton() {
                        set(r.isPressed(), bit::RS);
                    }
                    let d = g.dpad();
                    set(d.up().isPressed(), bit::UP);
                    set(d.down().isPressed(), bit::DOWN);
                    set(d.left().isPressed(), bit::LEFT);
                    set(d.right().isPressed(), bit::RIGHT);
                    let (l, r) = (g.leftThumbstick(), g.rightThumbstick());
                    let p = Pad {
                        index: i as u8,
                        buttons: b,
                        lx: axis_f32(l.xAxis().value(), false),
                        ly: axis_f32(l.yAxis().value(), true),
                        rx: axis_f32(r.xAxis().value(), false),
                        ry: axis_f32(r.yAxis().value(), true),
                        lt: trigger_f32(g.leftTrigger().value()),
                        rt: trigger_f32(g.rightTrigger().value()),
                    };
                    if last[i] != Some(p) {
                        last[i] = Some(p);
                        send(p);
                    }
                }
                for (i, s) in seen.iter().enumerate() {
                    if !s && last[i].take().is_some() {
                        send(Pad {
                            index: i as u8,
                            ..Default::default()
                        });
                    }
                }
            }
            std::thread::sleep(period);
        }
    });
    4
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub fn spawn(_send: impl Fn(Pad) + Send + Clone + 'static, _poll_hz: u32) -> usize {
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

    #[test]
    fn xinput_and_float_axes() {
        let g = XGamepad {
            buttons: 0x1000 | 0x0010 | 0x0001 | 0x0400 | 0x0200,
            left_trigger: 255,
            right_trigger: 3,
            thumb_lx: -32768,
            thumb_ly: -32768,
            thumb_rx: 100,
            thumb_ry: 32767,
        };
        let p = from_xinput(2, &g);
        assert_eq!(p.index, 2);
        assert_eq!(
            p.buttons,
            bit::A | bit::START | bit::UP | bit::GUIDE | bit::RB
        );
        // XInput up is positive, the pad layout's down is
        assert_eq!((p.lx, p.ly, p.rx, p.ry), (-32768, 32767, 100, -32767));
        assert_eq!((p.lt, p.rt), (255, 3));
        assert_eq!(from_xinput(0, &XGamepad::default()), Pad::default());
        assert_eq!(axis_f32(1.0, false), 32767);
        assert_eq!(axis_f32(1.0, true), -32767);
        assert_eq!(axis_f32(-3.0, false), -32767);
        assert_eq!(trigger_f32(0.5), 128);
        assert_eq!(trigger_f32(2.0), 255);
    }
}
