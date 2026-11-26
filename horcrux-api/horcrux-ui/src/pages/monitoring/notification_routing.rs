//! Monitoring > Notifications > Routing: matcher list, tree editor with
//! drag and drop between groups, and a test panel. The model and the tree
//! operations live in `horcrux_common::notify` (unit tested natively).

use crate::api::*;
use horcrux_common::notify::{
    add_node, children_mut, move_node, node_at_mut, remove_node, Event, EventKind, MatchMode,
    MatchNode, Matcher, NodeTrace, Severity, TestResult, Weekday, DEFAULT_TIMEZONE,
};
use leptos::*;

fn csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// The browser's own time zone name, used as the default of new calendar
/// conditions (falls back to the documented node default).
fn browser_timezone() -> String {
    js_sys::Function::new_no_args(
        "try { return Intl.DateTimeFormat().resolvedOptions().timeZone || ''; } catch (e) { return ''; }",
    )
    .call0(&wasm_bindgen::JsValue::NULL)
    .ok()
    .and_then(|v| v.as_string())
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| DEFAULT_TIMEZONE.to_string())
}

fn new_leaf(kind: &str) -> MatchNode {
    match kind {
        "severity" => MatchNode::Severity {
            invert: false,
            levels: Vec::new(),
            min: Some(Severity::Warning),
        },
        "event_type" => MatchNode::EventType {
            invert: false,
            kinds: vec![EventKind::Alert],
            subtypes: Vec::new(),
            rule_ids: Vec::new(),
        },
        "node" => MatchNode::Node {
            invert: false,
            names: Vec::new(),
        },
        "guest" => MatchNode::Guest {
            invert: false,
            ids: Vec::new(),
            tags: Vec::new(),
        },
        "calendar" => MatchNode::Calendar {
            invert: false,
            weekdays: vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
            ],
            start: "09:00".into(),
            end: "17:00".into(),
            timezone: browser_timezone(),
        },
        "field" => MatchNode::Field {
            invert: false,
            field: "title".into(),
            regex: String::new(),
        },
        _ => MatchNode::empty_group(),
    }
}

fn upd(draft: RwSignal<Matcher>, path: &[usize], f: impl FnOnce(&mut MatchNode)) {
    draft.update(|d| {
        if let Some(n) = node_at_mut(&mut d.children, path) {
            f(n)
        }
    });
}

fn path_attr(path: &[usize]) -> String {
    path.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

fn sev_from(s: &str) -> Option<Severity> {
    Severity::ALL.iter().copied().find(|x| x.as_str() == s)
}

fn kind_from(s: &str) -> Option<EventKind> {
    EventKind::ALL.iter().copied().find(|x| x.as_str() == s)
}

fn kind_label(k: EventKind) -> &'static str {
    match k {
        EventKind::Alert => "Alert",
        EventKind::Backup => "Backup job result",
        EventKind::PackageUpdate => "Package updates",
        EventKind::Ha => "High availability",
        EventKind::Replication => "Replication",
        EventKind::CertificateExpiry => "Certificate expiry",
        EventKind::Security => "Login and security",
        EventKind::Test => "Test message",
    }
}

fn mode_from(s: &str) -> MatchMode {
    if s == "any" {
        MatchMode::Any
    } else {
        MatchMode::All
    }
}

type Drag = RwSignal<Option<Vec<usize>>>;

fn render_children(draft: RwSignal<Matcher>, drag: Drag, group: Vec<usize>) -> View {
    let g = group.clone();
    view! {
        <ul class="mt-list" data-group=path_attr(&group)>
            {move || {
                let nodes: Vec<MatchNode> = draft.with(|d| {
                    let mut c = d.children.clone();
                    children_mut(&mut c, &g).cloned().unwrap_or_default()
                });
                if nodes.is_empty() {
                    return view! { <li class="mt-empty">"No conditions here: everything matches. Add a condition or drop one here."</li> }.into_view();
                }
                nodes
                    .into_iter()
                    .enumerate()
                    .map(|(i, n)| {
                        let mut p = g.clone();
                        p.push(i);
                        render_node(draft, drag, p, n)
                    })
                    .collect_view()
            }}
        </ul>
    }
    .into_view()
}

fn over(e: &web_sys::DragEvent, on: bool) {
    use wasm_bindgen::JsCast;
    if let Some(el) = e
        .current_target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
    {
        let _ = if on {
            el.class_list().add_1("mt-over")
        } else {
            el.class_list().remove_1("mt-over")
        };
    }
}

