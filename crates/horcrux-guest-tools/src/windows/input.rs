//! Input injection with SendInput.
use super::layout;
use horcrux_guest_proto::Input;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn send(i: INPUT) {
    // SAFETY: a single valid INPUT and its true size.
    unsafe {
        SendInput(&[i], std::mem::size_of::<INPUT>() as i32);
    }
}

fn kbd(scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mouse(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Wheel units are WHEEL_DELTA (120 = one notch), positive = away from the
/// user (up); `scale_pct` scales (negative inverts).
pub fn inject(i: &Input, scale_pct: i32) {
    match *i {
        Input::Key { qnum, down } => {
            let mut f = KEYEVENTF_SCANCODE;
            if qnum & 0x80 != 0 {
                f |= KEYEVENTF_EXTENDEDKEY;
            }
            if !down {
                f |= KEYEVENTF_KEYUP;
            }
            send(kbd(qnum & 0x7f, f));
        }
        Input::Button { button, down } => {
            let (flags, data) = match (button, down) {
                (0, true) => (MOUSEEVENTF_LEFTDOWN, 0),
                (0, false) => (MOUSEEVENTF_LEFTUP, 0),
                (1, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
                (1, false) => (MOUSEEVENTF_MIDDLEUP, 0),
                (2, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
                (2, false) => (MOUSEEVENTF_RIGHTUP, 0),
                (3, d) => (if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, XBUTTON1 as i32),
                (4, d) => (if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, XBUTTON2 as i32),
                _ => return,
            };
            send(mouse(0, 0, data, flags));
        }
        Input::Rel { dx, dy } => send(mouse(dx, dy, 0, MOUSEEVENTF_MOVE)),
        Input::Abs { x, y, monitor } => {
            let mons = layout::monitors();
            let Some(m) = mons.get(monitor as usize).or(mons.first()) else {
                return;
            };
            // SAFETY: plain metric queries.
            let (vx, vy, vw, vh) = unsafe {
                (
                    GetSystemMetrics(SM_XVIRTUALSCREEN),
                    GetSystemMetrics(SM_YVIRTUALSCREEN),
                    GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2),
                    GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2),
                )
            };
            let px = m.x + (x as i32).clamp(0, m.w as i32 - 1);
            let py = m.y + (y as i32).clamp(0, m.h as i32 - 1);
            // Normalised over the virtual desktop; +vw/2 rounds to nearest.
            let nx = ((px - vx) as i64 * 65535 + (vw as i64 - 1) / 2) / (vw as i64 - 1);
            let ny = ((py - vy) as i64 * 65535 + (vh as i64 - 1) / 2) / (vh as i64 - 1);
            send(mouse(
                nx as i32,
                ny as i32,
                0,
                MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            ));
        }
        Input::Wheel { dx, dy } => {
            let s = |v: i32| (v as i64 * scale_pct as i64 / 100) as i32;
            if dy != 0 {
                send(mouse(0, 0, s(dy), MOUSEEVENTF_WHEEL));
            }
            if dx != 0 {
                send(mouse(0, 0, s(dx), MOUSEEVENTF_HWHEEL));
            }
        }
        Input::Gamepad(_) => {}
    }
}
