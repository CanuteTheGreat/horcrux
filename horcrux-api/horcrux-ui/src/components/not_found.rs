//! The page for an address no route matches (an old bookmark, a mistyped or
//! removed page). It used to render an empty page with no way on.
use leptos::*;
use leptos_router::*;

#[component]
pub fn NotFound() -> impl IntoView {
    let path = use_location().pathname;
    // per path, so the next page gets its own title again
    create_effect(move |_| {
        path.get();
        crate::titles::set_subject("Page not found");
    });
    view! {
        <div class="not-found-page" id="page-not-found">
            <div class="page-header"><h1>"There is no such page"</h1></div>
            <p class="form-help">
                "Nothing lives at " <code>{move || path.get()}</code>
                ". The link may be out of date, or this page is from a newer or older Horcrux version than the one on this node."
            </p>
            <div class="form-row">
                <A href="/" class="btn btn-primary">"Go to the dashboard"</A>
                <A href="/vms" class="btn">"Virtual machines"</A>
                <A href="/containers" class="btn">"Containers"</A>
                <A href="/resources" class="btn">"Search resources"</A>
            </div>
        </div>
    }
}
