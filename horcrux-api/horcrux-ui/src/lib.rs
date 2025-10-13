use leptos::*;
use leptos_meta::*;
use leptos_router::*;

mod api;
mod components;
mod pages;
mod websocket;

use pages::{Dashboard, VmList, VmCreate, Alerts, Login, ContainerList, SnapshotList, CloneList, ReplicationList, Monitoring};

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/horcrux-ui.css"/>
        <Title text="Horcrux - Virtualization Management"/>
        <Meta name="description" content="Horcrux Virtualization Management Platform"/>

        <Router>
            <nav class="navbar">
                <div class="navbar-brand">
                    <h1>"Horcrux"</h1>
                    <span class="tagline">"Gentoo Virtualization Platform"</span>
                </div>
                <div class="navbar-menu">
                    <A href="/" class="navbar-item">"Dashboard"</A>
                    <A href="/vms" class="navbar-item">"Virtual Machines"</A>
                    <A href="/containers" class="navbar-item">"Containers"</A>
                    <A href="/snapshots" class="navbar-item">"Snapshots"</A>
                    <A href="/clones" class="navbar-item">"Clones"</A>
                    <A href="/replication" class="navbar-item">"Replication"</A>
                    <A href="/monitoring" class="navbar-item">"Monitoring"</A>
                    <A href="/alerts" class="navbar-item">"Alerts"</A>
                </div>
            </nav>

            <main class="container">
                <Routes>
                    <Route path="/" view=Dashboard/>
                    <Route path="/vms" view=VmList/>
                    <Route path="/vms/create" view=VmCreate/>
                    <Route path="/containers" view=ContainerList/>
                    <Route path="/snapshots" view=SnapshotList/>
                    <Route path="/clones" view=CloneList/>
                    <Route path="/replication" view=ReplicationList/>
                    <Route path="/monitoring" view=Monitoring/>
                    <Route path="/alerts" view=Alerts/>
                    <Route path="/login" view=Login/>
                </Routes>
            </main>

            <footer class="footer">
                <p>"Horcrux v0.1.0 - Built with Rust + Leptos"</p>
                <p>"~13,400 lines of memory-safe code"</p>
            </footer>
        </Router>
    }
}
