//! Browser key codes (`KeyboardEvent.code`, layout-independent physical
//! keys) to QEMU "qnum" key numbers, which the D-Bus display's
//! `org.qemu.Display1.Keyboard.Press/Release` take: the PC/XT set 1
//! scancode, with 0x80 added for the 0xE0-prefixed (extended) keys.
//!
//! Physical codes are sent, never characters: the guest's own keyboard
//! layout turns them into characters, exactly like a USB keyboard.

/// qnum for a `KeyboardEvent.code`, None for keys a PC keyboard lacks.
pub fn qnum(code: &str) -> Option<u32> {
    let n = match code {
        "Escape" => 0x01,
        "Digit1" => 0x02,
        "Digit2" => 0x03,
        "Digit3" => 0x04,
        "Digit4" => 0x05,
        "Digit5" => 0x06,
        "Digit6" => 0x07,
        "Digit7" => 0x08,
        "Digit8" => 0x09,
        "Digit9" => 0x0a,
        "Digit0" => 0x0b,
        "Minus" => 0x0c,
        "Equal" => 0x0d,
        "Backspace" => 0x0e,
        "Tab" => 0x0f,
        "KeyQ" => 0x10,
        "KeyW" => 0x11,
        "KeyE" => 0x12,
        "KeyR" => 0x13,
        "KeyT" => 0x14,
        "KeyY" => 0x15,
        "KeyU" => 0x16,
        "KeyI" => 0x17,
        "KeyO" => 0x18,
        "KeyP" => 0x19,
        "BracketLeft" => 0x1a,
        "BracketRight" => 0x1b,
        "Enter" => 0x1c,
        "ControlLeft" => 0x1d,
        "KeyA" => 0x1e,
        "KeyS" => 0x1f,
        "KeyD" => 0x20,
        "KeyF" => 0x21,
        "KeyG" => 0x22,
        "KeyH" => 0x23,
        "KeyJ" => 0x24,
        "KeyK" => 0x25,
        "KeyL" => 0x26,
        "Semicolon" => 0x27,
        "Quote" => 0x28,
        "Backquote" => 0x29,
        "ShiftLeft" => 0x2a,
        "Backslash" => 0x2b,
        "KeyZ" => 0x2c,
        "KeyX" => 0x2d,
        "KeyC" => 0x2e,
        "KeyV" => 0x2f,
        "KeyB" => 0x30,
        "KeyN" => 0x31,
        "KeyM" => 0x32,
        "Comma" => 0x33,
        "Period" => 0x34,
        "Slash" => 0x35,
        "ShiftRight" => 0x36,
        "NumpadMultiply" => 0x37,
        "AltLeft" => 0x38,
        "Space" => 0x39,
        "CapsLock" => 0x3a,
        "F1" => 0x3b,
        "F2" => 0x3c,
        "F3" => 0x3d,
        "F4" => 0x3e,
        "F5" => 0x3f,
        "F6" => 0x40,
        "F7" => 0x41,
        "F8" => 0x42,
        "F9" => 0x43,
        "F10" => 0x44,
        "NumLock" => 0x45,
        "ScrollLock" => 0x46,
        "Numpad7" => 0x47,
        "Numpad8" => 0x48,
        "Numpad9" => 0x49,
        "NumpadSubtract" => 0x4a,
        "Numpad4" => 0x4b,
        "Numpad5" => 0x4c,
        "Numpad6" => 0x4d,
        "NumpadAdd" => 0x4e,
        "Numpad1" => 0x4f,
        "Numpad2" => 0x50,
        "Numpad3" => 0x51,
        "Numpad0" => 0x52,
        "NumpadDecimal" => 0x53,
        "IntlBackslash" => 0x56,
        "F11" => 0x57,
        "F12" => 0x58,
        "IntlRo" => 0x73,
        "IntlYen" => 0x7d,
        // 0xE0-prefixed keys
        "NumpadEnter" => 0x9c,
        "ControlRight" => 0x9d,
        "NumpadDivide" => 0xb5,
        "PrintScreen" => 0xb7,
        "AltRight" => 0xb8,
        "Home" => 0xc7,
        "ArrowUp" => 0xc8,
        "PageUp" => 0xc9,
        "ArrowLeft" => 0xcb,
        "ArrowRight" => 0xcd,
        "End" => 0xcf,
        "ArrowDown" => 0xd0,
        "PageDown" => 0xd1,
        "Insert" => 0xd2,
        "Delete" => 0xd3,
        "MetaLeft" => 0xdb,
        "MetaRight" => 0xdc,
        "ContextMenu" => 0xdd,
        "Pause" => 0xc6,
        _ => return None,
    };
    Some(n)
}

/// QEMU's `InputButton` numbering (qapi/ui.json) for a DOM `MouseEvent.button`.
pub fn mouse_button(dom: u8) -> Option<u32> {
    match dom {
        0 => Some(0), // left
        1 => Some(1), // middle
        2 => Some(2), // right
        3 => Some(7), // back (side)
        4 => Some(8), // forward (extra)
        _ => None,
    }
}

/// Wheel "buttons": up 3, down 4, left 5, right 6.
pub const WHEEL_UP: u32 = 3;
pub const WHEEL_DOWN: u32 = 4;
pub const WHEEL_LEFT: u32 = 5;
pub const WHEEL_RIGHT: u32 = 6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_keys() {
        assert_eq!(qnum("KeyA"), Some(0x1e));
        assert_eq!(qnum("Enter"), Some(0x1c));
        assert_eq!(qnum("ArrowUp"), Some(0xc8));
        assert_eq!(qnum("ControlRight"), Some(0x9d));
        assert_eq!(qnum("Fn"), None);
    }

    #[test]
    fn no_duplicates() {
        let codes = [
            "Escape",
            "KeyA",
            "KeyZ",
            "Digit0",
            "Enter",
            "NumpadEnter",
            "ControlLeft",
            "ControlRight",
            "AltLeft",
            "AltRight",
            "ArrowUp",
            "Numpad8",
            "Delete",
            "NumpadDecimal",
            "MetaLeft",
            "F12",
            "Space",
        ];
        let mut seen = std::collections::HashSet::new();
        for c in codes {
            assert!(seen.insert(qnum(c).unwrap()), "{c} collides");
        }
    }

    #[test]
    fn buttons() {
        assert_eq!(mouse_button(0), Some(0));
        assert_eq!(mouse_button(2), Some(2));
        assert_eq!(mouse_button(9), None);
    }
}