fn invert_box(
    draft: RwSignal<Matcher>,
    path: Vec<usize>,
    label: &'static str,
    value: bool,
) -> View {
    view! {
        <label class="mt-check">
            <input type="checkbox" aria-label=label prop:checked=value on:change=move |e| {
                let v = event_target_checked(&e);
                upd(draft, &path, |n| match n {
                    MatchNode::Group { invert, .. }
                    | MatchNode::Severity { invert, .. }
                    | MatchNode::EventType { invert, .. }
                    | MatchNode::Node { invert, .. }
                    | MatchNode::Guest { invert, .. }
                    | MatchNode::Calendar { invert, .. }
                    | MatchNode::Field { invert, .. } => *invert = v,
                });
            }/>
            " NOT"
        </label>
    }
    .into_view()
}

fn add_menu(draft: RwSignal<Matcher>, group: Vec<usize>, label: &'static str) -> View {
    view! {
        <select aria-label=label on:change=move |e| {
            let k = event_target_value(&e);
            if !k.is_empty() {
                draft.update(|d| { let _ = add_node(&mut d.children, &group, new_leaf(&k)); });
            }
            event_target::<web_sys::HtmlSelectElement>(&e).set_value("");
        }>
            <option value="">"Add condition..."</option>
            <option value="severity">"Severity"</option>
            <option value="event_type">"Event type"</option>
            <option value="calendar">"Calendar (weekdays and time)"</option>
            <option value="node">"Node"</option>
            <option value="guest">"Guest id or tag"</option>
            <option value="field">"Field matches text pattern"</option>
        </select>
    }
    .into_view()
}

fn text_field(
    draft: RwSignal<Matcher>,
    path: Vec<usize>,
    label: &'static str,
    aria: &'static str,
    placeholder: &'static str,
    value: String,
    set: fn(&mut MatchNode, String),
) -> View {
    view! {
        <label>{label}" "
            <input type="text" aria-label=aria placeholder=placeholder prop:value=value on:change=move |e| {
                let v = event_target_value(&e);
                upd(draft, &path, |n| set(n, v));
            }/>
        </label>
    }
    .into_view()
}

fn set_list(n: &mut MatchNode, which: u8, v: String) {
    let l = csv(&v);
    match (n, which) {
        (MatchNode::EventType { subtypes, .. }, 0) => *subtypes = l,
        (MatchNode::EventType { rule_ids, .. }, 1) => *rule_ids = l,
        (MatchNode::Node { names, .. }, 2) => *names = l,
        (MatchNode::Guest { ids, .. }, 3) => *ids = l,
        (MatchNode::Guest { tags, .. }, 4) => *tags = l,
        _ => {}
    }
}

