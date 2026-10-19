//! Monitoring > Dashboards: user-defined dashboards of metric charts.
//!
//! Create a dashboard, add panels (a metric or Metrics Explorer query drawn
//! as a line/area/bar chart or a single-value stat), save it; it is stored by
//! the API (`/api/monitoring/dashboards`) and comes back after a reload or on
//! another browser. `?id=<dashboard>` opens one directly.
use super::metric_panel::MetricPanel;
use crate::api::dashboards::{self as dapi, Dashboard, DashboardPanel, DashboardRequest, RANGES};
use crate::api::{get_available_metrics, MetricDefinition};
use leptos::*;
use leptos_router::*;

const CHARTS: &[(&str, &str)] = &[
    ("line", "Line chart"),
    ("area", "Area chart"),
    ("bar", "Bar chart"),
    ("stat", "Single value"),
];

#[derive(Clone, Debug, PartialEq)]
struct PanelDraft {
    id: String,
    title: String,
    query: String,
    chart: String,
    width: u8,
}

impl PanelDraft {
    fn new() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            query: "horcrux_node_cpu_usage_percent".into(),
            chart: "line".into(),
            width: 1,
        }
    }
    fn of(p: &DashboardPanel) -> Self {
        Self {
            id: p.id.clone(),
            title: p.title.clone(),
            query: p.query.clone(),
            chart: p.chart.clone(),
            width: p.width,
        }
    }
}

