//! Horcrux Mobile UI
//!
//! Touch-optimized mobile interface built with Yew.
//! Provides a responsive, mobile-first experience for managing
//! virtualization infrastructure on phones and tablets.

mod api;
mod components;
mod pages;
mod router;

use yew::prelude::*;
use router::{switch, Route};
use yew_router::prelude::*;

/// Main mobile application component
#[function_component(App)]
pub fn app() -> Html {
    html! {
        <BrowserRouter>
            <div class="mobile-app">
                <Switch<Route> render={switch} />
                <BottomNav />
            </div>
        </BrowserRouter>
    }
}

/// Bottom navigation bar for mobile
#[function_component(BottomNav)]
fn bottom_nav() -> Html {
    let navigator = use_navigator().unwrap();

    let go_home = {
        let navigator = navigator.clone();
        Callback::from(move |_| navigator.push(&Route::Dashboard))
    };

    let go_vms = {
        let navigator = navigator.clone();
        Callback::from(move |_| navigator.push(&Route::VMs))
    };

    let go_cluster = {
        let navigator = navigator.clone();
        Callback::from(move |_| navigator.push(&Route::Cluster))
    };

    let go_settings = {
        let navigator = navigator.clone();
        Callback::from(move |_| navigator.push(&Route::Settings))
    };

    html! {
        <nav class="bottom-nav">
            <button class="nav-item" onclick={go_home}>
                <span class="icon">{"🏠"}</span>
                <span class="label">{"Home"}</span>
            </button>
            <button class="nav-item" onclick={go_vms}>
                <span class="icon">{"💻"}</span>
                <span class="label">{"VMs"}</span>
            </button>
            <button class="nav-item" onclick={go_cluster}>
                <span class="icon">{"🔗"}</span>
                <span class="label">{"Cluster"}</span>
            </button>
            <button class="nav-item" onclick={go_settings}>
                <span class="icon">{"⚙️"}</span>
                <span class="label">{"Settings"}</span>
            </button>
        </nav>
    }
}

/// Entry point for WASM
#[cfg(target_arch = "wasm32")]
pub fn run_app() {
    yew::Renderer::<App>::new().render();
}
