//! Link adaptation (phase C3): fits the video bitrate, resolution and
//! frame rate to the congestion-control estimate of the session's link
//! (str0m's transport-wide congestion control, TWCC), capped by the
//! profile, by the node's uplink for relayed (WAN) sessions, and by the
//! client's display refresh. Pure logic; the session applies the targets.
use horcrux_common::console::WanSettings;
use serde::Serialize;
use std::time::{Duration, Instant};

/// What limits the current target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Limit {
    /// The profile's maximum bitrate (the link has more).
    Profile,
    /// The measured link estimate.
    Link,
    /// The node's configured uplink (WAN sessions).
    Uplink,
    /// The adaptation floor (`wan.min_kbps`).
    Floor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Target {
    pub kbps: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub limit: Limit,
}

#[derive(Debug, Clone)]
pub struct AbrConfig {
    pub min_kbps: u32,
    pub max_kbps: u32,
    /// Uplink cap for this session (relayed/WAN sessions), when known.
    pub uplink_kbps: Option<u32>,
    pub min_millibits_per_pixel: u32,
    /// Source (guest) size; outputs are scaled from it.
    pub src_width: u32,
    pub src_height: u32,
    pub max_fps: u32,
    pub min_fps: u32,
    /// Gaming: keep the frame rate, lower the resolution first. Desktop:
    /// keep the resolution (text), lower the frame rate first.
    pub keep_fps: bool,
    pub down_interval: Duration,
    pub up_interval: Duration,
}

impl AbrConfig {
    pub fn new(w: &WanSettings, max_kbps: u32, max_fps: u32, keep_fps: bool) -> Self {
        AbrConfig {
            min_kbps: w.min_kbps.min(max_kbps),
            max_kbps,
            uplink_kbps: None,
            min_millibits_per_pixel: w.min_millibits_per_pixel.max(1),
            src_width: 0,
            src_height: 0,
            max_fps: max_fps.max(1),
            min_fps: w.min_fps.clamp(1, max_fps.max(1)),
            keep_fps,
            down_interval: Duration::from_millis(w.down_interval_ms as u64),
            up_interval: Duration::from_millis(w.up_interval_ms as u64),
        }
    }
}

/// Output scales tried, largest first (1, 3/4, 2/3, 1/2, 1/3, 1/4).
const SCALES: [(u32, u32); 6] = [(1, 1), (3, 4), (2, 3), (1, 2), (1, 3), (1, 4)];

fn even(x: u32) -> u32 {
    (x & !1).max(2)
}

/// Target for an available link rate (kbit/s), without hysteresis.
pub fn plan(c: &AbrConfig, link_kbps: u32) -> Target {
    let mut kbps = link_kbps;
    let mut limit = Limit::Link;
    if let Some(u) = c.uplink_kbps.filter(|u| *u > 0) {
        if u <= kbps {
            kbps = u;
            limit = Limit::Uplink;
        }
    }
    if c.max_kbps <= kbps {
        kbps = c.max_kbps;
        limit = Limit::Profile;
    }
    if kbps < c.min_kbps {
        kbps = c.min_kbps;
        limit = Limit::Floor;
    }
    let (sw, sh) = (c.src_width.max(2), c.src_height.max(2));
    // bits per frame the quality floor needs at scale s and fps f
    let fits = |s: (u32, u32), f: u32| -> bool {
        let w = (sw * s.0 / s.1) as u64;
        let h = (sh * s.0 / s.1) as u64;
        let need = w * h * f as u64 * c.min_millibits_per_pixel as u64 / 1000;
        need <= kbps as u64 * 1000
    };
    let mut fps = c.max_fps;
    let mut scale = SCALES[SCALES.len() - 1];
    if c.keep_fps {
        // resolution first, then frame rate
        match SCALES.iter().find(|s| fits(**s, fps)) {
            Some(s) => scale = *s,
            None => {
                while fps > c.min_fps && !fits(scale, fps) {
                    fps -= 1;
                }
            }
        }
    } else {
        // frame rate first, then resolution
        scale = SCALES[0];
        while fps > c.min_fps && !fits(scale, fps) {
            fps -= 1;
        }
        if !fits(scale, fps) {
            scale = *SCALES
                .iter()
                .find(|s| fits(**s, fps))
                .unwrap_or(&SCALES[SCALES.len() - 1]);
        }
    }
    Target {
        kbps,
        width: even(sw * scale.0 / scale.1),
        height: even(sh * scale.0 / scale.1),
        fps,
        limit,
    }
}

/// Hysteresis around [`plan`]: step down quickly (every `down_interval`),
/// step up only after the estimate stayed higher for `up_interval`, and
/// ignore bitrate wobble under 15% that would not change size or rate.
pub struct Abr {
    pub cfg: AbrConfig,
    current: Option<Target>,
    last_change: Option<Instant>,
    higher_since: Option<Instant>,
    pub last_estimate_kbps: u32,
}

impl Abr {
    pub fn new(cfg: AbrConfig) -> Self {
        Abr {
            cfg,
            current: None,
            last_change: None,
            higher_since: None,
            last_estimate_kbps: 0,
        }
    }

    pub fn current(&self) -> Option<Target> {
        self.current
    }

    /// Change the source size or the display rate cap; the next estimate
    /// re-plans immediately.
    pub fn set_source(&mut self, w: u32, h: u32) {
        if (w, h) != (self.cfg.src_width, self.cfg.src_height) {
            self.cfg.src_width = w;
            self.cfg.src_height = h;
            self.current = None;
        }
    }