fn render_node(draft: RwSignal<Matcher>, drag: Drag, path: Vec<usize>, node: MatchNode) -> View {
    let parent: Vec<usize> = path[..path.len() - 1].to_vec();
    let index = *path.last().unwrap_or(&0);
    let is_group = matches!(node, MatchNode::Group { .. });

    let drop_into = move |to_group: Vec<usize>, at: usize| {
        if let Some(from) = drag.get_untracked() {
            draft.update(|d| {
                let _ = move_node(&mut d.children, &from, &to_group, at);
            });
        }
        drag.set(None);
    };

    let p_start = path.clone();
    let on_start = move |e: web_sys::DragEvent| {
        e.stop_propagation();
        drag.set(Some(p_start.clone()));
        if let Some(dt) = e.data_transfer() {
            let _ = dt.set_data("text/plain", "matcher-node");
            dt.set_effect_allowed("move");
        }
    };
    let on_end = move |_e: web_sys::DragEvent| drag.set(None);
    let on_over = move |e: web_sys::DragEvent| {
        e.prevent_default();
        e.stop_propagation();
        over(&e, true);
    };
    let on_leave = move |e: web_sys::DragEvent| over(&e, false);
    // a leaf takes the dragged node in front of itself, a group at its end
    let drop_target = if is_group {
        path.clone()
    } else {
        parent.clone()
    };
    let drop_at = if is_group { usize::MAX } else { index };
    let on_drop = move |e: web_sys::DragEvent| {
        e.prevent_default();
        e.stop_propagation();
        over(&e, false);
        drop_into(drop_target.clone(), drop_at);
    };

    let p_rm = path.clone();
    let remove = move |_| {
        draft.update(|d| {
            let _ = remove_node(&mut d.children, &p_rm);
        })
    };
    let rm_label = if is_group {
        "Remove group"
    } else {
        "Remove condition"
    };

    let body: View = match node.clone() {
        MatchNode::Group { mode, invert, .. } => {
            let p1 = path.clone();
            let p4 = path.clone();
            view! {
                <div class="mt-group-head">
                    <span class="mt-kind">"Group"</span>
                    <label>"Match "
                        <select aria-label="Group match mode" on:change=move |e| {
                            let m = mode_from(&event_target_value(&e));
                            upd(draft, &p1, |n| if let MatchNode::Group { mode, .. } = n { *mode = m });
                        }>
                            <option value="all" selected=mode == MatchMode::All>"all of (AND)"</option>
                            <option value="any" selected=mode == MatchMode::Any>"any of (OR)"</option>
                        </select>
                    </label>
                    {invert_box(draft, path.clone(), "Negate group (NOT)", invert)}
                    {add_menu(draft, path.clone(), "Add a condition to this group")}
                    <button type="button" class="btn btn-secondary btn-small" on:click=move |_| {
                        draft.update(|d| { let _ = add_node(&mut d.children, &p4, MatchNode::empty_group()); });
                    }>"Add group"</button>
                </div>
                <div class="mt-group-body">{render_children(draft, drag, path.clone())}</div>
            }
            .into_view()
        }
        MatchNode::Severity {
            invert,
            levels,
            min,
        } => {
            let p_min = path.clone();
            view! {
                <span class="mt-kind">"Severity"</span>
                <span class="mt-fields">
                    {invert_box(draft, path.clone(), "Negate severity condition", invert)}
                    <label>"At least "
                        <select aria-label="Minimum severity" on:change=move |e| {
                            let v = sev_from(&event_target_value(&e));
                            upd(draft, &p_min, |n| if let MatchNode::Severity { min, .. } = n { *min = v });
                        }>
                            <option value="" selected=min.is_none()>"any"</option>
                            {Severity::ALL.iter().map(|s| view! {
                                <option value=s.as_str() selected=min == Some(*s)>{s.as_str()}</option>
                            }).collect_view()}
                        </select>
                    </label>
                    <span class="mt-set" role="group" aria-label="Only these severities">
                        {Severity::ALL.iter().map(|s| {
                            let s = *s;
                            let p = path.clone();
                            let on = levels.contains(&s);
                            view! {
                                <label class="mt-check">
                                    <input type="checkbox" aria-label=format!("Severity {}", s.as_str()) prop:checked=on on:change=move |e| {
                                        let v = event_target_checked(&e);
                                        upd(draft, &p, |n| if let MatchNode::Severity { levels, .. } = n {
                                            levels.retain(|x| *x != s);
                                            if v { levels.push(s); }
                                        });
                                    }/>{s.as_str()}
                                </label>
                            }
                        }).collect_view()}
                    </span>
                </span>
            }
            .into_view()
        }
        MatchNode::EventType {
            invert,
            kinds,
            subtypes,
            rule_ids,
        } => view! {
            <span class="mt-kind">"Event type"</span>
            <span class="mt-fields">
                {invert_box(draft, path.clone(), "Negate event type condition", invert)}
                <span class="mt-set" role="group" aria-label="Event types">
                    {EventKind::ALL.iter().map(|k| {
                        let k = *k;
                        let p = path.clone();
                        let on = kinds.contains(&k);
                        view! {
                            <label class="mt-check">
                                <input type="checkbox" aria-label=format!("Event type {}", kind_label(k)) prop:checked=on on:change=move |e| {
                                    let v = event_target_checked(&e);
                                    upd(draft, &p, |n| if let MatchNode::EventType { kinds, .. } = n {
                                        kinds.retain(|x| *x != k);
                                        if v { kinds.push(k); }
                                    });
                                }/>{kind_label(k)}
                            </label>
                        }
                    }).collect_view()}
                </span>
                {text_field(draft, path.clone(), "Subtypes", "Event subtypes, comma separated", "backup_failed, login_failed", subtypes.join(", "), |n, v| set_list(n, 0, v))}
                {text_field(draft, path.clone(), "Alert rule ids", "Alert rule ids, comma separated", "rule id", rule_ids.join(", "), |n, v| set_list(n, 1, v))}
            </span>
        }
        .into_view(),
        MatchNode::Node { invert, names } => view! {
            <span class="mt-kind">"Node"</span>
            <span class="mt-fields">
                {invert_box(draft, path.clone(), "Negate node condition", invert)}
                {text_field(draft, path.clone(), "Node names", "Node names, comma separated", "", names.join(", "), |n, v| set_list(n, 2, v))}
            </span>
        }
        .into_view(),
        MatchNode::Guest { invert, ids, tags } => view! {
            <span class="mt-kind">"Guest"</span>
            <span class="mt-fields">
                {invert_box(draft, path.clone(), "Negate guest condition", invert)}
                {text_field(draft, path.clone(), "Guest ids", "Guest ids, comma separated", "", ids.join(", "), |n, v| set_list(n, 3, v))}
                {text_field(draft, path.clone(), "Guest tags", "Guest tags, comma separated", "", tags.join(", "), |n, v| set_list(n, 4, v))}
            </span>
        }
        .into_view(),
        MatchNode::Calendar {
            invert,
            weekdays,
            start,
            end,
            timezone,
        } => {
            let p_s = path.clone();
            let p_e = path.clone();
            let p_tz = path.clone();
            view! {
                <span class="mt-kind">"Calendar"</span>
                <span class="mt-fields">
                    {invert_box(draft, path.clone(), "Negate calendar condition", invert)}
                    <span class="mt-set" role="group" aria-label="Weekdays (none selected means every day)">
                        {Weekday::ALL.iter().map(|d| {
                            let d = *d;
                            let p = path.clone();
                            let on = weekdays.contains(&d);
                            view! {
                                <label class="mt-check mt-day">
                                    <input type="checkbox" aria-label=format!("Weekday {}", d.label()) prop:checked=on on:change=move |e| {
                                        let v = event_target_checked(&e);
                                        upd(draft, &p, |n| if let MatchNode::Calendar { weekdays, .. } = n {
                                            weekdays.retain(|x| *x != d);
                                            if v { weekdays.push(d); weekdays.sort(); }
                                        });
                                    }/>{d.label()}
                                </label>
                            }
                        }).collect_view()}
                    </span>
                    <label>"From "
                        <input type="time" aria-label="Start time" prop:value=start on:change=move |e| {
                            let v = event_target_value(&e);
                            upd(draft, &p_s, |n| if let MatchNode::Calendar { start, .. } = n { *start = v });
                        }/>
                    </label>
                    <label>"To "
                        <input type="time" aria-label="End time (earlier than the start crosses midnight)" prop:value=end on:change=move |e| {
                            let v = event_target_value(&e);
                            upd(draft, &p_e, |n| if let MatchNode::Calendar { end, .. } = n { *end = v });
                        }/>
                    </label>
                    <label>"Time zone "
                        <input type="text" list="mt-zones" aria-label="Time zone, for example Europe/Berlin" prop:value=timezone on:change=move |e| {
                            let v = event_target_value(&e);
                            upd(draft, &p_tz, |n| if let MatchNode::Calendar { timezone, .. } = n { *timezone = v });
                        }/>
                    </label>
                </span>
            }
            .into_view()
        }
        MatchNode::Field {
            invert,
            field,
            regex,
        } => {
            let p_f = path.clone();
            let p_r = path.clone();
            view! {
                <span class="mt-kind">"Field"</span>
                <span class="mt-fields">
                    {invert_box(draft, path.clone(), "Negate field condition", invert)}
                    <label>"Field "
                        <input type="text" list="mt-fields-list" aria-label="Event field to test" prop:value=field on:change=move |e| {
                            let v = event_target_value(&e);
                            upd(draft, &p_f, |n| if let MatchNode::Field { field, .. } = n { *field = v });
                        }/>
                    </label>
                    <label>"Matches pattern "
                        <input type="text" aria-label="Regular expression" prop:value=regex on:change=move |e| {
                            let v = event_target_value(&e);
                            upd(draft, &p_r, |n| if let MatchNode::Field { regex, .. } = n { *regex = v });
                        }/>
                    </label>
                </span>
            }
            .into_view()
        }
    };

    view! {
        <li class="mt-node" class:mt-group=is_group draggable="true" data-path=path_attr(&path)
            on:dragstart=on_start on:dragend=on_end on:dragover=on_over on:dragleave=on_leave on:drop=on_drop>
            <span class="mt-handle" title="Drag to move" aria-hidden="true">"Drag"</span>
            <div class="mt-body">{body}</div>
            <button type="button" class="btn btn-danger btn-small" aria-label=rm_label on:click=remove>"Remove"</button>
        </li>
    }
    .into_view()
}

