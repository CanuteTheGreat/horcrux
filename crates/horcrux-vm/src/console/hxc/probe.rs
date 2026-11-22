//! Per-stage timing of input-to-picture on the node (host-encode path).
//!
//! The first input after a still picture starts a probe; the node notes when
//! it handed the input to the guest, when the guest's display first changed,
//! when that picture went into the encoder, when it came out and when it was
//! written to the WebRTC stack. The client receives these as a ctl
//! `{"t":"timing",...}` message and adds its own stages (network, decode,
//! presentation), so a slow input-to-picture can be pinned to one stage.

use serde::Serialize;
use std::time::{Duration, Instant};

/// An input older than this without a picture change stops counting (the
/// client page uses the same window).
pub const PROBE_WINDOW: Duration = Duration::from_secs(2);

/// Node-side state of the running probe (kept in the session stats).
#[derive(Debug, Clone, Copy, Default)]
pub struct LatencyProbe {
    /// The input that started the probe arrived from the client.
    pub input_at: Option<Instant>,
    /// It was handed to the guest (QEMU input or the guest tools).
    pub injected_at: Option<Instant>,
    /// The guest's display changed for the first time after it.
    pub update_at: Option<Instant>,
}

impl LatencyProbe {
    /// An input arrived: start a probe unless one is running.
    pub fn mark_input(&mut self, now: Instant) {
        let stale = self
            .input_at
            .map(|t| now.saturating_duration_since(t) > PROBE_WINDOW)
            .unwrap_or(true);
        if stale {
            *self = LatencyProbe {
                input_at: Some(now),
                ..Default::default()
            };
        }
    }

    /// The input was handed to the guest.
    pub fn mark_injected(&mut self, now: Instant) {
        if self.input_at.is_some() && self.injected_at.is_none() {
            self.injected_at = Some(now);
        }
    }

    /// The guest's display changed. Returns true for the first change of a
    /// running probe (the picture that answers the input).
    pub fn mark_update(&mut self, now: Instant) -> bool {
        match self.input_at {
            Some(t)
                if self.update_at.is_none() && now.saturating_duration_since(t) <= PROBE_WINDOW =>
            {
                self.update_at = Some(now);
                true
            }
            _ => false,
        }
    }

    /// The answering picture is on its way: end the probe and return it.
    pub fn take(&mut self) -> Option<LatencyProbe> {
        if self.input_at.is_some() && self.update_at.is_some() {
            Some(std::mem::take(self))
        } else {
            None
        }
    }
}

/// The frame carrying the first change after an input, through the node.
#[derive(Debug, Clone, Copy)]
pub struct FrameTiming {
    pub probe: LatencyProbe,
    pub submitted: Instant,
    pub encoded: Option<Instant>,
}

/// The ctl message sent to the client (milliseconds per stage).
#[derive(Debug, Serialize, PartialEq)]
pub struct TimingMsg {
    pub t: &'static str,
    /// Client input received -> handed to the guest.
    pub input_to_inject_ms: f64,
    /// Handed to the guest -> the guest's display changed (guest reaction
    /// plus the display device's own refresh).
    pub inject_to_update_ms: f64,
    /// Display changed -> picture handed to the encoder (frame-rate cap,
    /// compositor wake-up).
    pub update_to_submit_ms: f64,
    /// Encoder in -> encoded access unit out.
    pub encode_ms: f64,
    /// Encoded -> written to the WebRTC stack.
    pub encoded_to_sent_ms: f64,
    /// Client input received -> written to the WebRTC stack.
    pub node_ms: f64,
}

fn ms(a: Instant, b: Instant) -> f64 {
    (b.saturating_duration_since(a).as_secs_f64() * 1e6).round() / 1e3
}

impl FrameTiming {
    pub fn message(&self, sent: Instant) -> Option<TimingMsg> {
        let input = self.probe.input_at?;
        let update = self.probe.update_at?;
        let injected = self.probe.injected_at.unwrap_or(input);
        let encoded = self.encoded.unwrap_or(sent);
        Some(TimingMsg {
            t: "timing",
            input_to_inject_ms: ms(input, injected),
            inject_to_update_ms: ms(injected, update),
            update_to_submit_ms: ms(update, self.submitted),
            encode_ms: ms(self.submitted, encoded),
            encoded_to_sent_ms: ms(encoded, sent),
            node_ms: ms(input, sent),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_stages_add_up() {
        let t0 = Instant::now();
        let at = |m: u64| t0 + Duration::from_millis(m);
        let mut p = LatencyProbe::default();
        assert!(!p.mark_update(at(0)), "no probe without an input");
        p.mark_input(at(0));
        p.mark_input(at(5)); // a second input does not restart it
        p.mark_injected(at(1));
        p.mark_injected(at(9));
        assert!(p.mark_update(at(20)));
        assert!(!p.mark_update(at(25)), "only the first change counts");
        let probe = p.take().unwrap();
        assert!(
            p.take().is_none() && p.input_at.is_none(),
            "taking ends the probe"
        );
        let ft = FrameTiming {
            probe,
            submitted: at(24),
            encoded: Some(at(36)),
        };
        let m = ft.message(at(37)).unwrap();
        assert_eq!(
            m,
            TimingMsg {
                t: "timing",
                input_to_inject_ms: 1.0,
                inject_to_update_ms: 19.0,
                update_to_submit_ms: 4.0,
                encode_ms: 12.0,
                encoded_to_sent_ms: 1.0,
                node_ms: 37.0,
            }
        );
    }

    #[test]
    fn stale_probe_restarts_and_late_changes_do_not_count() {
        let t0 = Instant::now();
        let mut p = LatencyProbe::default();
        p.mark_input(t0);
        assert!(
            !p.mark_update(t0 + Duration::from_secs(3)),
            "a change long after the input is not its answer"
        );
        p.mark_input(t0 + Duration::from_secs(3));
        assert_eq!(p.input_at, Some(t0 + Duration::from_secs(3)));
        assert!(p.mark_update(t0 + Duration::from_millis(3010)));
    }
}
