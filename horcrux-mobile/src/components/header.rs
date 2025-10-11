//! Header component with back button

use yew::prelude::*;
use yew_router::prelude::*;

#[derive(Properties, PartialEq)]
pub struct HeaderProps {
    pub title: String,
    #[prop_or(false)]
    pub show_back: bool,
}

#[function_component(Header)]
pub fn header(props: &HeaderProps) -> Html {
    let navigator = use_navigator();

    let go_back = {
        let navigator = navigator.clone();
        Callback::from(move |_| {
            if let Some(nav) = &navigator {
                nav.back();
            }
        })
    };

    html! {
        <header class="mobile-header">
            {if props.show_back {
                html! {
                    <button class="back-button" onclick={go_back}>
                        {"←"}
                    </button>
                }
            } else {
                html! {}
            }}
            <h1 class="header-title">{&props.title}</h1>
            <div class="header-spacer"></div>
        </header>
    }
}
