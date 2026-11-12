//! System accounts -> Horcrux roles.
//!
//! The web UI/API signs in host (PAM) users. What they may do comes from
//! their *system group memberships*, resolved through NSS (`getgrouplist`),
//! so local `/etc/group` and SSSD/FreeIPA/LDAP directories behave the same.
//! A user in none of the mapped groups is denied. root is refused unless
//! the operator allowed it.

use serde::{Deserialize, Serialize};

/// Built-in roles a group can map to (see `privileges::builtin_roles`).
pub const ROLE_ADMIN: &str = "Administrator";
pub const ROLE_OPERATOR: &str = "VMAdmin";
pub const ROLE_READONLY: &str = "Auditor";
/// Roles this mapping manages on `/` (a login re-syncs exactly these).
pub const MANAGED_ROLES: &[&str] = &[ROLE_ADMIN, ROLE_OPERATOR, ROLE_READONLY];

/// `[auth]` settings of `/etc/horcrux/config.toml` that decide who may log in.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupMap {
    /// Members get full admin rights.
    pub admin_groups: Vec<String>,
    /// Members get the `VMAdmin` role (manage guests, no user/ACL admin).
    pub operator_groups: Vec<String>,
    /// Members get the read-only `Auditor` role.
    pub readonly_groups: Vec<String>,
    /// Allow root (uid 0) to log in over the web (off by default).
    pub allow_root: bool,
}

impl Default for GroupMap {
    fn default() -> Self {
        Self {
            admin_groups: vec!["horcrux-admin".into(), "wheel".into()],
            operator_groups: vec![],
            readonly_groups: vec![],
            allow_root: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Grant this built-in role on `/`.
    Role(&'static str),
    DeniedRoot,
    DeniedNoGroup,
}

impl GroupMap {
    /// The role for a user with the given uid (None = unknown) and groups.
    /// The highest matching role wins (admin > operator > read-only).
    pub fn decide(&self, username: &str, uid: Option<u32>, groups: &[String]) -> Decision {
        if (uid == Some(0) || username == "root") && !self.allow_root {
            return Decision::DeniedRoot;
        }
        let has = |wanted: &[String]| {
            wanted
                .iter()
                .filter(|g| !g.is_empty())
                .any(|g| groups.iter().any(|m| m == g))
        };
        if has(&self.admin_groups) {
            Decision::Role(ROLE_ADMIN)
        } else if has(&self.operator_groups) {
            Decision::Role(ROLE_OPERATOR)
        } else if has(&self.readonly_groups) {
            Decision::Role(ROLE_READONLY)
        } else {
            Decision::DeniedNoGroup
        }
    }
}

/// uid and group names of a system user through NSS (None if unknown).
pub fn lookup(username: &str) -> Option<(u32, Vec<String>)> {
    use nix::unistd::{getgrouplist, Group, User};
    let u = User::from_name(username).ok().flatten()?;
    let name = std::ffi::CString::new(username).ok()?;
    let gids = getgrouplist(&name, u.gid).ok()?;
    let groups = gids
        .into_iter()
        .filter_map(|g| Group::from_gid(g).ok().flatten().map(|g| g.name))
        .collect();
    Some((u.uid.as_raw(), groups))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn wheel_and_horcrux_admin_are_admin_by_default() {
        let m = GroupMap::default();
        assert_eq!(
            m.decide("ron", Some(1000), &g(&["ron", "wheel"])),
            Decision::Role(ROLE_ADMIN)
        );
        assert_eq!(
            m.decide("ron", Some(1000), &g(&["horcrux-admin"])),
            Decision::Role(ROLE_ADMIN)
        );
    }

    #[test]
    fn user_outside_every_mapped_group_is_denied() {
        let m = GroupMap::default();
        assert_eq!(
            m.decide("bob", Some(1001), &g(&["bob", "users"])),
            Decision::DeniedNoGroup
        );
        assert_eq!(m.decide("bob", Some(1001), &[]), Decision::DeniedNoGroup);
    }

    #[test]
    fn root_is_denied_by_default_even_in_wheel() {
        let m = GroupMap::default();
        assert_eq!(
            m.decide("root", Some(0), &g(&["root", "wheel"])),
            Decision::DeniedRoot
        );
        // a renamed uid-0 account is root too
        assert_eq!(
            m.decide("toor", Some(0), &g(&["wheel"])),
            Decision::DeniedRoot
        );
    }

    #[test]
    fn root_can_be_allowed() {
        let m = GroupMap {
            allow_root: true,
            ..GroupMap::default()
        };
        assert_eq!(
            m.decide("root", Some(0), &g(&["root", "wheel"])),
            Decision::Role(ROLE_ADMIN)
        );
    }

    #[test]
    fn operator_and_readonly_groups_map_to_roles_highest_wins() {
        let m = GroupMap {
            operator_groups: g(&["hx-ops"]),
            readonly_groups: g(&["hx-view"]),
            ..GroupMap::default()
        };
        assert_eq!(
            m.decide("a", Some(1), &g(&["hx-ops"])),
            Decision::Role(ROLE_OPERATOR)
        );
        assert_eq!(
            m.decide("a", Some(1), &g(&["hx-view"])),
            Decision::Role(ROLE_READONLY)
        );
        assert_eq!(
            m.decide("a", Some(1), &g(&["hx-view", "hx-ops"])),
            Decision::Role(ROLE_OPERATOR)
        );
        assert_eq!(
            m.decide("a", Some(1), &g(&["hx-view", "wheel"])),
            Decision::Role(ROLE_ADMIN)
        );
    }

    #[test]
    fn empty_group_names_never_match() {
        let m = GroupMap {
            admin_groups: g(&[""]),
            ..GroupMap::default()
        };
        assert_eq!(m.decide("a", Some(1), &g(&[""])), Decision::DeniedNoGroup);
    }

    #[test]
    fn lookup_finds_root_and_not_nonsense() {
        let (uid, _) = lookup("root").expect("root exists");
        assert_eq!(uid, 0);
        assert!(lookup("no-such-user-hxpam").is_none());
    }
}
