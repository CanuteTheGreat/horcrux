//! FlexFEC repair packets in the `flexfec-03` format
//! (draft-ietf-payload-flexible-fec-scheme-03, flexible mask, R=0 F=0),
//! written from the draft.
//!
//! `flexfec-03` is the payload name browsers negotiate, and their receivers
//! parse the draft-03 header: it names the protected SSRC in the FEC header
//! (SSRCCount, reserved bytes, SSRC, SN base) and its mask k-bits mark the
//! LAST mask chunk. RFC 8627 later moved the SSRC to the CSRC list and
//! flipped the k-bit, so an RFC 8627 header sent under `flexfec-03` is
//! discarded by the receiver ("protecting multiple media SSRCs"), which the
//! WAN e2e caught with the browser's own log.
//!
//! A repair packet protects a set of source RTP packets of one SSRC that lie
//! within [`MAX_SPAN`] sequence numbers. Its FEC header carries the XOR of the
//! source packets' first header bits, lengths (minus the 12-byte fixed
//! header) and timestamps, the protected SSRC, the lowest sequence number and
//! a bitmask of the protected packets; the repair payload is the XOR of
//! everything after the sources' fixed headers (zero-padded to the longest).
//! Any one lost source packet of the set is rebuilt from the repair packet
//! and the other sources.
//!
//! [`Protector`] groups the outgoing packets of each video frame (up to the
//! marker bit) and emits interleaved repair packets for it: `overhead`
//! percent of the frame's packets, at least one, so a frame's repair is on
//! the wire right after the frame itself and a single loss costs no round
//! trip.

/// Largest set one repair packet covers. The draft-03 mask has room for 109
/// packets, but browser receivers decode at most 48 per repair packet (their
/// masks are ULPFEC's), so sets stay within 48.
pub const MAX_SPAN: usize = 48;
/// Mask bits per chunk (each chunk is led by its k-bit).
const CHUNK_BITS: [usize; 3] = [15, 31, 63];
/// Bytes before the mask: 8 recovery bytes, SSRCCount, 3 reserved, SSRC,
/// SN base.
const MASK_OFFSET: usize = 18;
/// FEC header size with a 15-, 46- and 109-bit mask.
const HEADER_SIZES: [usize; 3] = [MASK_OFFSET + 2, MASK_OFFSET + 6, MASK_OFFSET + 14];

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}
fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Sequence number, timestamp, SSRC and marker of a raw RTP packet.
pub fn rtp_fields(pkt: &[u8]) -> Option<(u16, u32, u32, bool)> {
    if pkt.len() < 12 || pkt[0] >> 6 != 2 {
        return None;
    }
    Some((
        be16(&pkt[2..]),
        be32(&pkt[4..]),
        be32(&pkt[8..]),
        pkt[1] & 0x80 != 0,
    ))
}

