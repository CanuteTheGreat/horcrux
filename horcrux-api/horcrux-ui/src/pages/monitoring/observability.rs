//! Monitoring > Observability: OpenTelemetry (OTLP/HTTP) metrics export
//! over `GET/POST /api/observability/config`, "Export now"
//! (`POST /api/observability/export/metrics`) and the last export's outcome
//! (`GET /api/observability/status`). Prometheus scraping needs no setup:
//! every node serves `/metrics`.
use crate::api;
use leptos::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct OtelConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub protocol: String,
    pub service_name: String,
    pub service_version: String,
    pub export_interval_secs: u64,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub tls_enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct OtelExportStatus {
    pub last_attempt: Option<String>,
    pub last_success: Option<String>,
    pub last_error: Option<String>,
    pub exports: u64,
}

fn headers_text(h: &HashMap<String, String>) -> String {
    let mut lines: Vec<String> = h.iter().map(|(k, v)| format!("{k}={v}")).collect();
    lines.sort();
    lines.join("\n")
}

fn parse_headers(t: &str) -> HashMap<String, String> {
    t.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

#[component]
pub fn ObservabilityPage() -> impl IntoView {
    let cfg = create_rw_signal(None::<OtelConfig>);
    let headers = create_rw_signal(String::new());
    let (status, set_status) = create_signal(None::<OtelExportStatus>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (error, set_error) = create_signal(None::<String>);
    let load = move || {
        spawn_local(async move {
            match api::fetch_json::<OtelConfig>("/observability/config").await {
                Ok(c) => {
                    headers.set(headers_text(&c.headers));
                    cfg.set(Some(c));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            if let Ok(s) = api::fetch_json::<OtelExportStatus>("/observability/status").await {
                set_status.set(Some(s));
            }
        });
    };
    load();
    let set = move |f: Box<dyn Fn(&mut OtelConfig)>| {
        cfg.update(|c| {
            if let Some(c) = c {
                f(c)
            }
        })
    };
    let save = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let Some(mut c) = cfg.get() else { return };
        c.headers = parse_headers(&headers.get());
        spawn_local(async move {
            match api::post_json::<serde_json::Value, _>("/observability/config", &c).await {
                Ok(_) => {
                    set_error.set(None);
                    set_notice.set(Some(if c.enabled {
                        format!(
                            "Saved: metrics are exported to {} every {} s",
                            c.endpoint, c.export_interval_secs
                        )
                    } else {
                        "Saved: export is off".to_string()
                    }));
                }
                Err(e) => {
                    set_notice.set(None);
                    set_error.set(Some(e.message));
                }
            }
            load();
        });
    };
    let export_now = move |_| {
        spawn_local(async move {
            match api::post_json::<serde_json::Value, _>(
                "/observability/export/metrics",
                &serde_json::json!({}),
            )
            .await
            {
                Ok(_) => {
                    set_error.set(None);
                    set_notice.set(Some("Exported the current metrics".into()));
                }
                Err(e) => {
                    set_notice.set(None);
                    set_error.set(Some(format!("Export failed: {}", e.message)));
                }
            }
            load();
        });
    };
    view! {
        <div class="observability-page">
            <div class="page-header">
                <h1>"Observability"</h1>
            </div>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="otel-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="otel-error">{e}</div> })}
            <section class="section">
                <h2>"OpenTelemetry metrics export (OTLP/HTTP)"</h2>
                <p class="form-help">"Sends this node's CPU, memory and disk metrics to an OpenTelemetry collector (Grafana Alloy, the OTel Collector, ...) at a fixed interval."</p>
                {move || cfg.get().map(|c| view! {
                    <form id="otel-form" on:submit=save>
                        <label><input id="otel-enabled" type="checkbox" prop:checked=c.enabled
                            on:change=move |ev| { let v = event_target_checked(&ev); set(Box::new(move |c| c.enabled = v)) }/>" Export enabled"</label>
                        <div class="form-group">
                            <label for="otel-endpoint">"Collector endpoint"</label>
                            <input id="otel-endpoint" type="url" placeholder="http://collector:4318" prop:value=c.endpoint.clone()
                                on:change=move |ev| { let v = event_target_value(&ev).trim().to_string(); set(Box::new(move |c| c.endpoint = v.clone())) }/>
                            <small class="form-help">"Base URL; /v1/metrics is appended. HTTP protocol (OTLP/gRPC is not supported)."</small>
                        </div>
                        <div class="form-row">
                            <div class="form-group">
                                <label for="otel-service">"Service name"</label>
                                <input id="otel-service" type="text" prop:value=c.service_name.clone()
                                    on:change=move |ev| { let v = event_target_value(&ev).trim().to_string(); set(Box::new(move |c| c.service_name = v.clone())) }/>
                            </div>
                            <div class="form-group">
                                <label for="otel-interval">"Interval (seconds)"</label>
                                <input id="otel-interval" type="number" min="5" prop:value=c.export_interval_secs.to_string()
                                    on:change=move |ev| { let v = event_target_value(&ev).parse().unwrap_or(60); set(Box::new(move |c| c.export_interval_secs = v)) }/>
                            </div>
                        </div>
                        <div class="form-group">
                            <label for="otel-headers">"Extra HTTP headers (one name=value per line, e.g. an API key)"</label>
                            <textarea id="otel-headers" rows="3" prop:value=headers
                                on:input=move |ev| headers.set(event_target_value(&ev))></textarea>
                        </div>
                        <div class="form-actions">
                            <button type="submit" id="otel-save" class="btn btn-primary">"Save"</button>
                            <button type="button" id="otel-export-now" class="btn btn-secondary"
                                disabled=!c.enabled on:click=export_now>"Export now"</button>
                        </div>
                    </form>
                })}
                {move || status.get().map(|s| view! {
                    <p class="form-help" id="otel-status">
                        {format!(
                            "Exports sent: {}. Last success: {}. {}",
                            s.exports,
                            s.last_success.clone().unwrap_or_else(|| "never".into()),
                            s.last_error.clone().map(|e| format!("Last error: {e}")).unwrap_or_default()
                        )}
                    </p>
                })}
            </section>
            <section class="section">
                <h2>"Prometheus"</h2>
                <p>"Every node serves Prometheus metrics at " <code>"/metrics"</code> " (no setup needed): add the node to your Prometheus scrape configuration."</p>
            </section>
        </div>
    }
}
