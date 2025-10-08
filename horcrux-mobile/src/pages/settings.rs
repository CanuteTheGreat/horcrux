//! Settings page for mobile UI

use yew::prelude::*;
use yew_router::prelude::*;
use crate::api::ApiClient;
use crate::components::Header;
use crate::router::Route;

#[function_component(Settings)]
pub fn settings() -> Html {
    let navigator = use_navigator().unwrap();

    let logout = {
        let navigator = navigator.clone();
        Callback::from(move |_| {
            ApiClient::clear_token();
            navigator.push(&Route::Login);
        })
    };

    html! {
        <div class="settings-page">
            <Header title="Settings" />
            <div class="page-content">
                <div class="settings-section">
                    <h2>{"Account"}</h2>
                    <button class="mobile-button danger" onclick={logout}>
                        {"Logout"}
                    </button>
                </div>

                <div class="settings-section">
                    <h2>{"About"}</h2>
                    <p>{"Horcrux Mobile v0.1.0"}</p>
                    <p>{"Built with Rust + Yew"}</p>
                </div>
            </div>
        </div>
    }
}
