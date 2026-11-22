//! Where client keyboard and pointer input goes.
//!
//! While the guest tools run and offer their own input devices (Linux
//! uinput keyboard, mouse and tablet), keys, buttons, the wheel and the
//! absolute pointer go to them: an event reaches the guest OS directly
//! instead of through QEMU's emulated PS/2 keyboard, whose controller
//! emulation and guest interrupt handling cost several milliseconds per
//! key. Before the tools start (firmware, boot loader, disk unlock), when
//! they stop, when they do not offer it (Windows, whose injected input does
//! not reach the secure desktop) or when the setting is off, QEMU's input
//! is used. A key or button is released through the path that pressed it,
//! so a path change between the two never leaves a key held down.

use super::keymap;
use super::session::InputEvent;
use horcrux_guest_proto as gproto;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Held {
    Key(u32),
    Button(u8),
}

/// Per-session routing state (which path pressed each held key/button).
#[derive(Debug, Default)]
pub struct InputRouter {
    held: HashMap<Held, bool>,
}

/// What [`InputRouter::route`] did with an event.
#[derive(Debug, PartialEq)]
pub enum Routed {
    /// Handed to the guest tools.
    Tools,
    /// For QEMU's input (the caller sends it).
    Qemu(InputEvent),
}

impl InputRouter {
    /// Route one event. `tools` are the running guest tools' capabilities
    /// when routing through them is allowed (setting on, tools connected);
    /// `send` hands an event to them and says whether it was taken.
    /// Relative motion is not routed here (gaming pointer lock has its own
    /// rule).
    pub fn route(
        &mut self,
        ev: InputEvent,
        tools: Option<&gproto::Caps>,
        send: impl FnOnce(gproto::Input) -> bool,
    ) -> Routed {
        let keyboard = tools.map(|c| c.keyboard).unwrap_or(false);
        let pointer = tools.map(|c| c.absolute_pointer).unwrap_or(false);
        let (held, down, want, input) = match &ev {
            InputEvent::Key { code, down } => {
                let Some(q) = keymap::qnum(code).filter(|q| *q <= u16::MAX as u32) else {
                    return Routed::Qemu(ev);
                };
                let input = gproto::Input::Key {
                    qnum: q as u16,
                    down: *down,
                };
                (Some(Held::Key(q)), *down, keyboard, input)
            }
            InputEvent::Button { button, down } => {
                let input = gproto::Input::Button {
                    button: *button,
                    down: *down,
                };
                (Some(Held::Button(*button)), *down, pointer, input)
            }
            InputEvent::Wheel { dx, dy } => (
                None,
                false,
                pointer,
                gproto::Input::Wheel { dx: *dx, dy: *dy },
            ),
            InputEvent::Abs { x, y } => (
                None,
                false,
                pointer,
                gproto::Input::Abs {
                    x: *x,
                    y: *y,
                    monitor: 0,
                },
            ),
            InputEvent::Rel { .. } => return Routed::Qemu(ev),
        };
        // a release follows its press; a press (or a stateless event)
        // takes the preferred path
        let via_tools = match held {
            Some(h) if !down => self.held.remove(&h).unwrap_or(want),
            _ => want,
        };
        let taken = via_tools && send(input);
        if let (Some(h), true) = (held, down) {
            self.held.insert(h, taken);
        }
        if taken {
            Routed::Tools
        } else {
            Routed::Qemu(ev)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(keyboard: bool, absolute_pointer: bool) -> gproto::Caps {
        gproto::Caps {
            keyboard,
            absolute_pointer,
            ..Default::default()
        }
    }

    fn key(code: &str, down: bool) -> InputEvent {
        InputEvent::Key {
            code: code.into(),
            down,
        }
    }

    #[test]
    fn keys_go_to_the_tools_when_they_offer_a_keyboard() {
        let mut r = InputRouter::default();
        let mut sent = Vec::new();
        let c = caps(true, true);
        assert_eq!(
            r.route(key("KeyA", true), Some(&c), |i| {
                sent.push(i);
                true
            }),
            Routed::Tools
        );
        assert_eq!(
            r.route(key("KeyA", false), Some(&c), |i| {
                sent.push(i);
                true
            }),
            Routed::Tools
        );
        assert_eq!(
            sent,
            vec![
                gproto::Input::Key {
                    qnum: 0x1e,
                    down: true
                },
                gproto::Input::Key {
                    qnum: 0x1e,
                    down: false
                }
            ]
        );
        let abs = r.route(InputEvent::Abs { x: 10, y: 20 }, Some(&c), |i| {
            assert_eq!(
                i,
                gproto::Input::Abs {
                    x: 10,
                    y: 20,
                    monitor: 0
                }
            );
            true
        });
        assert_eq!(abs, Routed::Tools);
    }

    #[test]
    fn qemu_without_tools_or_capability() {
        let mut r = InputRouter::default();
        assert_eq!(
            r.route(key("KeyA", true), None, |_| panic!("no tools")),
            Routed::Qemu(key("KeyA", true))
        );
        let c = caps(false, false);
        assert_eq!(
            r.route(key("KeyB", true), Some(&c), |_| panic!("not offered")),
            Routed::Qemu(key("KeyB", true))
        );
        let rel = InputEvent::Rel { dx: 1, dy: 1 };
        assert_eq!(
            r.route(rel, Some(&caps(true, true)), |_| panic!("rel")),
            Routed::Qemu(InputEvent::Rel { dx: 1, dy: 1 })
        );
    }

    #[test]
    fn a_release_follows_its_press_across_a_path_change() {
        let mut r = InputRouter::default();
        let c = caps(true, true);
        // pressed through QEMU (pre-OS), the tools start, released: QEMU
        assert_eq!(
            r.route(key("ShiftLeft", true), None, |_| true),
            Routed::Qemu(key("ShiftLeft", true))
        );
        assert_eq!(
            r.route(key("ShiftLeft", false), Some(&c), |_| panic!(
                "must follow the press"
            )),
            Routed::Qemu(key("ShiftLeft", false))
        );
        // pressed through the tools, the tools go away, released: still
        // tried there first, QEMU when they are gone
        assert_eq!(
            r.route(
                InputEvent::Button {
                    button: 0,
                    down: true
                },
                Some(&c),
                |_| true
            ),
            Routed::Tools
        );
        assert_eq!(
            r.route(
                InputEvent::Button {
                    button: 0,
                    down: false
                },
                None,
                |_| false
            ),
            Routed::Qemu(InputEvent::Button {
                button: 0,
                down: false
            })
        );
    }

    #[test]
    fn a_refused_hand_off_falls_back_to_qemu_and_is_remembered() {
        let mut r = InputRouter::default();
        let c = caps(true, true);
        assert_eq!(
            r.route(key("KeyC", true), Some(&c), |_| false),
            Routed::Qemu(key("KeyC", true))
        );
        assert_eq!(
            r.route(key("KeyC", false), Some(&c), |_| panic!(
                "pressed through QEMU"
            )),
            Routed::Qemu(key("KeyC", false))
        );
    }
}
