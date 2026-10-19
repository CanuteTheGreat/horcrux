//! A chart panel over one Metrics Explorer query (`POST /api/metrics/query`),
//! shared by Monitoring > Overview ("Performance Trends") and user-defined
//! dashboards (Monitoring > Dashboards).
use crate::api::{query_metrics, MetricQuery, MetricResult};
use leptos::*;

/// Latest value of every series in `results`, formatted for a stat panel.
pub fn stat_html(results: &[MetricResult]) -> String {
    let mut rows: Vec<(String, f64)> = Vec::new();
    for r in results {
        let mut by_series: std::collections::BTreeMap<String, (i64, f64)> = Default::default();
        for p in &r.values {
            let mut key: Vec<String> = p
                .labels
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect();
            key.sort();
            let t = p.timestamp.timestamp();
            let e = by_series.entry(key.join(", ")).or_insert((t, p.value));
            if t >= e.0 {
                *e = (t, p.value);
            }
        }
        rows.extend(by_series.into_iter().map(|(k, (_, v))| (k, v)));
    }
    if rows.is_empty() {
        return "<p>No data points in this range</p>".to_string();
    }
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");
    rows.iter()
        .map(|(k, v)| {
            format!(
                "<div class='stat-value' data-series='{}'><span class='stat-number'>{}</span> <span class='stat-label'>{}</span></div>",
                esc(k).replace('\'', "&#39;"),
                format_value(*v),
                esc(k)
            )
        })
        .collect()
}

fn format_value(v: f64) -> String {
    let a = v.abs();
    if a >= 1e12 {
        format!("{:.2} T", v / 1e12)
    } else if a >= 1e9 {
        format!("{:.2} G", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.2} M", v / 1e6)
    } else if a >= 1e4 {
        format!("{:.1} k", v / 1e3)
    } else {
        format!("{:.2}", v)
    }
}

/// One panel: title, a chart (line/area/bar) or stat, refreshed whenever
/// `range_secs` or `tick` changes.
#[component]
pub fn MetricPanel(
    #[prop(into)] title: String,
    #[prop(into)] query: String,
    #[prop(into)] chart: String,
    /// Seconds back from now
    #[prop(into)]
    range_secs: Signal<i64>,
    /// Bump to reload
    #[prop(into)]
    tick: Signal<u64>,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let (html, set_html) = create_signal(String::from("<p class='loading'>Loading...</p>"));
    let (err, set_err) = create_signal(None::<String>);
    let q = query.clone();
    let c = chart.clone();
    create_effect(move |_| {
        let secs = range_secs.get();
        let _ = tick.get();
        let q = q.clone();
        let c = c.clone();
        spawn_local(async move {
            let end = chrono::Utc::now();
            let start = end - chrono::Duration::seconds(secs);
            let step = ((secs / 120).max(15)) as u64;
            let mq = MetricQuery {
                metric: q,
                labels: Default::default(),
                start_time: Some(start),
                end_time: Some(end),
                step,
            };
            match query_metrics(mq).await {
                Ok(r) => {
                    let results = vec![r];
                    set_err.set(None);
                    set_html.set(if c == "stat" {
                        stat_html(&results)
                    } else {
                        crate::pages::monitoring::metrics_explorer::render_chart_svg(&results, &c)
                    });
                }
                Err(e) => set_err.set(Some(e.message)),
            }
        });
    });
    view! {
        <div class=format!("metric-panel {}", class) data-query=query.clone() data-chart=chart.clone()>
            <h3 class="metric-panel-title">{title}</h3>
            {move || match err.get() {
                Some(e) => view! { <p class="error">{e}</p> }.into_view(),
                None => view! { <div class="metric-panel-body" inner_html=move || html.get()></div> }.into_view(),
            }}
        </div>
    }
}
