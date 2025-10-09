//! Pull-to-refresh component for mobile

use yew::prelude::*;
use web_sys::TouchEvent;
use wasm_bindgen::JsCast;

#[derive(Properties, PartialEq)]
pub struct PullToRefreshProps {
    #[prop_or_default]
    pub children: Children,
    pub on_refresh: Callback<()>,
}

#[function_component(PullToRefresh)]
pub fn pull_to_refresh(props: &PullToRefreshProps) -> Html {
    let pull_distance = use_state(|| 0.0);
    let is_pulling = use_state(|| false);
    let start_y = use_state(|| 0.0);

    let on_touch_start = {
        let start_y = start_y.clone();
        let is_pulling = is_pulling.clone();

        Callback::from(move |e: TouchEvent| {
            if let Some(touch) = e.touches().get(0) {
                start_y.set(touch.client_y() as f64);
                is_pulling.set(true);
            }
        })
    };

    let on_touch_move = {
        let start_y = start_y.clone();
        let pull_distance = pull_distance.clone();
        let is_pulling = is_pulling.clone();

        Callback::from(move |e: TouchEvent| {
            if *is_pulling {
                if let Some(touch) = e.touches().get(0) {
                    let current_y = touch.client_y() as f64;
                    let distance = (current_y - *start_y).max(0.0).min(150.0);
                    pull_distance.set(distance);
                }
            }
        })
    };

    let on_touch_end = {
        let pull_distance = pull_distance.clone();
        let is_pulling = is_pulling.clone();
        let on_refresh = props.on_refresh.clone();

        Callback::from(move |_: TouchEvent| {
            is_pulling.set(false);

            if *pull_distance > 80.0 {
                on_refresh.emit(());
            }

            pull_distance.set(0.0);
        })
    };

    let style = format!(
        "transform: translateY({}px); transition: transform 0.2s;",
        *pull_distance
    );

    html! {
        <div
            class="pull-to-refresh-container"
            ontouchstart={on_touch_start}
            ontouchmove={on_touch_move}
            ontouchend={on_touch_end}
        >
            {if *pull_distance > 0.0 {
                html! {
                    <div class="pull-indicator" style={format!("opacity: {}", (*pull_distance / 80.0).min(1.0))}>
                        {if *pull_distance > 80.0 { "Release to refresh" } else { "Pull to refresh" }}
                    </div>
                }
            } else {
                html! {}
            }}

            <div style={style}>
                {props.children.clone()}
            </div>
        </div>
    }
}
