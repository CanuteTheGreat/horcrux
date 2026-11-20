//! STUN message codec (RFC 8489) with the TURN attributes (RFC 8656).
//!
//! Clean-room from the RFC text. Only what the Horcrux relay and its
//! clients use: Binding, Allocate, Refresh, CreatePermission, ChannelBind,
//! Send/Data indications, ChannelData framing, long-term credentials
//! (MESSAGE-INTEGRITY, HMAC-SHA1) and FINGERPRINT.
use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use sha1::Sha1;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub const MAGIC: u32 = 0x2112_A442;
pub const HEADER_LEN: usize = 20;

/// Methods.
pub mod method {
    pub const BINDING: u16 = 0x001;
    pub const ALLOCATE: u16 = 0x003;
    pub const REFRESH: u16 = 0x004;
    pub const SEND: u16 = 0x006;
    pub const DATA: u16 = 0x007;
    pub const CREATE_PERMISSION: u16 = 0x008;
    pub const CHANNEL_BIND: u16 = 0x009;
}

/// Attribute types.
pub mod attr {
    pub const MAPPED_ADDRESS: u16 = 0x0001;
    pub const USERNAME: u16 = 0x0006;
    pub const MESSAGE_INTEGRITY: u16 = 0x0008;
    pub const ERROR_CODE: u16 = 0x0009;
    pub const UNKNOWN_ATTRIBUTES: u16 = 0x000A;
    pub const CHANNEL_NUMBER: u16 = 0x000C;
    pub const LIFETIME: u16 = 0x000D;
    pub const XOR_PEER_ADDRESS: u16 = 0x0012;
    pub const DATA: u16 = 0x0013;
    pub const REALM: u16 = 0x0014;
    pub const NONCE: u16 = 0x0015;
    pub const XOR_RELAYED_ADDRESS: u16 = 0x0016;
    pub const REQUESTED_ADDRESS_FAMILY: u16 = 0x0017;
    pub const EVEN_PORT: u16 = 0x0018;
    pub const REQUESTED_TRANSPORT: u16 = 0x0019;
    pub const DONT_FRAGMENT: u16 = 0x001A;
    pub const XOR_MAPPED_ADDRESS: u16 = 0x0020;
    pub const RESERVATION_TOKEN: u16 = 0x0022;
    pub const SOFTWARE: u16 = 0x8022;
    pub const FINGERPRINT: u16 = 0x8028;
}

/// Message class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Request,
    Indication,
    Success,
    Error,
}

impl Class {
    fn bits(self) -> u16 {
        match self {
            Class::Request => 0b00,
            Class::Indication => 0b01,
            Class::Success => 0b10,
            Class::Error => 0b11,
        }
    }
}

/// Message type field from method and class (RFC 8489 section 5).
pub fn msg_type(m: u16, c: Class) -> u16 {
    let c = c.bits();
    (m & 0x000F) | ((m & 0x0070) << 1) | ((m & 0x0F80) << 2) | ((c & 1) << 4) | ((c & 2) << 7)
}

/// Method and class of a message type field.
pub fn split_type(t: u16) -> (u16, Class) {
    let m = (t & 0x000F) | ((t >> 1) & 0x0070) | ((t >> 2) & 0x0F80);
    let c = match ((t >> 4) & 1) | ((t >> 7) & 2) {
        0 => Class::Request,
        1 => Class::Indication,
        2 => Class::Success,
        _ => Class::Error,
    };
    (m, c)
}

/// Is this datagram a STUN message (top two bits zero, magic cookie)?
pub fn is_stun(b: &[u8]) -> bool {
    b.len() >= HEADER_LEN
        && b[0] & 0xC0 == 0
        && u32::from_be_bytes([b[4], b[5], b[6], b[7]]) == MAGIC
}

/// Is this a TURN ChannelData message (channel 0x4000..=0x4FFF)?
pub fn is_channel_data(b: &[u8]) -> bool {
    b.len() >= 4 && (0x40..=0x4F).contains(&b[0])
}

/// A parsed message. Attributes keep their raw values; `raw` keeps the
/// bytes so MESSAGE-INTEGRITY can be checked.
#[derive(Debug, Clone)]
pub struct Message {
    pub method: u16,
    pub class: Class,
    pub txid: [u8; 12],
    pub attrs: Vec<(u16, Vec<u8>)>,
    /// (attribute type, offset of its header in `raw`)
    offsets: Vec<(u16, usize)>,
    raw: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Short,
    NotStun,
    BadLength,
}

