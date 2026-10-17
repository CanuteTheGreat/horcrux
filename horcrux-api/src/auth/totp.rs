//! TOTP (RFC 6238) second factor and recovery codes.
//!
//! Standard authenticator-app parameters: HMAC-SHA1, 6 digits, 30 s step,
//! one step of clock skew either way. The last accepted step is stored per
//! factor so a code can't be used twice.

use hmac::{Hmac, Mac};
use rand::Rng;
use sha1::Sha1;

pub const DIGITS: u32 = 6;
pub const STEP_SECS: u64 = 30;
pub const SKEW_STEPS: i64 = 1;

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32 without padding (what authenticator apps expect).
pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &b in data {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(B32[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(B32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=' && *c != '-')
    {
        let v = B32
            .iter()
            .position(|&x| x as char == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push(((buffer >> (bits - 8)) & 0xff) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

/// A new random 160-bit secret, base32 encoded.
pub fn generate_secret() -> String {
    let mut bytes = [0u8; 20];
    rand::thread_rng().fill(&mut bytes);
    base32_encode(&bytes)
}

/// HOTP value for `counter` (RFC 4226).
pub fn hotp(secret: &[u8], counter: u64) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let offset = (h[19] & 0x0f) as usize;
    let bin = ((h[offset] as u32 & 0x7f) << 24)
        | ((h[offset + 1] as u32) << 16)
        | ((h[offset + 2] as u32) << 8)
        | (h[offset + 3] as u32);
    bin % 10u32.pow(DIGITS)
}

pub fn current_step(unix: u64) -> i64 {
    (unix / STEP_SECS) as i64
}

pub fn code_at(secret_b32: &str, unix: u64) -> Option<String> {
    let secret = base32_decode(secret_b32)?;
    Some(format!(
        "{:0width$}",
        hotp(&secret, unix / STEP_SECS),
        width = DIGITS as usize
    ))
}

/// Check `code` at time `unix`. Returns the matched step if it's valid and
/// newer than `last_step` (replay protection).
pub fn verify(secret_b32: &str, code: &str, unix: u64, last_step: i64) -> Option<i64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != DIGITS as usize || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let secret = base32_decode(secret_b32)?;
    let now = current_step(unix);
    for delta in -SKEW_STEPS..=SKEW_STEPS {
        let step = now + delta;
        if step < 0 || step <= last_step {
            continue;
        }
        let expected = format!(
            "{:0width$}",
            hotp(&secret, step as u64),
            width = DIGITS as usize
        );
        if subtle::ConstantTimeEq::ct_eq(expected.as_bytes(), code.as_bytes()).into() {
            return Some(step);
        }
    }
    None
}

/// `otpauth://` provisioning URI for QR codes.
pub fn provisioning_uri(issuer: &str, account: &str, secret_b32: &str) -> String {
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={}&period={}",
        urlencoding::encode(issuer),
        urlencoding::encode(account),
        secret_b32,
        urlencoding::encode(issuer),
        DIGITS,
        STEP_SECS
    )
}

/// Ten fresh recovery codes, `xxxx-xxxx-xxxx` from an unambiguous alphabet.
pub fn generate_recovery_codes() -> Vec<String> {
    const ALPHA: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..10)
        .map(|_| {
            (0..3)
                .map(|_| {
                    (0..4)
                        .map(|_| ALPHA[rng.gen_range(0..ALPHA.len())] as char)
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 Appendix B test vectors (SHA1, secret "12345678901234567890").
    #[test]
    fn rfc6238_vectors() {
        let secret = base32_encode(b"12345678901234567890");
        let cases = [
            (59u64, "94287082"),
            (1111111109, "07081804"),
            (1111111111, "14050471"),
            (1234567890, "89005924"),
            (2000000000, "69279037"),
        ];
        for (t, eight) in cases {
            // our codes are 6 digits: the low six of the RFC's 8-digit values
            assert_eq!(code_at(&secret, t).unwrap(), eight[2..]);
        }
    }

    #[test]
    fn base32_round_trip() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
            assert_eq!(base32_decode(&base32_encode(data)).unwrap(), data);
        }
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
    }

    #[test]
    fn verify_window_and_replay() {
        let s = generate_secret();
        let t = 1_700_000_000u64;
        let code = code_at(&s, t).unwrap();
        let step = verify(&s, &code, t, 0).unwrap();
        // same code again is a replay
        assert!(verify(&s, &code, t, step).is_none());
        // previous step still accepted once (clock skew)
        let prev = code_at(&s, t - 30).unwrap();
        assert!(verify(&s, &prev, t, 0).is_some());
        // two steps old is rejected
        let old = code_at(&s, t - 90).unwrap();
        assert!(verify(&s, &old, t, 0).is_none());
        assert!(verify(&s, "12345", t, 0).is_none());
        assert!(verify(&s, "abcdef", t, 0).is_none());
    }

    #[test]
    fn recovery_codes_shape() {
        let c = generate_recovery_codes();
        assert_eq!(c.len(), 10);
        assert!(c
            .iter()
            .all(|x| x.len() == 14 && x.matches('-').count() == 2));
    }
}
