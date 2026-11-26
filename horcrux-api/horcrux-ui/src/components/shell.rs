//! App shell (docs/design/ui-system.md "Layout"): the header bar's global search and live
//! counters, and the sidebar with the live resource tree above the grouped navigation.

use crate::api;
use crate::components::ui::{state_tone, usage_tone};
use crate::features::IfFeature;
use leptos::*;
use leptos_router::*;
use std::time::Duration;

/// Global search in the header: Enter opens Resources with the query; "/" focuses it from anywhere.
#[component]
pub fn GlobalSearch() -> impl IntoView {
    let input = create_node_ref::<html::Input>();
    let navigate = use_navigate();
    let _slash = window_event_listener(ev::keydown, move |e| {
        if e.key() != "/" || e.ctrl_key() || e.meta_key() || e.alt_key() {
            return;
        }
        // not while typing in a field
        let typing = e
            .target()
            .and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok())
            .map(|el| {
                matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                    || el.closest("[contenteditable]").ok().flatten().is_some()
            })
            .unwrap_or(false);
        if typing {
            return;
        }
        if let Some(i) = input.get() {
            e.prevent_default();
            let _ = i.focus();
        }
    });
    view! {
        <form class="global-search" role="search" on:submit=move |ev| {
            ev.prevent_default();
            let q = input.get().map(|i| i.value()).unwrap_or_default();
            navigate(&format!("/resources?q={}", urlencoding::encode(q.trim())), Default::default());
        }>
            <span class="search-glyph" aria-hidden="true"></span>
            <input type="search" id="global-search" node_ref=input autocomplete="off"
                aria-label="Search guests, nodes, storage and pools"
                placeholder="Search guests, nodes, storage, pools"/>
            <kbd aria-hidden="true">"/"</kbd>
        </form>
    }
}

/// Header buttons: Create VM / CT and live counts of running tasks and active alerts (each a link to
/// the filtered list). Counts refresh every 10 s; a count the user's role can't read is not shown.
#[component]
pub fn HeaderActions() -> impl IntoView {
    let (running, set_running) = create_signal(None::<usize>);
    let (alerts, set_alerts) = create_signal(None::<usize>);
    let load = move || {
        spawn_local(async move {
            match api::list_tasks("running").await {
                Ok(t) => set_running.set(Some(t.len())),
                Err(_) => set_running.set(None),
            }
            match api::get_active_alerts().await {
                Ok(a) => set_alerts.set(Some(a.len())),
                Err(_) => set_alerts.set(None),
            }
        });
    };
    load();
    if let Ok(h) = set_interval_with_handle(load, Duration::from_secs(10)) {
        on_cleanup(move || h.clear());
    }
    view! {
        <div class="header-actions-bar">
            <A href="/vms/create" class="hdr-btn primary hdr-create" attr:id="hdr-create-vm">"Create VM"</A>
            <A href="/containers/create" class="hdr-btn hdr-create" attr:id="hdr-create-ct">"Create CT"</A>
            <A href="/tasks?status=running" class="hdr-btn" attr:id="hdr-tasks" attr:title="Running tasks">
                <span class="hdr-label">"Tasks"</span>
                <span class=move || { if running.get().unwrap_or(0) > 0 { "hdr-count is-busy" } else { "hdr-count" } }>
                    {move || running.get().map(|n| n.to_string()).unwrap_or_else(|| "-".into())}
                </span>
            </A>
            <A href="/alerts" class="hdr-btn" attr:id="hdr-alerts" attr:title="Active alerts">
                <span class="hdr-label">"Alerts"</span>
                <span class=move || { if alerts.get().unwrap_or(0) > 0 { "hdr-count is-bad" } else { "hdr-count" } }>
                    {move || alerts.get().map(|n| n.to_string()).unwrap_or_else(|| "-".into())}
                </span>
            </A>
        </div>
    }
}

/// One row of the resource tree.
fn tree_row(n: &api::InvTreeNode, depth: u8, path: &str) -> View {
    let tone = state_tone(&n.status);
    let link = if n.link.is_empty() {
        "/resources".to_string()
    } else {
        n.link.clone()
    };
    let active = link != "/" && path == link;
    let title = if n.status.is_empty() {
        n.name.clone()
    } else {
        format!("{} ({})", n.name, n.status)
    };
    let id_label = (n.kind == "vm")
        .then(|| n.id.clone())
        .filter(|id| id != &n.name);
    view! {
        <li class=format!("t{depth} tree-{}", n.kind) data-kind=n.kind.clone() data-id=n.id.clone()>
            <a href=link class="side-item" class:active=active title=title>
                <span class=format!("dot {tone}") aria-hidden="true"></span>
                <span class="side-label">{n.name.clone()}</span>
                {id_label.map(|i| view! { <span class="tree-id">{i}</span> })}
            </a>
        </li>
    }
    .into_view()
}