impl Message {
    pub fn parse(b: &[u8]) -> Result<Message, ParseError> {
        if b.len() < HEADER_LEN {
            return Err(ParseError::Short);
        }
        if !is_stun(b) {
            return Err(ParseError::NotStun);
        }
        let len = u16::from_be_bytes([b[2], b[3]]) as usize;
        if !len.is_multiple_of(4) || HEADER_LEN + len > b.len() {
            return Err(ParseError::BadLength);
        }
        let b = &b[..HEADER_LEN + len];
        let (method, class) = split_type(u16::from_be_bytes([b[0], b[1]]));
        let mut txid = [0u8; 12];
        txid.copy_from_slice(&b[8..20]);
        let mut attrs = Vec::new();
        let mut offsets = Vec::new();
        let mut o = HEADER_LEN;
        while o + 4 <= b.len() {
            let t = u16::from_be_bytes([b[o], b[o + 1]]);
            let l = u16::from_be_bytes([b[o + 2], b[o + 3]]) as usize;
            if o + 4 + l > b.len() {
                return Err(ParseError::BadLength);
            }
            attrs.push((t, b[o + 4..o + 4 + l].to_vec()));
            offsets.push((t, o));
            o += 4 + ((l + 3) & !3);
        }
        Ok(Message {
            method,
            class,
            txid,
            attrs,
            offsets,
            raw: b.to_vec(),
        })
    }

    pub fn get(&self, t: u16) -> Option<&[u8]> {
        self.attrs
            .iter()
            .find(|(a, _)| *a == t)
            .map(|(_, v)| v.as_slice())
    }

    pub fn get_str(&self, t: u16) -> Option<String> {
        self.get(t)
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(str::to_string)
    }

    pub fn get_u32(&self, t: u16) -> Option<u32> {
        self.get(t)
            .filter(|v| v.len() >= 4)
            .map(|v| u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
    }

    pub fn get_xor_addr(&self, t: u16) -> Option<SocketAddr> {
        self.get(t).and_then(|v| decode_xor_addr(v, &self.txid))
    }

    /// Comprehension-required attributes (type < 0x8000) this codec does
    /// not know: a request carrying one is answered 420.
    pub fn unknown_required(&self) -> Vec<u16> {
        const KNOWN: [u16; 18] = [
            attr::MAPPED_ADDRESS,
            attr::USERNAME,
            attr::MESSAGE_INTEGRITY,
            attr::ERROR_CODE,
            attr::UNKNOWN_ATTRIBUTES,
            attr::CHANNEL_NUMBER,
            attr::LIFETIME,
            attr::XOR_PEER_ADDRESS,
            attr::DATA,
            attr::REALM,
            attr::NONCE,
            attr::XOR_RELAYED_ADDRESS,
            attr::REQUESTED_ADDRESS_FAMILY,
            attr::EVEN_PORT,
            attr::REQUESTED_TRANSPORT,
            attr::DONT_FRAGMENT,
            attr::XOR_MAPPED_ADDRESS,
            attr::RESERVATION_TOKEN,
        ];
        self.attrs
            .iter()
            .map(|(t, _)| *t)
            .filter(|t| *t < 0x8000 && !KNOWN.contains(t))
            .collect()
    }

    /// Check MESSAGE-INTEGRITY with `key` (RFC 8489 section 14.5): HMAC-SHA1
    /// over the message up to the attribute, with the header length
    /// covering it. `false` when there is no such attribute.
    pub fn check_integrity(&self, key: &[u8]) -> bool {
        let Some(&(_, off)) = self
            .offsets
            .iter()
            .find(|(t, _)| *t == attr::MESSAGE_INTEGRITY)
        else {
            return false;
        };
        let Some(mi) = self.get(attr::MESSAGE_INTEGRITY) else {
            return false;
        };
        if mi.len() != 20 {
            return false;
        }
        let mut head = self.raw[..off].to_vec();
        let len = (off + 24 - HEADER_LEN) as u16;
        head[2..4].copy_from_slice(&len.to_be_bytes());
        let mut mac = match Hmac::<Sha1>::new_from_slice(key) {
            Ok(m) => m,
            Err(_) => return false,
        };
        mac.update(&head);
        mac.verify_slice(mi).is_ok()
    }

    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}

/// Long-term credential key (RFC 8489 section 9.2.2, MD5 variant):
/// MD5(username ":" realm ":" password).
pub fn long_term_key(username: &str, realm: &str, password: &str) -> [u8; 16] {
    let mut h = Md5::new();
    h.update(username.as_bytes());
    h.update(b":");
    h.update(realm.as_bytes());
    h.update(b":");
    h.update(password.as_bytes());
    h.finalize().into()
}

fn encode_xor_addr(a: SocketAddr, txid: &[u8; 12]) -> Vec<u8> {
    let port = a.port() ^ (MAGIC >> 16) as u16;
    let mut v = Vec::with_capacity(20);
    match a.ip() {
        IpAddr::V4(ip) => {
            v.extend_from_slice(&[0, 1]);
            v.extend_from_slice(&port.to_be_bytes());
            let x = u32::from(ip) ^ MAGIC;
            v.extend_from_slice(&x.to_be_bytes());
        }
        IpAddr::V6(ip) => {
            v.extend_from_slice(&[0, 2]);
            v.extend_from_slice(&port.to_be_bytes());
            let mut key = [0u8; 16];
            key[..4].copy_from_slice(&MAGIC.to_be_bytes());
            key[4..].copy_from_slice(txid);
            let o = ip.octets();
            for i in 0..16 {
                v.push(o[i] ^ key[i]);
            }
        }
    }
    v
}

pub fn decode_xor_addr(v: &[u8], txid: &[u8; 12]) -> Option<SocketAddr> {
    if v.len() < 8 {
        return None;
    }
    let port = u16::from_be_bytes([v[2], v[3]]) ^ (MAGIC >> 16) as u16;
    match v[1] {
        1 => {
            let x = u32::from_be_bytes([v[4], v[5], v[6], v[7]]) ^ MAGIC;
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(x)), port))
        }
        2 if v.len() >= 20 => {
            let mut key = [0u8; 16];
            key[..4].copy_from_slice(&MAGIC.to_be_bytes());
            key[4..].copy_from_slice(txid);
            let mut o = [0u8; 16];
            for i in 0..16 {
                o[i] = v[4 + i] ^ key[i];
            }
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(o)), port))
        }
        _ => None,
    }
}

