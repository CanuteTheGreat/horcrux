//! Node <-> native viewer on the same host (docs/design/horcrux-console.md
//! section 4.3): a Unix stream socket (`[console] local_socket`).
//!
//! The viewer gets a local console ticket from the API
//! (`POST /api/console/<vm>/hxc/local-ticket`, same RBAC as any console
//! ticket, bound to the viewer's Unix uid), connects, and sends `HELLO`
//! with it. The node answers `WELCOME`; when the VM has a frame ring the
//! answer carries the ring's file descriptor (SCM_RIGHTS), so the viewer
//! never needs file-system access to it. After that the viewer sends input
//! in the same encodings as the WebRTC data channels (`keys` JSON,
//! `input` binary motion, `pad` binary gamepad, `ctl` JSON) and the node
//! sends cursor and status messages.
//!
//! Framing is the guest-tools framing: `len u32 LE` (bytes that follow),
//! `kind u8`, payload.
use serde::{Deserialize, Serialize};

pub mod kind {
    /// viewer -> node: [`Hello`] JSON.
    pub const HELLO: u8 = 0x01;
    /// node -> viewer: [`Welcome`] JSON (+ the frame ring fd).
    pub const WELCOME: u8 = 0x02;
    /// node -> viewer: `{"message": ...}` and the connection closes.
    pub const ERROR: u8 = 0x03;
    /// viewer -> node: `keys` channel JSON (`{"t":"down","code":"KeyA"}`).
    pub const KEYS: u8 = 0x10;
    /// viewer -> node: `input` channel binary (kind, seq, x, y).
    pub const MOTION: u8 = 0x11;
    /// viewer -> node: `pad` channel binary (16 bytes).
    pub const PAD: u8 = 0x12;
    /// viewer -> node: `ctl` channel JSON (`resize`, `ping`).
    pub const CTL: u8 = 0x13;
    /// node -> viewer: `cursor` channel JSON (shape / pos).
    pub const CURSOR: u8 = 0x20;
    /// node -> viewer: `ctl` JSON (`pong`, `layout`, `guest_layout`).
    pub const STATUS: u8 = 0x21;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Hello {
    pub v: u32,
    pub ticket: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Welcome {
    pub v: u32,
    pub session: String,
    pub vm: String,
    /// Keyboard/pointer allowed (VM.Console.Input).
    pub input: bool,
    /// Gamepad and raw relative mouse reach the guest tools.
    pub gamepad: bool,
    pub relative: bool,
    /// The frame ring's size in bytes when its descriptor came with this
    /// message; 0 = no ring (the viewer uses the stream instead).
    pub frame_ring_bytes: u64,
    /// The guest's pointer is absolute (tablet) rather than relative.
    pub absolute: bool,
}

/// Largest local frame either side accepts.
pub const MAX_LOCAL_FRAME: usize = 1 << 20;

/// Read one `(kind, payload)` frame (blocking).
pub fn read_frame<R: std::io::Read>(r: &mut R) -> std::io::Result<(u8, Vec<u8>)> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_LOCAL_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("bad frame length {len}"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let k = buf.remove(0);
    Ok((k, buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_and_defaults() {
        let w = Welcome {
            v: 1,
            vm: "a".into(),
            frame_ring_bytes: 128 << 20,
            ..Default::default()
        };
        let back: Welcome = serde_json::from_str(&serde_json::to_string(&w).unwrap()).unwrap();
        assert_eq!(w, back);
        let h: Hello = serde_json::from_str(r#"{"ticket":"t","new":1}"#).unwrap();
        assert_eq!(h.ticket, "t");
        let f = crate::frame(kind::KEYS, b"{}");
        let (k, p) = read_frame(&mut &f[..]).unwrap();
        assert_eq!((k, p.as_slice()), (kind::KEYS, &b"{}"[..]));
        let mut big = ((MAX_LOCAL_FRAME + 2) as u32).to_le_bytes().to_vec();
        big.push(1);
        assert!(read_frame(&mut &big[..]).is_err());
    }
}