fn render_trace(t: &NodeTrace) -> View {
    let cls = if t.matched { "mt-hit" } else { "mt-miss" };
    view! {
        <li>
            <span class=cls>{if t.matched { "match" } else { "no match" }}</span>
            " "<strong>{t.label.clone()}</strong>" - "{t.reason.clone()}
            {(!t.children.is_empty()).then(|| view! {
                <ul class="mt-trace">{t.children.iter().map(render_trace).collect_view()}</ul>
            })}
        </li>
    }
    .into_view()
}

#[component]
pub fn RoutingTab(channels: ReadSignal<Vec<NotificationChannel>>) -> impl IntoView {
    let matchers = create_rw_signal(Vec::<Matcher>::new());
    let loaded = create_rw_signal(false);
    let error = create_rw_signal(None::<String>);
    let notice = create_rw_signal(None::<String>);
    let draft = create_rw_signal(Matcher::default_matcher());
    let editing = create_rw_signal(false);
    let drag: Drag = create_rw_signal(None);
    let result = create_rw_signal(None::<TestResult>);
    let sent = create_rw_signal(None::<String>);

    // sample event for the test panel
    let t_kind = create_rw_signal("package_update".to_string());
    let t_sub = create_rw_signal(String::new());
    let t_sev = create_rw_signal("info".to_string());
    let t_node = create_rw_signal(String::new());
    let t_guest = create_rw_signal(String::new());
    let t_title = create_rw_signal("Sample event".to_string());
    let t_at = create_rw_signal(String::new());

    let query = leptos_router::use_query_map();
    let navigate = leptos_router::use_navigate();

    let reload = move || {
        spawn_local(async move {
            match fetch_json::<Vec<Matcher>>("/notifications/matchers").await {
                Ok(m) => {
                    matchers.set(m);
                    loaded.set(true);
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    reload();

    // /monitoring/notifications?tab=routing&matcher=<id|new> opens the editor
    create_effect(move |_| {
        let want = query.with(|q| q.get("matcher").cloned());
        let list = matchers.get();
        match want.as_deref() {
            Some("new") => {
                if !editing.get_untracked() || !draft.with_untracked(|d| d.id.is_empty()) {
                    draft.set(Matcher {
                        id: String::new(),
                        name: String::new(),
                        description: None,
                        all_channels: false,
                        children: Vec::new(),
                        ..Matcher::default_matcher()
                    });
                    editing.set(true);
                }
            }
            Some(id) => {
                if let Some(m) = list.iter().find(|m| m.id == id) {
                    if !editing.get_untracked() || draft.with_untracked(|d| d.id != id) {
                        draft.set(m.clone());
                        editing.set(true);
                    }
                }
            }
            None => editing.set(false),
        }
    });

    let nav_close = navigate.clone();
    let close = move || nav_close("/monitoring/notifications?tab=routing", Default::default());
    let close_cancel = close.clone();

    let save = move |_| {
        error.set(None);
        let m = draft.get_untracked();
        let close = close.clone();
        spawn_local(async move {
            let r = if m.id.is_empty() {
                post_json::<Matcher, _>("/notifications/matchers", &m)
                    .await
                    .map(|_| ())
            } else {
                put_json::<Matcher, _>(&format!("/notifications/matchers/{}", m.id), &m)
                    .await
                    .map(|_| ())
            };
            match r {
                Ok(()) => {
                    notice.set(Some(format!("Saved matcher '{}'", m.name)));
                    reload();
                    close();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };

    let event_body = move || {
        let sev = sev_from(&t_sev.get_untracked()).unwrap_or(Severity::Info);
        let kind = kind_from(&t_kind.get_untracked()).unwrap_or(EventKind::Test);
        let opt = |s: String| (!s.trim().is_empty()).then(|| s.trim().to_string());
        let now =
            chrono::DateTime::from_timestamp_millis(js_sys::Date::now() as i64).unwrap_or_default();
        Event {
            kind,
            subtype: t_sub.get_untracked().trim().to_string(),
            severity: sev,
            title: t_title.get_untracked(),
            body: String::new(),
            node: opt(t_node.get_untracked()),
            guest_id: opt(t_guest.get_untracked()),
            guest_tags: Vec::new(),
            rule_id: None,
            fields: Default::default(),
            timestamp: now,
        }
    };
    let at_iso = move || {
        let v = t_at.get_untracked();
        (!v.trim().is_empty())
            .then(|| js_sys::Date::new(&wasm_bindgen::JsValue::from_str(&v)))
            .and_then(|d| d.to_iso_string().as_string())
    };

    let run_test = move |use_draft: bool| {
        error.set(None);
        sent.set(None);
        let mut body = serde_json::json!({ "event": event_body() });
        if let Some(at) = at_iso() {
            body["at"] = serde_json::Value::String(at);
        }
        if use_draft {
            body["matcher"] = serde_json::to_value(draft.get_untracked()).unwrap_or_default();
        }
        spawn_local(async move {
            match post_json::<TestResult, _>("/notifications/matchers/test", &body).await {
                Ok(r) => result.set(Some(r)),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let send_real = move |_| {
        error.set(None);
        let mut ev = serde_json::to_value(event_body()).unwrap_or_default();
        if let Some(at) = at_iso() {
            ev["timestamp"] = serde_json::Value::String(at);
        }
        spawn_local(async move {
            match post_json::<serde_json::Value, _>("/notifications/events", &ev).await {
                Ok(r) => sent.set(Some(format!(
                    "Sent to {} channel(s): {}; {} failed",
                    r["delivered"], r["channels"], r["failed"]
                ))),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };

    let delete = move |id: String, name: String| {
        spawn_local(async move {
            match delete_json(&format!("/notifications/matchers/{id}")).await {
                Ok(()) => {
                    notice.set(Some(format!("Deleted matcher '{name}'")));
                    reload();
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    let move_by = move |id: String, delta: i32| {
        let mut ids: Vec<String> = matchers.get_untracked().into_iter().map(|m| m.id).collect();
        if let Some(i) = ids.iter().position(|x| *x == id) {
            let j = i as i32 + delta;
            if j >= 0 && (j as usize) < ids.len() {
                ids.swap(i, j as usize);
                spawn_local(async move {
                    let _ = put_json::<serde_json::Value, _>(
                        "/notifications/matchers/order",
                        &serde_json::json!({ "ids": ids }),
                    )
                    .await;
                    reload();
                });
            }
        }
    };

    let channel_name = move |id: &str| {
        channels
            .get()
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| id.to_string())
    };

    let editor = move || {
        if !editing.get() {
            return view! {}.into_view();
        }
        let close_cancel = close_cancel.clone();
        let save = save.clone();
        view! {
            <div class="mt-editor" id="matcher-editor">
                <h2>{move || if draft.with(|d| d.id.is_empty()) { "New matcher" } else { "Edit matcher" }}</h2>
                <div class="form-group">
                    <label for="matcher-name">"Name"</label>
                    <input id="matcher-name" type="text" prop:value=move || draft.with(|d| d.name.clone())
                        on:change=move |e| { let v = event_target_value(&e); draft.update(|d| d.name = v); }/>
                </div>
                <div class="form-group">
                    <label for="matcher-description">"Description"</label>
                    <input id="matcher-description" type="text" prop:value=move || draft.with(|d| d.description.clone().unwrap_or_default())
                        on:change=move |e| { let v = event_target_value(&e); draft.update(|d| d.description = (!v.is_empty()).then_some(v)); }/>
                </div>
                <div class="form-group">
                    <label class="mt-check">
                        <input type="checkbox" id="matcher-enabled" prop:checked=move || draft.with(|d| d.enabled)
                            on:change=move |e| { let v = event_target_checked(&e); draft.update(|d| d.enabled = v); }/>
                        " Enabled"
                    </label>
                </div>
                <h3>"When"</h3>
                <div class="mt-root-head">
                    <label>"The event must match "
                        <select id="matcher-mode" aria-label="Top level match mode" on:change=move |e| {
                            let m = mode_from(&event_target_value(&e));
                            draft.update(|d| d.mode = m);
                        }>
                            <option value="all" selected=move || draft.with(|d| d.mode) == MatchMode::All>"all of (AND)"</option>
                            <option value="any" selected=move || draft.with(|d| d.mode) == MatchMode::Any>"any of (OR)"</option>
                        </select>
                        " these conditions"
                    </label>
                    <label class="mt-check">
                        <input type="checkbox" id="matcher-invert" aria-label="Negate the whole matcher (NOT)" prop:checked=move || draft.with(|d| d.invert)
                            on:change=move |e| { let v = event_target_checked(&e); draft.update(|d| d.invert = v); }/>
                        " NOT"
                    </label>
                    {add_menu(draft, Vec::new(), "Add a condition at the top level")}
                    <button type="button" id="matcher-add-group" class="btn btn-secondary btn-small" on:click=move |_| {
                        draft.update(|d| d.children.push(MatchNode::empty_group()));
                    }>"Add group"</button>
                </div>
                <div id="matcher-tree"
                    on:dragover=move |e: web_sys::DragEvent| { e.prevent_default(); }
                    on:drop=move |e: web_sys::DragEvent| {
                        e.prevent_default();
                        if let Some(from) = drag.get_untracked() {
                            draft.update(|d| { let _ = move_node(&mut d.children, &from, &[], usize::MAX); });
                        }
                        drag.set(None);
                    }>
                    {render_children(draft, drag, Vec::new())}
                </div>
                <datalist id="mt-fields-list">
                    <option value="title"/><option value="body"/><option value="node"/><option value="guest_id"/>
                    <option value="rule_id"/><option value="subtype"/><option value="kind"/><option value="severity"/>
                </datalist>
                <datalist id="mt-zones">
                    <option value="UTC"/><option value="Europe/Berlin"/><option value="Europe/London"/>
                    <option value="America/New_York"/><option value="America/Chicago"/><option value="America/Denver"/>
                    <option value="America/Los_Angeles"/><option value="Asia/Tokyo"/><option value="Australia/Sydney"/>
                </datalist>
                <h3>"Send to"</h3>
                <div class="form-group">
                    <label class="mt-check">
                        <input type="checkbox" id="matcher-all-channels" prop:checked=move || draft.with(|d| d.all_channels)
                            on:change=move |e| { let v = event_target_checked(&e); draft.update(|d| d.all_channels = v); }/>
                        " Every enabled channel, including ones added later"
                    </label>
                </div>
                <div class="mt-targets" role="group" aria-label="Notification targets">
                    {move || {
                        let list = channels.get();
                        if list.is_empty() {
                            return view! { <p class="mt-hint">"No notification channels yet. Create one on the Channels tab, then pick it here."</p> }.into_view();
                        }
                        list.into_iter().map(|c| {
                            let id = c.id.clone();
                            let id2 = c.id.clone();
                            view! {
                                <label class="mt-check">
                                    <input type="checkbox" aria-label=format!("Target {}", c.name)
                                        prop:checked=move || draft.with(|d| d.targets.contains(&id))
                                        on:change=move |e| {
                                            let v = event_target_checked(&e);
                                            let id = id2.clone();
                                            draft.update(|d| { d.targets.retain(|t| *t != id); if v { d.targets.push(id); } });
                                        }/>
                                    {c.name.clone()}
                                </label>
                            }
                        }).collect_view()
                    }}
                </div>
                <div class="mt-actions">
                    <button type="button" id="matcher-save" class="btn btn-primary" on:click=save>"Save matcher"</button>
                    <button type="button" id="matcher-test-draft" class="btn btn-secondary" on:click=move |_| run_test(true)>"Test this matcher with the sample event"</button>
                    <button type="button" id="matcher-cancel" class="btn btn-secondary" on:click=move |_| close_cancel()>"Cancel"</button>
                </div>
            </div>
        }
        .into_view()
    };

    view! {
        <div class="routing-section" id="routing-section">
            <p class="mt-hint">
                "A matcher decides which channels get an event. Every event is checked against every enabled matcher and goes to the targets of all that match. "
                "Groups combine conditions with all-of (AND) or any-of (OR) and can be negated (NOT); drag conditions and groups between groups to rearrange them."
            </p>
            {move || notice.get().map(|m| view! { <div class="alert alert-success" id="routing-notice">{m}</div> })}
            {move || error.get().map(|m| view! { <div class="alert alert-error" id="routing-error">{m}</div> })}
            <div class="header-actions">
                <a class="btn btn-primary" id="matcher-new" href="/monitoring/notifications?tab=routing&matcher=new">"New matcher"</a>
            </div>
            {move || if !loaded.get() {
                view! { <div class="loading">"Loading matchers..."</div> }.into_view()
            } else if matchers.get().is_empty() {
                view! { <div class="empty-state">"No matchers: no alert, backup or other event reaches any channel. Create a matcher to route events."</div> }.into_view()
            } else {
                let list = matchers.get();
                view! {
                    <table class="table" id="matcher-table">
                        <thead><tr>
                            <th>"Name"</th><th>"Enabled"</th><th>"Conditions"</th><th>"Sends to"</th><th>"Actions"</th>
                        </tr></thead>
                        <tbody>
                        {list.into_iter().map(|m| {
                            let id = m.id.clone();
                            let id_up = m.id.clone();
                            let id_dn = m.id.clone();
                            let id_del = m.id.clone();
                            let name_del = m.name.clone();
                            let targets = if m.all_channels {
                                "every enabled channel".to_string()
                            } else if m.targets.is_empty() {
                                "nothing (no targets)".to_string()
                            } else {
                                m.targets.iter().map(|t| channel_name(t)).collect::<Vec<_>>().join(", ")
                            };
                            view! {
                                <tr data-matcher=m.name.clone()>
                                    <td><a href=format!("/monitoring/notifications?tab=routing&matcher={id}")>{m.name.clone()}</a>
                                        {m.description.clone().map(|d| view! { <div class="mt-hint">{d}</div> })}</td>
                                    <td>{if m.enabled { "yes" } else { "no" }}</td>
                                    <td>{m.children.len()}</td>
                                    <td>{targets}</td>
                                    <td>
                                        <button type="button" class="btn btn-secondary btn-small" aria-label=format!("Move {} up", m.name) on:click=move |_| move_by(id_up.clone(), -1)>"Up"</button>
                                        <button type="button" class="btn btn-secondary btn-small" aria-label=format!("Move {} down", m.name) on:click=move |_| move_by(id_dn.clone(), 1)>"Down"</button>
                                        <button type="button" class="btn btn-danger btn-small" aria-label=format!("Delete matcher {}", m.name) on:click=move |_| delete(id_del.clone(), name_del.clone())>"Delete"</button>
                                    </td>
                                </tr>
                            }
                        }).collect_view()}
                        </tbody>
                    </table>
                }.into_view()
            }}

            {editor}

            <div class="mt-test" id="matcher-test-panel">
                <h2>"Test routing"</h2>
                <p class="mt-hint">"Describe a sample event and see which matchers and channels it hits, with the reason for every condition. Testing sends nothing."</p>
                <div class="mt-test-form">
                    <label>"Event type "
                        <select id="test-kind" on:change=move |e| t_kind.set(event_target_value(&e))>
                            {EventKind::ALL.iter().map(|k| view! {
                                <option value=k.as_str() selected=move || t_kind.get() == k.as_str()>{kind_label(*k)}</option>
                            }).collect_view()}
                        </select>
                    </label>
                    <label>"Subtype "
                        <input id="test-subtype" type="text" placeholder="backup_failed" prop:value=move || t_sub.get() on:change=move |e| t_sub.set(event_target_value(&e))/>
                    </label>
                    <label>"Severity "
                        <select id="test-severity" on:change=move |e| t_sev.set(event_target_value(&e))>
                            {Severity::ALL.iter().map(|s| view! {
                                <option value=s.as_str() selected=move || t_sev.get() == s.as_str()>{s.as_str()}</option>
                            }).collect_view()}
                        </select>
                    </label>
                    <label>"Node "
                        <input id="test-node" type="text" prop:value=move || t_node.get() on:change=move |e| t_node.set(event_target_value(&e))/>
                    </label>
                    <label>"Guest id "
                        <input id="test-guest" type="text" prop:value=move || t_guest.get() on:change=move |e| t_guest.set(event_target_value(&e))/>
                    </label>
                    <label>"Title "
                        <input id="test-title" type="text" prop:value=move || t_title.get() on:change=move |e| t_title.set(event_target_value(&e))/>
                    </label>
                    <label>"Event time (your browser's time zone, empty = now) "
                        <input id="test-at" type="datetime-local" prop:value=move || t_at.get() on:change=move |e| t_at.set(event_target_value(&e))/>
                    </label>
                </div>
                <div class="mt-actions">
                    <button type="button" id="test-run" class="btn btn-secondary" on:click=move |_| run_test(false)>"Test saved matchers"</button>
                    <button type="button" id="test-send" class="btn btn-secondary" on:click=send_real>"Send this event for real"</button>
                </div>
                {move || sent.get().map(|m| view! { <div class="alert alert-success" id="test-sent">{m}</div> })}
                {move || result.get().map(|r| view! {
                    <div id="test-result">
                        <p><strong>"Goes to: "</strong>
                            {if r.targets.is_empty() { "no channel".to_string() } else { r.targets.iter().map(|t| t.name.clone()).collect::<Vec<_>>().join(", ") }}</p>
                        <ul class="mt-trace">
                            {r.matchers.iter().map(|m| view! {
                                <li data-verdict=m.name.clone()>
                                    <span class=if m.matched { "mt-hit" } else { "mt-miss" }>{if m.matched { "match" } else { "no match" }}</span>
                                    " "<strong>{m.name.clone()}</strong>
                                    {m.matched.then(|| format!(" -> {}", m.targets.iter().map(|t| t.name.clone()).collect::<Vec<_>>().join(", ")))}
                                    <ul class="mt-trace">{m.trace.children.iter().map(render_trace).collect_view()}</ul>
                                    <div class="mt-hint">{m.trace.reason.clone()}</div>
                                </li>
                            }).collect_view()}
                        </ul>
                    </div>
                })}
            </div>
        </div>
    }
}
