//! Signed-in user menu for the header (account, Appearance, log out), and the redirect to /login
//! when there is no valid session.

use crate::api;
use crate::components::ui::PrefSegment;
use leptos::*;
use leptos_router::*;

#[component]
pub fn UserMenu() -> impl IntoView {
    let (me, set_me) = create_signal(None::<api::Me>);
    let location = use_location();
    let navigate = use_navigate();
    let menu = create_node_ref::<html::Details>();

    // Re-check on every navigation: a missing/expired session goes to the
    // login page instead of leaving every page with "HTTP 401".
    create_effect(move |_| {
        let path = location.pathname.get();
        let navigate = navigate.clone();
        if let Some(m) = menu.get_untracked() {
            let _ = m.remove_attribute("open");
        }
        if path == "/login" {
            set_me.set(None);
            return;
        }
        spawn_local(async move {
            match api::get_me().await {
                Ok(m) => {
                    if m.scope.as_deref() == Some("tfa_setup") && path != "/auth/mfa" {
                        navigate("/auth/mfa?setup=required", Default::default());
                    }
                    set_me.set(Some(m));
                }
                Err(_) => {
                    set_me.set(None);
                    navigate("/login", Default::default());
                }
            }
        });
    });
    // Escape or a click elsewhere closes the menu.
    let _esc = window_event_listener(ev::keydown, move |e| {
        if e.key() == "Escape" {
            if let Some(m) = menu.get_untracked() {
                let _ = m.remove_attribute("open");
            }
        }
    });
    let _outside = window_event_listener(ev::click, move |e| {
        let inside = e
            .target()
            .and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok())
            .and_then(|el| el.closest("#user-menu").ok().flatten())
            .is_some();
        if !inside {
            if let Some(m) = menu.get_untracked() {
                let _ = m.remove_attribute("open");
            }
        }
    });

    let logout = move |_| {
        spawn_local(async move {
            let _ = api::logout().await;
            if let Some(w) = web_sys::window() {
                let _ = w.location().set_href("/login");
            }
        });
    };

    view! {
        {move || me.get().map(|m| {
            let name = m.user.username.clone();
            let initial = name.chars().next().map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_default();
            let role = if m.is_admin { "Administrator".to_string() } else { m.user.role.clone() };
            view! {
                <div class="navbar-user" id="navbar-user">
                    <details class="user-menu" id="user-menu" node_ref=menu>
                        <summary id="user-menu-toggle" aria-label=format!("User menu for {name}")>
                            <span class="avatar" aria-hidden="true">{initial}</span>
                            <span class="user-name">{name.clone()}</span>
                            <span class="caret" aria-hidden="true"></span>
                        </summary>
                        <div class="menu" role="menu">
                            <div class="menu-head">
                                <strong>{name.clone()}</strong>
                                <small>{format!("{} \u{b7} realm {}", role, m.user.realm)}</small>
                            </div>
                            <A href="/auth/mfa">"Account security (2FA, passkeys)"</A>
                            <A href="/auth/sessions">"Active sessions"</A>
                            <A href="/auth/api-keys">"API keys"</A>
                            <div class="menu-section">"Theme"</div>
                            <PrefSegment key="hx.theme" default_value="system" label="Theme"
                                options=vec![("system", "System"), ("light", "Light"), ("dark", "Dark")]/>
                            <div class="menu-section">"Accent"</div>
                            <PrefSegment key="hx.accent" default_value="verdigris" label="Accent colour"
                                options=vec![("verdigris", "Verdigris"), ("cobalt", "Cobalt"), ("garnet", "Garnet"), ("graphite", "Graphite")]/>
                            <div class="menu-section">"Density"</div>
                            <PrefSegment key="hx.density" default_value="normal" label="Density"
                                options=vec![("compact", "Compact"), ("normal", "Normal"), ("comfortable", "Roomy")]/>
                            <div style="border-top: 1px solid var(--c-border); margin-top: 6px; padding-top: 4px">
                                <button class="menu-item" id="logout-button" type="button" on:click=logout>"Log out"</button>
                            </div>
                        </div>
                    </details>
                </div>
            }
        })}
    }
}