/// Builds a message attribute by attribute.
pub struct Builder {
    buf: Vec<u8>,
    txid: [u8; 12],
}

impl Builder {
    pub fn new(method: u16, class: Class, txid: [u8; 12]) -> Self {
        let mut buf = Vec::with_capacity(128);
        buf.extend_from_slice(&msg_type(method, class).to_be_bytes());
        buf.extend_from_slice(&[0, 0]);
        buf.extend_from_slice(&MAGIC.to_be_bytes());
        buf.extend_from_slice(&txid);
        Builder { buf, txid }
    }

    /// A request with a random transaction id.
    pub fn request(method: u16) -> Self {
        Self::new(method, Class::Request, rand::random())
    }

    pub fn txid(&self) -> [u8; 12] {
        self.txid
    }

    pub fn attr(mut self, t: u16, v: &[u8]) -> Self {
        self.buf.extend_from_slice(&t.to_be_bytes());
        self.buf.extend_from_slice(&(v.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(v);
        while !self.buf.len().is_multiple_of(4) {
            self.buf.push(0);
        }
        self.set_len();
        self
    }

    pub fn u32(self, t: u16, v: u32) -> Self {
        self.attr(t, &v.to_be_bytes())
    }

    pub fn xor_addr(self, t: u16, a: SocketAddr) -> Self {
        let v = encode_xor_addr(a, &self.txid);
        self.attr(t, &v)
    }

    pub fn error(self, code: u16, reason: &str) -> Self {
        let mut v = vec![0, 0, (code / 100) as u8, (code % 100) as u8];
        v.extend_from_slice(reason.as_bytes());
        self.attr(attr::ERROR_CODE, &v)
    }

    fn set_len(&mut self) {
        let len = (self.buf.len() - HEADER_LEN) as u16;
        self.buf[2..4].copy_from_slice(&len.to_be_bytes());
    }

    /// Append MESSAGE-INTEGRITY computed with `key`.
    pub fn integrity(mut self, key: &[u8]) -> Self {
        let len = (self.buf.len() + 24 - HEADER_LEN) as u16;
        self.buf[2..4].copy_from_slice(&len.to_be_bytes());
        let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("hmac key");
        mac.update(&self.buf);
        let tag = mac.finalize().into_bytes();
        self.attr(attr::MESSAGE_INTEGRITY, &tag)
    }

    /// Append FINGERPRINT and return the bytes.
    pub fn fingerprint(mut self) -> Vec<u8> {
        let len = (self.buf.len() + 8 - HEADER_LEN) as u16;
        self.buf[2..4].copy_from_slice(&len.to_be_bytes());
        let crc = crc32fast::hash(&self.buf) ^ 0x5354_554E;
        self.attr(attr::FINGERPRINT, &crc.to_be_bytes()).buf
    }

    pub fn build(self) -> Vec<u8> {
        self.buf
    }
}

/// ChannelData framing (RFC 8656 section 12.4). `pad` adds the padding to a
/// multiple of four that stream transports need.
pub fn channel_data(channel: u16, data: &[u8], pad: bool) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + data.len() + 3);
    v.extend_from_slice(&channel.to_be_bytes());
    v.extend_from_slice(&(data.len() as u16).to_be_bytes());
    v.extend_from_slice(data);
    if pad {
        while !v.len().is_multiple_of(4) {
            v.push(0);
        }
    }
    v
}

