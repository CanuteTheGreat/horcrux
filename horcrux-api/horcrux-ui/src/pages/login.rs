use leptos::*;

#[component]
pub fn Login() -> impl IntoView {
    let (username, set_username) = create_signal(String::new());
    let (password, set_password) = create_signal(String::new());

    let submit = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        // TODO: Implement authentication
        logging::log!("Login: {}", username.get());
    };

    view! {
        <div class="login-page">
            <div class="login-card">
                <h1>"Horcrux Login"</h1>
                <p class="tagline">"Gentoo Virtualization Platform"</p>

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

                    <button type="submit" class="btn btn-primary btn-block">
                        "Login"
                    </button>
                </form>

                <p class="version">"Version 0.1.0"</p>
            </div>
        </div>
    }
}