/// FEC header + repair payload protecting `sources` (raw RTP packets of one
/// SSRC). `sources` may come in any order; their sequence numbers must lie
/// within [`MAX_SPAN`] of the lowest one. None for an empty or invalid set.
pub fn repair(sources: &[&[u8]]) -> Option<Vec<u8>> {
    if sources.is_empty() {
        return None;
    }
    let mut seqs = Vec::with_capacity(sources.len());
    let ssrc = rtp_fields(sources[0])?.2;
    for s in sources {
        let f = rtp_fields(s)?;
        if f.2 != ssrc {
            return None;
        }
        seqs.push(f.0);
    }
    // lowest sequence number, taking wrap-around into account: the one all
    // others are at most MAX_SPAN-1 after
    let base = *seqs.iter().find(|&&b| {
        seqs.iter()
            .all(|&s| (s.wrapping_sub(b) as usize) < MAX_SPAN)
    })?;
    let span = seqs.iter().map(|&s| s.wrapping_sub(base) as usize).max()? + 1;
    // chunks needed for the span
    let chunks = if span <= CHUNK_BITS[0] {
        1
    } else if span <= CHUNK_BITS[0] + CHUNK_BITS[1] {
        2
    } else {
        3
    };
    let hsize = HEADER_SIZES[chunks - 1];
    let longest = sources.iter().map(|s| s.len() - 12).max()?;
    let mut out = vec![0u8; hsize + longest];
    for s in sources {
        out[0] ^= s[0];
        out[1] ^= s[1];
        let len = ((s.len() - 12) as u16).to_be_bytes();
        out[2] ^= len[0];
        out[3] ^= len[1];
        for i in 0..4 {
            out[4 + i] ^= s[4 + i];
        }
        for (o, b) in out[hsize..].iter_mut().zip(&s[12..]) {
            *o ^= b;
        }
    }
    // R=0, F=0 replace the version bits
    out[0] &= 0x3f;
    out[8] = 1; // SSRCCount; bytes 9..12 reserved (zero)
    out[12..16].copy_from_slice(&ssrc.to_be_bytes());
    out[16..18].copy_from_slice(&base.to_be_bytes());
    // mask bit j = base + j, MSB first across the chunks; each chunk is led
    // by its k-bit, set on the last chunk only
    let mut bits = [false; 109];
    for &s in &seqs {
        bits[s.wrapping_sub(base) as usize] = true;
    }
    let mut at = MASK_OFFSET;
    let mut j = 0;
    for (c, &n) in CHUNK_BITS.iter().enumerate().take(chunks) {
        let mut v: u64 = (c + 1 == chunks) as u64;
        for b in &bits[j..j + n] {
            v = (v << 1) | *b as u64;
        }
        let bytes = (n + 1) / 8;
        out[at..at + bytes].copy_from_slice(&v.to_be_bytes()[8 - bytes..]);
        at += bytes;
        j += n;
    }
    Some(out)
}

/// A parsed repair packet payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair<'a> {
    pub base: u16,
    /// The protected SSRC named in the FEC header.
    pub ssrc: u32,
    /// Sequence numbers this repair packet protects.
    pub protected: Vec<u16>,
    header: [u8; 8],
    payload: &'a [u8],
}

/// Parse a repair payload (after the repair packet's RTP header) for a
/// single protected SSRC.
pub fn parse(fec: &[u8]) -> Option<Repair<'_>> {
    if fec.len() < HEADER_SIZES[0] || fec[0] & 0xc0 != 0 || fec[8] != 1 {
        return None;
    }
    let ssrc = be32(&fec[12..]);
    let base = be16(&fec[16..]);
    let mut protected = Vec::new();
    let (mut at, mut j) = (MASK_OFFSET, 0);
    for &n in &CHUNK_BITS {
        let bytes = (n + 1) / 8;
        if fec.len() < at + bytes {
            return None;
        }
        let mut b8 = [0u8; 8];
        b8[8 - bytes..].copy_from_slice(&fec[at..at + bytes]);
        let v = u64::from_be_bytes(b8);
        for i in 0..n {
            if v >> (n - 1 - i) & 1 == 1 {
                protected.push(base.wrapping_add((j + i) as u16));
            }
        }
        at += bytes;
        j += n;
        if v >> n & 1 == 1 {
            // k-bit: last chunk
            let mut header = [0u8; 8];
            header.copy_from_slice(&fec[..8]);
            return Some(Repair {
                base,
                ssrc,
                protected,
                header,
                payload: &fec[at..],
            });
        }
    }
    None
}

