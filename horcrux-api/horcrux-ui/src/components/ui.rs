//! UI system components (docs/design/ui-system.md): state tones, badges, usage bars and gauges,
//! page header, panels, empty states, wizard steps and the Appearance preferences.
//! Pages use these instead of their own colours or markup for the same job.

use leptos::*;

/// The tone a state word gets everywhere in the UI: `ok`, `warn`, `crit`, `info`, `busy` or `off`.
/// One mapping, so "running" is the same green in the tree, the grid and the dashboard.
pub fn state_tone(state: &str) -> &'static str {
    let s = state.trim().to_ascii_lowercase();
    match s.as_str() {
        "running" | "online" | "ok" | "healthy" | "active" | "up" | "enabled" | "success"
        | "succeeded" | "completed" | "done" | "available" | "connected" | "passed" | "valid"
        | "quorate" | "health_ok" | "ready" | "bound" => "ok",
        "stopped" | "shutoff" | "shut off" | "inactive" | "disabled" | "idle" | "none"
        | "created" | "exited" | "unknown" | "" => "off",
        "paused" | "suspended" | "degraded" | "warning" | "warn" | "pending" | "maintenance"
        | "saved" | "partial" | "stale" | "expiring" | "health_warn" => "warn",
        "failed" | "error" | "critical" | "crit" | "crashed" | "down" | "offline" | "dead"
        | "fenced" | "unreachable" | "health_err" | "invalid" | "expired" => "crit",
        "starting" | "stopping" | "migrating" | "creating" | "restoring" | "queued" | "syncing"
        | "saving" | "in_progress" | "in progress" => "busy",
        _ => "info",
    }
}

/// Tone for a task status: a running task is busy (not "ok": it has not finished).
pub fn task_tone(status: &str) -> &'static str {
    match status.trim().to_ascii_lowercase().as_str() {
        "running" => "busy",
        "ok" | "success" | "completed" | "done" => "ok",
        "failed" | "error" | "aborted" | "stopped" => "crit",
        _ => state_tone(status),
    }
}

/// Tone of a usage percentage: 75 % warns, 90 % is critical.
pub fn usage_tone(pct: f64) -> &'static str {
    if pct >= 90.0 {
        "crit"
    } else if pct >= 75.0 {
        "warn"
    } else {
        ""
    }
}

/// Byte count as GiB/TiB with one decimal ("16.7 GiB").
pub fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", b)
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

/// A state as a dot + word ("● running"); the dot's shape differs per tone, so colour is never the only cue.
#[component]
pub fn StateBadge(#[prop(into)] state: String) -> impl IntoView {
    let tone = state_tone(&state);
    view! {
        <span class=format!("state-badge {tone}")>
            <span class=format!("dot {tone}") aria-hidden="true"></span>
            {state}
        </span>
    }
}

/// Inline usage bar with its percentage as text.
#[component]
pub fn UsageBar(pct: f64, #[prop(optional, into)] text: Option<String>) -> impl IntoView {
    let p = pct.clamp(0.0, 100.0);
    let tone = usage_tone(p);
    let label = text.unwrap_or_else(|| format!("{:.0}%", p));
    view! {
        <span class=format!("usage {tone}") title=format!("{:.1}% used", p)>
            <span class="usage-track"><i style=format!("width: {:.1}%", p)></i></span>
            <span class="usage-text">{label}</span>
        </span>
    }
}

/// Ring gauge that links to the page behind the number. `value` is the text in the ring and is
/// also the element `.stat-value` (tests read it); `pct` drives the ring and the tone.
#[component]
pub fn Gauge(
    #[prop(into)] id: String,
    #[prop(into)] href: String,
    #[prop(into)] label: String,
    #[prop(into)] pct: Signal<Option<f64>>,
    #[prop(into)] value: Signal<View>,
    #[prop(into)] sub: Signal<String>,
) -> impl IntoView {
    view! {
        <a href=href id=id class=move || format!("gauge stat-link {}", pct.get().map(usage_tone).unwrap_or(""))>
            <span class="gauge-ring" style=move || format!("--p: {:.1}", pct.get().unwrap_or(0.0).clamp(0.0, 100.0))>
                <span class="stat-value">{move || value.get()}</span>
            </span>
            <span class="gauge-label">{label}</span>
            <span class="gauge-sub">{move || sub.get()}</span>
        </a>
    }
}

/// The page's title row: h1, an optional subtitle, and the page's primary actions on the right.
#[component]
pub fn PageHeader(
    #[prop(into)] title: String,
    #[prop(optional, into)] subtitle: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="page-header">
            <div>
                <h1>{title}</h1>
                {subtitle.map(|s| view! { <p class="page-subtitle">{s}</p> })}
            </div>
            {children.map(|c| view! { <div class="page-actions">{c()}</div> })}
        </div>
    }
}

