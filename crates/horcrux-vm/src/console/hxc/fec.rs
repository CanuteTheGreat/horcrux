//! FlexFEC on the console video (`flexfec-03` repair packets from
//! `horcrux_media::flexfec`).
//!
//! Negotiated only when the client's offer carries `flexfec-03` on the video
//! m-line (Chromium advertises it behind its FlexFEC field trial; the
//! Horcrux desktop client does not offer it yet, str0m has no FEC
//! receiver): the answer then names the payload type and a
//! repair stream (`a=ssrc-group:FEC-FR <media> <repair>`, same CNAME), and
//! the transport turns every sent video frame into repair packets while the
//! measured round trip is long enough for a retransmission to hurt.
//!
//! str0m has no FEC sender: the session sees its sent RTP packets as raw
//! packets (`RtcConfig::enable_raw_packets`) and writes the repair packets
//! on a second send stream of the video m-line (direct API). Both streams
//! of the m-line then carry an internal RID so every str0m lookup (writer,
//! pacer, padding) is exact; the client demultiplexes by MID, then by
//! payload type.

/// The client's FlexFEC payload type and the video m-line's MID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offered {
    pub pt: u8,
    pub mid: String,
}

/// Internal RIDs of the media and repair streams on the video MID (str0m
/// keys send queues by MID+RID, and RID `None` would match either).
pub const MEDIA_RID: &str = "v";
pub const REPAIR_RID: &str = "fec";

fn sections(sdp: &str) -> Vec<Vec<&str>> {
    let mut out: Vec<Vec<&str>> = vec![Vec::new()];
    for l in sdp.split("\r\n").flat_map(|l| l.split('\n')) {
        if l.is_empty() {
            continue;
        }
        if l.starts_with("m=") {
            out.push(Vec::new());
        }
        out.last_mut().expect("one section").push(l);
    }
    out
}

/// FlexFEC (`flexfec-03/90000`) offered on the first video m-line.
pub fn offered(offer_sdp: &str) -> Option<Offered> {
    let video = sections(offer_sdp)
        .into_iter()
        .find(|s| s.first().map(|l| l.starts_with("m=video")).unwrap_or(false))?;
    let mid = video
        .iter()
        .find_map(|l| l.strip_prefix("a=mid:"))?
        .trim()
        .to_string();
    let pt = video.iter().find_map(|l| {
        let rest = l.strip_prefix("a=rtpmap:")?;
        let (pt, name) = rest.split_once(' ')?;
        name.trim()
            .to_ascii_lowercase()
            .starts_with("flexfec-03/90000")
            .then(|| pt.trim().parse::<u8>().ok())?
    })?;
    Some(Offered { pt, mid })
}

/// The MID of the first video m-line of an SDP.
pub fn video_mid(sdp: &str) -> Option<String> {
    sections(sdp)
        .into_iter()
        .find(|s| s.first().map(|l| l.starts_with("m=video")).unwrap_or(false))?
        .iter()
        .find_map(|l| l.strip_prefix("a=mid:"))
        .map(|m| m.trim().to_string())
}

/// Add the repair stream to the answer: the payload type on the video
/// m-line with its rtpmap/fmtp, and the FEC-FR group with the media SSRC's
/// CNAME (and MSID). None when the media SSRC or its CNAME isn't there.
pub fn answer(answer_sdp: &str, o: &Offered, media_ssrc: u32, repair_ssrc: u32) -> Option<String> {
    let mut secs = sections(answer_sdp);
    let idx = secs.iter().position(|s| {
        s.first().map(|l| l.starts_with("m=video")).unwrap_or(false)
            && s.iter()
                .any(|l| l.strip_prefix("a=mid:").map(str::trim) == Some(o.mid.as_str()))
    })?;
    let ms = media_ssrc.to_string();
    let attr = |s: &Vec<&str>, key: &str| {
        s.iter().find_map(|l| {
            let rest = l.strip_prefix("a=ssrc:")?;
            let (ssrc, kv) = rest.split_once(' ')?;
            (ssrc == ms).then(|| kv.strip_prefix(key).map(|v| v.to_string()))?
        })
    };
    let cname = attr(&secs[idx], "cname:")?;
    let msid = attr(&secs[idx], "msid:");
    let pt = o.pt.to_string();
    let sec: Vec<String> = secs[idx]
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            if i == 0 {
                let mut parts: Vec<&str> = l.split(' ').collect();
                if !parts[3..].contains(&pt.as_str()) {
                    parts.push(pt.as_str());
                }
                return Some(parts.join(" "));
            }
            // whatever was written for this payload type is replaced
            for p in ["a=rtpmap:", "a=fmtp:", "a=rtcp-fb:"] {
                if l.strip_prefix(p).and_then(|r| r.split(' ').next()) == Some(pt.as_str()) {
                    return None;
                }
            }
            Some(l.to_string())
        })
        .collect();
    let mut sec = sec;
    sec.push(format!("a=rtpmap:{pt} flexfec-03/90000"));
    sec.push(format!("a=fmtp:{pt} repair-window=10000000"));
    sec.push(format!("a=ssrc-group:FEC-FR {media_ssrc} {repair_ssrc}"));
    sec.push(format!("a=ssrc:{repair_ssrc} cname:{cname}"));
    if let Some(m) = msid {
        sec.push(format!("a=ssrc:{repair_ssrc} msid:{m}"));
    }
    let owned: Vec<Vec<String>> = secs
        .drain(..)
        .enumerate()
        .map(|(i, s)| {
            if i == idx {
                sec.clone()
            } else {
                s.into_iter().map(String::from).collect()
            }
        })
        .collect();
    let mut out = String::new();
    for l in owned.into_iter().flatten() {
        out.push_str(&l);
        out.push_str("\r\n");
    }
    Some(out)
}

