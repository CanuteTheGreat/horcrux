//! Empty state for a page whose backing service is absent on this node
//! (API answered 424 `NOT_AVAILABLE`): what is missing, what to do, and a way
//! forward (links + Retry). Not an error banner: nothing is broken.
use crate::api::Unavailable;
use leptos::*;

/// (label, href, external) links offered for a reason code.
fn links(reason: &str) -> Vec<(&'static str, &'static str, bool)> {
    match reason {
        "docker-missing" => vec![
            ("Docker on the Gentoo wiki", "https://wiki.gentoo.org/wiki/Docker", true),
            ("Use LXC containers instead", "/containers", false),
        ],
        "docker-not-running" => vec![("Open Node Services", "/system/services", false)],
        "podman-missing" => vec![
            ("Podman on the Gentoo wiki", "https://wiki.gentoo.org/wiki/Podman", true),
            ("Use LXC containers instead", "/containers", false),
        ],
        "no-log-source" => vec![("Syslog-ng on the Gentoo wiki", "https://wiki.gentoo.org/wiki/Syslog-ng", true)],
        "no-kernel-log" => vec![("Nftables on the Gentoo wiki", "https://wiki.gentoo.org/wiki/Nftables", true)],
        "no-init-system" => vec![("Open Node Health", "/system/health", false)],
        _ => vec![],
    }
}

#[component]
pub fn UnavailableState(
    info: Unavailable,
    /// Reload the page's data.
    #[prop(into)]
    on_retry: Callback<()>,
) -> impl IntoView {
    let reason = info.reason.clone();
    view! {
        <div class="empty-state unavailable-state" data-reason=info.reason.clone() role="status">
            <h2 class="empty-title">{info.what.clone()}</h2>
            {(!info.todo.is_empty()).then(|| view! { <p class="empty-todo">{info.todo.clone()}</p> })}
            <div class="empty-actions">
                {links(&reason).into_iter().map(|(label, href, external)| {
                    if external {
                        view! { <a class="btn btn-secondary" href=href target="_blank" rel="noopener noreferrer">{label}</a> }.into_view()
                    } else {
                        view! { <a class="btn btn-secondary" href=href>{label}</a> }.into_view()
                    }
                }).collect_view()}
                <button class="btn btn-primary" type="button" on:click=move |_| on_retry.call(())>"Check again"</button>
            </div>
        </div>
    }
}