/// Channel number and payload of a ChannelData message.
pub fn parse_channel_data(b: &[u8]) -> Option<(u16, &[u8])> {
    if !is_channel_data(b) {
        return None;
    }
    let ch = u16::from_be_bytes([b[0], b[1]]);
    let len = u16::from_be_bytes([b[2], b[3]]) as usize;
    (b.len() >= 4 + len).then(|| (ch, &b[4..4 + len]))
}

/// Length of the next framed message at the start of a stream buffer
/// (STUN or padded ChannelData), when the header is complete.
pub fn stream_frame_len(b: &[u8]) -> Option<usize> {
    if b.len() < 4 {
        return None;
    }
    let l = u16::from_be_bytes([b[2], b[3]]) as usize;
    if b[0] & 0xC0 == 0 {
        Some(HEADER_LEN + l)
    } else {
        Some(4 + ((l + 3) & !3))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// RFC 5769 section 2.1, sample request (short-term credential).
    #[test]
    fn rfc5769_sample_request_integrity() {
        let b = hex(
            "00 01 00 58 21 12 a4 42 b7 e7 a7 01 bc 34 d6 86 fa 87 df ae
             80 22 00 10 53 54 55 4e 20 74 65 73 74 20 63 6c 69 65 6e 74
             00 24 00 04 6e 00 01 ff 80 29 00 08 93 2f f9 b1 51 26 3b 36
             00 06 00 09 65 76 74 6a 3a 68 36 76 59 20 20 20 00 08 00 14
             9a ea a7 0c bf d8 cb 56 78 1e f2 b5 b2 d3 f2 49 c1 b5 71 a2
             80 28 00 04 e5 7a 3b cf",
        );
        let m = Message::parse(&b).unwrap();
        assert_eq!((m.method, m.class), (method::BINDING, Class::Request));
        assert_eq!(m.get_str(attr::USERNAME).unwrap(), "evtj:h6vY");
        assert!(m.check_integrity(b"VOkJxbRl1RmTxUk/WvJxBt"));
        assert!(!m.check_integrity(b"wrong"));
    }

    /// RFC 5769 section 2.2, sample IPv4 response: XOR-MAPPED-ADDRESS.
    #[test]
    fn rfc5769_ipv4_response_address() {
        let b = hex(
            "01 01 00 3c 21 12 a4 42 b7 e7 a7 01 bc 34 d6 86 fa 87 df ae
             80 22 00 0b 74 65 73 74 20 76 65 63 74 6f 72 20 00 20 00 08
             00 01 a1 47 e1 12 a6 43 00 08 00 14 2b 91 f5 99 fd 9e 90 c3
             8c 74 89 f9 2a f9 ba 53 f0 6b e7 d7 80 28 00 04 c0 7d 4c 96",
        );
        let m = Message::parse(&b).unwrap();
        assert_eq!(m.class, Class::Success);
        assert_eq!(
            m.get_xor_addr(attr::XOR_MAPPED_ADDRESS).unwrap(),
            "192.0.2.1:32853".parse().unwrap()
        );
        assert!(m.check_integrity(b"VOkJxbRl1RmTxUk/WvJxBt"));
    }

    /// RFC 5769 section 2.3, IPv6 response.
    #[test]
    fn rfc5769_ipv6_response_address() {
        let b = hex(
            "01 01 00 48 21 12 a4 42 b7 e7 a7 01 bc 34 d6 86 fa 87 df ae
             80 22 00 0b 74 65 73 74 20 76 65 63 74 6f 72 20 00 20 00 14
             00 02 a1 47 01 13 a9 fa a5 d3 f1 79 bc 25 f4 b5 be d2 b9 d9
             00 08 00 14 a3 82 95 4e 4b e6 7b f1 17 84 c9 7c 82 92 c2 75
             bf e3 ed 41 80 28 00 04 c8 fb 0b 4c",
        );
        let m = Message::parse(&b).unwrap();
        assert_eq!(
            m.get_xor_addr(attr::XOR_MAPPED_ADDRESS).unwrap(),
            "[2001:db8:1234:5678:11:2233:4455:6677]:32853"
                .parse()
                .unwrap()
        );
    }

    /// RFC 5769 section 2.4, long-term credential request (the MD5 key).
    #[test]
    fn rfc5769_long_term_key() {
        let b = hex(
            "00 01 00 60 21 12 a4 42 78 ad 34 33 c6 ad 72 c0 29 da 41 2e
             00 06 00 12 e3 83 9e e3 83 88 e3 83 aa e3 83 83 e3 82 af e3
             82 b9 00 00 00 15 00 1c 66 2f 2f 34 39 39 6b 39 35 34 64 36
             4f 4c 33 34 6f 4c 39 46 53 54 76 79 36 34 73 41 00 14 00 0b
             65 78 61 6d 70 6c 65 2e 6f 72 67 00 00 08 00 14 f6 70 24 65
             6d d6 4a 3e 02 b8 e0 71 2e 85 c9 a2 8c a8 96 66",
        );
        let m = Message::parse(&b).unwrap();
        let user = m.get_str(attr::USERNAME).unwrap();
        let realm = m.get_str(attr::REALM).unwrap();
        assert_eq!(realm, "example.org");
        // password "TheMatrIX" after SASLprep (RFC 5769 lists it so)
        let key = long_term_key(&user, &realm, "TheMatrIX");
        assert!(m.check_integrity(&key));
    }

    #[test]
    fn build_round_trip_with_integrity_and_fingerprint() {
        let key = long_term_key("u", "r", "p");
        let peer: SocketAddr = "10.1.2.3:61201".parse().unwrap();
        let b = Builder::request(method::CREATE_PERMISSION)
            .xor_addr(attr::XOR_PEER_ADDRESS, peer)
            .attr(attr::USERNAME, b"u")
            .integrity(&key)
            .fingerprint();
        let m = Message::parse(&b).unwrap();
        assert_eq!(m.method, method::CREATE_PERMISSION);
        assert_eq!(m.get_xor_addr(attr::XOR_PEER_ADDRESS), Some(peer));
        assert!(m.check_integrity(&key));
        let fp = m.get_u32(attr::FINGERPRINT).unwrap();
        let n = b.len() - 8;
        assert_eq!(crc32fast::hash(&b[..n]) ^ 0x5354_554E, fp);
    }

    #[test]
    fn types_and_channel_data() {
        for m in [method::BINDING, method::ALLOCATE, method::CHANNEL_BIND] {
            for c in [
                Class::Request,
                Class::Indication,
                Class::Success,
                Class::Error,
            ] {
                assert_eq!(split_type(msg_type(m, c)), (m, c));
            }
        }
        assert_eq!(msg_type(method::ALLOCATE, Class::Error), 0x0113);
        assert_eq!(msg_type(method::DATA, Class::Indication), 0x0017);
        let cd = channel_data(0x4001, b"abcde", true);
        assert_eq!(cd.len(), 12);
        assert_eq!(stream_frame_len(&cd), Some(12));
        assert_eq!(parse_channel_data(&cd), Some((0x4001, &b"abcde"[..])));
        assert!(!is_stun(&cd));
    }
}
