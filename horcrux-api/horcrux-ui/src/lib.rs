#![allow(unused_variables)]

use leptos::*;
use leptos_meta::*;
use leptos_router::*;

use features::{provide_features, Gated, IfFeature};
use pages::nas::{
    DirectoryPage as NasDirectoryPage, GroupsPage as NasGroupsPage, IscsiPage as NasIscsiPage,
    NasDashboard, NasUsersPage, PoolsPage as NasPoolsPage, S3Page as NasS3Page,
    SchedulerPage as NasSchedulerPage, ServicesPage as NasServicesPage,
    SharesPage as NasSharesPage,
};

pub mod api;
mod components;
pub mod features;
pub mod pages;
pub mod utils;
mod webauthn;
mod websocket;

use pages::{
    AlertCenterPage, Alerts, ApiKeysPage, BackupDashboard, BackupJobsPage, BackupVerifyPage,
    CertificatesPage, CloneList, ClusterDashboard, ClusterManagementPage, ConfigMapsPage,
    ContainerCreate, ContainerEventsPage, ContainerExecPage, ContainerList, ContainerLogsPage,
    ContainerNetworksPage, Dashboard, DashboardsPage, DeploymentsPage, DiskManagementPage,
    GpuManagement, HaDashboard, HaFailoverPage, HaGroupsPage, HelmChartsPage, HelmReleasesPage,
    HelmRepositoriesPage, ImagesPage, IngressesPage, KubernetesManagement, Login, LxcContainerPage,
    LxcTemplatesPage, MetricsExplorerPage, MfaPage, MigrationCenterPage, Monitoring,
    NetworkManagement, NotificationsPage, ObservabilityPage, PermissionsPage, PodsPage, RealmsPage,
    ReplicationList, RetentionPoliciesPage, RolesPage, SecretsPage, ServicesPage, SessionsPage,
    SmartMonitoringPage, SnapshotList, SnapshotManagerPage, StacksPage, StorageManagement,
    TemplateManagerPage, UserMenu, UsersPage, VmConsole, VmCreate, VmList, VolumesPage,
};

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    // Which USE-flag features the server was built with: menu entries and
    // pages of the others are hidden (see features.rs).
    let features = provide_features();

    view! {
        <Title text="Horcrux - Virtualization Management"/>
        <Meta name="description" content="Horcrux Virtualization Management Platform"/>

        <Router>
            <nav class="navbar" data-features-loaded=move || features.loaded().to_string()>
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
                            <IfFeature feature="docker|podman">
                                <A href="/containers/images" class="dropdown-item">"Images"</A>
                                <A href="/containers/volumes" class="dropdown-item">"Volumes"</A>
                                <A href="/containers/networks" class="dropdown-item">"Container Networks"</A>
                                <A href="/containers/stacks" class="dropdown-item">"Stacks (Compose)"</A>
                                <A href="/containers/events" class="dropdown-item">"Container Events"</A>
                            </IfFeature>
                            <IfFeature feature="kubernetes">
                                <A href="/kubernetes" class="dropdown-item">"Kubernetes"</A>
                            </IfFeature>
                        </div>
                    </div>
                    <div class="navbar-dropdown">
                        <span class="navbar-item dropdown-trigger">"Infrastructure ▾"</span>
                        <div class="dropdown-content">
                            <A href="/storage" class="dropdown-item">"Storage"</A>
                            <A href="/storage/disks" class="dropdown-item">"Disks"</A>
                            <A href="/storage/smart" class="dropdown-item">"SMART Health"</A>
                            <A href="/storage/isos" class="dropdown-item">"ISO Library"</A>
                            <A href="/network" class="dropdown-item">"Network"</A>
                            <A href="/network/bridges" class="dropdown-item">"Host Bridges"</A>
                            <A href="/gpu" class="dropdown-item">"GPU Passthrough"</A>
                            <IfFeature feature="cluster">
                                <A href="/ha" class="dropdown-item">"High Availability"</A>
                            </IfFeature>
                        </div>
                    </div>
                    <IfFeature feature="nas">
                        <div class="navbar-dropdown" id="nav-nas">
                            <span class="navbar-item dropdown-trigger">"NAS ▾"</span>
                            <div class="dropdown-content">
                                <A href="/nas" class="dropdown-item">"NAS Overview"</A>
                                <A href="/nas/shares" class="dropdown-item">"Shares"</A>
                                <A href="/nas/users" class="dropdown-item">"Users"</A>
                                <A href="/nas/groups" class="dropdown-item">"Groups"</A>
                                <A href="/nas/pools" class="dropdown-item">"Pools"</A>
                                <A href="/nas/services" class="dropdown-item">"Services"</A>
                                <A href="/nas/scheduler" class="dropdown-item">"Scheduler"</A>
                                <IfFeature feature="ldap|ldap-server|kerberos|ad">
                                    <A href="/nas/directory" class="dropdown-item">"Directory Services"</A>
                                </IfFeature>
                                <IfFeature feature="iscsi-target">
                                    <A href="/nas/iscsi" class="dropdown-item">"iSCSI Targets"</A>
                                </IfFeature>
                                <IfFeature feature="s3-gateway">
                                    <A href="/nas/s3" class="dropdown-item">"S3 Gateway"</A>
                                </IfFeature>
                            </div>
                        </div>
                    </IfFeature>
                    <div class="navbar-dropdown">
                        <span class="navbar-item dropdown-trigger">"Operations ▾"</span>
                        <div class="dropdown-content">
                            <IfFeature feature="backup">
                                <A href="/backup" class="dropdown-item">"Backup & Protection"</A>
                            </IfFeature>
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
                            <A href="/auth/roles" class="dropdown-item">"Roles"</A>
                            <A href="/auth/permissions" class="dropdown-item">"Permissions & Groups"</A>
                            <A href="/auth/realms" class="dropdown-item">"Authentication Realms"</A>
                            <A href="/auth/mfa" class="dropdown-item">"Account Security (2FA)"</A>
                            <A href="/system/certificates" class="dropdown-item">"Certificates"</A>
                            <A href="/auth/sessions" class="dropdown-item">"Active Sessions"</A>
                            <A href="/auth/api-keys" class="dropdown-item">"API Keys"</A>
                        </div>
                    </div>
                </div>
                <UserMenu/>
            </nav>

            <main class="container">
                <Routes>
                    <Route path="/" view=Dashboard/>
                    <Route path="/vms" view=VmList/>
                    <Route path="/vms/create" view=VmCreate/>
                    <Route path="/vms/:id/console" view=VmConsole/>
                    <Route path="/vms/:id/hardware" view=crate::pages::VmHardwarePage/>
                    <Route path="/storage/isos" view=crate::pages::IsoLibraryPage/>
                    <Route path="/network/bridges" view=crate::pages::HostBridgesPage/>
                    <Route path="/containers" view=ContainerList/>
                    <Route path="/containers/create" view=ContainerCreate/>
                    <Route path="/containers/images" view=|| view! { <Gated feature="docker|podman"><ImagesPage/></Gated> }/>
                    <Route path="/containers/volumes" view=|| view! { <Gated feature="docker|podman"><VolumesPage/></Gated> }/>
                    <Route path="/containers/networks" view=|| view! { <Gated feature="docker|podman"><ContainerNetworksPage/></Gated> }/>
                    <Route path="/containers/stacks" view=|| view! { <Gated feature="docker|podman"><StacksPage/></Gated> }/>
                    <Route path="/containers/events" view=|| view! { <Gated feature="docker|podman"><ContainerEventsPage/></Gated> }/>
                    <Route path="/containers/:id/logs" view=|| view! { <Gated feature="docker|podman|lxc|lxd|incus"><ContainerLogsPage/></Gated> }/>
                    <Route path="/containers/:id/firewall" view=|| view! { <Gated feature="docker|podman|lxc|lxd|incus"><crate::pages::ContainerFirewallPage/></Gated> }/>
                    <Route path="/containers/:id/exec" view=|| view! { <Gated feature="docker|podman|lxc|lxd|incus"><ContainerExecPage/></Gated> }/>
                    <Route path="/containers/:id/console" view=|| view! { <Gated feature="lxc|lxd|incus"><crate::pages::ContainerSystemConsolePage/></Gated> }/>
                    <Route path="/containers/lxc/templates" view=|| view! { <Gated feature="lxc"><LxcTemplatesPage/></Gated> }/>
                    <Route path="/containers/:id/lxc" view=|| view! { <Gated feature="lxc"><LxcContainerPage/></Gated> }/>
                    <Route path="/kubernetes" view=|| view! { <Gated feature="kubernetes"><KubernetesManagement/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/dashboard" view=|| view! { <Gated feature="kubernetes"><ClusterDashboard/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/pods" view=|| view! { <Gated feature="kubernetes"><PodsPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/deployments" view=|| view! { <Gated feature="kubernetes"><DeploymentsPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/services" view=|| view! { <Gated feature="kubernetes"><ServicesPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/ingresses" view=|| view! { <Gated feature="kubernetes"><IngressesPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/:namespace/pods" view=|| view! { <Gated feature="kubernetes"><PodsPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/:namespace/deployments" view=|| view! { <Gated feature="kubernetes"><DeploymentsPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/:namespace/services" view=|| view! { <Gated feature="kubernetes"><ServicesPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/:namespace/ingresses" view=|| view! { <Gated feature="kubernetes"><IngressesPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/helm/repositories" view=|| view! { <Gated feature="kubernetes"><HelmRepositoriesPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/helm/charts" view=|| view! { <Gated feature="kubernetes"><HelmChartsPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/helm/releases" view=|| view! { <Gated feature="kubernetes"><HelmReleasesPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/config/configmaps" view=|| view! { <Gated feature="kubernetes"><ConfigMapsPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/config/secrets" view=|| view! { <Gated feature="kubernetes"><SecretsPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/:namespace/configmaps" view=|| view! { <Gated feature="kubernetes"><ConfigMapsPage/></Gated> }/>
                    <Route path="/kubernetes/:cluster_id/:namespace/secrets" view=|| view! { <Gated feature="kubernetes"><SecretsPage/></Gated> }/>
                    <Route path="/storage" view=StorageManagement/>
                    <Route path="/storage/disks" view=DiskManagementPage/>
                    <Route path="/storage/smart" view=SmartMonitoringPage/>
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
                    <Route path="/auth/mfa" view=MfaPage/>
                    <Route path="/auth/permissions" view=PermissionsPage/>
                    <Route path="/auth/realms" view=RealmsPage/>
                    <Route path="/system/certificates" view=CertificatesPage/>
                    <Route path="/backup" view=|| view! { <Gated feature="backup"><BackupDashboard/></Gated> }/>
                    <Route path="/backup/jobs" view=|| view! { <Gated feature="backup"><BackupJobsPage/></Gated> }/>
                    <Route path="/backup/retention" view=|| view! { <Gated feature="backup"><RetentionPoliciesPage/></Gated> }/>
                    <Route path="/backup/verify" view=|| view! { <Gated feature="backup"><BackupVerifyPage/></Gated> }/>
                    <Route path="/backup/snapshots" view=SnapshotManagerPage/>
                    <Route path="/backup/templates" view=TemplateManagerPage/>
                    <Route path="/ha" view=|| view! { <Gated feature="cluster"><HaDashboard/></Gated> }/>
                    <Route path="/ha/cluster" view=|| view! { <Gated feature="cluster"><ClusterManagementPage/></Gated> }/>
                    <Route path="/ha/failover" view=|| view! { <Gated feature="cluster"><HaFailoverPage/></Gated> }/>
                    <Route path="/ha/groups" view=|| view! { <Gated feature="cluster"><HaGroupsPage/></Gated> }/>
                    <Route path="/ha/migration" view=|| view! { <Gated feature="cluster"><MigrationCenterPage/></Gated> }/>
                    <Route path="/monitoring/alerts" view=AlertCenterPage/>
                    <Route path="/monitoring/notifications" view=NotificationsPage/>
                    <Route path="/monitoring/dashboards" view=DashboardsPage/>
                    <Route path="/monitoring/metrics" view=MetricsExplorerPage/>
                    <Route path="/monitoring/observability" view=ObservabilityPage/>
                    // NAS (USE=nas and its protocol/service flags)
                    <Route path="/nas" view=|| view! { <Gated feature="nas"><NasDashboard/></Gated> }/>
                    <Route path="/nas/shares" view=|| view! { <Gated feature="nas"><NasSharesPage/></Gated> }/>
                    <Route path="/nas/users" view=|| view! { <Gated feature="nas"><NasUsersPage/></Gated> }/>
                    <Route path="/nas/groups" view=|| view! { <Gated feature="nas"><NasGroupsPage/></Gated> }/>
                    <Route path="/nas/pools" view=|| view! { <Gated feature="nas"><NasPoolsPage/></Gated> }/>
                    <Route path="/nas/services" view=|| view! { <Gated feature="nas"><NasServicesPage/></Gated> }/>
                    <Route path="/nas/scheduler" view=|| view! { <Gated feature="nas"><NasSchedulerPage/></Gated> }/>
                    <Route path="/nas/directory" view=|| view! { <Gated feature="ldap|ldap-server|kerberos|ad"><NasDirectoryPage/></Gated> }/>
                    <Route path="/nas/iscsi" view=|| view! { <Gated feature="iscsi-target"><NasIscsiPage/></Gated> }/>
                    <Route path="/nas/s3" view=|| view! { <Gated feature="s3-gateway"><NasS3Page/></Gated> }/>
                </Routes>
            </main>

            <footer class="footer">
                <p>"Horcrux v" {env!("CARGO_PKG_VERSION")} " - Built with Rust + Leptos"</p>
                <p>"50,000+ lines of memory-safe code"</p>
            </footer>
        </Router>
    }
}
