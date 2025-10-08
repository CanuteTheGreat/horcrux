//! Card component for mobile UI

use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct CardProps {
    #[prop_or_default]
    pub title: Option<String>,
    #[prop_or_default]
    pub children: Children,
    #[prop_or_default]
    pub onclick: Option<Callback<()>>,
}

#[function_component(Card)]
pub fn card(props: &CardProps) -> Html {
    let onclick = props.onclick.clone();
    let handle_click = move |_| {
        if let Some(ref callback) = onclick {
            callback.emit(());
        }
    };

    let class = if props.onclick.is_some() {
        "card clickable"
    } else {
        "card"
    };

    html! {
        <div class={class} onclick={handle_click}>
            {if let Some(ref title) = props.title {
                html! { <div class="card-title">{title}</div> }
            } else {
                html! {}
            }}
            <div class="card-content">
                {props.children.clone()}
            </div>
        </div>
    }
}
