//! Network page for mobile UI

use yew::prelude::*;
use crate::components::Header;

#[function_component(NetworkView)]
pub fn network_view() -> Html {
    html! {
        <div class="network-page">
            <Header title="Network" />
            <div class="page-content">
                <p>{"Network management - Coming soon"}</p>
            </div>
        </div>
    }
}
