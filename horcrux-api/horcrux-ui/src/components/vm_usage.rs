//! Live usage graphs on a VM's Summary (Proxmox's guest Summary shows CPU,
//! memory, network and disk I/O graphs). Reads the node's per-VM history
//! (`GET /api/monitoring/history/vm.<id>.<series>`), refreshed while the
//! page is open.
use crate::api;
use leptos::*;

/// (series key below `vm.<id>.`, title, unit)
pub const SERIES: &[(&str, &str, Unit)] = &[
    ("cpu.usage", "CPU", Unit::Percent),
    ("mem.usage", "Memory", Unit::Percent),
    ("net.rx", "Network in", Unit::BytesPerSec),
    ("net.tx", "Network out", Unit::BytesPerSec),
    ("disk.read", "Disk read", Unit::BytesPerSec),
    ("disk.write", "Disk write", Unit::BytesPerSec),
];

/// Time window of the graphs and refresh interval (seconds).
pub const WINDOW_SECS: i64 = 3600;
pub const REFRESH_SECS: u64 = 15;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Percent,
    BytesPerSec,
}

pub fn fmt_value(v: f64, unit: Unit) -> String {
    match unit {
        Unit::Percent => format!("{v:.0}%"),
        Unit::BytesPerSec => {
            let (n, u) = if v >= 1e9 {
                (v / 1e9, "GB/s")
            } else if v >= 1e6 {
                (v / 1e6, "MB/s")
            } else if v >= 1e3 {
                (v / 1e3, "kB/s")
            } else {
                (v, "B/s")
            };
            if u == "B/s" {
                format!("{n:.0} {u}")
            } else {
                format!("{n:.1} {u}")
            }
        }
    }
}

/// SVG polyline points for `values` in a `w` x `h` box (newest at the right;
/// percent series scale to 100, rates to their own maximum).
pub fn polyline(points: &[(i64, f64)], from: i64, to: i64, unit: Unit, w: f64, h: f64) -> String {
    if points.is_empty() || to <= from {
        return String::new();
    }
    let max = match unit {
        Unit::Percent => points.iter().map(|p| p.1).fold(100.0, f64::max),
        Unit::BytesPerSec => points.iter().map(|p| p.1).fold(1.0, f64::max),
    };
    points
        .iter()
        .map(|&(t, v)| {
            let x = (t - from) as f64 / (to - from) as f64 * w;
            let y = h - (v.max(0.0) / max * h);
            format!("{:.1},{:.1}", x.clamp(0.0, w), y.clamp(0.0, h))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[component]
pub fn VmUsageGraphs(
    #[prop(into)] vm_id: Signal<String>,
    #[prop(into)] running: Signal<bool>,
) -> impl IntoView {
    // series key -> points (timestamp, value)
    let (data, set_data) = create_signal(Vec::<(&'static str, Vec<(i64, f64)>)>::new());
    let (window, set_window) = create_signal((0i64, 0i64));
    let (error, set_error) = create_signal(None::<String>);
    let load = move || {
        let id = vm_id.get_untracked();
        if id.is_empty() {
            return;
        }
        spawn_local(async move {
            let to = (js_sys::Date::now() / 1000.0) as i64;
            let from = to - WINDOW_SECS;
            let mut out = Vec::new();
            for &(key, _, _) in SERIES {
                match api::get_metric_history(&format!("vm.{id}.{key}"), from, to).await {
                    Ok(p) => {
                        out.push((key, p.into_iter().map(|x| (x.timestamp, x.value)).collect()))
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Could not load usage history: {}", e.message)));
                        return;
                    }
                }
            }
            set_error.set(None);
            set_window.set((from, to));
            set_data.set(out);
        });
    };
    create_effect(move |_| {
        vm_id.track();
        load();
    });
    let poll = set_interval_with_handle(load, std::time::Duration::from_secs(REFRESH_SECS)).ok();
    on_cleanup(move || {
        if let Some(p) = poll {
            p.clear();
        }
    });
    let has_any = move || data.with(|d| d.iter().any(|(_, p)| !p.is_empty()));
    view! {
        <section class="vm-usage" id="vm-usage">
            <h3>"Usage (last hour)"</h3>
            {move || error.get().map(|e| view! { <div class="alert alert-error">{e}</div> })}
            {move || (!has_any() && error.get().is_none()).then(|| view! {
                <p class="help-text" id="vm-usage-empty">{move || if running.get() {
                    "No samples yet: the node records this VM's usage while it runs; the first points appear within a minute."
                } else {
                    "No usage recorded in the last hour: the VM is not running. Start it to see live graphs."
                }}</p>
            })}
            <Show when=has_any fallback=|| ()>
                <div class="vm-usage-grid">
                    {move || {
                        let (from, to) = window.get();
                        data.get()
                            .into_iter()
                            .filter_map(|(key, pts)| {
                                let &(_, title, unit) = SERIES.iter().find(|s| s.0 == key)?;
                                let last = pts.last().map(|p| fmt_value(p.1, unit)).unwrap_or_else(|| "-".into());
                                let peak = pts.iter().map(|p| p.1).fold(0.0, f64::max);
                                let svg = format!(
                                    "<svg viewBox='0 0 240 60' preserveAspectRatio='none' role='img' aria-label='{title}'><polyline points='{}' fill='none' stroke='currentColor' stroke-width='1.5'/></svg>",
                                    polyline(&pts, from, to, unit, 240.0, 60.0)
                                );
                                Some(view! {
                                    <div class="vm-usage-card" data-series=key>
                                        <div class="vm-usage-head"><span>{title}</span>
                                            <span class="vm-usage-now">{last}</span></div>
                                        <div class="vm-usage-chart" inner_html=svg></div>
                                        <div class="vm-usage-peak">{format!("peak {}", fmt_value(peak, unit))}</div>
                                    </div>
                                })
                            })
                            .collect_view()
                    }}
                </div>
            </Show>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_in_human_units() {
        assert_eq!(fmt_value(42.4, Unit::Percent), "42%");
        assert_eq!(fmt_value(512.0, Unit::BytesPerSec), "512 B/s");
        assert_eq!(fmt_value(2_500_000.0, Unit::BytesPerSec), "2.5 MB/s");
    }

    #[test]
    fn polyline_spans_the_window_and_scales() {
        let p = polyline(
            &[(0, 0.0), (50, 50.0), (100, 100.0)],
            0,
            100,
            Unit::Percent,
            100.0,
            10.0,
        );
        assert_eq!(p, "0.0,10.0 50.0,5.0 100.0,0.0");
        assert!(polyline(&[], 0, 100, Unit::Percent, 1.0, 1.0).is_empty());
        // a rate series scales to its own peak
        let r = polyline(
            &[(0, 5.0), (10, 10.0)],
            0,
            10,
            Unit::BytesPerSec,
            10.0,
            10.0,
        );
        assert_eq!(r, "0.0,5.0 10.0,0.0");
    }

    #[test]
    fn every_series_is_recorded_by_the_node() {
        // keys must match horcrux-monitoring's vm_history_series
        let keys: Vec<&str> = SERIES.iter().map(|s| s.0).collect();
        assert_eq!(
            keys,
            [
                "cpu.usage",
                "mem.usage",
                "net.rx",
                "net.tx",
                "disk.read",
                "disk.write"
            ]
        );
    }
}
