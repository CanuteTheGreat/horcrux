#![allow(unused_variables)]

use leptos::*;
use leptos_meta::*;
use leptos_router::*;

use features::{provide_features, Gated};
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
pub mod titles;
pub mod utils;
mod webauthn;
mod websocket;

use pages::{
    AlertCenterPage, Alerts, ApiKeysPage, BackupDashboard, BackupJobsPage, BackupVerifyPage,
    CertificatesPage, CloneList, ClusterDashboard, ClusterManagementPage, ConfigMapsPage,
    ContainerCreate, ContainerEventsPage, ContainerExecPage, ContainerList, ContainerLogsPage,
    ContainerNetworksPage, Dashboard, DashboardsPage, DeploymentsPage, DiskManagementPage,
    GamingVmWizard, GpuManagement, HaDashboard, HaFailoverPage, HaGroupsPage, HelmChartsPage,
    HelmReleasesPage, HelmRepositoriesPage, ImagesPage, IngressesPage, KubernetesManagement, Login,
    LxcContainerPage, LxcTemplatesPage, MaintenancePage, MetricServersPage, MetricsExplorerPage,
    MfaPage, MigrationCenterPage, Monitoring, NetworkManagement, NotificationsPage,
    ObservabilityPage, PermissionsPage, PodsPage, RealmsPage, ReplicationList,
    RetentionPoliciesPage, RolesPage, SecretsPage, ServicesPage, SessionsPage, SmartMonitoringPage,
    SnapshotList, SnapshotManagerPage, StacksPage, StorageManagement, TemplateManagerPage,
    UserMenu, UsersPage, VmConsole, VmCreate, VmImportEsxiPage, VmImportLibvirtPage,
    VmImportOvfPage, VmList, VolumesPage,
};

