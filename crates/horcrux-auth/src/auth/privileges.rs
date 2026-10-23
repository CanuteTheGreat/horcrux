//! What each API route requires (privilege + ACL path), and the built-in roles.
//!
//! Every request that gets past authentication is checked against
//! [`requirement`] by `middleware::rbac::rbac_middleware`. The mapping is by
//! resource area (the first path segment after `/api/`), method and action,
//! so a new route inside a known area is covered automatically. A route in an
//! area this file doesn't know falls through to the deny-by-default rule
//! (`Sys.Audit` on `/` to read, `Sys.Modify` on `/` to change anything), which
//! only administrators hold. `tests::every_registered_route_is_mapped` fails CI
//! when a new top-level area appears without an explicit entry here.

use horcrux_common::auth::Privilege;
use horcrux_common::auth::Privilege as P;

/// What a request needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requirement {
    /// No authentication at all (login, health, OIDC callback, ...).
    Public,
    /// Any authenticated user; the handler itself restricts what they see
    /// or change (own sessions, own password, own second factors, tickets).
    Authenticated,
    /// `privilege` on ACL `path`.
    Privilege { privilege: Privilege, path: String },
    /// Collection read: allowed if the user holds `privilege` on `prefix` or
    /// anywhere below it. The response is then filtered item by item.
    AnyBelow {
        privilege: Privilege,
        prefix: String,
    },
    /// The caller acting on their own account (user id or username in the
    /// URL), or anyone holding `privilege` on `path`.
    SelfOr {
        user: String,
        privilege: Privilege,
        path: String,
    },
}

fn need(privilege: Privilege, path: impl Into<String>) -> Requirement {
    Requirement::Privilege {
        privilege,
        path: path.into(),
    }
}

fn read_write(
    method: &str,
    read: Privilege,
    write: Privilege,
    path: impl Into<String>,
) -> Requirement {
    if is_read(method) {
        need(read, path)
    } else {
        need(write, path)
    }
}

fn is_read(method: &str) -> bool {
    matches!(method, "GET" | "HEAD" | "OPTIONS")
}

const POWER_ACTIONS: &[&str] = &[
    "start",
    "stop",
    "shutdown",
    "reboot",
    "reset",
    "pause",
    "resume",
    "suspend",
    "save-state",
    "resume-state",
];

/// Areas with an explicit rule below. Kept as a list so the route-coverage
/// test can tell "mapped" from "fell through to the default".
pub const KNOWN_AREAS: &[&str] = &[
    "auth",
    "users",
    "roles",
    "privileges",
    "groups",
    "acl",
    "permissions",
    "access",
    "vms",
    "containers",
    "images",
    "volumes",
    "container-networks",
    "stacks",
    "snapshot-schedules",
    "snapshot-quotas",
    "clone-jobs",
    "replication",
    "console",
    "migrate",
    "migration",
    "backups",
    "backup-jobs",
    "backup-storages",
    "cloudinit",
    "templates",
    "audit",
    "storage",
    "ceph",
    "gpu",
    "tls",
    "vault",
    "observability",
    "webhooks",
    "alerts",
    "notifications",
    "monitoring",
    "metrics",
    "cluster",
    "ha",
    "firewall",
    "cni",
    "network-policies",
    "sdn",
    "network",
    "k8s",
    "nas",
    "ws",
    "health",
    "docs",
    "openapi.json",
    "system",
    "nodes",
    "host-agent",
    "tasks",
    "lxc",
];

