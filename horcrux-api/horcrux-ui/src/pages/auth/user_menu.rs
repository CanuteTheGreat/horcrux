//! Signed-in user badge for the navbar, and the redirect to /login when
//! there is no valid session.

use crate::api;
use leptos::*;
use leptos_router::*;

#[component]
pub fn UserMenu() -> impl IntoView {
    let (me, set_me) = create_signal(None::<api::Me>);
    let location = use_location();
    let navigate = use_navigate();

    // Re-check on every navigation: a missing/expired session goes to the
    // login page instead of leaving every page with "HTTP 401".
    create_effect(move |_| {
        let path = location.pathname.get();
        let navigate = navigate.clone();
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

    let logout = move |_| {
        spawn_local(async move {
            let _ = api::logout().await;
            if let Some(w) = web_sys::window() {
                let _ = w.location().set_href("/login");
            }
        });
    };

    view! {
        {move || me.get().map(|m| view! {
            <div class="navbar-user" id="navbar-user">
                <A href="/auth/mfa" class="navbar-item">{m.user.username.clone()}</A>
                <button class="btn btn-sm" id="logout-button" on:click=logout>"Log out"</button>
            </div>
        })}
    }
}