/// Pages mount only for a signed-in browser. Without this, a first visit rendered the dashboard,
/// which fired a dozen API calls that all answered 401 (console errors, wasted requests) before
/// UserMenu's session check moved the browser to /login. A browser without a stored token asks
/// the server once (a session cookie may still be valid) and goes to /login if that fails.
#[component]
fn SessionGate(children: ChildrenFn) -> impl IntoView {
    let location = use_location();
    let (server_session, set_server_session) = create_signal(false);
    let allowed = create_memo(move |_| {
        location.pathname.get() == "/login"
            || api::get_auth_token().is_some()
            || server_session.get()
    });
    create_effect(move |_| {
        if allowed.get() {
            return;
        }
        spawn_local(async move {
            match api::get_me().await {
                Ok(_) => set_server_session.set(true),
                // A full page load, not the router's navigate: called from this task the
                // router navigation was silently dropped and the page stayed blank. `next`
                // brings the user back to the page they asked for after signing in.
                Err(_) => api::go_to_login(),
            }
        });
    });
    view! { <Show when=move || allowed.get() fallback=|| ()>{children()}</Show> }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    // Which USE-flag features the server was built with: menu entries and
    // pages of the others are hidden (see features.rs).
    let features = provide_features();

    // Per-page <title> ("VMs · Horcrux"); detail pages add the object's name via titles::set_subject.
    titles::provide_subject();

    // Phone layout: the nav is an off-canvas drawer opened by the hamburger, closed by a tap on a link,
    // a tap outside (backdrop) or Escape.
    let (nav_open, set_nav_open) = create_signal(false);
    let _esc = window_event_listener(ev::keydown, move |e| {
        if e.key() == "Escape" {
            set_nav_open.set(false);
        }
    });

    view! {
        <Meta name="description" content="Horcrux Virtualization Management Platform"/>

        <Router>
            <titles::PageTitle/>
            <div class="app-shell" class:nav-open=move || nav_open.get() class:login-page=move || use_location().pathname.get() == "/login">
            <div class="nav-backdrop" class:open=move || nav_open.get() on:click=move |_| set_nav_open.set(false)></div>
            <nav class="navbar" aria-label="Header" class:nav-open=move || nav_open.get() class:login-page=move || use_location().pathname.get() == "/login" data-features-loaded=move || features.loaded().to_string()>
                <button class="menu-toggle" type="button" aria-label="Open menu"
                    aria-expanded=move || nav_open.get().to_string()
                    on:click=move |_| set_nav_open.update(|o| *o = !*o)>"\u{2630}"</button>
                <A href="/" class="navbar-brand" attr:title="Datacenter summary">
                    <img class="brand-mark" src="/brand/mark.png" alt="" width="26" height="26"/>
                    <h1>"Horcrux"</h1>
                    <span class="brand-version">{concat!("v", env!("CARGO_PKG_VERSION"))}</span>
                </A>
                <components::shell::GlobalSearch/>
                <components::shell::HeaderActions/>
                <UserMenu/>
            </nav>
            <nav class="sidebar" aria-label="Resources and pages" on:click=move |ev| {
                if event_target::<web_sys::Element>(&ev).closest("a").ok().flatten().is_some() {
                    set_nav_open.set(false);
                }
            }>
                <components::shell::Sidebar/>
            </nav>

            <main class="container">
                <SessionGate>
                <Routes>
                    <Route path="/" view=Dashboard/>
                    <Route path="/vms" view=VmList/>
                    <Route path="/vms/create" view=VmCreate/>
                    <Route path="/vms/gaming/new" view=GamingVmWizard/>
                    <Route path="/vms/import-ovf" view=VmImportOvfPage/>
                    <Route path="/vms/import-libvirt" view=VmImportLibvirtPage/>
                    <Route path="/vms/import-esxi" view=VmImportEsxiPage/>
                    <Route path="/vms/:id" view=crate::pages::VmDetailPage/>
                    <Route path="/resources" view=crate::pages::ResourcesPage/>
                    <Route path="/vms/:id/console" view=VmConsole/>
                    <Route path="/vms/:id/hardware" view=crate::pages::VmHardwarePage/>
                    <Route path="/vms/:id/remote-play" view=crate::pages::VmRemotePlayPage/>
                    <Route path="/storage/isos" view=crate::pages::IsoLibraryPage/>
                    <Route path="/storage/content" view=crate::pages::StorageContentPage/>
                    <Route path="/pools" view=crate::pages::ResourcePoolsPage/>
                    <Route path="/network/firewall-objects" view=crate::pages::FirewallObjectsPage/>
                    <Route path="/network/bridges" view=crate::pages::HostBridgesPage/>
                    <Route path="/network/dhcp" view=crate::pages::DhcpNetworksPage/>
                    <Route path="/network/sdn" view=crate::pages::sdn_zones::SdnZonesPage/>
                    <Route path="/system/power" view=crate::pages::GuestPowerPage/>
                    <Route path="/system/health" view=crate::pages::system::health::SystemHealthPage/>
                    <Route path="/system/config" view=crate::pages::system::configuration::SystemConfigurationPage/>
                    <Route path="/system/console" view=crate::pages::system::console_settings::ConsoleSettingsPage/>
                    <Route path="/system/services" view=crate::pages::system::services::ServicesPage/>
                    <Route path="/system/shell" view=crate::pages::system::shell::NodeShellPage/>
                    <Route path="/security" view=crate::pages::security::SecurityOverviewPage/>
                    <Route path="/security/vulnerabilities" view=crate::pages::security::SecurityVulnsPage/>
                    <Route path="/security/vulnerabilities/:id" view=crate::pages::security::SecurityScanPage/>
                    <Route path="/security/hardening" view=crate::pages::security::SecurityHardeningPage/>
                    <Route path="/security/settings" view=crate::pages::security::SecuritySettingsPage/>
                    <Route path="/audit" view=crate::pages::AuditLogPage/>
                    <Route path="/system/logs" view=crate::pages::system::logs::SystemLogsPage/>
                    <Route path="/system/packages" view=crate::pages::system::package_manager::PackageManagerPage/>
                    <Route path="/system/boot-environments" view=crate::pages::system::boot_environments::BootEnvironmentsPage/>
                    <Route path="/system/diagnostics" view=crate::pages::system::diagnostics::DiagnosticsPage/>
                    <Route path="/system/about" view=crate::pages::system::about::AboutPage/>
                    <Route path="/system/licenses" view=crate::pages::system::about::LicensesPage/>
                    <Route path="/tasks" view=crate::pages::system::tasks::TasksPage/>
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
                    <Route path="/storage/san" view=crate::pages::SanPage/>
                    <Route path="/storage/san/new" view=crate::pages::SanWizard/>
                    <Route path="/storage/ceph" view=crate::pages::ceph::CephPage/>
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
                    <Route path="/ha/maintenance" view=|| view! { <Gated feature="cluster"><MaintenancePage/></Gated> }/>
                    <Route path="/ha/groups" view=|| view! { <Gated feature="cluster"><HaGroupsPage/></Gated> }/>
                    <Route path="/ha/resources" view=|| view! { <Gated feature="cluster"><crate::pages::HaResourcesPage/></Gated> }/>
                    <Route path="/ha/migration" view=|| view! { <Gated feature="cluster"><MigrationCenterPage/></Gated> }/>
                    <Route path="/monitoring/alerts" view=AlertCenterPage/>
                    <Route path="/monitoring/notifications" view=NotificationsPage/>
                    <Route path="/monitoring/metric-servers" view=MetricServersPage/>
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
                    // Unknown address: say so and offer a way on (used to render an empty page).
                    <Route path="/*any" view=crate::pages::not_found::NotFoundPage/>
                </Routes>
                </SessionGate>
            </main>

            </div>
        </Router>
    }
}