/// The requirement for `method` on `path` (the full request path, e.g.
/// `/api/vms/abc/start`).
pub fn requirement(method: &str, path: &str) -> Requirement {
    let method = method.to_ascii_uppercase();
    let m = method.as_str();
    if path == "/metrics" {
        return need(P::SysAudit, "/");
    }
    let rest = match path.strip_prefix("/api/") {
        Some(r) => r,
        None if path == "/api" => "",
        // Static UI files etc. never reach the protected router.
        None => return Requirement::Public,
    };
    let s: Vec<&str> = rest.split('/').filter(|x| !x.is_empty()).collect();
    let area = s.first().copied().unwrap_or("");
    let arg = |i: usize| s.get(i).copied().unwrap_or("");

    match area {
        "health" | "docs" | "openapi.json" => Requirement::Public,

        "auth" => match arg(1) {
            "login" | "register" | "tfa" | "oidc" | "realms" | "bootstrap" => Requirement::Public,
            "mfa" => match (arg(2), m) {
                ("policy", "GET") => Requirement::Authenticated,
                ("policy", _) => need(P::UserModify, "/access"),
                ("users", "GET") => need(P::SysAudit, "/access"),
                ("users", _) => need(P::UserModify, "/access/users"),
                _ => Requirement::Authenticated,
            },
            // logout, verify, sessions, password, me, permissions
            _ => Requirement::Authenticated,
        },

        "access" => match arg(1) {
            "realms" => read_write(m, P::SysAudit, P::SysModify, "/access/realms"),
            "acl" => acl_rule(m),
            "roles" => read_write(m, P::SysAudit, P::PermissionsModify, "/"),
            "groups" => read_write(m, P::SysAudit, P::UserModify, "/access/groups"),
            _ => read_write(m, P::SysAudit, P::UserModify, "/access"),
        },
        "roles" => read_write(m, P::SysAudit, P::PermissionsModify, "/"),
        "privileges" => Requirement::Authenticated,
        "acl" => acl_rule(m),
        "groups" => read_write(m, P::SysAudit, P::UserModify, "/access/groups"),

        "users" | "permissions" => {
            let id = arg(1);
            if id.is_empty() {
                return read_write(m, P::SysAudit, P::UserModify, "/access/users");
            }
            let sub = if area == "permissions" {
                "permissions"
            } else {
                arg(2)
            };
            let (privilege, p) = match (sub, is_read(m)) {
                ("permissions", true) => (P::SysAudit, "/access"),
                // checked by the handler against the ACL path being changed
                ("permissions", false) => return Requirement::Authenticated,
                // own password / API keys / sessions / profile read
                ("password" | "api-keys" | "sessions" | "tfa", _) | ("", true) => {
                    (P::UserModify, "/access/users")
                }
                // enable/disable, edit, delete, group membership: admins only
                _ => {
                    return need(P::UserModify, "/access/users");
                }
            };
            Requirement::SelfOr {
                user: id.to_string(),
                privilege,
                path: p.to_string(),
            }
        }

        "vms" => {
            let id = arg(1);
            if id.is_empty() {
                return if is_read(m) {
                    Requirement::AnyBelow {
                        privilege: P::VmAudit,
                        prefix: "/vms".into(),
                    }
                } else {
                    need(P::VmAllocate, "/vms")
                };
            }
            if id == "import" {
                return need(P::VmAllocate, "/vms");
            }
            // What the VM create/edit forms can offer (bridges, ISOs).
            if id == "host-options" {
                return Requirement::AnyBelow {
                    privilege: P::VmAudit,
                    prefix: "/vms".into(),
                };
            }
            guest_action(m, &format!("/vms/{}", id), arg(2))
        }

        "containers" => {
            let id = arg(1);
            match id {
                "" => {
                    if is_read(m) {
                        Requirement::AnyBelow {
                            privilege: P::VmAudit,
                            prefix: "/containers".into(),
                        }
                    } else {
                        need(P::VmAllocate, "/containers")
                    }
                }
                // websocket attach with a ticket minted under VM.Console/VM.Audit
                "ws" => Requirement::Authenticated,
                "events-ticket" => Requirement::AnyBelow {
                    privilege: P::VmAudit,
                    prefix: "/containers".into(),
                },
                _ => guest_action(m, &format!("/containers/{}", id), arg(2)),
            }
        }
        "images" | "volumes" | "stacks" => read_write(m, P::VmAudit, P::VmAllocate, "/containers"),
        "container-networks" => read_write(m, P::SdnAudit, P::SdnAllocate, "/sdn"),

        "snapshot-schedules" => read_write(m, P::VmAudit, P::VmSnapshot, "/vms"),
        "snapshot-quotas" => read_write(m, P::DatastoreAudit, P::DatastoreAllocate, "/storage"),
        "clone-jobs" => read_write(m, P::VmAudit, P::VmClone, "/vms"),
        "replication" => read_write(m, P::VmAudit, P::VmMigrate, "/vms"),
        // The peer API is node-to-node only: the handlers accept nothing but
        // a `cluster-migration` service token (403 for every user role).
        "migration" if arg(1) == "peer" => Requirement::Authenticated,
        "migration" => read_write(m, P::VmAudit, P::VmMigrate, "/vms"),
        "cloudinit" => {
            let id = arg(1);
            read_write(m, P::VmAudit, P::VmConfig, format!("/vms/{}", id))
        }
        "templates" => {
            if arg(2) == "clone" {
                need(P::VmAllocate, "/vms")
            } else {
                read_write(m, P::VmAudit, P::VmAllocate, "/vms")
            }
        }

        "console" => match arg(1) {
            // ticket redemption: the ticket was minted under VM.Console
            "ticket" | "ws" => Requirement::Authenticated,
            "" => need(P::SysAudit, "/"),
            vm => need(P::VmConsole, format!("/vms/{}", vm)),
        },
        "migrate" => {
            let vm = arg(1);
            if arg(2) == "status" || is_read(m) {
                need(P::VmAudit, format!("/vms/{}", vm))
            } else {
                need(P::VmMigrate, format!("/vms/{}", vm))
            }
        }

        "backups" => match (arg(1), arg(2), m) {
            ("", _, "GET") => need(P::DatastoreAudit, "/storage"),
            ("", _, _) => need(P::VmBackup, "/vms"),
            ("retention", _, _) => need(P::DatastoreAllocateSpace, "/storage"),
            (_, "restore", _) => need(P::VmAllocate, "/vms"),
            (_, "verify", _) => need(P::DatastoreAudit, "/storage"),
            (_, _, "GET") => need(P::DatastoreAudit, "/storage"),
            _ => need(P::DatastoreAllocateSpace, "/storage"),
        },
        "backup-jobs" => read_write(m, P::SysAudit, P::SysModify, "/"),
        "backup-storages" => read_write(m, P::DatastoreAudit, P::DatastoreAllocate, "/storage"),

        "audit" => need(P::SysAudit, "/"),

        "ceph" => read_write(m, P::DatastoreAudit, P::DatastoreAllocate, "/storage"),
        "storage" => match (arg(1), arg(2), arg(3)) {
            ("pools", "", _) => {
                if is_read(m) {
                    Requirement::AnyBelow {
                        privilege: P::DatastoreAudit,
                        prefix: "/storage".into(),
                    }
                } else {
                    need(P::DatastoreAllocate, "/storage")
                }
            }
            ("pools", id, "") => read_write(
                m,
                P::DatastoreAudit,
                P::DatastoreAllocate,
                format!("/storage/{}", id),
            ),
            ("pools", id, _) => read_write(
                m,
                P::DatastoreAudit,
                P::DatastoreAllocateSpace,
                format!("/storage/{}", id),
            ),
            _ => read_write(m, P::DatastoreAudit, P::DatastoreAllocate, "/storage"),
        },

        // LXC host capabilities, and the template store (Proxmox pveam):
        // downloaded templates take space on the node's storage.
        "lxc" => match arg(1) {
            "templates" => read_write(m, P::DatastoreAudit, P::DatastoreAllocateSpace, "/storage"),
            _ => read_write(m, P::SysAudit, P::SysModify, "/nodes"),
        },
        // The node shell is a root shell: always Sys.Modify, even for the
        // WebSocket GET that redeems the ticket.
        "nodes" if arg(1) == "shell" => need(P::SysModify, "/nodes"),
        "gpu" | "system" | "nodes" | "host-agent" => {
            read_write(m, P::SysAudit, P::SysModify, "/nodes")
        }
        "tls" => match arg(1) {
            // the node's CA certificate: public, needed before logging in
            "ca.pem" => Requirement::Public,
            // reads arbitrary certificate files: admin only
            "certificate" if arg(2) == "info" => need(P::SysModify, "/system"),
            _ => read_write(m, P::SysAudit, P::SysModify, "/system"),
        },
        "vault" => need(P::SysModify, "/system"),
        // metrics queries are POSTed but only read
        "metrics" => need(P::SysAudit, "/"),
        "observability" | "webhooks" | "alerts" | "notifications" => {
            read_write(m, P::SysAudit, P::SysModify, "/")
        }
        "monitoring" => match (arg(1), arg(2)) {
            ("vms", "") => Requirement::AnyBelow {
                privilege: P::VmAudit,
                prefix: "/vms".into(),
            },
            ("vms", id) => need(P::VmAudit, format!("/vms/{}", id)),
            ("containers", "") => Requirement::AnyBelow {
                privilege: P::VmAudit,
                prefix: "/containers".into(),
            },
            ("containers", id) => need(P::VmAudit, format!("/containers/{}", id)),
            ("storage", _) => Requirement::AnyBelow {
                privilege: P::DatastoreAudit,
                prefix: "/storage".into(),
            },
            // user-defined dashboards: shared, editing them changes what
            // every operator sees
            ("dashboards", _) => read_write(m, P::SysAudit, P::SysModify, "/"),
            _ => need(P::SysAudit, "/nodes"),
        },
        "cluster" => match arg(1) {
            "find-node" => need(P::SysAudit, "/cluster"),
            _ => read_write(m, P::SysAudit, P::SysModify, "/cluster"),
        },
        "ha" => read_write(m, P::SysAudit, P::SysModify, "/cluster"),
        "firewall" | "cni" | "network-policies" | "sdn" | "network" => {
            if arg(1) == "check" {
                need(P::SdnAudit, "/sdn")
            } else {
                read_write(m, P::SdnAudit, P::SdnAllocate, "/sdn")
            }
        }
        "k8s" => {
            // secrets and configmaps can hold credentials
            if s.contains(&"secrets") {
                need(P::SysModify, "/k8s")
            } else {
                read_write(m, P::SysAudit, P::SysModify, "/k8s")
            }
        }
        "nas" => {
            // reading object data out of the S3 gateway is not an audit
            if arg(1) == "s3" && s.contains(&"download") {
                need(P::SysModify, "/nas")
            } else {
                read_write(m, P::SysAudit, P::SysModify, "/nas")
            }
        }
        "ws" => need(P::SysAudit, "/"),
        // task logs can carry package/service output
        "tasks" => read_write(m, P::SysAudit, P::SysModify, "/nodes"),

        // Unknown area: deny by default (administrators only).
        _ => read_write(m, P::SysAudit, P::SysModify, "/"),
    }
}

