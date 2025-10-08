use leptos::*;
use leptos_meta::*;
use leptos_router::*;

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/horcrux-ui.css"/>
        <Title text="Horcrux - Gentoo Virtualization Platform"/>
        <Router>
            <main>
                <Routes>
                    <Route path="" view=Dashboard/>
                    <Route path="/vms" view=VirtualMachines/>
                    <Route path="/containers" view=Containers/>
                    <Route path="/storage" view=Storage/>
                    <Route path="/cluster" view=Cluster/>
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn Dashboard() -> impl IntoView {
    view! {
        <div class="dashboard">
            <h1>"Horcrux Dashboard"</h1>
            <div class="stats">
                <div class="stat-card">
                    <h3>"Virtual Machines"</h3>
                    <p class="stat-value">"0"</p>
                </div>
                <div class="stat-card">
                    <h3>"Containers"</h3>
                    <p class="stat-value">"0"</p>
                </div>
                <div class="stat-card">
                    <h3>"Storage"</h3>
                    <p class="stat-value">"0 GB"</p>
                </div>
                <div class="stat-card">
                    <h3>"Nodes"</h3>
                    <p class="stat-value">"1"</p>
                </div>
            </div>
            <nav class="quick-links">
                <A href="/vms">"Virtual Machines"</A>
                <A href="/containers">"Containers"</A>
                <A href="/storage">"Storage"</A>
                <A href="/cluster">"Cluster"</A>
            </nav>
        </div>
    }
}

#[component]
fn VirtualMachines() -> impl IntoView {
    view! {
        <div>
            <h1>"Virtual Machines"</h1>
            <p>"VM management coming soon..."</p>
            <A href="/">"Back to Dashboard"</A>
        </div>
    }
}

#[component]
fn Containers() -> impl IntoView {
    view! {
        <div>
            <h1>"Containers"</h1>
            <p>"Container management coming soon..."</p>
            <A href="/">"Back to Dashboard"</A>
        </div>
    }
}

#[component]
fn Storage() -> impl IntoView {
    view! {
        <div>
            <h1>"Storage"</h1>
            <p>"Storage management coming soon..."</p>
            <A href="/">"Back to Dashboard"</A>
        </div>
    }
}

#[component]
fn Cluster() -> impl IntoView {
    view! {
        <div>
            <h1>"Cluster"</h1>
            <p>"Cluster management coming soon..."</p>
            <A href="/">"Back to Dashboard"</A>
        </div>
    }
}
