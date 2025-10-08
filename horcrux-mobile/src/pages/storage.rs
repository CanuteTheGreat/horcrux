//! Storage page for mobile UI

use yew::prelude::*;
use crate::components::Header;

#[function_component(StorageView)]
pub fn storage_view() -> Html {
    html! {
        <div class="storage-page">
            <Header title="Storage" />
            <div class="page-content">
                <p>{"Storage management - Coming soon"}</p>
            </div>
        </div>
    }
}