/// ACL edits need `Permissions.Modify` on the ACL's own path (as in PVE);
/// the handler checks that against the path in the request body.
fn acl_rule(m: &str) -> Requirement {
    if is_read(m) {
        need(P::SysAudit, "/access")
    } else {
        Requirement::Authenticated
    }
}

/// VM / container sub-resources.
fn guest_action(m: &str, base: &str, action: &str) -> Requirement {
    if action.is_empty() {
        return match m {
            "GET" | "HEAD" => need(P::VmAudit, base),
            "DELETE" => need(P::VmAllocate, base),
            _ => need(P::VmConfig, base),
        };
    }
    if POWER_ACTIONS.contains(&action) {
        return need(P::VmPowerMgmt, base);
    }
    match action {
        "snapshots" => read_write(m, P::VmAudit, P::VmSnapshot, base),
        "clone" | "clone-cross-node" => need(P::VmClone, base),
        "migrate" => read_write(m, P::VmAudit, P::VmMigrate, base),
        // console-ticket: a container's interactive system console
        "console" | "vnc" | "spice" | "serial" | "exec" | "exec-ticket" | "console-ticket"
        | "terminal" => need(P::VmConsole, base),
        "logs" | "logs-ticket" | "status" | "stats" | "metrics" | "rrd" => need(P::VmAudit, base),
        "backup" | "backups" => read_write(m, P::VmAudit, P::VmBackup, base),
        _ => read_write(m, P::VmAudit, P::VmConfig, base),
    }
}

