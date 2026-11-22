//! Frame-rate cap of the node's encoder: a token bucket instead of a fixed
//! minimum gap between frames.
//!
//! With a fixed gap, a frame that carries only part of a change (the guest
//! was still drawing when QEMU's display poll caught it) holds the frame
//! that completes the change back for a whole frame interval (33 ms at
//! 30 fps). The bucket keeps the same average rate but lets up to `burst`
//! frames leave back to back after a pause, so the completing frame goes
//! out as soon as it is drawn. `burst` 1 is the strict minimum gap.
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct FramePacer {
    gap: Duration,
    burst: f64,
    credit: f64,
    at: Instant,
}

impl FramePacer {
    /// `fps` frames per second on average, up to `burst` (at least 1) in a row.
    pub fn new(fps: u32, burst: u32, now: Instant) -> Self {
        let burst = burst.max(1) as f64;
        FramePacer {
            gap: Self::gap_for(fps),
            burst,
            credit: burst,
            at: now,
        }
    }

    fn gap_for(fps: u32) -> Duration {
        Duration::from_secs_f64(1.0 / fps.max(1) as f64)
    }

    fn refill(&mut self, now: Instant) {
        let earned = now.saturating_duration_since(self.at).as_secs_f64() / self.gap.as_secs_f64();
        self.credit = (self.credit + earned).min(self.burst);
        self.at = now;
    }

    /// May a frame leave now?
    pub fn ready(&mut self, now: Instant) -> bool {
        self.refill(now);
        self.credit >= 1.0
    }

    /// A frame left now.
    pub fn sent(&mut self, now: Instant) {
        self.refill(now);
        self.credit = (self.credit - 1.0).max(0.0);
    }

    /// New average rate (link adaptation); earned credit is kept.
    pub fn set_fps(&mut self, fps: u32, now: Instant) {
        self.refill(now);
        self.gap = Self::gap_for(fps);
    }

    pub fn gap(&self) -> Duration {
        self.gap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn burst_one_is_a_strict_minimum_gap() {
        let t0 = Instant::now();
        let mut p = FramePacer::new(30, 1, t0);
        assert!(p.ready(t0));
        p.sent(t0);
        assert!(!p.ready(t0 + ms(20)));
        assert!(p.ready(t0 + ms(34)));
    }

    #[test]
    fn a_completing_frame_follows_a_partial_one_at_once_after_a_pause() {
        let t0 = Instant::now();
        let mut p = FramePacer::new(30, 2, t0);
        // idle for a while: full bucket
        let t1 = t0 + ms(1000);
        assert!(p.ready(t1));
        p.sent(t1); // partial frame
        assert!(
            p.ready(t1 + ms(8)),
            "the completing frame 8 ms later is not held back"
        );
        p.sent(t1 + ms(8));
        // the bucket is empty: the next waits for the earned interval
        assert!(!p.ready(t1 + ms(20)));
        assert!(p.ready(t1 + ms(42)));
    }

    #[test]
    fn the_average_rate_never_exceeds_the_cap() {
        let t0 = Instant::now();
        let mut p = FramePacer::new(30, 2, t0);
        let mut sent = 0;
        // try every millisecond for 10 s
        for i in 0..10_000u64 {
            let t = t0 + ms(i);
            if p.ready(t) {
                p.sent(t);
                sent += 1;
            }
        }
        // 30/s for 10 s plus the initial burst
        assert!(sent <= 30 * 10 + 2, "sent {sent}");
        assert!(sent >= 30 * 10 - 1, "sent {sent}");
    }

    #[test]
    fn a_new_rate_applies_to_the_next_interval() {
        let t0 = Instant::now();
        let mut p = FramePacer::new(60, 1, t0);
        p.sent(t0);
        p.set_fps(10, t0 + ms(5));
        assert_eq!(p.gap(), ms(100));
        assert!(!p.ready(t0 + ms(50)));
        assert!(p.ready(t0 + ms(110)));
    }
}
