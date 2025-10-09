//! Swipe action component for mobile lists

use yew::prelude::*;
use web_sys::TouchEvent;

#[derive(Properties, PartialEq)]
pub struct SwipeActionProps {
    #[prop_or_default]
    pub children: Children,
    #[prop_or_default]
    pub on_swipe_left: Option<Callback<()>>,
    #[prop_or_default]
    pub on_swipe_right: Option<Callback<()>>,
    #[prop_or_default]
    pub left_action_label: Option<String>,
    #[prop_or_default]
    pub right_action_label: Option<String>,
}

#[function_component(SwipeAction)]
pub fn swipe_action(props: &SwipeActionProps) -> Html {
    let swipe_offset = use_state(|| 0.0);
    let start_x = use_state(|| 0.0);
    let is_swiping = use_state(|| false);

    let on_touch_start = {
        let start_x = start_x.clone();
        let is_swiping = is_swiping.clone();

        Callback::from(move |e: TouchEvent| {
            if let Some(touch) = e.touches().get(0) {
                start_x.set(touch.client_x() as f64);
                is_swiping.set(true);
            }
        })
    };

    let on_touch_move = {
        let start_x = start_x.clone();
        let swipe_offset = swipe_offset.clone();
        let is_swiping = is_swiping.clone();

        Callback::from(move |e: TouchEvent| {
            if *is_swiping {
                if let Some(touch) = e.touches().get(0) {
                    let current_x = touch.client_x() as f64;
                    let offset = (current_x - *start_x).max(-100.0).min(100.0);
                    swipe_offset.set(offset);
                }
            }
        })
    };

    let on_touch_end = {
        let swipe_offset = swipe_offset.clone();
        let is_swiping = is_swiping.clone();
        let on_swipe_left = props.on_swipe_left.clone();
        let on_swipe_right = props.on_swipe_right.clone();

        Callback::from(move |_: TouchEvent| {
            is_swiping.set(false);

            if *swipe_offset < -50.0 {
                if let Some(ref callback) = on_swipe_left {
                    callback.emit(());
                }
            } else if *swipe_offset > 50.0 {
                if let Some(ref callback) = on_swipe_right {
                    callback.emit(());
                }
            }

            swipe_offset.set(0.0);
        })
    };

    let style = format!(
        "transform: translateX({}px); transition: transform 0.2s;",
        *swipe_offset
    );

    html! {
        <div class="swipe-action-container">
            {if *swipe_offset < -20.0 {
                html! {
                    <div class="swipe-action-left">
                        {props.left_action_label.as_ref().unwrap_or(&"Action".to_string())}
                    </div>
                }
            } else {
                html! {}
            }}

            {if *swipe_offset > 20.0 {
                html! {
                    <div class="swipe-action-right">
                        {props.right_action_label.as_ref().unwrap_or(&"Action".to_string())}
                    </div>
                }
            } else {
                html! {}
            }}

            <div
                class="swipe-content"
                style={style}
                ontouchstart={on_touch_start}
                ontouchmove={on_touch_move}
                ontouchend={on_touch_end}
            >
                {props.children.clone()}
            </div>
        </div>
    }
}