/// Built-in roles: (name, description, privileges). Recreated on every start
/// so a new release's privilege lists take effect; they can't be edited.
pub fn builtin_roles() -> Vec<(&'static str, &'static str, Vec<Privilege>)> {
    vec![
        (
            "Administrator",
            "Full access to everything",
            Privilege::ALL.to_vec(),
        ),
        (
            "NoAccess",
            "No privileges (use to revoke access below a grant)",
            vec![],
        ),
        (
            "Auditor",
            "Read-only access to everything",
            vec![
                P::VmAudit,
                P::DatastoreAudit,
                P::PoolAudit,
                P::SysAudit,
                P::SdnAudit,
            ],
        ),
        (
            "VMAdmin",
            "Full control of VMs and containers",
            vec![
                P::VmAudit,
                P::VmConsole,
                P::VmConfig,
                P::VmPowerMgmt,
                P::VmAllocate,
                P::VmMigrate,
                P::VmSnapshot,
                P::VmBackup,
                P::VmClone,
            ],
        ),
        (
            "VMUser",
            "View, console, power and back up VMs and containers",
            vec![P::VmAudit, P::VmConsole, P::VmPowerMgmt, P::VmBackup],
        ),
        (
            "DatastoreAdmin",
            "Manage storage pools and their contents",
            vec![
                P::DatastoreAudit,
                P::DatastoreAllocate,
                P::DatastoreAllocateSpace,
            ],
        ),
        (
            "DatastoreUser",
            "Allocate space on storage pools",
            vec![P::DatastoreAudit, P::DatastoreAllocateSpace],
        ),
        (
            "UserAdmin",
            "Manage user accounts and groups (not permissions)",
            vec![P::UserModify, P::SysAudit],
        ),
        (
            "SysAdmin",
            "Manage host and cluster settings",
            vec![P::SysAudit, P::SysModify, P::SysConsole],
        ),
        (
            "SDNAdmin",
            "Manage networks and firewall",
            vec![P::SdnAudit, P::SdnAllocate],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priv_of(r: Requirement) -> (Privilege, String) {
        match r {
            Requirement::Privilege { privilege, path } => (privilege, path),
            other => panic!("expected a privilege requirement, got {:?}", other),
        }
    }

    #[test]
    fn vm_actions_map_to_pve_privileges() {
        assert_eq!(
            priv_of(requirement("POST", "/api/vms/abc/start")),
            (P::VmPowerMgmt, "/vms/abc".into())
        );
        assert_eq!(
            priv_of(requirement("DELETE", "/api/vms/abc")),
            (P::VmAllocate, "/vms/abc".into())
        );
        assert_eq!(
            priv_of(requirement("POST", "/api/vms/abc/snapshots")),
            (P::VmSnapshot, "/vms/abc".into())
        );
        assert_eq!(
            priv_of(requirement("GET", "/api/vms/abc/snapshots")),
            (P::VmAudit, "/vms/abc".into())
        );
        assert_eq!(
            priv_of(requirement("POST", "/api/console/abc/vnc")),
            (P::VmConsole, "/vms/abc".into())
        );
        assert_eq!(
            priv_of(requirement("POST", "/api/console/abc/spice")),
            (P::VmConsole, "/vms/abc".into())
        );
        // A container's system console is a console, not a config change.
        assert_eq!(
            priv_of(requirement("POST", "/api/containers/ct1/console-ticket")),
            (P::VmConsole, "/containers/ct1".into())
        );
        assert_eq!(
            priv_of(requirement("POST", "/api/vms")),
            (P::VmAllocate, "/vms".into())
        );
        assert!(matches!(
            requirement("GET", "/api/vms"),
            Requirement::AnyBelow {
                privilege: P::VmAudit,
                ..
            }
        ));
    }

    #[test]
    fn self_service_routes() {
        assert_eq!(
            requirement("POST", "/api/auth/logout"),
            Requirement::Authenticated
        );
        assert_eq!(
            requirement("GET", "/api/auth/mfa/status"),
            Requirement::Authenticated
        );
        assert_eq!(requirement("POST", "/api/auth/login"), Requirement::Public);
        assert!(matches!(
            requirement("POST", "/api/users/bob/api-keys"),
            Requirement::SelfOr { .. }
        ));
        assert!(matches!(
            requirement("POST", "/api/users/bob/password"),
            Requirement::SelfOr { .. }
        ));
        // toggling or deleting an account is never self-service
        assert_eq!(
            priv_of(requirement("POST", "/api/users/bob/toggle")),
            (P::UserModify, "/access/users".into())
        );
        assert_eq!(
            priv_of(requirement("DELETE", "/api/users/bob")),
            (P::UserModify, "/access/users".into())
        );
    }

    #[test]
    fn secrets_need_modify_even_to_read() {
        assert_eq!(
            priv_of(requirement("GET", "/api/vault/secret/x")).0,
            P::SysModify
        );
        assert_eq!(
            priv_of(requirement(
                "GET",
                "/api/k8s/clusters/c/namespaces/n/secrets"
            ))
            .0,
            P::SysModify
        );
        assert_eq!(
            priv_of(requirement("GET", "/api/tls/certificate/info/etc")).0,
            P::SysModify
        );
    }

    #[test]
    fn unknown_area_is_admin_only() {
        assert_eq!(
            priv_of(requirement("GET", "/api/brand-new")),
            (P::SysAudit, "/".into())
        );
        assert_eq!(
            priv_of(requirement("POST", "/api/brand-new/x")),
            (P::SysModify, "/".into())
        );
    }

    /// Every `.route("/api/<area>/...")` registered in the server sources must
    /// belong to an area with an explicit rule above. If this fails after you
    /// added routes: add the area to `KNOWN_AREAS` and give it a rule in
    /// `requirement()` (which privilege on which ACL path), instead of letting
    /// it fall through to the administrators-only default.
    #[test]
    fn every_registered_route_is_mapped() {
        // The router and handlers live in horcrux-api: src/app/ (formerly
        // main.rs) and the src/api/ route modules.
        let src_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../horcrux-api/src");
        let mut files = Vec::new();
        let mut dirs = vec![src_dir.join("app"), src_dir.join("api")];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    files.push(p);
                }
            }
        }
        let re = regex::Regex::new(r#"\.route\(\s*"(/[^"]*)""#).unwrap();
        let mut unmapped = Vec::new();
        let mut count = 0;
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            for cap in re.captures_iter(&text) {
                count += 1;
                let path = &cap[1];
                if path == "/metrics" {
                    continue;
                }
                let area = path
                    .strip_prefix("/api/")
                    .and_then(|r| r.split('/').next())
                    .unwrap_or("");
                if !KNOWN_AREAS.contains(&area) {
                    unmapped.push(format!("{} ({})", path, f.display()));
                }
            }
        }
        assert!(
            count > 100,
            "found only {} routes - did the route layout change?",
            count
        );
        assert!(
            unmapped.is_empty(),
            "routes in areas with no RBAC rule (add them to auth/privileges.rs): {:#?}",
            unmapped
        );
    }

    #[test]
    fn builtin_roles_are_sane() {
        let roles = builtin_roles();
        let admin = roles.iter().find(|r| r.0 == "Administrator").unwrap();
        assert_eq!(admin.2.len(), Privilege::ALL.len());
        let user_admin = roles.iter().find(|r| r.0 == "UserAdmin").unwrap();
        // granting permissions is effectively root: not part of UserAdmin
        assert!(!user_admin.2.contains(&P::PermissionsModify));
    }
}
