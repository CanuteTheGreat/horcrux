//! Loading spinner component

use yew::prelude::*;

#[function_component(Loading)]
pub fn loading() -> Html {
    html! {
        <div class="loading-container">
            <div class="spinner"></div>
            <p>{"Loading..."}</p>
        </div>
    }
}