/// Live resource tree: Datacenter > node > its VMs, containers and storage (from GET /api/tree),
/// refreshed every 15 s, with the current object highlighted.
#[component]
pub fn ResourceTree() -> impl IntoView {
    let (tree, set_tree) = create_signal(None::<api::InvTreeNode>);
    let (denied, set_denied) = create_signal(false);
    let (pools, set_pools) = create_signal(Vec::<api::StoragePoolInfo>::new());
    let load = move || {
        spawn_local(async move {
            match api::get_resource_tree().await {
                Ok(t) => set_tree.set(Some(t)),
                Err(e) if e.is_forbidden() => set_denied.set(true),
                Err(_) => {}
            }
            if let Ok(p) = api::get_storage_pools().await {
                set_pools.set(p);
            }
        });
    };
    load();
    if let Ok(h) = set_interval_with_handle(load, Duration::from_secs(15)) {
        on_cleanup(move || h.clear());
    }
    let location = use_location();
    view! {
        <div class="side-heading"><span>"Resources"</span><A href="/resources">"Search"</A></div>
        <ul class="tree" id="side-tree">
            {move || {
                let path = location.pathname.get();
                if denied.get() {
                    return view! { <li class="tree-empty">"Your role can't list resources."</li> }.into_view();
                }
                let Some(dc) = tree.get() else {
                    return view! { <li class="tree-empty"><span class="skeleton" style="display:block;width:70%"></span></li> }.into_view();
                };
                let usage: std::collections::HashMap<String, f64> = pools
                    .get()
                    .into_iter()
                    .filter(|p| p.total_bytes > 0)
                    .map(|p| (p.id.clone(), p.used_bytes as f64 * 100.0 / p.total_bytes as f64))
                    .collect();
                let mut rows = vec![view! {
                    <li class="t0 tree-datacenter">
                        <a href="/" class="side-item" class:active=path == "/" title="Datacenter summary">
                            <span class="dot ok" aria-hidden="true"></span>
                            <span class="side-label">"Datacenter"</span>
                        </a>
                    </li>
                }.into_view()];
                for node in dc.children.iter().filter(|c| c.kind == "node") {
                    rows.push(tree_row(node, 1, &path));
                    for group in &node.children {
                        if group.children.is_empty() {
                            if group.id == "vms" {
                                rows.push(view! {
                                    <li class="tree-empty">"0 VMs \u{b7} " <A href="/vms/create">"Create VM"</A></li>
                                }.into_view());
                            }
                            continue;
                        }
                        for item in &group.children {
                            if item.kind == "storage" {
                                let pct = usage.get(&item.id).copied();
                                let link = format!("/storage?pool={}", item.id);
                                let active = path == "/storage";
                                rows.push(view! {
                                    <li class="t2 tree-storage" data-kind="storage" data-id=item.id.clone()>
                                        <a href=link.clone() class="side-item" class:active=active
                                            title=pct.map(|p| format!("{} ({:.0}% used)", item.name, p)).unwrap_or_else(|| item.name.clone())>
                                            <span class="side-storage-glyph" aria-hidden="true"></span>
                                            <span class="side-label">{item.name.clone()}</span>
                                            {pct.map(|p| view! {
                                                <span class=format!("usage-mini {}", usage_tone(p))><i style=format!("width:{:.0}%", p)></i></span>
                                            })}
                                        </a>
                                    </li>
                                }.into_view());
                            } else {
                                rows.push(tree_row(item, 2, &path));
                            }
                        }
                    }
                }
                rows.into_view()
            }}
        </ul>
    }
}

/// A collapsible navigation group. It opens itself when the current page is one of its links.
#[component]
fn NavGroup(
    #[prop(into)] title: String,
    prefixes: Vec<&'static str>,
    children: Children,
) -> impl IntoView {
    let details = create_node_ref::<html::Details>();
    let location = use_location();
    create_effect(move |_| {
        let path = location.pathname.get();
        let hit = prefixes
            .iter()
            .any(|p| path == *p || (p.len() > 1 && path.starts_with(&format!("{p}/"))));
        if hit {
            if let Some(d) = details.get() {
                let _ = d.set_attribute("open", "");
            }
        }
    });
    view! {
        <details class="side-group" node_ref=details>
            <summary>{title}</summary>
            <div class="side-items">{children()}</div>
        </details>
    }
}

