//! Status badge component

use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct StatusBadgeProps {
    pub status: String,
}

#[function_component(StatusBadge)]
pub fn status_badge(props: &StatusBadgeProps) -> Html {
    let class = match props.status.to_lowercase().as_str() {
        "running" => "status-badge status-running",
        "stopped" => "status-badge status-stopped",
        "paused" => "status-badge status-paused",
        "error" => "status-badge status-error",
        _ => "status-badge status-unknown",
    };

    html! {
        <span class={class}>{&props.status}</span>
    }
}
