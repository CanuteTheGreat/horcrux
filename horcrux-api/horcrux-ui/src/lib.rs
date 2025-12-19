#![allow(unused_variables)]

use leptos::*;
use leptos_meta::*;
use leptos_router::*;

mod api;
mod components;
mod pages;
pub mod utils;
mod websocket;

use pages::{
    Dashboard, VmList, VmCreate, Alerts, Login, ContainerList, SnapshotList,
    CloneList, ReplicationList, Monitoring, GpuManagement, KubernetesManagement,
    StorageManagement, NetworkManagement, UsersPage, RolesPage, SessionsPage, ApiKeysPage,
    PodsPage, DeploymentsPage, ServicesPage, IngressesPage, ClusterDashboard,
    HelmRepositoriesPage, HelmChartsPage, HelmReleasesPage, ConfigMapsPage, SecretsPage,
    BackupDashboard, BackupJobsPage, RetentionPoliciesPage, SnapshotManagerPage, TemplateManagerPage,
    HaDashboard, ClusterManagementPage, HaGroupsPage, MigrationCenterPage,
    AlertCenterPage, DashboardsPage, MetricsExplorerPage, NotificationsPage, ObservabilityPage
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
                            <A href="/ha" class="dropdown-item">"High Availability"</A>
                        </div>
                    </div>
                    <div class="navbar-dropdown">
                        <span class="navbar-item dropdown-trigger">"Operations ▾"</span>
                        <div class="dropdown-content">
                            <A href="/backup" class="dropdown-item">"Backup & Protection"</A>
                            <A href="/snapshots" class="dropdown-item">"Snapshots"</A>
                            <A href="/clones" class="dropdown-item">"Clones"</A>
                            <A href="/replication" class="dropdown-item">"Replication"</A>
                        </div>
                    </div>
                    <div class="navbar-dropdown">
                        <span class="navbar-item dropdown-trigger">"Monitoring ▾"</span>
                        <div class="dropdown-content">
                            <A href="/monitoring" class="dropdown-item">"Overview"</A>
                            <A href="/monitoring/alerts" class="dropdown-item">"Alert Center"</A>
                            <A href="/monitoring/notifications" class="dropdown-item">"Notifications"</A>
                            <A href="/monitoring/dashboards" class="dropdown-item">"Dashboards"</A>
                            <A href="/monitoring/metrics" class="dropdown-item">"Metrics Explorer"</A>
                        </div>
                    </div>
                    <A href="/alerts" class="navbar-item">"System Alerts"</A>
                    <div class="navbar-dropdown">
                        <span class="navbar-item dropdown-trigger">"Administration ▾"</span>
                        <div class="dropdown-content">
                            <A href="/auth/users" class="dropdown-item">"Users"</A>
                            <A href="/auth/roles" class="dropdown-item">"Roles & Permissions"</A>
                            <A href="/auth/sessions" class="dropdown-item">"Active Sessions"</A>
                            <A href="/auth/api-keys" class="dropdown-item">"API Keys"</A>
                        </div>
                    </div>
                </div>
            </nav>

            <main class="container">
                <Routes>
                    <Route path="/" view=Dashboard/>
                    <Route path="/vms" view=VmList/>
                    <Route path="/vms/create" view=VmCreate/>
                    <Route path="/containers" view=ContainerList/>
                    <Route path="/kubernetes" view=KubernetesManagement/>
                    <Route path="/kubernetes/:cluster_id/dashboard" view=ClusterDashboard/>
                    <Route path="/kubernetes/:cluster_id/pods" view=PodsPage/>
                    <Route path="/kubernetes/:cluster_id/deployments" view=DeploymentsPage/>
                    <Route path="/kubernetes/:cluster_id/services" view=ServicesPage/>
                    <Route path="/kubernetes/:cluster_id/ingresses" view=IngressesPage/>
                    <Route path="/kubernetes/:cluster_id/:namespace/pods" view=PodsPage/>
                    <Route path="/kubernetes/:cluster_id/:namespace/deployments" view=DeploymentsPage/>
                    <Route path="/kubernetes/:cluster_id/:namespace/services" view=ServicesPage/>
                    <Route path="/kubernetes/:cluster_id/:namespace/ingresses" view=IngressesPage/>
                    <Route path="/kubernetes/:cluster_id/helm/repositories" view=HelmRepositoriesPage/>
                    <Route path="/kubernetes/:cluster_id/helm/charts" view=HelmChartsPage/>
                    <Route path="/kubernetes/:cluster_id/helm/releases" view=HelmReleasesPage/>
                    <Route path="/kubernetes/:cluster_id/config/configmaps" view=ConfigMapsPage/>
                    <Route path="/kubernetes/:cluster_id/config/secrets" view=SecretsPage/>
                    <Route path="/kubernetes/:cluster_id/:namespace/configmaps" view=ConfigMapsPage/>
                    <Route path="/kubernetes/:cluster_id/:namespace/secrets" view=SecretsPage/>
                    <Route path="/storage" view=StorageManagement/>
                    <Route path="/network" view=NetworkManagement/>
                    <Route path="/gpu" view=GpuManagement/>
                    <Route path="/snapshots" view=SnapshotList/>
                    <Route path="/clones" view=CloneList/>
                    <Route path="/replication" view=ReplicationList/>
                    <Route path="/monitoring" view=Monitoring/>
                    <Route path="/alerts" view=Alerts/>
                    <Route path="/login" view=Login/>
                    <Route path="/auth/users" view=UsersPage/>
                    <Route path="/auth/roles" view=RolesPage/>
                    <Route path="/auth/sessions" view=SessionsPage/>
                    <Route path="/auth/api-keys" view=ApiKeysPage/>
                    <Route path="/backup" view=BackupDashboard/>
                    <Route path="/backup/jobs" view=BackupJobsPage/>
                    <Route path="/backup/retention" view=RetentionPoliciesPage/>
                    <Route path="/backup/snapshots" view=SnapshotManagerPage/>
                    <Route path="/backup/templates" view=TemplateManagerPage/>
                    <Route path="/ha" view=HaDashboard/>
                    <Route path="/ha/cluster" view=ClusterManagementPage/>
                    <Route path="/ha/groups" view=HaGroupsPage/>
                    <Route path="/ha/migration" view=MigrationCenterPage/>
                    <Route path="/monitoring/alerts" view=AlertCenterPage/>
                    <Route path="/monitoring/notifications" view=NotificationsPage/>
                    <Route path="/monitoring/dashboards" view=DashboardsPage/>
                    <Route path="/monitoring/metrics" view=MetricsExplorerPage/>
                    <Route path="/monitoring/observability" view=ObservabilityPage/>
                </Routes>
            </main>

            <footer class="footer">
                <p>"Horcrux v0.1.1 - Built with Rust + Leptos"</p>
                <p>"50,000+ lines of memory-safe code"</p>
            </footer>
        </Router>
    }
}