    pub fn set_max_fps(&mut self, fps: u32) {
        let fps = fps.max(1);
        if fps != self.cfg.max_fps {
            self.cfg.max_fps = fps;
            self.cfg.min_fps = self.cfg.min_fps.min(fps);
            self.current = None;
        }
    }

    pub fn set_uplink(&mut self, kbps: Option<u32>) {
        if kbps != self.cfg.uplink_kbps {
            self.cfg.uplink_kbps = kbps;
            self.current = None;
        }
    }

    /// A new link estimate (kbit/s); returns a target to apply when it
    /// should change now.
    pub fn on_estimate(&mut self, estimate_kbps: u32, now: Instant) -> Option<Target> {
        self.last_estimate_kbps = estimate_kbps;
        if self.cfg.src_width == 0 {
            return None;
        }
        // keep ~10% headroom for audio, RTCP, retransmissions
        let usable = estimate_kbps.saturating_mul(9) / 10;
        let t = plan(&self.cfg, usable);
        let Some(cur) = self.current else {
            self.apply(t, now);
            return Some(t);
        };
        if t == cur {
            self.higher_since = None;
            return None;
        }
        let since = self
            .last_change
            .map(|l| now.duration_since(l))
            .unwrap_or(Duration::MAX);
        let shape_changed = (t.width, t.height, t.fps) != (cur.width, cur.height, cur.fps);
        if t.kbps < cur.kbps || (t.width * t.height * t.fps) < (cur.width * cur.height * cur.fps)
        {
            self.higher_since = None;
            let big = shape_changed || t.kbps * 100 < cur.kbps * 85;
            if big && since >= self.cfg.down_interval {
                self.apply(t, now);
                return Some(t);
            }
            return None;
        }
        // higher
        let first = *self.higher_since.get_or_insert(now);
        let big = shape_changed || t.kbps * 100 > cur.kbps * 115 || t.limit != cur.limit;
        if big && now.duration_since(first) >= self.cfg.up_interval && since >= self.cfg.up_interval
        {
            self.apply(t, now);
            return Some(t);
        }
        None
    }

    fn apply(&mut self, t: Target, now: Instant) {
        self.current = Some(t);
        self.last_change = Some(now);
        self.higher_since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(keep_fps: bool) -> AbrConfig {
        let mut c = AbrConfig::new(&WanSettings::default(), 50_000, 120, keep_fps);
        c.src_width = 1920;
        c.src_height = 1080;
        c
    }

    #[test]
    fn plan_fits_the_quality_floor() {
        // 1080p120 at 0.05 bpp needs ~12.4 Mbit/s
        let t = plan(&cfg(true), 20_000);
        assert_eq!((t.width, t.height, t.fps, t.limit), (1920, 1080, 120, Limit::Link));
        // gaming keeps 120 fps and drops resolution
        let t = plan(&cfg(true), 5_000);
        assert_eq!(t.fps, 120);
        assert!(t.width < 1920 && t.width >= 960, "{t:?}");
        // desktop keeps 1080p and drops the frame rate
        let mut d = cfg(false);
        d.max_fps = 30;
        let t = plan(&d, 2_000);
        assert_eq!((t.width, t.height), (1920, 1080));
        assert!(t.fps < 30 && t.fps >= 10, "{t:?}");
        // profile cap and floor
        assert_eq!(plan(&cfg(true), 900_000).limit, Limit::Profile);
        assert_eq!(plan(&cfg(true), 900_000).kbps, 50_000);
        let t = plan(&cfg(true), 10);
        assert_eq!((t.kbps, t.limit), (250, Limit::Floor));
    }

    #[test]
    fn uplink_caps_wan_sessions() {
        let mut c = cfg(true);
        c.uplink_kbps = Some(8_000);
        let t = plan(&c, 100_000);
        assert_eq!((t.kbps, t.limit), (8_000, Limit::Uplink));
        // a home upload of ~50 Mbit/s: 4K120 does not fit, 1440p-ish does
        let mut c = cfg(true);
        c.src_width = 3840;
        c.src_height = 2160;
        c.uplink_kbps = Some(45_000);
        let t = plan(&c, 1_000_000);
        assert_eq!(t.limit, Limit::Uplink);
        assert!(t.width < 3840 && t.fps == 120, "{t:?}");
    }

    #[test]
    fn hysteresis_fast_down_slow_up() {
        let mut a = Abr::new(cfg(true));
        let t0 = Instant::now();
        let first = a.on_estimate(30_000, t0).unwrap();
        assert_eq!(first.width, 1920);
        // small wobble: nothing
        assert!(a.on_estimate(29_000, t0 + Duration::from_millis(1500)).is_none());
        // big drop after the down interval: immediate
        let d = a.on_estimate(4_000, t0 + Duration::from_millis(2600)).unwrap();
        assert!(d.kbps <= 3_600 && d.width < 1920);
        // recovery needs up_interval (4 s) of higher estimates
        assert!(a.on_estimate(30_000, t0 + Duration::from_millis(3000)).is_none());
        assert!(a.on_estimate(30_000, t0 + Duration::from_millis(5000)).is_none());
        let u = a.on_estimate(30_000, t0 + Duration::from_millis(7100)).unwrap();
        assert_eq!(u.width, 1920);
    }

    #[test]
    fn display_refresh_caps_fps() {
        let mut a = Abr::new(cfg(true));
        let now = Instant::now();
        a.on_estimate(40_000, now);
        a.set_max_fps(60);
        let t = a.on_estimate(40_000, now + Duration::from_millis(10)).unwrap();
        assert_eq!(t.fps, 60);
    }
}
