//! Login page for mobile UI

use yew::prelude::*;
use yew_router::prelude::*;
use web_sys::HtmlInputElement;
use wasm_bindgen_futures::spawn_local;

use crate::api::ApiClient;
use crate::router::Route;

#[function_component(Login)]
pub fn login() -> Html {
    let navigator = use_navigator().unwrap();
    let username = use_state(|| String::new());
    let password = use_state(|| String::new());
    let error = use_state(|| None::<String>);
    let loading = use_state(|| false);

    let on_username_change = {
        let username = username.clone();
        Callback::from(move |e: Event| {
            let input: HtmlInputElement = e.target_unchecked_into();
            username.set(input.value());
        })
    };

    let on_password_change = {
        let password = password.clone();
        Callback::from(move |e: Event| {
            let input: HtmlInputElement = e.target_unchecked_into();
            password.set(input.value());
        })
    };

    let on_submit = {
        let username = username.clone();
        let password = password.clone();
        let error = error.clone();
        let loading = loading.clone();
        let navigator = navigator.clone();

        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();

            let username = (*username).clone();
            let password = (*password).clone();
            let error = error.clone();
            let loading = loading.clone();
            let navigator = navigator.clone();

            loading.set(true);

            spawn_local(async move {
                match ApiClient::login(&username, &password).await {
                    Ok(response) => {
                        ApiClient::set_token(response.token);
                        navigator.push(&Route::Dashboard);
                    }
                    Err(e) => {
                        error.set(Some(format!("Login failed: {}", e)));
                        loading.set(false);
                    }
                }
            });
        })
    };

    html! {
        <div class="login-page">
            <div class="login-container">
                <div class="logo">
                    <h1>{"Horcrux"}</h1>
                    <p class="subtitle">{"Mobile"}</p>
                </div>

                <form class="login-form" onsubmit={on_submit}>
                    {if let Some(err) = (*error).as_ref() {
                        html! { <div class="error-message">{err}</div> }
                    } else {
                        html! {}
                    }}

                    <div class="form-group">
                        <input
                            type="text"
                            placeholder="Username"
                            value={(*username).clone()}
                            onchange={on_username_change}
                            disabled={*loading}
                            class="mobile-input"
                        />
                    </div>

                    <div class="form-group">
                        <input
                            type="password"
                            placeholder="Password"
                            value={(*password).clone()}
                            onchange={on_password_change}
                            disabled={*loading}
                            class="mobile-input"
                        />
                    </div>

                    <button
                        type="submit"
                        disabled={*loading}
                        class="mobile-button primary"
                    >
                        {if *loading { "Logging in..." } else { "Login" }}
                    </button>
                </form>
            </div>
        </div>
    }
}
