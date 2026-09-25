use leptos::*;
use leptos_router::*;
use crate::api;
use crate::api::LoginRequest;

#[component]
pub fn Login() -> impl IntoView {
    let (username, set_username) = create_signal(String::new());
    let (password, set_password) = create_signal(String::new());
    let (error, set_error) = create_signal(None::<String>);
    let (logging_in, set_logging_in) = create_signal(false);

    let navigate = use_navigate();

    let submit = move |ev: ev::SubmitEvent| {
        ev.prevent_default();

        set_error.set(None);
        set_logging_in.set(true);

        let request = LoginRequest {
            username: username.get(),
            password: password.get(),
            realm: None,
        };

        let navigate = navigate.clone();
        spawn_local(async move {
            match api::login(request).await {
                Ok(_) => {
                    navigate("/", Default::default());
                }
                Err(e) => {
                    set_error.set(Some(e.message));
                    set_logging_in.set(false);
                }
            }
        });
    };

    view! {
        <div class="login-page">
            <div class="login-card">
                <h1>"Horcrux Login"</h1>
                <p class="tagline">"Gentoo Virtualization Platform"</p>

                {move || error.get().map(|msg| view! {
                    <div class="alert alert-error">{msg}</div>
                })}

                <form on:submit=submit>
                    <div class="form-group">
                        <label>"Username"</label>
                        <input
                            type="text"
                            required
                            placeholder="admin"
                            on:input=move |ev| set_username.set(event_target_value(&ev))
                            prop:value=username
                        />
                    </div>

                    <div class="form-group">
                        <label>"Password"</label>
                        <input
                            type="password"
                            required
                            on:input=move |ev| set_password.set(event_target_value(&ev))
                            prop:value=password
                        />
                    </div>

                    <button type="submit" class="btn btn-primary btn-block" disabled=logging_in>
                        {move || if logging_in.get() { "Logging in..." } else { "Login" }}
                    </button>
                </form>

                <p class="version">"Version 0.1.0"</p>
            </div>
        </div>
    }
}