#[component]
pub fn DashboardsPage() -> impl IntoView {
    let query = use_query_map();
    let navigate = use_navigate();

    let (dashboards, set_dashboards) = create_signal(Vec::<Dashboard>::new());
    let (metrics, set_metrics) = create_signal(Vec::<MetricDefinition>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (loaded, set_loaded) = create_signal(false);

    // Editor state: None = viewing; Some(None) = new; Some(Some(id)) = edit
    let (editing, set_editing) = create_signal(None::<Option<String>>);
    let (f_name, set_f_name) = create_signal(String::new());
    let (f_desc, set_f_desc) = create_signal(String::new());
    let (f_range, set_f_range) = create_signal("1h".to_string());
    let (f_refresh, set_f_refresh) = create_signal(30u32);
    let (f_panels, set_f_panels) = create_signal(Vec::<PanelDraft>::new());

    // Viewing state
    let (view_range, set_view_range) = create_signal(None::<String>);
    let (tick, set_tick) = create_signal(0u64);

    create_effect(move |_| {
        spawn_local(async move {
            match dapi::list().await {
                Ok(list) => {
                    set_dashboards.set(list);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(format!("Failed to load dashboards: {}", e.message))),
            }
            set_loaded.set(true);
            if let Ok(m) = get_available_metrics().await {
                set_metrics.set(m);
            }
        });
    });

    let selected_id = move || query.with(|q| q.get("id").cloned());
    let selected = move || {
        let list = dashboards.get();
        match selected_id() {
            Some(id) => list.into_iter().find(|d| d.id == id),
            None => list.into_iter().next(),
        }
    };

    // Auto-refresh of the dashboard being viewed.
    let interval_handle = store_value(None::<leptos::leptos_dom::helpers::IntervalHandle>);
    create_effect(move |_| {
        if let Some(h) = interval_handle.get_value() {
            h.clear();
        }
        let secs = selected().map(|d| d.refresh_secs).unwrap_or(0);
        if secs > 0 && editing.get().is_none() {
            interval_handle.set_value(
                set_interval_with_handle(
                    move || set_tick.update(|t| *t += 1),
                    std::time::Duration::from_secs(secs as u64),
                )
                .ok(),
            );
        } else {
            interval_handle.set_value(None);
        }
    });
    on_cleanup(move || {
        if let Some(h) = interval_handle.get_value() {
            h.clear();
        }
    });

    let open_new = move |_| {
        set_f_name.set(String::new());
        set_f_desc.set(String::new());
        set_f_range.set("1h".into());
        set_f_refresh.set(30);
        set_f_panels.set(vec![PanelDraft {
            title: "CPU usage (%)".into(),
            ..PanelDraft::new()
        }]);
        set_notice.set(None);
        set_editing.set(Some(None));
    };
    let open_edit = move |d: Dashboard| {
        set_f_name.set(d.name.clone());
        set_f_desc.set(d.description.clone());
        set_f_range.set(d.time_range.clone());
        set_f_refresh.set(d.refresh_secs);
        set_f_panels.set(d.panels.iter().map(PanelDraft::of).collect());
        set_notice.set(None);
        set_editing.set(Some(Some(d.id)));
    };

    let nav_save = navigate.clone();
    let save = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let req = DashboardRequest {
            name: f_name.get(),
            description: f_desc.get(),
            time_range: f_range.get(),
            refresh_secs: f_refresh.get(),
            panels: f_panels
                .get()
                .into_iter()
                .map(|p| DashboardPanel {
                    id: p.id,
                    title: p.title,
                    query: p.query,
                    chart: p.chart,
                    width: p.width,
                })
                .collect(),
        };
        let target = editing.get().flatten();
        let navigate = nav_save.clone();
        spawn_local(async move {
            let res = match &target {
                Some(id) => dapi::update(id, &req).await,
                None => dapi::create(&req).await,
            };
            match res {
                Ok(d) => {
                    set_editing.set(None);
                    set_error.set(None);
                    set_notice.set(Some(format!("Dashboard \"{}\" saved", d.name)));
                    if let Ok(list) = dapi::list().await {
                        set_dashboards.set(list);
                    }
                    navigate(
                        &format!("/monitoring/dashboards?id={}", d.id),
                        Default::default(),
                    );
                }
                Err(e) => set_error.set(Some(format!("Save failed: {}", e.message))),
            }
        });
    };

    let nav_del = navigate.clone();
    let delete = move |d: Dashboard| {
        let ok = web_sys::window()
            .and_then(|w| {
                w.confirm_with_message(&format!("Delete dashboard \"{}\"?", d.name))
                    .ok()
            })
            .unwrap_or(false);
        if !ok {
            return;
        }
        let navigate = nav_del.clone();
        spawn_local(async move {
            match dapi::delete(&d.id).await {
                Ok(()) => {
                    set_notice.set(Some(format!("Dashboard \"{}\" deleted", d.name)));
                    if let Ok(list) = dapi::list().await {
                        set_dashboards.set(list);
                    }
                    navigate("/monitoring/dashboards", Default::default());
                }
                Err(e) => set_error.set(Some(format!("Delete failed: {}", e.message))),
            }
        });
    };

    let set_panel = move |i: usize, f: &dyn Fn(&mut PanelDraft)| {
        set_f_panels.update(|ps| {
            if let Some(p) = ps.get_mut(i) {
                f(p)
            }
        })
    };

    let editor = move |target: Option<String>| {
        let title = if target.is_some() {
            "Edit dashboard"
        } else {
            "New dashboard"
        };
        view! {
            <form class="dashboard-editor" id="dashboard-editor" on:submit=save.clone()>
                <h2>{title}</h2>
                <div class="form-row">
                    <div class="form-group">
                        <label>"Name"</label>
                        <input type="text" id="dash-name" required prop:value=f_name
                            on:input=move |ev| set_f_name.set(event_target_value(&ev))/>
                    </div>
                    <div class="form-group">
                        <label>"Description"</label>
                        <input type="text" id="dash-desc" prop:value=f_desc
                            on:input=move |ev| set_f_desc.set(event_target_value(&ev))/>
                    </div>
                    <div class="form-group">
                        <label>"Time range"</label>
                        <select id="dash-range" on:change=move |ev| set_f_range.set(event_target_value(&ev))>
                            {RANGES.iter().map(|(id, label, _)| {
                                let id = *id;
                                view! { <option value=id selected=move || f_range.get() == id>{*label}</option> }
                            }).collect_view()}
                        </select>
                    </div>
                    <div class="form-group">
                        <label>"Refresh (s, 0 = off)"</label>
                        <input type="number" id="dash-refresh" min="0" max="3600"
                            prop:value=move || f_refresh.get().to_string()
                            on:input=move |ev| set_f_refresh.set(event_target_value(&ev).parse().unwrap_or(0))/>
                    </div>
                </div>

                <h3>"Panels"</h3>
                <datalist id="metric-names">
                    {move || metrics.get().into_iter().map(|m| view! {
                        <option value=m.name.clone()>{m.description.clone()}</option>
                    }).collect_view()}
                </datalist>
                <div class="panel-editor-list">
                    {move || {
                        let panels = f_panels.get();
                        let n = panels.len();
                        panels.into_iter().enumerate().map(|(i, p)| {
                            let chart = p.chart.clone();
                            view! {
                                <div class="panel-editor" data-index=i>
                                    <input type="text" class="panel-title" placeholder="Title" required
                                        prop:value=p.title.clone()
                                        on:change=move |ev| { let v = event_target_value(&ev); set_panel(i, &|p| p.title = v.clone()) }/>
                                    <input type="text" class="panel-query" list="metric-names" placeholder="Metric or query" required
                                        prop:value=p.query.clone()
                                        on:change=move |ev| { let v = event_target_value(&ev); set_panel(i, &|p| p.query = v.clone()) }/>
                                    <select class="panel-chart"
                                        on:change=move |ev| { let v = event_target_value(&ev); set_panel(i, &|p| p.chart = v.clone()) }>
                                        {CHARTS.iter().map(|(id, label)| {
                                            let sel = chart == *id;
                                            view! { <option value=*id selected=sel>{*label}</option> }
                                        }).collect_view()}
                                    </select>
                                    <select class="panel-width"
                                        on:change=move |ev| { let v: u8 = event_target_value(&ev).parse().unwrap_or(1); set_panel(i, &|p| p.width = v) }>
                                        <option value="1" selected=p.width == 1>"Half width"</option>
                                        <option value="2" selected=p.width == 2>"Full width"</option>
                                    </select>
                                    <button type="button" class="btn btn-sm panel-up" disabled=i == 0
                                        on:click=move |_| set_f_panels.update(|ps| if i > 0 { ps.swap(i, i - 1) })>"↑"</button>
                                    <button type="button" class="btn btn-sm panel-down" disabled=i + 1 >= n
                                        on:click=move |_| set_f_panels.update(|ps| if i + 1 < ps.len() { ps.swap(i, i + 1) })>"↓"</button>
                                    <button type="button" class="btn btn-sm btn-danger panel-remove"
                                        on:click=move |_| set_f_panels.update(|ps| { ps.remove(i); })>"Remove"</button>
                                </div>
                            }
                        }).collect_view()
                    }}
                </div>
                <button type="button" class="btn" id="add-panel"
                    on:click=move |_| set_f_panels.update(|ps| ps.push(PanelDraft::new()))>"+ Add panel"</button>
                <div class="form-actions">
                    <button type="submit" class="btn btn-primary" id="save-dashboard">"Save dashboard"</button>
                    <button type="button" class="btn" id="cancel-dashboard" on:click=move |_| set_editing.set(None)>"Cancel"</button>
                </div>
            </form>
        }
        .into_view()
    };

    let viewer = move |d: Dashboard| {
        let delete = delete.clone();
        let d_edit = d.clone();
        let d_del = d.clone();
        let range_id = d.time_range.clone();
        let range = Signal::derive(move || {
            dapi::range_secs(&view_range.get().unwrap_or_else(|| range_id.clone()))
        });
        let tick: Signal<u64> = tick.into();
        let current_range = view_range
            .get_untracked()
            .unwrap_or_else(|| d.time_range.clone());
        view! {
            <div class="dashboard-view" id="dashboard-view" data-id=d.id.clone()>
                <div class="dashboard-view-header">
                    <div>
                        <h2 id="dashboard-title">{d.name.clone()}</h2>
                        {(!d.description.is_empty()).then(|| view! { <p class="subtitle">{d.description.clone()}</p> })}
                    </div>
                    <div class="header-actions">
                        <select id="view-range" on:change=move |ev| set_view_range.set(Some(event_target_value(&ev)))>
                            {RANGES.iter().map(|(id, label, _)| {
                                let sel = current_range == *id;
                                view! { <option value=*id selected=sel>{*label}</option> }
                            }).collect_view()}
                        </select>
                        <button class="btn" id="refresh-dashboard" on:click=move |_| set_tick.update(|t| *t += 1)>"Refresh"</button>
                        <button class="btn" id="edit-dashboard" on:click=move |_| open_edit(d_edit.clone())>"Edit"</button>
                        <button class="btn btn-danger" id="delete-dashboard" on:click=move |_| delete(d_del.clone())>"Delete"</button>
                    </div>
                </div>
                {if d.panels.is_empty() {
                    view! { <p class="no-data">"No panels yet - Edit to add charts."</p> }.into_view()
                } else {
                    view! {
                        <div class="dashboard-grid">
                            {d.panels.iter().map(|p| view! {
                                <MetricPanel
                                    title=p.title.clone()
                                    query=p.query.clone()
                                    chart=p.chart.clone()
                                    range_secs=range
                                    tick=tick
                                    class=if p.width == 2 { "panel-full" } else { "panel-half" }
                                />
                            }).collect_view()}
                        </div>
                    }.into_view()
                }}
            </div>
        }
        .into_view()
    };

    view! {
        <div class="dashboards-page">
            <div class="page-header">
                <h1>"Dashboards"</h1>
                <div class="header-actions">
                    <button class="btn btn-primary" id="create-dashboard" on:click=open_new>
                        "Create Dashboard"
                    </button>
                </div>
            </div>

            {move || error.get().map(|e| view! { <div class="alert alert-error" id="dashboard-error">{e}</div> })}
            {move || notice.get().map(|n| view! { <div class="alert alert-success" id="dashboard-notice">{n}</div> })}

            <div class="dashboard-tabs" id="dashboard-tabs">
                {move || {
                    let sel = selected().map(|d| d.id);
                    dashboards.get().into_iter().map(|d| {
                        let active = sel.as_deref() == Some(d.id.as_str());
                        view! {
                            <a
                                class=if active { "tab active dashboard-tab" } else { "tab dashboard-tab" }
                                href=format!("/monitoring/dashboards?id={}", d.id)
                            >{d.name.clone()}</a>
                        }
                    }).collect_view()
                }}
            </div>

            {move || match editing.get() {
                Some(target) => editor(target),
                None => match selected() {
                    Some(d) => viewer(d),
                    None => view! {
                        <div class="no-data" id="no-dashboards">
                            {move || if loaded.get() {
                                "No dashboards yet. Create one to chart any metric Horcrux samples (CPU, memory, disk, load, VMs, containers, storage)."
                            } else {
                                "Loading dashboards..."
                            }}
                        </div>
                    }.into_view(),
                },
            }}
        </div>
    }
}
