//! Permission evaluation (Proxmox VE ACL model).
//!
//! An ACL entry grants a role to a user or group on a path such as
//! `/vms/<id>`, `/storage` or `/`. With `propagate` it also applies to every
//! path below. To find a user's privileges on a path we walk from the path up
//! to `/` and stop at the *most specific* level that has any entry applying
//! to the user; at that level, entries for the user themself win over group
//! entries. That level's roles (unioned) are the user's privileges there, so
//! a `NoAccess` grant on `/vms/100` really does revoke an `Administrator`
//! grant on `/` for that one VM. This mirrors PVE's semantics.

use horcrux_common::auth::Privilege;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

/// One ACL line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AclEntry {
    #[serde(default)]
    pub id: i64,
    pub path: String,
    /// `user` or `group`.
    pub subject_type: String,
    /// User id (UUID) or group name.
    pub subject: String,
    pub role: String,
    #[serde(default = "default_true")]
    pub propagate: bool,
}

fn default_true() -> bool {
    true
}

/// Normalize an ACL / resource path: leading `/`, no trailing `/`, no empty
/// segments. `""` and `"/"` both become `/`.
pub fn normalize_path(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segs.join("/"))
    }
}

/// `path` and each of its ancestors, most specific first, ending with `/`.
fn path_chain(path: &str) -> Vec<String> {
    let norm = normalize_path(path);
    let mut out = vec![norm.clone()];
    let mut cur = norm;
    while cur != "/" {
        cur = match cur.rfind('/') {
            Some(0) | None => "/".to_string(),
            Some(i) => cur[..i].to_string(),
        };
        out.push(cur.clone());
    }
    out
}

/// The privileges `user_id` (member of `groups`) holds on `path`.
pub fn effective_privileges(
    acl: &[AclEntry],
    roles: &HashMap<String, Vec<Privilege>>,
    user_id: &str,
    groups: &[String],
    path: &str,
) -> BTreeSet<Privilege> {
    let chain = path_chain(path);
    for (depth, level) in chain.iter().enumerate() {
        let exact = depth == 0;
        let applies = |e: &&AclEntry| normalize_path(&e.path) == *level && (exact || e.propagate);
        let user_entries: Vec<&AclEntry> = acl
            .iter()
            .filter(applies)
            .filter(|e| e.subject_type == "user" && e.subject == user_id)
            .collect();
        let entries = if !user_entries.is_empty() {
            user_entries
        } else {
            acl.iter()
                .filter(applies)
                .filter(|e| e.subject_type == "group" && groups.contains(&e.subject))
                .collect()
        };
        if entries.is_empty() {
            continue;
        }
        let mut privs = BTreeSet::new();
        for e in entries {
            if let Some(p) = roles.get(&e.role) {
                privs.extend(p.iter().copied());
            }
        }
        return privs;
    }
    BTreeSet::new()
}

/// Whether the user holds any privilege anywhere at or below `prefix`
/// (used to decide e.g. whether a list endpoint may be shown at all).
pub fn has_privilege_below(
    acl: &[AclEntry],
    roles: &HashMap<String, Vec<Privilege>>,
    user_id: &str,
    groups: &[String],
    prefix: &str,
    privilege: Privilege,
) -> bool {
    if effective_privileges(acl, roles, user_id, groups, prefix).contains(&privilege) {
        return true;
    }
    let prefix = normalize_path(prefix);
    let below = |p: &str| {
        let p = normalize_path(p);
        prefix == "/" || p.starts_with(&format!("{}/", prefix))
    };
    acl.iter()
        .filter(|e| below(&e.path))
        .filter(|e| match e.subject_type.as_str() {
            "user" => e.subject == user_id,
            "group" => groups.contains(&e.subject),
            _ => false,
        })
        .any(|e| effective_privileges(acl, roles, user_id, groups, &e.path).contains(&privilege))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles() -> HashMap<String, Vec<Privilege>> {
        let mut r = HashMap::new();
        r.insert("Administrator".into(), Privilege::ALL.to_vec());
        r.insert("NoAccess".into(), vec![]);
        r.insert(
            "VMUser".into(),
            vec![
                Privilege::VmAudit,
                Privilege::VmPowerMgmt,
                Privilege::VmConsole,
            ],
        );
        r.insert(
            "Auditor".into(),
            vec![Privilege::VmAudit, Privilege::SysAudit],
        );
        r
    }

    fn e(path: &str, st: &str, subj: &str, role: &str, propagate: bool) -> AclEntry {
        AclEntry {
            id: 0,
            path: path.into(),
            subject_type: st.into(),
            subject: subj.into(),
            role: role.into(),
            propagate,
        }
    }

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path("vms//100/"), "/vms/100");
        assert_eq!(
            path_chain("/vms/100"),
            vec!["/vms/100".to_string(), "/vms".to_string(), "/".to_string()]
        );
    }

    #[test]
    fn no_acl_means_no_privileges() {
        let p = effective_privileges(&[], &roles(), "u1", &[], "/vms/1");
        assert!(p.is_empty());
    }

    #[test]
    fn root_admin_propagates_everywhere() {
        let acl = vec![e("/", "user", "u1", "Administrator", true)];
        let p = effective_privileges(&acl, &roles(), "u1", &[], "/vms/1");
        assert!(p.contains(&Privilege::SysModify));
        assert!(p.contains(&Privilege::VmAllocate));
        // someone else gets nothing
        assert!(effective_privileges(&acl, &roles(), "u2", &[], "/vms/1").is_empty());
    }

    #[test]
    fn non_propagating_entry_only_applies_to_its_own_path() {
        let acl = vec![e("/vms", "user", "u1", "VMUser", false)];
        assert!(
            effective_privileges(&acl, &roles(), "u1", &[], "/vms").contains(&Privilege::VmAudit)
        );
        assert!(effective_privileges(&acl, &roles(), "u1", &[], "/vms/7").is_empty());
    }

    #[test]
    fn deeper_entry_overrides_and_noaccess_revokes() {
        let acl = vec![
            e("/", "user", "u1", "Administrator", true),
            e("/vms/secret", "user", "u1", "NoAccess", true),
        ];
        assert!(effective_privileges(&acl, &roles(), "u1", &[], "/vms/secret").is_empty());
        assert!(
            effective_privileges(&acl, &roles(), "u1", &[], "/vms/other")
                .contains(&Privilege::VmAllocate)
        );
    }

    #[test]
    fn user_entry_beats_group_entry_at_same_level() {
        let acl = vec![
            e("/vms", "group", "ops", "Administrator", true),
            e("/vms", "user", "u1", "Auditor", true),
        ];
        let p = effective_privileges(&acl, &roles(), "u1", &["ops".into()], "/vms/1");
        assert!(p.contains(&Privilege::VmAudit));
        assert!(!p.contains(&Privilege::VmAllocate));
        // a group member without a user entry gets the group's role
        let p2 = effective_privileges(&acl, &roles(), "u2", &["ops".into()], "/vms/1");
        assert!(p2.contains(&Privilege::VmAllocate));
    }

    #[test]
    fn privilege_below_prefix() {
        let acl = vec![e("/vms/42", "user", "u1", "VMUser", true)];
        assert!(has_privilege_below(
            &acl,
            &roles(),
            "u1",
            &[],
            "/vms",
            Privilege::VmAudit
        ));
        assert!(!has_privilege_below(
            &acl,
            &roles(),
            "u1",
            &[],
            "/storage",
            Privilege::VmAudit
        ));
        assert!(!has_privilege_below(
            &acl,
            &roles(),
            "u1",
            &[],
            "/vms",
            Privilege::VmAllocate
        ));
    }
}
