use leptos::*;
use leptos_meta::*;
use leptos_router::*;

mod api;
mod components;
mod pages;
mod websocket;

use pages::{
    Dashboard, VmList, VmCreate, Alerts, Login, ContainerList, SnapshotList,
    CloneList, ReplicationList, Monitoring, GpuManagement, KubernetesManagement,
    StorageManagement, NetworkManagement
};

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
                    <div class="navbar-dropdown">
                        <span class="navbar-item dropdown-trigger">"Compute ▾"</span>
                        <div class="dropdown-content">
                            <A href="/vms" class="dropdown-item">"Virtual Machines"</A>
                            <A href="/containers" class="dropdown-item">"Containers"</A>
                            <A href="/kubernetes" class="dropdown-item">"Kubernetes"</A>
                        </div>
                    </div>
                    <div class="navbar-dropdown">
                        <span class="navbar-item dropdown-trigger">"Infrastructure ▾"</span>
                        <div class="dropdown-content">
                            <A href="/storage" class="dropdown-item">"Storage"</A>
                            <A href="/network" class="dropdown-item">"Network"</A>
                            <A href="/gpu" class="dropdown-item">"GPU Passthrough"</A>
                        </div>
                    </div>
                    <div class="navbar-dropdown">
                        <span class="navbar-item dropdown-trigger">"Operations ▾"</span>
                        <div class="dropdown-content">
                            <A href="/snapshots" class="dropdown-item">"Snapshots"</A>
                            <A href="/clones" class="dropdown-item">"Clones"</A>
                            <A href="/replication" class="dropdown-item">"Replication"</A>
                        </div>
                    </div>
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
                    <Route path="/kubernetes" view=KubernetesManagement/>
                    <Route path="/storage" view=StorageManagement/>
                    <Route path="/network" view=NetworkManagement/>
                    <Route path="/gpu" view=GpuManagement/>
                    <Route path="/snapshots" view=SnapshotList/>
                    <Route path="/clones" view=CloneList/>
                    <Route path="/replication" view=ReplicationList/>
                    <Route path="/monitoring" view=Monitoring/>
                    <Route path="/alerts" view=Alerts/>
                    <Route path="/login" view=Login/>
                </Routes>
            </main>

            <footer class="footer">
                <p>"Horcrux v0.1.1 - Built with Rust + Leptos"</p>
                <p>"50,000+ lines of memory-safe code"</p>
            </footer>
        </Router>
    }
}