/// A titled panel. `flush` drops the body padding (for a grid or list that fills the panel).
#[component]
pub fn Panel(
    #[prop(into)] title: String,
    #[prop(optional, into)] id: Option<String>,
    #[prop(optional)] flush: bool,
    #[prop(optional, into)] class: Option<String>,
    #[prop(optional)] actions: Option<View>,
    children: Children,
) -> impl IntoView {
    let cls = format!("panel {}", class.unwrap_or_default());
    view! {
        <section class=cls id=id>
            <div class="panel-head">
                <h2>{title}</h2>
                {actions.map(|a| view! { <div class="panel-actions">{a}</div> })}
            </div>
            <div class=if flush { "panel-body flush" } else { "panel-body" }>{children()}</div>
        </section>
    }
}

/// What is missing, why, and the action that fixes it.
#[component]
pub fn EmptyState(
    #[prop(into)] title: String,
    #[prop(optional, into)] hint: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="empty-state">
            <div class="empty-title">{title}</div>
            {hint.map(|h| view! { <p class="empty-todo">{h}</p> })}
            {children.map(|c| view! { <div class="empty-actions">{c()}</div> })}
        </div>
    }
}

/// Wizard steps: numbered, with done / current / to-do states.
#[component]
pub fn StepTabs(steps: Vec<&'static str>, #[prop(into)] current: Signal<usize>) -> impl IntoView {
    view! {
        <ol class="steps" aria-label="Steps">
            {steps.into_iter().enumerate().map(|(i, s)| view! {
                <li class=move || {
                    let c = current.get();
                    if i < c { "done" } else if i == c { "current" } else { "" }
                } aria-current=move || (current.get() == i).then_some("step")>{s}</li>
            }).collect_view()}
        </ol>
    }
}

// ---------- Appearance (theme / accent / density) ----------

/// The user's Appearance choices, stored in localStorage and applied as attributes on <html>
/// (index.html applies them again before first paint). Empty/default = follow the device.
pub const PREF_KEYS: [(&str, &str); 3] = [
    ("hx.theme", "data-theme"),
    ("hx.accent", "data-accent"),
    ("hx.density", "data-density"),
];

pub fn get_pref(key: &str) -> String {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(key).ok().flatten())
        .unwrap_or_default()
}

/// Store a preference and apply it now. `default_value` ("system", "verdigris", "normal") removes
/// the stored value and the attribute, so the CSS default (device scheme, default accent) applies.
pub fn set_pref(key: &str, value: &str, default_value: &str) {
    let attr = PREF_KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, a)| *a)
        .unwrap_or("data-theme");
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    let root = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element());
    if value == default_value || value.is_empty() {
        if let Some(s) = &storage {
            let _ = s.remove_item(key);
        }
        if let Some(r) = &root {
            let _ = r.remove_attribute(attr);
        }
    } else {
        if let Some(s) = &storage {
            let _ = s.set_item(key, value);
        }
        if let Some(r) = &root {
            let _ = r.set_attribute(attr, value);
        }
    }
}

/// A segmented control for one preference.
#[component]
pub fn PrefSegment(
    key: &'static str,
    default_value: &'static str,
    options: Vec<(&'static str, &'static str)>,
    #[prop(into)] label: String,
) -> impl IntoView {
    let initial = {
        let v = get_pref(key);
        if v.is_empty() {
            default_value.to_string()
        } else {
            v
        }
    };
    let (cur, set_cur) = create_signal(initial);
    view! {
        <div class="seg" role="group" aria-label=label>
            {options.into_iter().map(|(value, text)| view! {
                <button type="button" data-pref=key data-value=value
                    aria-pressed=move || (cur.get() == value).to_string()
                    on:click=move |_| { set_pref(key, value, default_value); set_cur.set(value.to_string()); }>
                    {text}
                </button>
            }).collect_view()}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_map_to_one_tone_each() {
        assert_eq!(state_tone("running"), "ok");
        assert_eq!(state_tone("Online"), "ok");
        assert_eq!(state_tone("stopped"), "off");
        assert_eq!(state_tone("paused"), "warn");
        assert_eq!(state_tone("failed"), "crit");
        assert_eq!(state_tone("offline"), "crit");
        assert_eq!(state_tone("migrating"), "busy");
        assert_eq!(task_tone("running"), "busy");
        assert_eq!(task_tone("OK"), "ok");
        assert_eq!(task_tone("failed"), "crit");
    }

    #[test]
    fn usage_thresholds() {
        assert_eq!(usage_tone(10.0), "");
        assert_eq!(usage_tone(75.0), "warn");
        assert_eq!(usage_tone(90.0), "crit");
    }

    #[test]
    fn bytes_are_binary_units() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(
            fmt_bytes(16 * 1024 * 1024 * 1024 + 700 * 1024 * 1024),
            "16.7 GiB"
        );
    }
}
