//! Router configuration for mobile UI

use yew::prelude::*;
use yew_router::prelude::*;

use crate::pages::*;

/// Application routes
#[derive(Clone, Routable, PartialEq)]
pub enum Route {
    #[at("/")]
    Dashboard,
    #[at("/login")]
    Login,
    #[at("/vms")]
    VMs,
    #[at("/vms/:id")]
    VMDetail { id: String },
    #[at("/cluster")]
    Cluster,
    #[at("/nodes/:id")]
    NodeDetail { id: String },
    #[at("/storage")]
    Storage,
    #[at("/network")]
    Network,
    #[at("/settings")]
    Settings,
    #[not_found]
    #[at("/404")]
    NotFound,
}

/// Switch function to render pages
pub fn switch(route: Route) -> Html {
    match route {
        Route::Dashboard => html! { <dashboard::Dashboard /> },
        Route::Login => html! { <login::Login /> },
        Route::VMs => html! { <vms::VMList /> },
        Route::VMDetail { id } => html! { <vms::VMDetail vm_id={id} /> },
        Route::Cluster => html! { <cluster::ClusterView /> },
        Route::NodeDetail { id } => html! { <cluster::NodeDetail node_id={id} /> },
        Route::Storage => html! { <storage::StorageView /> },
        Route::Network => html! { <network::NetworkView /> },
        Route::Settings => html! { <settings::Settings /> },
        Route::NotFound => html! { <h1>{"404 - Page Not Found"}</h1> },
    }
}
