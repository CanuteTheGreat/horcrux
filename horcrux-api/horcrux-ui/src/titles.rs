//! Per-page `<title>`: "VMs · Horcrux", "captainjacksparrow · Console · Horcrux".
//!
//! `title_for(path)` is pure (unit-tested; every route in `lib.rs` has an entry, see the test
//! `every_route_has_its_own_title`). Pages whose subject has a name (a VM, a container) call
//! `set_subject(path, name)` once it is loaded; the name replaces the raw id in the title.
use leptos::*;
use leptos_meta::Title;
use leptos_router::use_location;

pub const SUFFIX: &str = "Horcrux";

/// Name of the thing a detail page shows, for the path it was loaded on (stale paths are ignored).
#[derive(Clone, Copy)]
pub struct Subject(pub RwSignal<Option<(String, String)>>);

pub fn provide_subject() {
    provide_context(Subject(create_rw_signal(None)));
}

/// Call from a detail page once the object's name is known.
pub fn set_subject(name: &str) {
    if let Some(s) = use_context::<Subject>() {
        let path = web_sys::window()
            .and_then(|w| w.location().pathname().ok())
            .unwrap_or_default();
        s.0.set(Some((path, name.to_string())));
    }
}

/// Renders the `<title>` for the current route (place inside `<Router>`).
#[component]
pub fn PageTitle() -> impl IntoView {
    let loc = use_location();
    let subject = use_context::<Subject>();
    let text = move || {
        let path = loc.pathname.get();
        let name = subject
            .and_then(|s| s.0.get())
            .filter(|(p, _)| *p == path)
            .map(|(_, n)| n);
        title_for(&path, name.as_deref())
    };
    view! { <Title text=text/> }
}

fn exact(path: &str) -> Option<&'static str> {
    Some(match path {
        "/" => "Dashboard",
        "/login" => "Sign in",
        "/vms" => "VMs",
        "/vms/create" => "Create VM",
        "/vms/import-ovf" => "Import OVF",
        "/vms/import-libvirt" => "Import from libvirt",
        "/vms/import-esxi" => "Import from ESXi",
        "/resources" => "Resources",
        "/storage" => "Storage",
        "/storage/isos" => "ISO Library",
        "/storage/content" => "Storage Content",
        "/storage/disks" => "Disks",
        "/storage/smart" => "SMART Health",
        "/storage/ceph" => "Ceph",
        "/pools" => "Resource Pools",
        "/network" => "Network",
        "/network/firewall-objects" => "Firewall Objects",
        "/network/bridges" => "Host Bridges",
        "/network/dhcp" => "DHCP & DNS",
        "/network/sdn" => "SDN",
        "/system/power" => "Guest Power",
        "/system/health" => "Node Health",
        "/system/config" => "Node Configuration",
        "/system/services" => "Node Services",
        "/system/shell" => "Node Shell",
        "/system/logs" => "System Log",
        "/system/packages" => "Packages",
        "/system/boot-environments" => "Boot Environments",
        "/system/diagnostics" => "Diagnostics",
        "/system/certificates" => "Certificates",
        "/audit" => "Audit Log",
        "/tasks" => "Tasks",
        "/gpu" => "GPU / PCI Passthrough",
        "/containers" => "Containers",
        "/containers/create" => "Create Container",
        "/containers/images" => "Container Images",
        "/containers/volumes" => "Container Volumes",
        "/containers/networks" => "Container Networks",
        "/containers/stacks" => "Container Stacks",
        "/containers/events" => "Container Events",
        "/containers/lxc/templates" => "LXC Templates",
        "/kubernetes" => "Kubernetes",
        "/snapshots" => "Snapshots",
        "/clones" => "Clones",
        "/replication" => "Replication",
        "/monitoring" => "Monitoring",
        "/monitoring/alerts" => "Alert Center",
        "/monitoring/notifications" => "Notifications",
        "/monitoring/metric-servers" => "Metric Servers",
        "/monitoring/dashboards" => "Dashboards",
        "/monitoring/metrics" => "Metrics Explorer",
        "/monitoring/observability" => "Observability",
        "/alerts" => "System Alerts",
        "/auth/users" => "Users",
        "/auth/roles" => "Roles",
        "/auth/sessions" => "Active Sessions",
        "/auth/api-keys" => "API Keys",
        "/auth/mfa" => "Account Security",
        "/auth/permissions" => "Permissions & Groups",
        "/auth/realms" => "Authentication Realms",
        "/backup" => "Backups",
        "/backup/jobs" => "Backup Jobs",
        "/backup/retention" => "Retention",
        "/backup/verify" => "Verify Backups",
        "/backup/snapshots" => "Backup Snapshots",
        "/backup/templates" => "Backup Templates",
        "/ha" => "High Availability",
        "/ha/cluster" => "Cluster",
        "/ha/failover" => "Failover & Fencing",
        "/ha/maintenance" => "Maintenance & Rebalance",
        "/ha/groups" => "HA Groups",
        "/ha/migration" => "Migration Center",
        "/nas" => "NAS",
        "/nas/shares" => "NAS Shares",
        "/nas/users" => "NAS Users",
        "/nas/groups" => "NAS Groups",
        "/nas/pools" => "NAS Pools",
        "/nas/services" => "NAS Services",
        "/nas/scheduler" => "NAS Scheduler",
        "/nas/directory" => "Directory Services",
        "/nas/iscsi" => "iSCSI Targets",
        "/nas/s3" => "S3 Gateway",
        _ => return None,
    })
}