/// Protect now? (`min_rtt_ms` 0: always; unknown RTT: not yet.)
pub fn active(enabled: bool, rtt_ms: Option<f64>, min_rtt_ms: u32) -> bool {
    enabled && (min_rtt_ms == 0 || rtt_ms.map(|r| r >= min_rtt_ms as f64).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFER: &str = "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0 1\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=rtpmap:111 opus/48000/2\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 96 97 35\r\na=mid:1\r\na=recvonly\r\na=rtpmap:96 H264/90000\r\n\
a=rtpmap:97 rtx/90000\r\na=fmtp:97 apt=96\r\na=rtpmap:35 flexfec-03/90000\r\na=rtcp-fb:35 transport-cc\r\n\
a=fmtp:35 repair-window=10000000\r\n";

    const ANSWER: &str = "v=0\r\no=- 3 4 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=ssrc:11 cname:abc\r\n\
m=video 9 UDP/TLS/RTP/SAVPF 96 97\r\na=mid:1\r\na=sendonly\r\na=rtpmap:96 H264/90000\r\n\
a=rtpmap:97 rtx/90000\r\na=fmtp:97 apt=96\r\na=ssrc-group:FID 1000 1001\r\n\
a=ssrc:1000 cname:abc\r\na=ssrc:1000 msid:s t\r\na=ssrc:1001 cname:abc\r\n";

    #[test]
    fn finds_the_offered_payload_type() {
        assert_eq!(
            offered(OFFER),
            Some(Offered {
                pt: 35,
                mid: "1".into()
            })
        );
        assert_eq!(offered(&OFFER.replace("flexfec-03", "ulpfec")), None);
        // only on the video m-line
        let audio_only =
            OFFER.split("m=video").next().unwrap().to_string() + "a=rtpmap:35 flexfec-03/90000\r\n";
        assert_eq!(offered(&audio_only), None);
    }

    #[test]
    fn answer_names_the_repair_stream() {
        let o = offered(OFFER).unwrap();
        let a = answer(ANSWER, &o, 1000, 4242).unwrap();
        assert!(
            a.contains("m=video 9 UDP/TLS/RTP/SAVPF 96 97 35\r\n"),
            "{a}"
        );
        assert!(a.contains("a=rtpmap:35 flexfec-03/90000\r\na=fmtp:35 repair-window=10000000\r\n"));
        assert!(a.contains(
            "a=ssrc-group:FEC-FR 1000 4242\r\na=ssrc:4242 cname:abc\r\na=ssrc:4242 msid:s t\r\n"
        ));
        assert!(
            a.starts_with("v=0\r\n")
                && a.contains("m=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\n")
        );
        assert_eq!(a.matches("flexfec").count(), 1);
        // str0m's own line for the payload type is replaced, not doubled
        let b = ANSWER.replace(
            "m=video 9 UDP/TLS/RTP/SAVPF 96 97",
            "m=video 9 UDP/TLS/RTP/SAVPF 96 97 35",
        ) + "a=rtpmap:35 unknown/90000\r\n";
        let a2 = answer(&b, &o, 1000, 4242).unwrap();
        assert!(!a2.contains("unknown") && a2.contains("SAVPF 96 97 35\r\n"));
        // no CNAME for the media SSRC: no FEC
        assert!(answer(ANSWER, &o, 999, 4242).is_none());
    }

    #[test]
    fn finds_the_video_mid() {
        assert_eq!(video_mid(OFFER).as_deref(), Some("1"));
        assert_eq!(video_mid(ANSWER).as_deref(), Some("1"));
        assert_eq!(video_mid("v=0\r\nm=audio 9 X 1\r\na=mid:0\r\n"), None);
    }

    #[test]
    fn active_only_on_long_round_trips() {
        assert!(!active(true, None, 40));
        assert!(!active(true, Some(20.0), 40));
        assert!(active(true, Some(40.0), 40));
        assert!(active(true, None, 0));
        assert!(!active(false, Some(100.0), 0));
    }
}
