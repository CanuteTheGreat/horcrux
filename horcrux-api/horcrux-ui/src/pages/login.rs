use crate::api;
use crate::api::{LoginOutcome, LoginRealms, LoginRequest, TfaChallenge};
use leptos::*;
use leptos_router::*;

/// Parse `#key=value&...` (the OpenID Connect callback hands the token over
/// in the URL fragment so it never reaches a server log).
fn fragment_params() -> Vec<(String, String)> {
    let hash = web_sys::window()
        .and_then(|w| w.location().hash().ok())
        .unwrap_or_default();
    hash.trim_start_matches('#')
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| {
            (
                k.to_string(),
                urlencoding::decode(v)
                    .map(|c| c.into_owned())
                    .unwrap_or_default(),
            )
        })
        .collect()
}

fn clear_fragment() {
    if let Some(w) = web_sys::window() {
        if let Ok(h) = w.history() {
            let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("/login"));
        }
    }
}

#[component]
pub fn Login() -> impl IntoView {
    let (username, set_username) = create_signal(String::new());
    let (password, set_password) = create_signal(String::new());
    let (realm, set_realm) = create_signal("local".to_string());
    let (realms, set_realms) = create_signal(LoginRealms::default());
    let (error, set_error) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    let (challenge, set_challenge) = create_signal(None::<TfaChallenge>);
    let (code, set_code) = create_signal(String::new());
    let (use_recovery, set_use_recovery) = create_signal(false);

    let navigate = use_navigate();

    // Logged in: full session, or straight to second-factor enrollment.
    let finish = {
        let navigate = navigate.clone();
        move |outcome: LoginOutcome| match outcome {
            LoginOutcome::LoggedIn(r) => {
                if r.tfa_setup_required {
                    navigate("/auth/mfa?setup=required", Default::default());
                } else {
                    navigate("/", Default::default());
                }
            }
            LoginOutcome::SecondFactor(ch) => {
                set_challenge.set(Some(ch));
                set_busy.set(false);
            }
        }
    };

    // OpenID Connect return (#ticket=... or #error=...).
    {
        let navigate = navigate.clone();
        let params = fragment_params();
        if let Some((_, t)) = params.iter().find(|(k, _)| k == "ticket") {
            api::set_auth_token(t);
            let to = params
                .iter()
                .find(|(k, _)| k == "redirect")
                .map(|(_, v)| v.clone())
                .filter(|r| r.starts_with('/') && !r.starts_with("//"))
                .unwrap_or_else(|| "/".into());
            clear_fragment();
            navigate(&to, Default::default());
        } else if let Some((_, e)) = params.iter().find(|(k, _)| k == "error") {
            set_error.set(Some(e.clone()));
            clear_fragment();
        }
    }
    create_effect(move |_| {
        spawn_local(async move {
            if let Ok(r) = api::get_login_realms().await {
                set_realms.set(r);
            }
        });
    });

    let selected_type = move || {
        realms
            .get()
            .realms
            .iter()
            .find(|r| r.name == realm.get())
            .map(|r| r.realm_type.clone())
            .unwrap_or_else(|| "local".into())
    };

    let submit = {
        let finish = finish.clone();
        move |ev: ev::SubmitEvent| {
            ev.prevent_default();
            set_error.set(None);
            if selected_type() == "openid" {
                // the provider's login page takes over from here
                if let Some(w) = web_sys::window() {
                    let _ = w
                        .location()
                        .set_href(&format!("/api/auth/oidc/{}/login?redirect=/", realm.get()));
                }
                return;
            }
            set_busy.set(true);
            let request = LoginRequest {
                username: username.get(),
                password: password.get(),
                realm: Some(realm.get()),
            };
            let finish = finish.clone();
            spawn_local(async move {
                match api::login(request).await {
                    Ok(o) => finish(o),
                    Err(e) => {
                        set_error.set(Some(e.message));
                        set_busy.set(false);
                    }
                }
            });
        }
    };

    let submit_code = {
        let finish = finish.clone();
        move |ev: ev::SubmitEvent| {
            ev.prevent_default();
            let Some(ch) = challenge.get() else { return };
            set_error.set(None);
            set_busy.set(true);
            let method = if use_recovery.get() {
                "recovery"
            } else {
                "totp"
            };
            let c = code.get();
            let finish = finish.clone();
            spawn_local(async move {
                match api::login_tfa(&ch.ticket, method, Some(c), None).await {
                    Ok(o) => finish(o),
                    Err(e) => {
                        set_error.set(Some(format!("Code not accepted ({})", e.message)));
                        set_busy.set(false);
                    }
                }
            });
        }
    };

    let use_key = {
        let finish = finish.clone();
        move |_| {
            let Some(ch) = challenge.get() else { return };
            let Some(opts) = ch.webauthn.clone() else {
                return;
            };
            set_error.set(None);
            set_busy.set(true);
            let finish = finish.clone();
            spawn_local(async move {
                let result = async {
                    let cred = crate::webauthn::get(&opts).await?;
                    api::login_tfa(&ch.ticket, "webauthn", None, Some(cred)).await
                }
                .await;
                match result {
                    Ok(o) => finish(o),
                    Err(e) => {
                        set_error.set(Some(e.message));
                        set_busy.set(false);
                    }
                }
            });
        }
    };

    let tfa_view = move || {
        let ch = challenge.get()?;
        let has_totp = ch.methods.iter().any(|m| m == "totp");
        let has_key = ch.methods.iter().any(|m| m == "webauthn");
        let has_recovery = ch.methods.iter().any(|m| m == "recovery");
        if !has_totp && has_recovery {
            set_use_recovery.set(true);
        }
        Some(view! {
            <div class="tfa-step" id="tfa-step">
                <h2>"Second factor"</h2>
                <p>{format!("Signing in as {}", ch.username)}</p>
                {has_key.then(|| view! {
                    <button class="btn btn-primary btn-block" id="tfa-webauthn"
                        on:click=use_key.clone() disabled=busy>
                        "Use security key / passkey"
                    </button>
                })}
                {(has_totp || has_recovery).then(|| view! {
                    <form on:submit=submit_code.clone()>
                        <div class="form-group">
                            <label>{move || if use_recovery.get() { "Recovery code" } else { "Authenticator app code" }}</label>
                            <input type="text" id="tfa-code" required autocomplete="one-time-code"
                                on:input=move |ev| set_code.set(event_target_value(&ev))
                                prop:value=code/>
                        </div>
                        <button type="submit" class="btn btn-primary btn-block" id="tfa-submit" disabled=busy>
                            "Verify"
                        </button>
                    </form>
                })}
                {(has_recovery && has_totp).then(|| view! {
                    <p>
                        <a href="#" id="tfa-use-recovery" on:click=move |ev| {
                            ev.prevent_default();
                            set_use_recovery.update(|v| *v = !*v);
                        }>
                            {move || if use_recovery.get() { "Use the authenticator app instead" } else { "Use a recovery code" }}
                        </a>
                    </p>
                })}
                <p>
                    <a href="#" on:click=move |ev| {
                        ev.prevent_default();
                        set_challenge.set(None);
                        set_code.set(String::new());
                    }>"Back"</a>
                </p>
            </div>
        })
    };

    let realm_options = move || {
        realms
            .get()
            .realms
            .into_iter()
            .map(|r| {
                let label = match r.comment.clone().filter(|c| !c.is_empty()) {
                    Some(c) => format!("{} - {}", r.name, c),
                    None => format!("{} ({})", r.name, r.realm_type),
                };
                let selected = r.name == realm.get_untracked();
                view! { <option value=r.name.clone() selected=selected>{label}</option> }
            })
            .collect_view()
    };

    view! {
        <div class="login-page">
            <div class="login-card">
                <h1>"Horcrux Login"</h1>
                <p class="tagline">"Gentoo Virtualization Platform"</p>

                {move || error.get().map(|msg| view! {
                    <div class="alert alert-error" id="login-error">{msg}</div>
                })}

                {tfa_view}

                <Show when=move || challenge.get().is_none() fallback=|| ()>
                    <form on:submit=submit.clone()>
                        <Show when=move || { realms.get().realms.len() > 1 } fallback=|| ()>
                            <div class="form-group">
                                <label>"Realm"</label>
                                <select id="login-realm" on:change=move |ev| set_realm.set(event_target_value(&ev))>
                                    {realm_options}
                                </select>
                            </div>
                        </Show>
                        <Show when=move || selected_type() != "openid" fallback=|| ()>
                            <div class="form-group">
                                <label>"Username"</label>
                                <input type="text" id="login-username" required placeholder="admin"
                                    autocomplete="username"
                                    on:input=move |ev| set_username.set(event_target_value(&ev))
                                    prop:value=username/>
                            </div>
                            <div class="form-group">
                                <label>"Password"</label>
                                <input type="password" id="login-password" required autocomplete="current-password"
                                    on:input=move |ev| set_password.set(event_target_value(&ev))
                                    prop:value=password/>
                            </div>
                        </Show>
                        <button type="submit" class="btn btn-primary btn-block" id="login-submit" disabled=busy>
                            {move || if busy.get() {
                                "Logging in...".to_string()
                            } else if selected_type() == "openid" {
                                format!("Continue with {}", realm.get())
                            } else {
                                "Login".to_string()
                            }}
                        </button>
                    </form>
                </Show>

                {move || realms.get().initial_password_file.map(|f| view! {
                    <p class="hint" id="initial-password-hint">
                        "First start: the generated password for user admin is in "
                        <code>{f}</code>
                        " on the server. Change it after logging in."
                    </p>
                })}
                <p class="version">"Version 0.1.0"</p>
            </div>
        </div>
    }
}
