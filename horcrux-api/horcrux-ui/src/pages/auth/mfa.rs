//! Account security: second factors (authenticator app / TOTP, security
//! keys and passkeys / WebAuthn, one-time recovery codes), own password, and
//! for user administrators the second-factor policy and per-user status.

use crate::api::{self, MfaStatus, UserMfaStatus};
use leptos::*;
use leptos_router::*;

fn qr_svg(data: &str) -> String {
    qrcode::QrCode::new(data.as_bytes())
        .map(|c| {
            c.render::<qrcode::render::svg::Color>()
                .min_dimensions(200, 200)
                .build()
        })
        .unwrap_or_default()
}

#[component]
fn RecoveryCodes(codes: Vec<String>) -> impl IntoView {
    view! {
        <div class="alert alert-warning" id="recovery-codes">
            <strong>"Recovery codes - store them somewhere safe. Each works once."</strong>
            <pre class="recovery-codes">{codes.join("\n")}</pre>
        </div>
    }
}

#[component]
pub fn MfaPage() -> impl IntoView {
    let query = use_query_map();
    let setup_required =
        move || query.with(|q| q.get("setup").map(|s| s == "required").unwrap_or(false));

    let (status, set_status) = create_signal(None::<MfaStatus>);
    let (me, set_me) = create_signal(None::<api::Me>);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (codes, set_codes) = create_signal(Vec::<String>::new());

    // TOTP enrollment
    let (totp, set_totp) = create_signal(None::<api::TotpSetup>);
    let (totp_code, set_totp_code) = create_signal(String::new());
    let (factor_name, set_factor_name) = create_signal(String::new());

    // removal confirmation
    let (removing, set_removing) = create_signal(None::<String>);
    let (proof, set_proof) = create_signal(String::new());

    // password change
    let (old_pw, set_old_pw) = create_signal(String::new());
    let (new_pw, set_new_pw) = create_signal(String::new());

    let reload = move || {
        spawn_local(async move {
            match api::get_mfa_status().await {
                Ok(s) => set_status.set(Some(s)),
                Err(e) => set_error.set(Some(e.message)),
            }
            if let Ok(m) = api::get_me().await {
                set_me.set(Some(m));
            }
        });
    };
    reload();

    let start_totp = move |_| {
        set_error.set(None);
        spawn_local(async move {
            match api::initiate_totp_setup().await {
                Ok(t) => set_totp.set(Some(t)),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    let confirm_totp = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let Some(t) = totp.get() else { return };
        let name = factor_name.get();
        let name = if name.trim().is_empty() {
            "Authenticator app".to_string()
        } else {
            name
        };
        let code = totp_code.get();
        spawn_local(async move {
            match api::verify_totp_setup(t.setup_id.clone(), code, name).await {
                Ok(c) => {
                    set_totp.set(None);
                    set_totp_code.set(String::new());
                    set_codes.set(c);
                    set_notice.set(Some("Authenticator app added.".into()));
                    reload();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    let add_key = move |_| {
        set_error.set(None);
        if !crate::webauthn::available() {
            set_error.set(Some(
                "This browser has no WebAuthn support (it needs HTTPS)".into(),
            ));
            return;
        }
        let name = factor_name.get();
        let name = if name.trim().is_empty() {
            "Security key".to_string()
        } else {
            name
        };
        spawn_local(async move {
            match api::register_webauthn_credential(name).await {
                Ok(c) => {
                    set_codes.set(c);
                    set_notice.set(Some("Security key added.".into()));
                    reload();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    let regenerate = move |_| {
        spawn_local(async move {
            match api::regenerate_mfa_backup_codes().await {
                Ok(c) => {
                    set_codes.set(c);
                    reload();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    let confirm_remove = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let Some(id) = removing.get() else { return };
        let p = proof.get();
        spawn_local(async move {
            match api::disable_mfa(id, p).await {
                Ok(()) => {
                    set_removing.set(None);
                    set_proof.set(String::new());
                    set_notice.set(Some("Second factor removed.".into()));
                    reload();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    let change_password = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let (o, n) = (old_pw.get(), new_pw.get());
        spawn_local(async move {
            match api::change_own_password(o, n).await {
                Ok(()) => {
                    set_old_pw.set(String::new());
                    set_new_pw.set(String::new());
                    set_notice.set(Some("Password changed.".into()));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    view! {
        <div class="page mfa-page">
            <h1>"Account Security"</h1>
            {move || (setup_required() && status.get().map(|s| !s.enabled).unwrap_or(true)).then(|| view! {
                <div class="alert alert-warning" id="tfa-setup-required">
                    "Your account requires a second factor. Add an authenticator app or a security key to continue."
                </div>
            })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="mfa-error">{e}</div> })}
            {move || notice.get().map(|n| view! { <div class="alert alert-success" id="mfa-notice">{n}</div> })}
            {move || (!codes.get().is_empty()).then(|| view! { <RecoveryCodes codes=codes.get()/> })}

            <section class="card">
                <h2>"Second factors"</h2>
                {move || status.get().map(|s| view! {
                    <p id="mfa-summary">
                        {if s.enabled { "Two-factor authentication is on." } else { "Two-factor authentication is off." }}
                        {format!(" Recovery codes left: {}.", s.backup_codes_remaining)}
                    </p>
                    <table class="table" id="mfa-methods">
                        <thead><tr><th>"Type"</th><th>"Name"</th><th>"Added"</th><th>"Last used"</th><th></th></tr></thead>
                        <tbody>
                            {s.methods.into_iter().map(|m| {
                                let id = m.id.clone();
                                view! {
                                    <tr>
                                        <td>{if m.method_type == "totp" { "Authenticator app" } else { "Security key" }}</td>
                                        <td>{m.name.clone()}</td>
                                        <td>{m.registered_at.clone()}</td>
                                        <td>{m.last_used.clone().unwrap_or_else(|| "never".into())}</td>
                                        <td><button class="btn btn-danger btn-sm" on:click=move |_| set_removing.set(Some(id.clone()))>"Remove"</button></td>
                                    </tr>
                                }
                            }).collect_view()}
                        </tbody>
                    </table>
                })}
                {move || removing.get().map(|_| view! {
                    <form class="inline-form" on:submit=confirm_remove>
                        <label>"Confirm with a current code or your password"</label>
                        <input type="password" id="mfa-remove-proof" required
                            on:input=move |ev| set_proof.set(event_target_value(&ev)) prop:value=proof/>
                        <button type="submit" class="btn btn-danger">"Remove"</button>
                        <button type="button" class="btn" on:click=move |_| set_removing.set(None)>"Cancel"</button>
                    </form>
                })}

                <div class="form-group">
                    <label>"Name for the new factor"</label>
                    <input type="text" id="mfa-factor-name" placeholder="e.g. Phone, YubiKey"
                        on:input=move |ev| set_factor_name.set(event_target_value(&ev)) prop:value=factor_name/>
                </div>
                <div class="btn-row">
                    <button class="btn btn-primary" id="mfa-add-totp" on:click=start_totp>"Add authenticator app"</button>
                    <button class="btn btn-primary" id="mfa-add-webauthn" on:click=add_key>"Add security key / passkey"</button>
                    <button class="btn" id="mfa-regenerate-codes" on:click=regenerate
                        disabled=move || status.get().map(|s| !s.enabled).unwrap_or(true)>
                        "New recovery codes"
                    </button>
                </div>

                {move || totp.get().map(|t| view! {
                    <div class="totp-setup" id="totp-setup">
                        <p>"Scan with your authenticator app (or enter the key by hand), then type the 6-digit code it shows."</p>
                        <div class="qr" inner_html=qr_svg(&t.qr_code_url)></div>
                        <p>"Key: " <code id="totp-secret">{t.secret.clone()}</code></p>
                        <form on:submit=confirm_totp>
                            <input type="text" id="totp-code" required inputmode="numeric" autocomplete="one-time-code"
                                on:input=move |ev| set_totp_code.set(event_target_value(&ev)) prop:value=totp_code/>
                            <button type="submit" class="btn btn-primary" id="totp-confirm">"Confirm"</button>
                        </form>
                    </div>
                })}
            </section>

            {move || me.get().filter(|m| m.user.realm == "local").map(|_| view! {
                <section class="card">
                    <h2>"Password"</h2>
                    <form on:submit=change_password>
                        <div class="form-group">
                            <label>"Current password"</label>
                            <input type="password" id="pw-old" required autocomplete="current-password"
                                on:input=move |ev| set_old_pw.set(event_target_value(&ev)) prop:value=old_pw/>
                        </div>
                        <div class="form-group">
                            <label>"New password (at least 8 characters)"</label>
                            <input type="password" id="pw-new" required minlength="8" autocomplete="new-password"
                                on:input=move |ev| set_new_pw.set(event_target_value(&ev)) prop:value=new_pw/>
                        </div>
                        <button type="submit" class="btn btn-primary" id="pw-change">"Change password"</button>
                    </form>
                </section>
            })}

            {move || me.get().filter(|m| m.can("User.Modify")).map(|_| view! { <MfaAdmin/> })}
        </div>
    }
}

/// Second-factor policy and per-user status (needs User.Modify).
#[component]
fn MfaAdmin() -> impl IntoView {
    let (users, set_users) = create_signal(Vec::<UserMfaStatus>::new());
    let (policy, set_policy) = create_signal(None::<api::MfaEnforcementPolicy>);
    let (error, set_error) = create_signal(None::<String>);

    let reload = move || {
        spawn_local(async move {
            match api::get_users_mfa_status().await {
                Ok(u) => set_users.set(u),
                Err(e) => set_error.set(Some(e.message)),
            }
            if let Ok(p) = api::get_mfa_enforcement_policy().await {
                set_policy.set(Some(p));
            }
        });
    };
    reload();

    let toggle_global = move |_| {
        let Some(mut p) = policy.get() else { return };
        p.global_enforcement = !p.global_enforcement;
        spawn_local(async move {
            match api::update_mfa_enforcement_policy(p).await {
                Ok(()) => reload(),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    view! {
        <section class="card" id="mfa-admin">
            <h2>"Second factors - all users"</h2>
            {move || error.get().map(|e| view! { <div class="alert alert-error">{e}</div> })}
            {move || policy.get().map(|p| view! {
                <label class="checkbox">
                    <input type="checkbox" id="mfa-global-enforcement" prop:checked=p.global_enforcement on:change=toggle_global/>
                    " Require a second factor for every account (users without one must enroll at their next login)"
                </label>
            })}
            <table class="table" id="mfa-users">
                <thead><tr><th>"User"</th><th>"Factors"</th><th>"Status"</th><th>"Last used"</th><th></th></tr></thead>
                <tbody>
                    {move || users.get().into_iter().map(|u| {
                        let (id1, id2) = (u.user_id.clone(), u.user_id.clone());
                        view! {
                            <tr>
                                <td>{u.username.clone()}</td>
                                <td>{u.methods_count}</td>
                                <td>{u.enforcement_status.replace('_', " ")}</td>
                                <td>{u.last_mfa_activity.clone().unwrap_or_default()}</td>
                                <td>
                                    <button class="btn btn-sm" on:click=move |_| {
                                        let id = id1.clone();
                                        spawn_local(async move {
                                            match api::enforce_user_mfa(id).await {
                                                Ok(()) => reload(),
                                                Err(e) => set_error.set(Some(e.message)),
                                            }
                                        });
                                    }>"Require"</button>
                                    <button class="btn btn-sm btn-danger" on:click=move |_| {
                                        let id = id2.clone();
                                        spawn_local(async move {
                                            match api::reset_user_mfa(id).await {
                                                Ok(()) => reload(),
                                                Err(e) => set_error.set(Some(e.message)),
                                            }
                                        });
                                    }>"Reset"</button>
                                </td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
        </section>
    }
}
