//! Unknown address: say so, suggest the page the user most likely meant, and
//! offer a way back (an unknown route used to render an empty page).
use leptos::*;
use leptos_router::{use_location, A};

/// The page an unknown path most likely meant (old links, Proxmox habits, typos of a section).
pub fn suggestion(path: &str) -> Option<(&'static str, &'static str)> {
    let first = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    Some(match first.as_str() {
        "cluster" | "nodes" | "node" | "datacenter" => ("/ha/cluster", "Cluster Nodes"),
        "ha" => ("/ha", "High Availability"),
        "vm" | "qemu" | "guests" => ("/vms", "Virtual Machines"),
        "ct" | "lxc" => ("/containers", "Containers"),
        "users" | "user" | "access" => ("/auth/users", "Users"),
        "roles" => ("/auth/roles", "Roles"),
        "permissions" | "acl" => ("/auth/permissions", "Permissions & Groups"),
        "realms" => ("/auth/realms", "Authentication Realms"),
        "task" | "tasks" | "log" => ("/tasks", "Tasks"),
        "notifications" | "notification" => ("/monitoring/notifications", "Notifications"),
        "backups" | "backup" => ("/backup", "Backup & Protection"),
        "storage" => ("/storage", "Storage"),
        "network" => ("/network", "Network"),
        "system" | "shell" => ("/system/health", "Node Health"),
        _ => return None,
    })
}

#[component]
pub fn NotFoundPage() -> impl IntoView {
    let loc = use_location();
    let path = move || loc.pathname.get();
    view! {
        <div class="page not-found-page" id="not-found">
            <crate::components::ui::EmptyState title="Page not found"
                hint="This address is not a page of Horcrux. It may be an old link or a typo.">
                {move || suggestion(&path()).map(|(href, label)| view! {
                    <A href=href class="btn btn-primary" attr:id="not-found-suggestion">{format!("Go to {label}")}</A>
                })}
                <A href="/" class="btn" attr:id="not-found-home">"Dashboard"</A>
            </crate::components::ui::EmptyState>
            <p class="muted" id="not-found-path">{move || format!("Address: {}", path())}</p>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_wrong_addresses_point_somewhere_useful() {
        assert_eq!(suggestion("/cluster").unwrap().0, "/ha/cluster");
        assert_eq!(suggestion("/nodes/node-01").unwrap().0, "/ha/cluster");
        assert_eq!(suggestion("/Users").unwrap().0, "/auth/users");
        assert_eq!(suggestion("/tasks/xyz").unwrap().0, "/tasks");
        assert_eq!(suggestion("/qwerty"), None);
        assert_eq!(suggestion("/"), None);
    }
}