fn last(path: &str, what: &str) -> Option<String> {
    let (head, tail) = path.rsplit_once('/')?;
    let _ = head;
    Some(format!("{what}{tail}"))
}

/// Title for `path`. `subject` is the loaded name of the VM / container the path is about, if any.
pub fn title_for(path: &str, subject: Option<&str>) -> String {
    let path = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    };
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    let named = |id: &str| subject.unwrap_or(id).to_string();
    let page = if let Some(t) = exact(path) {
        t.to_string()
    } else {
        match segs.as_slice() {
            ["vms", id] => named(id),
            ["vms", id, "console"] => format!("{} · Console", named(id)),
            ["vms", id, "hardware"] => format!("{} · Hardware", named(id)),
            ["containers", id, sub] => format!(
                "{} · {}",
                named(id),
                match *sub {
                    "logs" => "Logs",
                    "firewall" => "Firewall",
                    "exec" => "Exec",
                    "console" => "Console",
                    "lxc" => "LXC",
                    _ => "Container",
                }
            ),
            ["kubernetes", cl, rest @ ..] => {
                let leaf = rest.last().copied().unwrap_or("dashboard");
                let leaf = match leaf {
                    "dashboard" => "Dashboard",
                    "pods" => "Pods",
                    "deployments" => "Deployments",
                    "services" => "Services",
                    "ingresses" => "Ingresses",
                    "repositories" => "Helm Repositories",
                    "charts" => "Helm Charts",
                    "releases" => "Helm Releases",
                    "configmaps" => "ConfigMaps",
                    "secrets" => "Secrets",
                    other => other,
                };
                // /kubernetes/:cluster/:namespace/pods -> "Pods (namespace) · cluster"
                if rest.len() == 2 && !matches!(rest[0], "helm" | "config") {
                    format!("{leaf} ({}) · {cl}", rest[0])
                } else {
                    format!("{leaf} · {cl}")
                }
            }
            _ => last(path, "")
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Not found".into()),
        }
    };
    format!("{page} · {SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_per_page() {
        assert_eq!(title_for("/vms", None), "VMs · Horcrux");
        assert_eq!(title_for("/", None), "Dashboard · Horcrux");
        assert_eq!(
            title_for("/vms/abc/console", Some("captainjacksparrow")),
            "captainjacksparrow · Console · Horcrux"
        );
        assert_eq!(
            title_for("/vms/abc/console", None),
            "abc · Console · Horcrux"
        );
        assert_eq!(
            title_for("/containers/web1/logs", None),
            "web1 · Logs · Horcrux"
        );
        assert_eq!(
            title_for("/kubernetes/c1/pods", None),
            "Pods · c1 · Horcrux"
        );
        assert_eq!(
            title_for("/kubernetes/c1/dev/pods", None),
            "Pods (dev) · c1 · Horcrux"
        );
        assert_eq!(title_for("/ha/", None), "High Availability · Horcrux");
    }

    #[test]
    fn every_route_has_its_own_title() {
        // Every static <Route path="/..."> in lib.rs resolves to a title that is not the generic fallback,
        // and no two routes share one.
        let lib = include_str!("lib.rs");
        let mut seen = std::collections::HashMap::new();
        for part in lib.split("<Route path=\"").skip(1) {
            let path = part.split('"').next().unwrap();
            if path.contains(':') {
                continue;
            }
            let t = title_for(path, None);
            assert!(!t.starts_with("Not found"), "{path} has no title");
            if let Some(prev) = seen.insert(t.clone(), path.to_string()) {
                panic!("{path} and {prev} share the title {t:?}");
            }
        }
        assert!(seen.len() > 60);
    }
}
