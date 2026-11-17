//! Access control: roles, users, groups and ACL entries.
//!
//! ```yaml
//! roles:                                   # custom roles only (built-ins are never listed)
//!   - {name: Operator, description: Run VMs, privileges: [VM.PowerMgmt, VM.Audit]}
//! users:
//!   - username: alice                      # `name@realm` for external realms
//!     email: alice@example.org
//!     enabled: true
//!     password_hash: "$argon2id$..."       # a hash, or
//!     password_ref: {file: /etc/horcrux/secrets/alice}   # a reference: never a plaintext password
//! groups:
//!   - {name: ops, members: [alice]}        # members absent = membership left alone
//! acl:
//!   - {path: /vms, subject_type: group, subject: ops, role: Operator, propagate: true}
//! ```
//!
//! Safety (all enforced in the plan, and again by the managers):
//! - applying these sections needs `Permissions.Modify` on `/` (more than
//!   the `Sys.Modify` that apply itself needs, or a Sys.Modify holder could
//!   promote itself);
//! - the calling user is never deleted or disabled and never has their own
//!   password changed here;
//! - a plan that would leave no enabled administrator is refused;
//! - export omits password hashes unless asked (`include_hashes`).
use super::ctx::Ctx;
use super::secret::SecretRef;
use super::{ch, items, to_json, Kind, KindPlan};
use crate::declarative::{Action, Change, Live, Spec};
use crate::AppState;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The four access sections of a spec (each `None` = not managed).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct AccessSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<RoleSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub users: Option<Vec<UserSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<GroupSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acl: Option<Vec<AclSpec>>,
}

fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RoleSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub privileges: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UserSpec {
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub tfa_required: bool,
    /// An argon2 PHC hash (`$argon2id$...`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_ref: Option<SecretRef>,
}