/// Rebuild the one missing packet `missing` (sequence number) of SSRC
/// `ssrc` from a repair packet and the other protected packets
/// (`received`, raw RTP). None when more than one is missing or the data
/// does not add up.
pub fn recover(rep: &Repair, ssrc: u32, missing: u16, received: &[&[u8]]) -> Option<Vec<u8>> {
    if !rep.protected.contains(&missing) {
        return None;
    }
    let mut have: Vec<&[u8]> = Vec::new();
    for &s in &rep.protected {
        if s == missing {
            continue;
        }
        have.push(
            *received
                .iter()
                .find(|p| rtp_fields(p).map(|f| f.0) == Some(s))?,
        );
    }
    let mut h = rep.header;
    for p in &have {
        h[0] ^= p[0];
        h[1] ^= p[1];
        let len = ((p.len() - 12) as u16).to_be_bytes();
        h[2] ^= len[0];
        h[3] ^= len[1];
        for i in 0..4 {
            h[4 + i] ^= p[4 + i];
        }
    }
    let y = be16(&h[2..]) as usize;
    if y > rep.payload.len() {
        return None;
    }
    let mut out = vec![0u8; 12 + y];
    out[0] = 0x80 | (h[0] & 0x3f);
    out[1] = h[1];
    out[2..4].copy_from_slice(&missing.to_be_bytes());
    out[4..8].copy_from_slice(&h[4..8]);
    out[8..12].copy_from_slice(&ssrc.to_be_bytes());
    out[12..].copy_from_slice(&rep.payload[..y]);
    for p in &have {
        for (o, b) in out[12..].iter_mut().zip(&p[12..]) {
            *o ^= b;
        }
    }
    Some(out)
}

/// Which of `n` packets (indices) each repair packet covers: `overhead`
/// percent of `n` repair packets (at least one, at most `n`), interleaved
/// (repair j covers i where i % r == j) so a burst of r losses is still
/// repaired, in blocks of at most [`MAX_SPAN`] packets.
pub fn plan(n: usize, overhead_percent: u32) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < n {
        let len = (n - start).min(MAX_SPAN);
        let r = (len * overhead_percent as usize)
            .div_ceil(100)
            .clamp(1, len);
        for j in 0..r {
            out.push((j..len).step_by(r).map(|i| start + i).collect());
        }
        start += len;
    }
    out
}

/// Collects the outgoing packets of one SSRC and emits repair payloads at
/// each frame end (marker bit) or when a block is full.
pub struct Protector {
    pub ssrc: u32,
    pub overhead_percent: u32,
    frame: Vec<Vec<u8>>,
}

impl Protector {
    pub fn new(ssrc: u32, overhead_percent: u32) -> Self {
        Self {
            ssrc,
            overhead_percent,
            frame: Vec::new(),
        }
    }

    /// Feed one sent RTP packet (raw, unencrypted). Returns the repair
    /// payloads (with the RTP timestamp of the protected frame) due now.
    pub fn push(&mut self, pkt: &[u8]) -> Vec<(u32, Vec<u8>)> {
        let Some((_, ts, ssrc, marker)) = rtp_fields(pkt) else {
            return Vec::new();
        };
        if ssrc != self.ssrc {
            return Vec::new();
        }
        // a new timestamp without a marker before: close the old frame
        let mut out = Vec::new();
        if let Some(last) = self.frame.last() {
            if rtp_fields(last).map(|f| f.1) != Some(ts) {
                out = self.flush();
            }
        }
        self.frame.push(pkt.to_vec());
        if marker || self.frame.len() >= MAX_SPAN {
            out.extend(self.flush());
        }
        out
    }

    /// Repairs for what is collected (frame end).
    pub fn flush(&mut self) -> Vec<(u32, Vec<u8>)> {
        let frame = std::mem::take(&mut self.frame);
        let Some(ts) = frame.first().and_then(|p| rtp_fields(p)).map(|f| f.1) else {
            return Vec::new();
        };
        plan(frame.len(), self.overhead_percent)
            .into_iter()
            .filter_map(|set| {
                let refs: Vec<&[u8]> = set.iter().map(|&i| frame[i].as_slice()).collect();
                repair(&refs).map(|r| (ts, r))
            })
            .collect()
    }

