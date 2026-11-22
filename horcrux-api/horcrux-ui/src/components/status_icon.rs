//! Status indicator: a real inline SVG (shape differs per state, so colour is never the only
//! signal) with an accessible name. Replaces the ASCII placeholders `[OK]`, `[X]`, `[!]`, `[D]`.
//! Colours come from the theme variables, so it follows light and dark.
use leptos::*;

/// Which state to draw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusKind {
    /// Check mark in a circle.
    Ok,
    /// Cross in a circle.
    Bad,
    /// Exclamation mark in a triangle.
    Warn,
    /// Disk (neutral, a count of drives).
    Disk,
    /// Open circle (inactive / not applicable).
    Idle,
}

impl StatusKind {
    fn class(self) -> &'static str {
        match self {
            StatusKind::Ok => "status-icon-ok",
            StatusKind::Bad => "status-icon-bad",
            StatusKind::Warn => "status-icon-warn",
            StatusKind::Disk => "status-icon-neutral",
            StatusKind::Idle => "status-icon-idle",
        }
    }

    /// Default accessible name.
    pub fn default_label(self) -> &'static str {
        match self {
            StatusKind::Ok => "OK",
            StatusKind::Bad => "Not OK",
            StatusKind::Warn => "Warning",
            StatusKind::Disk => "Disk",
            StatusKind::Idle => "Inactive",
        }
    }

    /// SVG shapes (24x24 viewBox, stroke = currentColor).
    fn shapes(self) -> &'static str {
        match self {
            StatusKind::Ok => "<circle cx=\"12\" cy=\"12\" r=\"9\"/><path d=\"M7.5 12.5l3 3 6-6.5\"/>",
            StatusKind::Bad => "<circle cx=\"12\" cy=\"12\" r=\"9\"/><path d=\"M8.5 8.5l7 7M15.5 8.5l-7 7\"/>",
            StatusKind::Warn => "<path d=\"M12 3.5l9.5 16.5h-19z\"/><path d=\"M12 10v4.5M12 17.2v.3\"/>",
            StatusKind::Disk => "<ellipse cx=\"12\" cy=\"6.5\" rx=\"8\" ry=\"3\"/><path d=\"M4 6.5v11c0 1.7 3.6 3 8 3s8-1.3 8-3v-11M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3\"/>",
            StatusKind::Idle => "<circle cx=\"12\" cy=\"12\" r=\"8\"/>",
        }
    }
}

/// `ok` -> Ok, else Bad.
pub fn ok_or_bad(ok: bool) -> StatusKind {
    if ok {
        StatusKind::Ok
    } else {
        StatusKind::Bad
    }
}

/// Inline SVG status icon with an accessible name (`role="img"` + `aria-label`).
#[component]
pub fn StatusIcon(
    kind: StatusKind,
    /// Accessible name; defaults to the kind's generic label.
    #[prop(optional, into)]
    label: Option<String>,
) -> impl IntoView {
    let label = label.unwrap_or_else(|| kind.default_label().to_string());
    view! {
        <svg
            class=format!("status-icon {}", kind.class())
            viewBox="0 0 24 24"
            width="18"
            height="18"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            role="img"
            aria-label=label.clone()
            inner_html=kind.shapes()
        >
        </svg>
        <span class="visually-hidden">{label}</span>
    }
}