/// The sidebar: resource tree, then every page grouped by area (labels match the page titles).
#[component]
pub fn Sidebar() -> impl IntoView {
    let item = |href: &'static str, label: &'static str| view! { <A href=href class="side-item" exact=true>{label}</A> };
    view! {
        <ResourceTree/>
        <div class="side-heading"><span>"Navigation"</span></div>
        <div class="side-items">
            {item("/tasks", "Tasks")}
            {item("/alerts", "System Alerts")}
        </div>
        <NavGroup title="Compute" prefixes=vec!["/vms", "/containers", "/kubernetes", "/gpu"]>
            {item("/vms", "Virtual Machines")}
            {item("/containers", "Containers")}
            <IfFeature feature="docker|podman">
                {item("/containers/images", "Images")}
                {item("/containers/volumes", "Volumes")}
                {item("/containers/networks", "Container Networks")}
                {item("/containers/stacks", "Stacks (Compose)")}
                {item("/containers/events", "Container Events")}
            </IfFeature>
            <IfFeature feature="kubernetes">
                {item("/kubernetes", "Kubernetes")}
            </IfFeature>
            {item("/gpu", "GPU / PCI Passthrough")}
        </NavGroup>
        <NavGroup title="Storage" prefixes=vec!["/storage", "/pools"]>
            {item("/storage", "Storage")}
            {item("/storage/disks", "Disks")}
            {item("/storage/smart", "SMART Health")}
            {item("/storage/ceph", "Ceph")}
            {item("/storage/isos", "ISO Library")}
            {item("/storage/content", "Storage Content")}
            {item("/pools", "Resource Pools")}
        </NavGroup>
        <NavGroup title="Network" prefixes=vec!["/network"]>
            {item("/network", "Network")}
            {item("/network/bridges", "Host Bridges")}
            {item("/network/dhcp", "DHCP & DNS")}
            {item("/network/firewall-objects", "Firewall Objects")}
            {item("/network/sdn", "SDN (Zones, VNets, IPAM)")}
        </NavGroup>
        <IfFeature feature="cluster">
            <NavGroup title="Cluster" prefixes=vec!["/ha"]>
                {item("/ha", "High Availability")}
                {item("/ha/cluster", "Cluster Nodes")}
                {item("/ha/maintenance", "Maintenance & Rebalance")}
                {item("/ha/migration", "Migration Center")}
            </NavGroup>
        </IfFeature>
        <NavGroup title="Protection" prefixes=vec!["/backup", "/snapshots", "/clones", "/replication", "/system/power"]>
            <IfFeature feature="backup">
                {item("/backup", "Backup & Protection")}
            </IfFeature>
            {item("/snapshots", "Snapshots")}
            {item("/clones", "Clones")}
            {item("/replication", "Replication")}
            {item("/system/power", "Guest Power (Save State)")}
        </NavGroup>
        <IfFeature feature="nas">
            <NavGroup title="NAS" prefixes=vec!["/nas"]>
                {item("/nas", "NAS Overview")}
                {item("/nas/shares", "Shares")}
                {item("/nas/users", "NAS Users")}
                {item("/nas/groups", "NAS Groups")}
                {item("/nas/pools", "NAS Pools")}
                {item("/nas/services", "NAS Services")}
                {item("/nas/scheduler", "Scheduler")}
                <IfFeature feature="ldap|ldap-server|kerberos|ad">
                    {item("/nas/directory", "Directory Services")}
                </IfFeature>
                <IfFeature feature="iscsi-target">
                    {item("/nas/iscsi", "iSCSI Targets")}
                </IfFeature>
                <IfFeature feature="s3-gateway">
                    {item("/nas/s3", "S3 Gateway")}
                </IfFeature>
            </NavGroup>
        </IfFeature>
        <NavGroup title="Monitoring" prefixes=vec!["/monitoring"]>
            {item("/monitoring", "Overview")}
            {item("/monitoring/alerts", "Alert Center")}
            {item("/monitoring/notifications", "Notifications")}
            {item("/monitoring/metric-servers", "Metric Servers")}
            {item("/monitoring/dashboards", "Dashboards")}
            {item("/monitoring/metrics", "Metrics Explorer")}
        </NavGroup>
        <NavGroup title="Node" prefixes=vec!["/system/health", "/system/config", "/system/console", "/system/services", "/system/shell", "/system/logs", "/system/packages", "/system/boot-environments", "/system/diagnostics", "/system/about", "/system/licenses", "/audit", "/resources"]>
            {item("/resources", "Resources (search, tags)")}
            {item("/system/health", "Node Health")}
            {item("/system/config", "Node Configuration")}
            {item("/system/console", "Horcrux Console")}
            {item("/system/services", "Node Services")}
            {item("/system/shell", "Node Shell")}
            {item("/system/logs", "System Log")}
            {item("/audit", "Audit Log")}
            {item("/system/packages", "Packages")}
            {item("/system/boot-environments", "Boot Environments")}
            {item("/system/diagnostics", "Diagnostics")}
            {item("/system/about", "About & Licenses")}
        </NavGroup>
        <NavGroup title="Administration" prefixes=vec!["/auth", "/system/certificates"]>
            {item("/auth/users", "Users")}
            {item("/auth/roles", "Roles")}
            {item("/auth/permissions", "Permissions & Groups")}
            {item("/auth/realms", "Authentication Realms")}
            {item("/auth/mfa", "Account Security (2FA)")}
            {item("/system/certificates", "Certificates")}
            {item("/auth/sessions", "Active Sessions")}
            {item("/auth/api-keys", "API Keys")}
        </NavGroup>
    }
}