    /// Drop what is collected (protection turned off).
    pub fn reset(&mut self) {
        self.frame.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(seq: u16, ts: u32, marker: bool, ext: bool, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![
            0x80 | if ext { 0x10 } else { 0 },
            96 | if marker { 0x80 } else { 0 },
        ];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&ts.to_be_bytes());
        p.extend_from_slice(&0x1234_5678u32.to_be_bytes());
        if ext {
            p.extend_from_slice(&[0xbe, 0xde, 0, 1, 0x10, seq as u8, 0, 0]);
        }
        p.extend_from_slice(payload);
        p
    }

    fn frame(first: u16, n: usize, ts: u32) -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| {
                let len = 50 + (i * 37) % 900;
                let payload: Vec<u8> = (0..len).map(|b| (b * 7 + i * 13) as u8).collect();
                pkt(
                    first.wrapping_add(i as u16),
                    ts,
                    i == n - 1,
                    i % 2 == 0,
                    &payload,
                )
            })
            .collect()
    }

    #[test]
    fn every_single_loss_is_rebuilt_exactly() {
        for (first, n) in [
            (1000u16, 1usize),
            (7, 5),
            (65530, 12),
            (100, 15),
            (200, 16),
            (300, 46),
            (400, 47),
            (500, 48),
        ] {
            let f = frame(first, n, 90_000);
            let refs: Vec<&[u8]> = f.iter().map(|p| p.as_slice()).collect();
            let r = repair(&refs).unwrap();
            let rep = parse(&r).unwrap();
            assert_eq!(rep.base, first);
            assert_eq!(rep.protected.len(), n);
            for (lost, want) in f.iter().enumerate() {
                let others: Vec<&[u8]> = refs
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != lost)
                    .map(|(_, p)| *p)
                    .collect();
                let got =
                    recover(&rep, 0x1234_5678, first.wrapping_add(lost as u16), &others).unwrap();
                assert_eq!(&got, want, "first {first} n {n} lost {lost}");
            }
        }
    }

    #[test]
    fn header_layout_follows_flexfec_03() {
        let f = frame(10, 3, 0xdead_beef);
        let refs: Vec<&[u8]> = f.iter().map(|p| p.as_slice()).collect();
        let r = repair(&refs).unwrap();
        assert_eq!(r[0] >> 6, 0, "R=0 F=0");
        assert_eq!(
            be32(&r[4..]),
            0xdead_beef,
            "TS recovery of an odd count of equal timestamps"
        );
        assert_eq!(r[8], 1, "SSRCCount");
        assert_eq!(&r[9..12], &[0, 0, 0], "reserved");
        assert_eq!(be32(&r[12..]), 0x1234_5678, "protected SSRC");
        assert_eq!(be16(&r[16..]), 10, "SN base");
        // 15-bit mask, k=1 (last chunk), packets 0..2 -> bits 14,13,12
        assert_eq!(be16(&r[18..]), 0b1111_0000_0000_0000);
        assert_eq!(
            r.len(),
            20 + refs.iter().map(|p| p.len() - 12).max().unwrap()
        );
        // 46-bit mask: k=0 in the first chunk, k=1 in the second
        let f = frame(10, 20, 1);
        let refs: Vec<&[u8]> = f.iter().map(|p| p.as_slice()).collect();
        let r = repair(&refs).unwrap();
        assert_eq!(be16(&r[18..]), 0x7fff);
        assert_eq!(be32(&r[20..]), 0b1111_1100_0000_0000_0000_0000_0000_0000);
        // 109-bit mask: k=0, k=0, k=1; packet 46 is the first bit of chunk 3
        let f = frame(10, 47, 1);
        let refs: Vec<&[u8]> = f.iter().map(|p| p.as_slice()).collect();
        let r = repair(&refs).unwrap();
        assert_eq!(be16(&r[18..]), 0x7fff);
        assert_eq!(be32(&r[20..]), 0x7fff_ffff);
        assert_eq!(
            u64::from_be_bytes(r[24..32].try_into().unwrap()),
            0xc000_0000_0000_0000
        );
        let rep = parse(&r).unwrap();
        assert_eq!(rep.ssrc, 0x1234_5678);
        assert_eq!(rep.protected, (10..57).collect::<Vec<u16>>());
        // MAX_SPAN apart: refused
        let a = pkt(0, 0, false, false, &[1]);
        let b = pkt(MAX_SPAN as u16, 0, false, false, &[1]);
        assert!(repair(&[&a, &b]).is_none());
        assert!(repair(&[]).is_none());
    }

    #[test]
    fn two_losses_in_one_set_are_not_guessed() {
        let f = frame(0, 4, 5);
        let refs: Vec<&[u8]> = f.iter().map(|p| p.as_slice()).collect();
        let r = repair(&refs).unwrap();
        let rep = parse(&r).unwrap();
        assert!(recover(&rep, 0x1234_5678, 1, &[refs[0], refs[3]]).is_none());
        assert!(recover(&rep, 0x1234_5678, 9, &refs).is_none());
    }

    #[test]
    fn plan_interleaves_and_honours_overhead() {
        assert_eq!(plan(1, 10), vec![vec![0]]);
        assert_eq!(plan(4, 50), vec![vec![0, 2], vec![1, 3]]);
        assert_eq!(plan(10, 20), vec![vec![0, 2, 4, 6, 8], vec![1, 3, 5, 7, 9]]);
        let p = plan(250, 10);
        // blocks of MAX_SPAN, 10% of each (rounded up) repair packets
        let blocks = 250 / MAX_SPAN;
        let rest = 250 % MAX_SPAN;
        assert_eq!(
            p.len(),
            blocks * (MAX_SPAN * 10).div_ceil(100) + (rest * 10).div_ceil(100)
        );
        assert!(p.iter().all(|s| s.last().unwrap() - s[0] < MAX_SPAN));
        let mut all: Vec<usize> = p.into_iter().flatten().collect();
        all.sort();
        assert_eq!(
            all,
            (0..250).collect::<Vec<_>>(),
            "every packet protected once"
        );
    }

    #[test]
    fn protector_emits_at_frame_end_with_interleaved_sets() {
        let mut pr = Protector::new(0x1234_5678, 25);
        let f = frame(40, 8, 3000);
        let mut repairs = Vec::new();
        for (i, p) in f.iter().enumerate() {
            let out = pr.push(p);
            if i < 7 {
                assert!(out.is_empty());
            }
            repairs.extend(out);
        }
        assert_eq!(repairs.len(), 2);
        // lose packets 2 and 3 (a burst of two): both rebuilt
        let refs: Vec<&[u8]> = f.iter().map(|p| p.as_slice()).collect();
        let others: Vec<&[u8]> = refs
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 2 && *i != 3)
            .map(|(_, p)| *p)
            .collect();
        let mut rebuilt = 0;
        for (ts, r) in &repairs {
            assert_eq!(*ts, 3000);
            let rep = parse(r).unwrap();
            for lost in [42u16, 43] {
                if let Some(p) = recover(&rep, 0x1234_5678, lost, &others) {
                    assert_eq!(p, f[(lost - 40) as usize]);
                    rebuilt += 1;
                }
            }
        }
        assert_eq!(rebuilt, 2);
        // other SSRCs are ignored; a timestamp change closes a frame without marker
        let mut other = pkt(1, 1, true, false, &[1]);
        other[8] = 9;
        assert!(pr.push(&other).is_empty());
        let a = pkt(1, 100, false, false, &[1, 2]);
        let b = pkt(2, 200, false, false, &[3]);
        assert!(pr.push(&a).is_empty());
        assert_eq!(pr.push(&b).len(), 1);
    }
}