impl UserSpec {
    pub fn external(&self) -> bool {
        self.username.contains('@')
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GroupSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Usernames. Absent = membership is not managed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<String>>,
}

fn user_subject() -> String {
    "user".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AclSpec {
    pub path: String,
    #[serde(default = "user_subject")]
    pub subject_type: String,
    /// A username, or a group name.
    pub subject: String,
    pub role: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub propagate: bool,
}

impl AclSpec {
    fn key(&self) -> AclKey {
        (
            norm_path(&self.path),
            self.subject_type.clone(),
            self.subject.clone(),
            self.role.clone(),
        )
    }
}

type AclKey = (String, String, String, String);

/// Same normalisation as `auth::acl::normalize_path`: leading `/`, no
/// trailing `/`, no empty segments.
pub fn norm_path(p: &str) -> String {
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    format!("/{}", parts.join("/"))
}

// ---------------------------------------------------------------- live

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallerInfo {
    pub user_id: String,
    pub username: String,
    /// Holds Permissions.Modify on `/`.
    pub is_admin: bool,
}

#[derive(Debug, Clone, Default)]
pub struct LiveUser {
    pub id: String,
    pub username: String,
    pub email: String,
    pub comment: Option<String>,
    pub enabled: bool,
    pub tfa_required: bool,
    pub password_hash: String,
}

#[derive(Debug, Clone, Default)]
pub struct LiveRole {
    pub name: String,
    pub description: String,
    pub privileges: Vec<String>,
    pub builtin: bool,
}

#[derive(Debug, Clone, Default)]
pub struct LiveGroup {
    pub name: String,
    pub comment: Option<String>,
    /// Usernames.
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveAcl {
    pub path: String,
    pub subject_type: String,
    /// Username (for `user`) or group name.
    pub subject: String,
    pub role: String,
    pub propagate: bool,
}

#[derive(Debug, Clone, Default)]
pub struct LiveAccess {
    pub users: Vec<LiveUser>,
    pub roles: Vec<LiveRole>,
    pub groups: Vec<LiveGroup>,
    pub acl: Vec<LiveAcl>,
    /// username -> does the live hash match the referenced secret? Filled
    /// by the executor for users whose spec has a `password_ref`.
    pub pw_checks: BTreeMap<String, bool>,
    /// username -> why the referenced secret could not be read.
    pub pw_errors: BTreeMap<String, String>,
}

// ---------------------------------------------------------------- validate

pub fn valid_username(name: &str) -> bool {
    let (base, realm) = match name.split_once('@') {
        Some((b, r)) => (b, Some(r)),
        None => (name, None),
    };
    let ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    };
    ok(base) && realm.map(ok).unwrap_or(true)
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

pub fn validate(k: &AccessSpec) -> Result<(), String> {
    if let Some(roles) = &k.roles {
        let mut seen = BTreeSet::new();
        for r in roles {
            if !valid_name(&r.name) {
                return Err(format!(
                    "role {:?}: names are letters, digits, _ - . only",
                    r.name
                ));
            }
            if !seen.insert(&r.name) {
                return Err(format!("role {:?} is listed twice", r.name));
            }
            for p in &r.privileges {
                if horcrux_common::auth::Privilege::parse(p).is_none() {
                    return Err(format!("role {}: unknown privilege {p:?}", r.name));
                }
            }
        }
    }
    if let Some(users) = &k.users {
        let mut seen = BTreeSet::new();
        for u in users {
            if !valid_username(&u.username) {
                return Err(format!(
                    "user {:?}: 1-64 letters, digits, _ - . (optionally @realm)",
                    u.username
                ));
            }
            if !seen.insert(&u.username) {
                return Err(format!("user {:?} is listed twice", u.username));
            }
            if u.password_hash.is_some() && u.password_ref.is_some() {
                return Err(format!(
                    "user {}: give password_hash or password_ref, not both",
                    u.username
                ));
            }
            if let Some(h) = &u.password_hash {
                if !h.starts_with("$argon2") {
                    return Err(format!(
                        "user {}: password_hash must be an argon2 hash ($argon2id$...); \
                         plaintext passwords are never accepted in a spec",
                        u.username
                    ));
                }
            }
            if let Some(r) = &u.password_ref {
                r.validate()
                    .map_err(|e| format!("user {}: password_ref: {e}", u.username))?;
            }
            if u.external() && (u.password_hash.is_some() || u.password_ref.is_some()) {
                return Err(format!(
                    "user {}: authenticates through its realm; it has no password here",
                    u.username
                ));
            }
        }
    }
    if let Some(groups) = &k.groups {
        let mut seen = BTreeSet::new();
        for g in groups {
            if !valid_name(&g.name) {
                return Err(format!(
                    "group {:?}: names are letters, digits, _ - . only",
                    g.name
                ));
            }
            if !seen.insert(&g.name) {
                return Err(format!("group {:?} is listed twice", g.name));
            }
        }
    }
    if let Some(acl) = &k.acl {
        let mut seen = BTreeSet::new();
        for a in acl {
            if a.subject_type != "user" && a.subject_type != "group" {
                return Err(format!(
                    "acl {} {}: subject_type must be user or group",
                    a.path, a.subject
                ));
            }
            if !a.path.starts_with('/') {
                return Err(format!("acl path {:?} must start with /", a.path));
            }
            if !seen.insert(a.key()) {
                return Err(format!(
                    "acl entry {} {} {} is listed twice",
                    a.path, a.subject, a.role
                ));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- plan

/// The access state after the plan ran (used for reference checks and the
/// "last administrator" rule).
#[derive(Debug, Default)]
struct Final {
    users: BTreeMap<String, bool>, // username -> enabled
    roles: BTreeMap<String, Vec<String>>,
    groups: BTreeMap<String, BTreeSet<String>>,
    acl: BTreeSet<AclKey>,
}

fn final_state(want: &AccessSpec, live: &LiveAccess, prune: bool) -> Final {
    let mut f = Final::default();
    let users_pruned = prune && want.users.is_some();
    for u in &live.users {
        let listed = want
            .users
            .iter()
            .flatten()
            .any(|w| w.username == u.username);
        if !users_pruned || listed {
            f.users.insert(u.username.clone(), u.enabled);
        }
    }
    for w in want.users.iter().flatten() {
        let enabled = w.enabled;
        f.users.insert(w.username.clone(), enabled);
    }
    let roles_pruned = prune && want.roles.is_some();
    for r in &live.roles {
        let listed = want.roles.iter().flatten().any(|w| w.name == r.name);
        if r.builtin || !roles_pruned || listed {
            f.roles.insert(r.name.clone(), r.privileges.clone());
        }
    }
    for w in want.roles.iter().flatten() {
        f.roles.insert(w.name.clone(), canon_privs(&w.privileges));
    }
    let groups_pruned = prune && want.groups.is_some();
    for g in &live.groups {
        let listed = want.groups.iter().flatten().any(|w| w.name == g.name);
        if !groups_pruned || listed {
            f.groups
                .insert(g.name.clone(), g.members.iter().cloned().collect());
        }
    }
    for w in want.groups.iter().flatten() {
        let e = f.groups.entry(w.name.clone()).or_default();
        if let Some(m) = &w.members {
            *e = m.iter().cloned().collect();
        }
    }
    // members that no longer exist drop out (the DB cascades)
    for m in f.groups.values_mut() {
        m.retain(|u| f.users.contains_key(u));
    }
    let acl_pruned = prune && want.acl.is_some();
    for a in &live.acl {
        let key = (
            norm_path(&a.path),
            a.subject_type.clone(),
            a.subject.clone(),
            a.role.clone(),
        );
        let listed = want.acl.iter().flatten().any(|w| w.key() == key);
        if !acl_pruned || listed {
            f.acl.insert(key);
        }
    }
    for w in want.acl.iter().flatten() {
        f.acl.insert(w.key());
    }
    // grants of vanished principals / roles vanish with them
    f.acl.retain(|(_, st, subj, role)| {
        f.roles.contains_key(role)
            && if st == "user" {
                f.users.contains_key(subj)
            } else {
                f.groups.contains_key(subj)
            }
    });
    f
}

fn admins(f: &Final) -> BTreeSet<String> {
    let admin_role = |r: &str| {
        f.roles
            .get(r)
            .map(|p| p.iter().any(|x| x == "Permissions.Modify"))
            .unwrap_or(false)
    };
    let mut out = BTreeSet::new();
    for (path, st, subj, role) in &f.acl {
        if path != "/" || !admin_role(role) {
            continue;
        }
        if st == "user" {
            if f.users.get(subj) == Some(&true) {
                out.insert(subj.clone());
            }
        } else if let Some(m) = f.groups.get(subj) {
            for u in m {
                if f.users.get(u) == Some(&true) {
                    out.insert(u.clone());
                }
            }
        }
    }
    out
}

fn admins_live(live: &LiveAccess) -> BTreeSet<String> {
    let f = final_state(&AccessSpec::default(), live, false);
    admins(&f)
}

fn set(v: &[String]) -> BTreeSet<&str> {
    v.iter().map(String::as_str).collect()
}

/// `VmAudit` and `VM.Audit` both mean the same privilege.
fn canon_privs(v: &[String]) -> Vec<String> {
    v.iter()
        .map(|p| {
            horcrux_common::auth::Privilege::parse(p)
                .map(|x| x.name().to_string())
                .unwrap_or_else(|| p.clone())
        })
        .collect()
}

pub fn plan(want: &AccessSpec, live: Option<&LiveAccess>, prune: bool, ctx: &Ctx) -> KindPlan {
    let mut p = KindPlan::default();
    if want.roles.is_none() && want.users.is_none() && want.groups.is_none() && want.acl.is_none() {
        return p;
    }
    let Some(live) = live else {
        p.errors
            .push("users/groups/roles/acl: this node cannot manage access control".into());
        return p;
    };
    let caller = ctx.caller.as_ref();
    let is_caller = |name: &str| caller.map(|c| c.username == name).unwrap_or(false);

    // ---- roles
    if let Some(roles) = &want.roles {
        for w in roles {
            match live.roles.iter().find(|l| l.name == w.name) {
                None => p.changes.push(ch(
                    "role",
                    &w.name,
                    Action::Create,
                    vec![format!("{} privileges", w.privileges.len())],
                )),
                Some(l) if l.builtin => p.errors.push(format!(
                    "role {}: it is a built-in role and can't be changed",
                    w.name
                )),
                Some(l) => {
                    let mut d = Vec::new();
                    if l.description != w.description {
                        d.push(format!(
                            "description: {:?} -> {:?}",
                            l.description, w.description
                        ));
                    }
                    let wp = canon_privs(&w.privileges);
                    let (a, b) = (set(&l.privileges), set(&wp));
                    if a != b {
                        let add: Vec<_> = b.difference(&a).collect();
                        let rm: Vec<_> = a.difference(&b).collect();
                        d.push(format!("privileges: +{add:?} -{rm:?}"));
                    }
                    if !d.is_empty() {
                        p.changes.push(ch("role", &w.name, Action::Update, d));
                    }
                }
            }
        }
        if prune {
            for l in live
                .roles
                .iter()
                .filter(|l| !l.builtin && !roles.iter().any(|w| w.name == l.name))
            {
                p.deletes.push(ch(
                    "role",
                    &l.name,
                    Action::Delete,
                    vec!["its grants go with it".into()],
                ));
            }
        }
    }

    // ---- users
    if let Some(users) = &want.users {
        for w in users {
            if let Some(e) = live.pw_errors.get(&w.username) {
                p.errors
                    .push(format!("user {}: password_ref: {e}", w.username));
            }
            match live.users.iter().find(|l| l.username == w.username) {
                None => {
                    if !w.external() && w.password_hash.is_none() && w.password_ref.is_none() {
                        p.errors.push(format!(
                            "user {}: a new local user needs password_hash or password_ref",
                            w.username
                        ));
                    }
                    p.changes
                        .push(ch("user", &w.username, Action::Create, vec![]));
                }
                Some(l) => {
                    let mut d = Vec::new();
                    let mut blocked = Vec::new();
                    if let Some(e) = &w.email {
                        if &l.email != e {
                            d.push(format!("email: {:?} -> {:?}", l.email, e));
                        }
                    }
                    if let Some(c) = &w.comment {
                        if l.comment.as_deref().unwrap_or("") != c {
                            d.push(format!("comment: {:?} -> {:?}", l.comment, c));
                        }
                    }
                    if l.enabled != w.enabled {
                        d.push(format!("enabled: {} -> {}", l.enabled, w.enabled));
                        if !w.enabled && is_caller(&w.username) {
                            blocked.push("you can't disable your own account".to_string());
                        }
                    }
                    if l.tfa_required != w.tfa_required {
                        d.push(format!(
                            "tfa_required: {} -> {}",
                            l.tfa_required, w.tfa_required
                        ));
                    }
                    let pw_differs = match (&w.password_hash, &w.password_ref) {
                        (Some(h), _) => h != &l.password_hash,
                        (None, Some(_)) => live.pw_checks.get(&w.username) == Some(&false),
                        _ => false,
                    };
                    if pw_differs {
                        d.push("password: differs from the spec".into());
                        if is_caller(&w.username) {
                            blocked.push(
                                "you can't change your own password through apply: use the \
                                 password form (it asks for the old one)"
                                    .to_string(),
                            );
                        }
                    }
                    if !blocked.is_empty() {
                        d.extend(blocked);
                        p.changes.push(ch("user", &w.username, Action::Blocked, d));
                    } else if !d.is_empty() {
                        p.changes.push(ch("user", &w.username, Action::Update, d));
                    }
                }
            }
        }
        if prune {
            for l in live
                .users
                .iter()
                .filter(|l| !users.iter().any(|w| w.username == l.username))
            {
                if is_caller(&l.username) {
                    p.deletes.push(ch(
                        "user",
                        &l.username,
                        Action::Blocked,
                        vec!["prune: you can't delete your own account".into()],
                    ));
                } else {
                    p.deletes
                        .push(ch("user", &l.username, Action::Delete, vec![]));
                }
            }
        }
    }

    // ---- groups
    if let Some(groups) = &want.groups {
        for w in groups {
            match live.groups.iter().find(|l| l.name == w.name) {
                None => p.changes.push(ch(
                    "group",
                    &w.name,
                    Action::Create,
                    vec![format!(
                        "{} members",
                        w.members.as_ref().map(Vec::len).unwrap_or(0)
                    )],
                )),
                Some(l) => {
                    let mut d = Vec::new();
                    if let Some(c) = &w.comment {
                        if l.comment.as_deref().unwrap_or("") != c {
                            d.push(format!("comment: {:?} -> {:?}", l.comment, c));
                        }
                    }
                    if let Some(m) = &w.members {
                        let (a, b) = (set(&l.members), set(m));
                        if a != b {
                            let add: Vec<_> = b.difference(&a).collect();
                            let rm: Vec<_> = a.difference(&b).collect();
                            d.push(format!("members: +{add:?} -{rm:?}"));
                        }
                    }
                    if !d.is_empty() {
                        p.changes.push(ch("group", &w.name, Action::Update, d));
                    }
                }
            }
        }
        if prune {
            for l in live
                .groups
                .iter()
                .filter(|l| !groups.iter().any(|w| w.name == l.name))
            {
                p.deletes.push(ch(
                    "group",
                    &l.name,
                    Action::Delete,
                    vec!["its grants go with it".into()],
                ));
            }
        }
    }

    // ---- acl
    if let Some(acl) = &want.acl {
        for w in acl {
            let key = w.key();
            let name = format!("{} {} {} {}", key.0, key.1, key.2, key.3);
            let found = live.acl.iter().find(|l| {
                (
                    norm_path(&l.path),
                    l.subject_type.clone(),
                    l.subject.clone(),
                    l.role.clone(),
                ) == key
            });
            match found {
                None => p.changes.push(ch("acl", &name, Action::Create, vec![])),
                Some(l) if l.propagate != w.propagate => p.changes.push(ch(
                    "acl",
                    &name,
                    Action::Update,
                    vec![format!("propagate: {} -> {}", l.propagate, w.propagate)],
                )),
                Some(_) => {}
            }
        }
        if prune {
            for l in &live.acl {
                let key = (
                    norm_path(&l.path),
                    l.subject_type.clone(),
                    l.subject.clone(),
                    l.role.clone(),
                );
                if !acl.iter().any(|w| w.key() == key) {
                    let name = format!("{} {} {} {}", key.0, key.1, key.2, key.3);
                    p.deletes.push(ch("acl", &name, Action::Delete, vec![]));
                }
            }
        }
    }

    // ---- references and safety, on the resulting state
    let fin = final_state(want, live, prune);
    for w in want.groups.iter().flatten() {
        for m in w.members.iter().flatten() {
            if !fin.users.contains_key(m) {
                p.errors
                    .push(format!("group {}: member {m} is not a user", w.name));
            }
        }
    }
    for w in want.acl.iter().flatten() {
        let (path, st, subj, role) = w.key();
        if !fin.roles.contains_key(&role) {
            p.errors
                .push(format!("acl {path} {subj}: role {role} does not exist"));
        }
        let known = if st == "user" {
            fin.users.contains_key(&subj)
        } else {
            fin.groups.contains_key(&subj)
        };
        if !known {
            p.errors
                .push(format!("acl {path}: {st} {subj} does not exist"));
        }
    }
    let before = admins_live(live);
    let after = admins(&fin);
    if !before.is_empty() && after.is_empty() {
        p.errors.push(
            "refusing: this would leave no enabled administrator (Permissions.Modify on /)".into(),
        );
    }
    if let Some(c) = caller {
        if !c.is_admin && (!p.changes.is_empty() || !p.deletes.is_empty()) {
            p.errors
                .push("changing users, groups, roles or ACL needs Permissions.Modify on /".into());
        }
    }
    p
}

// ---------------------------------------------------------------- export

pub fn export_roles(l: &LiveAccess) -> Vec<RoleSpec> {
    let mut v: Vec<RoleSpec> = l
        .roles
        .iter()
        .filter(|r| !r.builtin)
        .map(|r| {
            let mut privileges = r.privileges.clone();
            privileges.sort();
            RoleSpec {
                name: r.name.clone(),
                description: r.description.clone(),
                privileges,
            }
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

pub fn export_users(l: &LiveAccess, include_hashes: bool) -> Vec<UserSpec> {
    let mut v: Vec<UserSpec> = l
        .users
        .iter()
        .map(|u| UserSpec {
            username: u.username.clone(),
            email: (!u.email.is_empty()).then(|| u.email.clone()),
            comment: u.comment.clone().filter(|c| !c.is_empty()),
            enabled: u.enabled,
            tfa_required: u.tfa_required,
            password_hash: (include_hashes
                && !u.username.contains('@')
                && u.password_hash.starts_with("$argon2"))
            .then(|| u.password_hash.clone()),
            password_ref: None,
        })
        .collect();
    v.sort_by(|a, b| a.username.cmp(&b.username));
    v
}

pub fn export_groups(l: &LiveAccess) -> Vec<GroupSpec> {
    let mut v: Vec<GroupSpec> = l
        .groups
        .iter()
        .map(|g| {
            let mut m = g.members.clone();
            m.sort();
            GroupSpec {
                name: g.name.clone(),
                comment: g.comment.clone().filter(|c| !c.is_empty()),
                members: Some(m),
            }
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

pub fn export_acl(l: &LiveAccess) -> Vec<AclSpec> {
    let mut v: Vec<AclSpec> = l
        .acl
        .iter()
        .map(|a| AclSpec {
            path: norm_path(&a.path),
            subject_type: a.subject_type.clone(),
            subject: a.subject.clone(),
            role: a.role.clone(),
            propagate: a.propagate,
        })
        .collect();
    v.sort_by_key(|a| a.key());
    v
}

// ------------------------------------------------------------------ kinds

fn spec_from(whole: &Spec) -> Result<AccessSpec, String> {
    let sec = |k: &str| whole.extra.get(k);
    Ok(AccessSpec {
        roles: sec("roles").map(items::<RoleSpec>).transpose()?,
        users: sec("users").map(items::<UserSpec>).transpose()?,
        groups: sec("groups").map(items::<GroupSpec>).transpose()?,
        acl: sec("acl").map(items::<AclSpec>).transpose()?,
    })
}

/// The live access state the pure plan wants, from the exported sections,
/// the private context and (for `password_ref` users) the referenced secret.
/// Files and environment variables are read here, synchronously; a vault
/// reference is only checked at apply time (async), never at plan time.
fn live_access(live: &Live, want: &AccessSpec) -> LiveAccess {
    let ctx = &live.kinds;
    let section = |k: &str| live.extra.get(k).cloned().unwrap_or(Value::Null);
    let mut a = LiveAccess::default();
    let users: Vec<UserSpec> = items(&section("users")).unwrap_or_default();
    for (name, p) in &ctx.users {
        let u = users.iter().find(|u| &u.username == name);
        a.users.push(LiveUser {
            id: p.id.clone(),
            username: name.clone(),
            email: u.and_then(|u| u.email.clone()).unwrap_or_default(),
            comment: u.and_then(|u| u.comment.clone()),
            enabled: u.map(|u| u.enabled).unwrap_or(true),
            tfa_required: u.map(|u| u.tfa_required).unwrap_or(false),
            password_hash: p.hash.clone(),
        });
    }
    for r in items::<RoleSpec>(&section("roles")).unwrap_or_default() {
        a.roles.push(LiveRole {
            name: r.name,
            description: r.description,
            privileges: r.privileges,
            builtin: false,
        });
    }
    for (name, privs) in &ctx.builtin_roles {
        a.roles.push(LiveRole {
            name: name.clone(),
            description: String::new(),
            privileges: privs.clone(),
            builtin: true,
        });
    }
    for g in items::<GroupSpec>(&section("groups")).unwrap_or_default() {
        a.groups.push(LiveGroup {
            name: g.name,
            comment: g.comment,
            members: g.members.unwrap_or_default(),
        });
    }
    for e in items::<AclSpec>(&section("acl")).unwrap_or_default() {
        a.acl.push(LiveAcl {
            path: e.path,
            subject_type: e.subject_type,
            subject: e.subject,
            role: e.role,
            propagate: e.propagate,
        });
    }
    for u in want.users.iter().flatten() {
        let Some(r) = &u.password_ref else { continue };
        match r.resolve_sync() {
            Err(e) => {
                a.pw_errors.insert(u.username.clone(), e);
            }
            Ok(None) => {}
            Ok(Some(plain)) => {
                if let Some(l) = a.users.iter().find(|l| l.username == u.username) {
                    let ok = crate::auth::password::verify_password(&plain, &l.password_hash)
                        .unwrap_or(false);
                    a.pw_checks.insert(u.username.clone(), ok);
                }
            }
        }
    }
    a
}

const ACCESS_KEYS: [&str; 4] = ["users", "groups", "roles", "acl"];

/// One pure plan serves the four kinds (references and the administrator
/// rule cross the sections): each keeps its own changes, and the first
/// section the spec lists reports the errors once.
fn plan_access(
    key: &'static str,
    mine: &[&str],
    prune: bool,
    whole: (&Spec, &Live),
    out: &mut KindPlan,
) {
    let want = match spec_from(whole.0) {
        Ok(w) => w,
        Err(e) => {
            out.errors.push(format!("{key}: {e}"));
            return;
        }
    };
    let live = live_access(whole.1, &want);
    let mut kp = plan(&want, Some(&live), prune, &whole.1.kinds);
    kp.changes.retain(|c| mine.contains(&c.kind.as_str()));
    kp.deletes.retain(|c| mine.contains(&c.kind.as_str()));
    let owner = ACCESS_KEYS
        .iter()
        .find(|k| whole.0.extra.contains_key(**k))
        .copied();
    if owner != Some(key) {
        kp.errors.clear();
    }
    out.merge(kp);
}

/// Users, groups, roles and ACL are for administrators.
async fn gate(state: &Arc<AppState>) -> Result<(), String> {
    match super::ctx::caller_is_admin(state).await {
        Some(false) => Err("needs Permissions.Modify on /".into()),
        _ => Ok(()),
    }
}

async fn gather_access(state: &Arc<AppState>, with_hashes: bool) -> Result<LiveAccess, String> {
    use crate::auth::store;
    gate(state).await?;
    let e = |x: horcrux_common::Error| format!("{x}");
    let pool = state.database.pool();
    let recs = store::list_user_records(pool).await.map_err(e)?;
    let mut users = Vec::new();
    for r in &recs {
        let hash = if with_hashes {
            store::find_user(pool, &r.id)
                .await
                .ok()
                .flatten()
                .map(|u| u.password_hash)
                .unwrap_or_default()
        } else {
            String::new()
        };
        users.push(LiveUser {
            id: r.id.clone(),
            username: r.username.clone(),
            email: r.email.clone(),
            comment: r.comment.clone(),
            enabled: r.enabled,
            tfa_required: r.tfa_required,
            password_hash: hash,
        });
    }
    let roles = store::list_roles(pool)
        .await
        .map_err(e)?
        .into_iter()
        .map(|r| LiveRole {
            name: r.name,
            description: r.description,
            privileges: r.privileges.iter().map(|p| p.name().to_string()).collect(),
            builtin: r.builtin,
        })
        .collect();
    let groups = store::list_groups(pool)
        .await
        .map_err(e)?
        .into_iter()
        .map(|g| LiveGroup {
            name: g.name,
            comment: g.comment,
            members: g.members,
        })
        .collect();
    let acl = store::list_acl(pool)
        .await
        .map_err(e)?
        .into_iter()
        .map(|a| LiveAcl {
            subject: if a.subject_type == "user" {
                users
                    .iter()
                    .find(|u| u.id == a.subject)
                    .map(|u| u.username.clone())
                    .unwrap_or(a.subject)
            } else {
                a.subject
            },
            path: a.path,
            subject_type: a.subject_type,
            role: a.role,
            propagate: a.propagate,
        })
        .collect();
    Ok(LiveAccess {
        users,
        roles,
        groups,
        acl,
        ..Default::default()
    })
}

/// A caller that is an administrator (checked again at apply time).
async fn admin_caller(state: &Arc<AppState>) -> Result<super::ctx::CallerCtx, String> {
    let c = super::ctx::caller().ok_or_else(|| {
        "changing users, groups, roles or ACL needs an authenticated administrator: apply it \
         through the API or CLI with your own login"
            .to_string()
    })?;
    if !super::ctx::caller_is_admin(state).await.unwrap_or(false) {
        return Err("changing users, groups, roles or ACL needs Permissions.Modify on /".into());
    }
    Ok(c)
}

macro_rules! access_kind {
    ($ty:ident, $key:literal, $change:literal, $rank:literal, $item:ty, $export:expr) => {
        pub struct $ty;

        #[async_trait]
        impl Kind for $ty {
            fn key(&self) -> &'static str {
                $key
            }
            fn change_kinds(&self) -> &'static [&'static str] {
                &[$change]
            }
            fn rank(&self) -> u32 {
                $rank
            }
            fn validate(&self, section: &Value) -> Result<(), String> {
                let mut k = AccessSpec::default();
                match $key {
                    "roles" => k.roles = Some(items(section)?),
                    "users" => k.users = Some(items(section)?),
                    "groups" => k.groups = Some(items(section)?),
                    _ => k.acl = Some(items(section)?),
                }
                validate(&k)
            }
            async fn gather(&self, state: &Arc<AppState>) -> Result<Value, String> {
                let with_hashes = $key == "users"
                    && super::ctx::caller()
                        .map(|c| c.include_hashes)
                        .unwrap_or(false);
                let l = gather_access(state, with_hashes).await?;
                let export: fn(&LiveAccess, bool) -> Vec<$item> = $export;
                to_json(&export(&l, with_hashes))
            }
            fn plan(
                &self,
                _section: &Value,
                _live: &Value,
                prune: bool,
                whole: (&Spec, &Live),
                out: &mut KindPlan,
            ) {
                plan_access($key, &[$change], prune, whole, out)
            }
            async fn apply(
                &self,
                state: &Arc<AppState>,
                section: &Value,
                _live: &Value,
                c: &Change,
            ) -> Result<Option<String>, String> {
                apply_access(state, $key, section, c).await
            }
        }
    };
}

access_kind!(Roles, "roles", "role", 2, RoleSpec, |l, _| export_roles(l));
access_kind!(Users, "users", "user", 3, UserSpec, |l, h| export_users(
    l, h
));
access_kind!(Groups, "groups", "group", 4, GroupSpec, |l, _| {
    export_groups(l)
});
access_kind!(Acl, "acl", "acl", 6, AclSpec, |l, _| export_acl(l));

async fn apply_access(
    state: &Arc<AppState>,
    key: &str,
    section: &Value,
    c: &Change,
) -> Result<Option<String>, String> {
    use crate::auth::store;
    let ctx = admin_caller(state).await?;
    let pool = state.database.pool();
    let e = |x: horcrux_common::Error| format!("{x}");
    let ex = |x: crate::error::ApiError| crate::declarative::exec::api_msg(x);
    match (key, c.action) {
        ("roles", Action::Create | Action::Update) => {
            let want: Vec<RoleSpec> = items(section)?;
            let w = want
                .iter()
                .find(|r| r.name == c.name)
                .ok_or("not in spec")?;
            let mut privileges = Vec::new();
            for p in &w.privileges {
                let p = horcrux_common::auth::Privilege::parse(p)
                    .ok_or_else(|| format!("unknown privilege {p}"))?;
                if !privileges.contains(&p) {
                    privileges.push(p);
                }
            }
            let rec = store::RoleRecord {
                name: w.name.clone(),
                description: w.description.clone(),
                privileges,
                builtin: false,
            };
            store::upsert_role(pool, &rec, c.action == Action::Create)
                .await
                .map_err(e)?;
            Ok(None)
        }
        ("roles", Action::Delete) => {
            store::delete_role(pool, &c.name).await.map_err(e)?;
            Ok(None)
        }
        ("users", Action::Create) => {
            let want: Vec<UserSpec> = items(section)?;
            let w = want
                .iter()
                .find(|u| u.username == c.name)
                .ok_or("not in spec")?;
            let (base, realm) = match w.username.split_once('@') {
                Some((b, r)) => (b.to_string(), Some(r.to_string())),
                None => (w.username.clone(), None),
            };
            // A hash goes in right after creation: the account starts with a
            // random password nobody knows (never usable in between).
            let plain = match (&w.password_ref, &w.password_hash) {
                (Some(r), _) => Some(r.resolve(&state.vault_manager).await?),
                (None, Some(h)) => {
                    crate::auth::password::verify_password("x", h)
                        .map_err(|_| "password_hash is not a valid argon2 hash".to_string())?;
                    Some(crate::auth::random_token(32))
                }
                _ => None,
            };
            let body: crate::api::auth::CreateUserBody =
                serde_json::from_value(serde_json::json!({
                    "username": base,
                    "password": plain,
                    "email": w.email.clone().unwrap_or_default(),
                    "realm": realm,
                    "enabled": w.enabled,
                    "comment": w.comment,
                    "tfa_required": w.tfa_required,
                }))
                .map_err(|x| x.to_string())?;
            let (_, rec) = crate::api::auth::create_user(
                axum::extract::State(state.clone()),
                Some(axum::Extension(ctx.user.clone())),
                axum::Json(body),
            )
            .await
            .map_err(ex)?;
            if let Some(h) = &w.password_hash {
                store::set_password_hash(pool, &rec.0.id, h)
                    .await
                    .map_err(e)?;
            }
            Ok(None)
        }
        ("users", Action::Update) => {
            let want: Vec<UserSpec> = items(section)?;
            let w = want
                .iter()
                .find(|u| u.username == c.name)
                .ok_or("not in spec")?;
            let l = store::find_user(pool, &w.username)
                .await
                .map_err(e)?
                .ok_or("user is gone")?;
            if !w.enabled && l.enabled {
                crate::api::auth::ensure_not_last_admin(state, &l)
                    .await
                    .map_err(ex)?;
            }
            store::update_user(
                pool,
                &l.id,
                &store::UserUpdate {
                    email: w.email.clone(),
                    comment: w.comment.clone(),
                    enabled: Some(w.enabled),
                    tfa_required: Some(w.tfa_required),
                },
            )
            .await
            .map_err(e)?;
            let new_hash = match (&w.password_hash, &w.password_ref) {
                (Some(h), _) if h != &l.password_hash => {
                    crate::auth::password::verify_password("x", h)
                        .map_err(|_| "password_hash is not a valid argon2 hash".to_string())?;
                    Some(h.clone())
                }
                (None, Some(r)) => {
                    let plain = r.resolve(&state.vault_manager).await?;
                    if crate::auth::password::verify_password(&plain, &l.password_hash)
                        .unwrap_or(false)
                    {
                        None
                    } else {
                        crate::api::auth::validate_password(&plain).map_err(ex)?;
                        Some(crate::auth::password::hash_password(&plain).map_err(e)?)
                    }
                }
                _ => None,
            };
            if let Some(h) = new_hash {
                store::set_password_hash(pool, &l.id, &h).await.map_err(e)?;
                store::delete_user_sessions(pool, &l.id).await.map_err(e)?;
            }
            Ok(None)
        }
        ("users", Action::Delete) => {
            let id = store::find_user(pool, &c.name)
                .await
                .map_err(e)?
                .ok_or("user is gone")?
                .id;
            crate::api::auth::delete_user(
                axum::extract::State(state.clone()),
                Some(axum::Extension(ctx.user.clone())),
                axum::extract::Path(id),
            )
            .await
            .map_err(ex)?;
            Ok(None)
        }
        ("groups", Action::Create | Action::Update) => {
            let want: Vec<GroupSpec> = items(section)?;
            let w = want
                .iter()
                .find(|g| g.name == c.name)
                .ok_or("not in spec")?;
            if c.action == Action::Create {
                store::create_group(pool, &w.name, w.comment.as_deref())
                    .await
                    .map_err(e)?;
            } else if let Some(cm) = &w.comment {
                sqlx::query("UPDATE auth_groups SET comment = ? WHERE name = ?")
                    .bind(cm)
                    .bind(&w.name)
                    .execute(pool)
                    .await
                    .map_err(|x| x.to_string())?;
            }
            if let Some(members) = &w.members {
                let mut ids = Vec::new();
                for m in members {
                    ids.push(
                        store::find_user(pool, m)
                            .await
                            .map_err(e)?
                            .ok_or_else(|| format!("user {m} does not exist"))?
                            .id,
                    );
                }
                store::set_group_members(pool, &w.name, &ids)
                    .await
                    .map_err(e)?;
            }
            Ok(None)
        }
        ("groups", Action::Delete) => {
            store::delete_group(pool, &c.name).await.map_err(e)?;
            Ok(None)
        }
        ("acl", act) => {
            let parts: Vec<&str> = c.name.split(' ').collect();
            let [path, st, subject, role] = parts[..] else {
                return Err(format!("bad acl change name {:?}", c.name));
            };
            let (subject_id, subject_user) = if st == "user" {
                let u = store::find_user(pool, subject)
                    .await
                    .map_err(e)?
                    .ok_or_else(|| format!("user {subject} does not exist"))?;
                (u.id.clone(), Some(u))
            } else {
                if !store::list_groups(pool)
                    .await
                    .map_err(e)?
                    .iter()
                    .any(|g| g.name == subject)
                {
                    return Err(format!("group {subject} does not exist"));
                }
                (subject.to_string(), None)
            };
            match act {
                Action::Create | Action::Update => {
                    let want: Vec<AclSpec> = items(section)?;
                    let w = want
                        .iter()
                        .find(|a| {
                            norm_path(&a.path) == path
                                && a.subject_type == st
                                && a.subject == subject
                                && a.role == role
                        })
                        .ok_or("not in spec")?;
                    store::add_acl(pool, path, st, &subject_id, role, w.propagate)
                        .await
                        .map_err(e)?;
                }
                Action::Delete => {
                    if let (true, Some(u)) = (
                        path == "/" && role == "Administrator",
                        subject_user.as_ref(),
                    ) {
                        crate::api::auth::ensure_not_last_admin(state, u)
                            .await
                            .map_err(ex)?;
                    }
                    let n = store::delete_acl(pool, path, st, &subject_id, Some(role))
                        .await
                        .map_err(e)?;
                    if n == 0 {
                        return Err("no such ACL entry".into());
                    }
                }
                _ => return Err(format!("no executor for acl {act:?}")),
            }
            Ok(None)
        }
        (k, a) => Err(format!("no executor for {k} {a:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: &str = "$argon2id$v=19$m=19456,t=2,p=1$aaaa$bbbb";
    const H2: &str = "$argon2id$v=19$m=19456,t=2,p=1$cccc$dddd";

    fn lu(name: &str, enabled: bool) -> LiveUser {
        LiveUser {
            id: format!("id-{name}"),
            username: name.into(),
            enabled,
            password_hash: H1.into(),
            ..Default::default()
        }
    }
    fn lr(name: &str, privs: &[&str], builtin: bool) -> LiveRole {
        LiveRole {
            name: name.into(),
            description: String::new(),
            privileges: privs.iter().map(|s| s.to_string()).collect(),
            builtin,
        }
    }
    fn la(path: &str, st: &str, subj: &str, role: &str) -> LiveAcl {
        LiveAcl {
            path: path.into(),
            subject_type: st.into(),
            subject: subj.into(),
            role: role.into(),
            propagate: true,
        }
    }
    /// admin (the caller) + Administrator on /
    fn base() -> LiveAccess {
        LiveAccess {
            users: vec![lu("admin", true)],
            roles: vec![
                lr("Administrator", &["Permissions.Modify", "Sys.Modify"], true),
                lr("Auditor", &["Sys.Audit"], true),
            ],
            groups: vec![],
            acl: vec![la("/", "user", "admin", "Administrator")],
            ..Default::default()
        }
    }
    fn ctx() -> Ctx {
        Ctx {
            caller: Some(CallerInfo {
                user_id: "id-admin".into(),
                username: "admin".into(),
                is_admin: true,
            }),
            ..Default::default()
        }
    }
    fn us(name: &str) -> UserSpec {
        UserSpec {
            username: name.into(),
            email: None,
            comment: None,
            enabled: true,
            tfa_required: false,
            password_hash: Some(H1.into()),
            password_ref: None,
        }
    }
    fn acts(p: &KindPlan) -> Vec<String> {
        p.changes
            .iter()
            .chain(&p.deletes)
            .map(|c| format!("{:?} {} {}", c.action, c.kind, c.name))
            .collect()
    }
    fn specs() -> AccessSpec {
        AccessSpec::default()
    }

    #[test]
    fn nothing_listed_means_nothing_managed() {
        let p = plan(&specs(), Some(&base()), true, &ctx());
        assert!(acts(&p).is_empty() && p.errors.is_empty());
    }

    #[test]
    fn creates_users_roles_groups_and_acl_in_spec_order() {
        let mut s = specs();
        s.roles = Some(vec![RoleSpec {
            name: "Op".into(),
            description: "d".into(),
            privileges: vec!["VM.Audit".into()],
        }]);
        s.users = Some(vec![us("admin"), us("alice"), us("bob")]);
        s.groups = Some(vec![GroupSpec {
            name: "ops".into(),
            comment: None,
            members: Some(vec!["alice".into(), "bob".into()]),
        }]);
        s.acl = Some(vec![
            AclSpec {
                path: "/vms/".into(),
                subject_type: "group".into(),
                subject: "ops".into(),
                role: "Op".into(),
                propagate: true,
            },
            AclSpec {
                path: "/".into(),
                subject_type: "user".into(),
                subject: "admin".into(),
                role: "Administrator".into(),
                propagate: true,
            },
        ]);
        let p = plan(&s, Some(&base()), false, &ctx());
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(
            acts(&p),
            [
                "Create role Op",
                "Create user alice",
                "Create user bob",
                "Create group ops",
                "Create acl /vms group ops Op"
            ]
        );
    }

    #[test]
    fn users_update_only_stated_fields_and_compare_hashes_not_print_them() {
        let mut w = us("admin");
        w.email = Some("a@x.org".into());
        w.password_hash = Some(H2.into());
        let mut s = specs();
        s.users = Some(vec![w]);
        // the caller can't change their own password through apply
        let p = plan(&s, Some(&base()), false, &ctx());
        assert_eq!(acts(&p), ["Blocked user admin"]);
        let all = p.changes[0].details.join(" ");
        assert!(
            all.contains("own password") && !all.contains("argon2"),
            "{all}"
        );
        // another user: a plain update
        let mut live = base();
        live.users.push(lu("alice", true));
        let mut a = us("alice");
        a.password_hash = Some(H2.into());
        s.users = Some(vec![us("admin"), a]);
        let p = plan(&s, Some(&live), false, &ctx());
        assert_eq!(acts(&p), ["Update user alice"]);
        assert!(!p.changes[0].details.join(" ").contains("argon2"));
    }

    #[test]
    fn password_ref_compares_through_the_executor_check() {
        let mut live = base();
        live.users.push(lu("alice", true));
        let mut a = us("alice");
        a.password_hash = None;
        a.password_ref = Some(SecretRef {
            env: Some("PW".into()),
            ..Default::default()
        });
        let mut s = specs();
        s.users = Some(vec![us("admin"), a.clone()]);
        // unknown (not checked): no drift
        assert!(acts(&plan(&s, Some(&live), false, &ctx())).is_empty());
        live.pw_checks.insert("alice".into(), true);
        assert!(acts(&plan(&s, Some(&live), false, &ctx())).is_empty());
        live.pw_checks.insert("alice".into(), false);
        assert_eq!(
            acts(&plan(&s, Some(&live), false, &ctx())),
            ["Update user alice"]
        );
        live.pw_errors
            .insert("alice".into(), "env PW is not set".into());
        let p = plan(&s, Some(&live), false, &ctx());
        assert!(p.errors.iter().any(|e| e.contains("password_ref")));
    }

    #[test]
    fn a_new_local_user_needs_a_password_source_but_an_external_one_does_not() {
        let mut a = us("alice");
        a.password_hash = None;
        let b = UserSpec {
            username: "bob@ldap".into(),
            password_hash: None,
            ..a.clone()
        };
        let mut s = specs();
        s.users = Some(vec![us("admin"), a, b]);
        let p = plan(&s, Some(&base()), false, &ctx());
        assert_eq!(p.errors.len(), 1, "{:?}", p.errors);
        assert!(p.errors[0].contains("alice"));
    }

    #[test]
    fn prune_never_deletes_the_caller_and_refuses_to_orphan_the_admins() {
        let mut live = base();
        live.users.push(lu("alice", true));
        live.acl.push(la("/", "user", "alice", "Administrator"));
        // spec lists only alice with prune: the caller (admin) would be deleted
        let mut s = specs();
        s.users = Some(vec![us("alice")]);
        let p = plan(&s, Some(&live), true, &ctx());
        assert!(
            acts(&p).contains(&"Blocked user admin".to_string()),
            "{:?}",
            acts(&p)
        );
        // spec lists nobody who is an administrator after prune: refused
        let mut live = base();
        live.users.push(lu("alice", true));
        let mut s = specs();
        s.users = Some(vec![us("alice")]);
        let p = plan(&s, Some(&live), true, &ctx());
        assert!(p
            .errors
            .iter()
            .any(|e| e.contains("no enabled administrator")));
        // without prune nothing is deleted and no admin is lost
        let p = plan(&s, Some(&live), false, &ctx());
        assert!(p.errors.is_empty());
    }

    #[test]
    fn disabling_the_last_admin_or_the_caller_is_refused() {
        let mut live = base();
        live.users.push(lu("alice", true));
        let mut a = us("admin");
        a.enabled = false;
        let mut s = specs();
        s.users = Some(vec![a, us("alice")]);
        let p = plan(&s, Some(&live), false, &ctx());
        assert!(p
            .errors
            .iter()
            .any(|e| e.contains("no enabled administrator")));
        assert!(acts(&p).contains(&"Blocked user admin".to_string()));
    }

    #[test]
    fn admin_through_a_group_counts() {
        let mut live = base();
        live.users.push(lu("alice", true));
        live.groups.push(LiveGroup {
            name: "adm".into(),
            comment: None,
            members: vec!["alice".into()],
        });
        live.acl.push(la("/", "group", "adm", "Administrator"));
        let mut s = specs();
        s.users = Some(vec![us("alice")]); // admin pruned, alice still admin via group
        let p = plan(&s, Some(&live), true, &ctx());
        assert!(p.errors.is_empty(), "{:?}", p.errors);
    }

    #[test]
    fn dangling_references_are_plan_errors() {
        let mut s = specs();
        s.groups = Some(vec![GroupSpec {
            name: "g".into(),
            comment: None,
            members: Some(vec!["ghost".into()]),
        }]);
        s.acl = Some(vec![AclSpec {
            path: "/".into(),
            subject_type: "user".into(),
            subject: "nobody".into(),
            role: "NoSuchRole".into(),
            propagate: true,
        }]);
        let p = plan(&s, Some(&base()), false, &ctx());
        assert_eq!(p.errors.len(), 3, "{:?}", p.errors);
    }

    #[test]
    fn builtin_roles_can_not_be_redefined_and_are_never_pruned_or_exported() {
        let mut s = specs();
        s.roles = Some(vec![RoleSpec {
            name: "Auditor".into(),
            description: String::new(),
            privileges: vec!["VM.Audit".into()],
        }]);
        let p = plan(&s, Some(&base()), true, &ctx());
        assert!(p.errors.iter().any(|e| e.contains("built-in")));
        s.roles = Some(vec![]);
        let p = plan(&s, Some(&base()), true, &ctx());
        assert!(acts(&p).is_empty(), "{:?}", acts(&p));
        assert!(export_roles(&base()).is_empty());
    }

    #[test]
    fn role_privilege_changes_update_and_prune_deletes_only_unlisted_custom_roles() {
        let mut live = base();
        live.roles.push(lr("Op", &["VM.Audit"], false));
        live.roles.push(lr("Old", &["VM.Audit"], false));
        let mut s = specs();
        s.roles = Some(vec![RoleSpec {
            name: "Op".into(),
            description: String::new(),
            privileges: vec!["VM.Audit".into(), "VM.PowerMgmt".into()],
        }]);
        let p = plan(&s, Some(&live), true, &ctx());
        assert_eq!(acts(&p), ["Update role Op", "Delete role Old"]);
        let p = plan(&s, Some(&live), false, &ctx());
        assert_eq!(acts(&p), ["Update role Op"]);
    }

    #[test]
    fn group_members_are_managed_only_when_listed() {
        let mut live = base();
        live.users.push(lu("alice", true));
        live.groups.push(LiveGroup {
            name: "ops".into(),
            comment: Some("c".into()),
            members: vec!["admin".into()],
        });
        let mut s = specs();
        s.groups = Some(vec![GroupSpec {
            name: "ops".into(),
            comment: None,
            members: None,
        }]);
        assert!(acts(&plan(&s, Some(&live), false, &ctx())).is_empty());
        s.groups = Some(vec![GroupSpec {
            name: "ops".into(),
            comment: None,
            members: Some(vec!["alice".into()]),
        }]);
        assert_eq!(
            acts(&plan(&s, Some(&live), false, &ctx())),
            ["Update group ops"]
        );
    }

    #[test]
    fn acl_propagate_changes_update_and_unlisted_entries_prune() {
        let mut live = base();
        live.acl.push(LiveAcl {
            propagate: false,
            ..la("/vms", "user", "admin", "Auditor")
        });
        live.acl.push(la("/storage", "user", "admin", "Auditor"));
        let mut s = specs();
        s.acl = Some(vec![
            AclSpec {
                path: "/".into(),
                subject_type: "user".into(),
                subject: "admin".into(),
                role: "Administrator".into(),
                propagate: true,
            },
            AclSpec {
                path: "/vms".into(),
                subject_type: "user".into(),
                subject: "admin".into(),
                role: "Auditor".into(),
                propagate: true,
            },
        ]);
        let p = plan(&s, Some(&live), true, &ctx());
        assert_eq!(
            acts(&p),
            [
                "Update acl /vms user admin Auditor",
                "Delete acl /storage user admin Auditor"
            ]
        );
    }

    #[test]
    fn a_caller_without_permissions_modify_cannot_apply_access_changes() {
        let mut c = ctx();
        c.caller.as_mut().unwrap().is_admin = false;
        let mut s = specs();
        s.users = Some(vec![us("admin"), us("alice")]);
        let p = plan(&s, Some(&base()), false, &c);
        assert!(p.errors.iter().any(|e| e.contains("Permissions.Modify")));
        // a read-only plan with nothing to do stays clean
        s.users = Some(vec![us("admin")]);
        assert!(plan(&s, Some(&base()), false, &c).errors.is_empty());
    }

    #[test]
    fn export_omits_hashes_unless_asked_and_is_a_fixed_point() {
        let mut live = base();
        let mut al = lu("alice", true);
        al.email = "a@x.org".into();
        live.users.push(al);
        live.users.push(LiveUser {
            password_hash: "!external".into(),
            ..lu("bob@ldap", true)
        });
        live.roles
            .push(lr("Op", &["VM.PowerMgmt", "VM.Audit"], false));
        live.groups.push(LiveGroup {
            name: "ops".into(),
            comment: None,
            members: vec!["alice".into()],
        });
        live.acl.push(la("/vms", "group", "ops", "Op"));
        let k = AccessSpec {
            roles: Some(export_roles(&live)),
            users: Some(export_users(&live, false)),
            groups: Some(export_groups(&live)),
            acl: Some(export_acl(&live)),
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&k).unwrap();
        assert!(
            !yaml.contains("argon2") && !yaml.contains("password"),
            "{yaml}"
        );
        validate(&k).unwrap();
        let p = plan(&k, Some(&live), true, &ctx());
        assert!(
            acts(&p).is_empty() && p.errors.is_empty(),
            "{:?} {:?}",
            acts(&p),
            p.errors
        );
        // with hashes: local users carry theirs, external ones never
        let with = export_users(&live, true);
        assert_eq!(with.iter().filter(|u| u.password_hash.is_some()).count(), 2);
        assert!(with
            .iter()
            .find(|u| u.username == "bob@ldap")
            .unwrap()
            .password_hash
            .is_none());
        let k2 = AccessSpec {
            users: Some(with),
            ..Default::default()
        };
        assert!(acts(&plan(&k2, Some(&live), false, &ctx())).is_empty());
    }

    #[test]
    fn validation_rejects_plaintext_and_bad_shapes() {
        let mut a = us("alice");
        a.password_hash = Some("hunter2".into());
        let mut s = specs();
        s.users = Some(vec![a]);
        assert!(validate(&s).unwrap_err().contains("argon2"));
        let mut a = us("alice");
        a.password_ref = Some(SecretRef {
            env: Some("X".into()),
            ..Default::default()
        });
        s.users = Some(vec![a]);
        assert!(validate(&s).unwrap_err().contains("not both"));
        s.users = Some(vec![us("bad name")]);
        assert!(validate(&s).is_err());
        s.users = Some(vec![us("a"), us("a")]);
        assert!(validate(&s).unwrap_err().contains("twice"));
        let mut e = us("bob@ldap");
        e.password_hash = Some(H1.into());
        s.users = Some(vec![e]);
        assert!(validate(&s).unwrap_err().contains("realm"));
        s.users = None;
        s.roles = Some(vec![RoleSpec {
            name: "R".into(),
            description: String::new(),
            privileges: vec!["Not.APrivilege".into()],
        }]);
        assert!(validate(&s).unwrap_err().contains("unknown privilege"));
        s.roles = None;
        s.acl = Some(vec![AclSpec {
            path: "vms".into(),
            subject_type: "user".into(),
            subject: "a".into(),
            role: "R".into(),
            propagate: true,
        }]);
        assert!(validate(&s).is_err());
    }

    #[test]
    fn unknown_yaml_keys_are_errors() {
        assert!(serde_yaml::from_str::<UserSpec>("username: a\npassword: plain").is_err());
        let u: UserSpec = serde_yaml::from_str("username: a\npassword_ref: {file: /x}").unwrap();
        assert!(u.password_ref.is_some());
    }

    #[test]
    fn paths_normalise_like_the_acl_store() {
        assert_eq!(norm_path("/vms/"), "/vms");
        assert_eq!(norm_path(""), "/");
        assert_eq!(norm_path("//a//b/"), "/a/b");
    }
}
