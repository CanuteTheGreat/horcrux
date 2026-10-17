//! Groups, ACL (who holds which role where) and authentication realms.

use crate::api::{self, AclEntry, Group, Realm};
use leptos::*;

/// Groups and permission grants (ACL).
#[component]
pub fn PermissionsPage() -> impl IntoView {
    let (acl, set_acl) = create_signal(Vec::<AclEntry>::new());
    let (groups, set_groups) = create_signal(Vec::<Group>::new());
    let (users, set_users) = create_signal(Vec::<api::User>::new());
    let (roles, set_roles) = create_signal(Vec::<api::Role>::new());
    let (error, set_error) = create_signal(None::<String>);

    // new grant
    let (path, set_path) = create_signal("/".to_string());
    let (subject_type, set_subject_type) = create_signal("user".to_string());
    let (subject, set_subject) = create_signal(String::new());
    let (role, set_role) = create_signal("Auditor".to_string());
    let (propagate, set_propagate) = create_signal(true);
    // new group
    let (gname, set_gname) = create_signal(String::new());
    let (gmembers, set_gmembers) = create_signal(String::new());

    let reload = move || {
        spawn_local(async move {
            match api::get_acl().await {
                Ok(a) => set_acl.set(a),
                Err(e) => set_error.set(Some(e.message)),
            }
            if let Ok(g) = api::get_groups().await {
                set_groups.set(g);
            }
            if let Ok(u) = api::get_users().await {
                set_users.set(u);
            }
            if let Ok(r) = api::get_roles().await {
                set_roles.set(r);
            }
        });
    };
    reload();

    let add_grant = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        set_error.set(None);
        let (p, st, s, r, pr) = (
            path.get(),
            subject_type.get(),
            subject.get(),
            role.get(),
            propagate.get(),
        );
        spawn_local(async move {
            match api::add_acl(p, st, s, r, pr).await {
                Ok(()) => reload(),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    let add_group = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        set_error.set(None);
        let name = gname.get();
        let members: Vec<String> = gmembers
            .get()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        spawn_local(async move {
            match api::create_group(name, None, members).await {
                Ok(()) => {
                    set_gname.set(String::new());
                    set_gmembers.set(String::new());
                    reload();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    view! {
        <div class="page permissions-page">
            <h1>"Permissions"</h1>
            <p class="help">
                "A grant gives a user or group a role on a path: " <code>"/"</code> " (everything), "
                <code>"/vms"</code> ", " <code>"/vms/<id>"</code> ", " <code>"/containers"</code> ", "
                <code>"/storage/<pool>"</code> ", " <code>"/sdn"</code> ", " <code>"/nodes"</code> ", "
                <code>"/cluster"</code> ", " <code>"/system"</code> ", " <code>"/access"</code>
                ". The most specific path wins; a user's own grant beats a group grant there. "
                "Grant " <code>"NoAccess"</code> " to take access away below a broader grant."
            </p>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="perm-error">{e}</div> })}

            <section class="card">
                <h2>"Grants"</h2>
                <table class="table" id="acl-table">
                    <thead><tr><th>"Path"</th><th>"Who"</th><th>"Role"</th><th>"Propagate"</th><th></th></tr></thead>
                    <tbody>
                        {move || acl.get().into_iter().map(|e| {
                            let entry = e.clone();
                            view! {
                                <tr>
                                    <td><code>{e.path.clone()}</code></td>
                                    <td>{format!("{} {}", e.subject_type, if e.subject_name.is_empty() { e.subject.clone() } else { e.subject_name.clone() })}</td>
                                    <td>{e.role.clone()}</td>
                                    <td>{if e.propagate { "yes" } else { "no" }}</td>
                                    <td><button class="btn btn-sm btn-danger" on:click=move |_| {
                                        let entry = entry.clone();
                                        spawn_local(async move {
                                            match api::remove_acl(&entry).await {
                                                Ok(()) => reload(),
                                                Err(e) => set_error.set(Some(e.message)),
                                            }
                                        });
                                    }>"Remove"</button></td>
                                </tr>
                            }
                        }).collect_view()}
                    </tbody>
                </table>
                <form class="inline-form" id="acl-add" on:submit=add_grant>
                    <input type="text" id="acl-path" placeholder="/vms/<id>" required
                        on:input=move |ev| set_path.set(event_target_value(&ev)) prop:value=path/>
                    <select id="acl-subject-type" on:change=move |ev| { set_subject_type.set(event_target_value(&ev)); set_subject.set(String::new()); }>
                        <option value="user">"User"</option>
                        <option value="group">"Group"</option>
                    </select>
                    <select id="acl-subject" on:change=move |ev| set_subject.set(event_target_value(&ev))>
                        <option value="">"-- choose --"</option>
                        {move || if subject_type.get() == "user" {
                            users.get().into_iter().map(|u| view! { <option value=u.username.clone()>{u.username.clone()}</option> }).collect_view()
                        } else {
                            groups.get().into_iter().map(|g| view! { <option value=g.name.clone()>{g.name.clone()}</option> }).collect_view()
                        }}
                    </select>
                    <select id="acl-role" on:change=move |ev| set_role.set(event_target_value(&ev))>
                        {move || roles.get().into_iter().map(|r| {
                            let sel = r.name == role.get_untracked();
                            view! { <option value=r.name.clone() selected=sel>{r.name.clone()}</option> }
                        }).collect_view()}
                    </select>
                    <label class="checkbox">
                        <input type="checkbox" prop:checked=propagate on:change=move |_| set_propagate.update(|v| *v = !*v)/>
                        " propagate"
                    </label>
                    <button type="submit" class="btn btn-primary" id="acl-submit">"Grant"</button>
                </form>
            </section>

            <section class="card">
                <h2>"Groups"</h2>
                <table class="table" id="groups-table">
                    <thead><tr><th>"Group"</th><th>"Members"</th><th></th></tr></thead>
                    <tbody>
                        {move || groups.get().into_iter().map(|g| {
                            let name = g.name.clone();
                            view! {
                                <tr>
                                    <td>{g.name.clone()}</td>
                                    <td>{g.members.join(", ")}</td>
                                    <td><button class="btn btn-sm btn-danger" on:click=move |_| {
                                        let name = name.clone();
                                        spawn_local(async move {
                                            match api::delete_group(&name).await {
                                                Ok(()) => reload(),
                                                Err(e) => set_error.set(Some(e.message)),
                                            }
                                        });
                                    }>"Delete"</button></td>
                                </tr>
                            }
                        }).collect_view()}
                    </tbody>
                </table>
                <form class="inline-form" id="group-add" on:submit=add_group>
                    <input type="text" id="group-name" placeholder="group name" required
                        on:input=move |ev| set_gname.set(event_target_value(&ev)) prop:value=gname/>
                    <input type="text" id="group-members" placeholder="members: alice, bob"
                        on:input=move |ev| set_gmembers.set(event_target_value(&ev)) prop:value=gmembers/>
                    <button type="submit" class="btn btn-primary" id="group-submit">"Create group"</button>
                </form>
            </section>
        </div>
    }
}

fn realm_template(t: &str) -> String {
    match t {
        "ldap" => {
            r#"{
  "server": "ldap.example.org",
  "mode": "starttls",
  "verify_tls": true,
  "base_dn": "ou=people,dc=example,dc=org",
  "user_attr": "uid",
  "bind_dn": null,
  "bind_password": null,
  "autocreate": true
}"#
        }
        "ad" => {
            r#"{
  "server": "dc1.corp.example",
  "mode": "ldaps",
  "domain": "corp.example",
  "base_dn": "dc=corp,dc=example",
  "autocreate": true
}"#
        }
        "openid" => {
            r#"{
  "issuer_url": "https://idp.example.org/realms/horcrux",
  "client_id": "horcrux",
  "client_secret": "",
  "username_claim": "sub",
  "autocreate": true
}"#
        }
        _ => "{}",
    }
    .to_string()
}

/// Authentication realms (PAM, LDAP, Active Directory, OpenID Connect).
#[component]
pub fn RealmsPage() -> impl IntoView {
    let (realms, set_realms) = create_signal(Vec::<Realm>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (name, set_name) = create_signal(String::new());
    let (rtype, set_rtype) = create_signal("ldap".to_string());
    let (config, set_config) = create_signal(realm_template("ldap"));
    let (comment, set_comment) = create_signal(String::new());
    let (default_group, set_default_group) = create_signal(String::new());
    let (tfa, set_tfa) = create_signal(false);
    let (test_user, set_test_user) = create_signal(String::new());
    let (test_pw, set_test_pw) = create_signal(String::new());

    let reload = move || {
        spawn_local(async move {
            match api::get_realms().await {
                Ok(r) => set_realms.set(r),
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    reload();

    let create = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        set_error.set(None);
        let cfg: serde_json::Value = match serde_json::from_str(&config.get()) {
            Ok(v) => v,
            Err(e) => {
                set_error.set(Some(format!("settings are not valid JSON: {}", e)));
                return;
            }
        };
        let r = Realm {
            name: name.get(),
            realm_type: rtype.get(),
            enabled: true,
            comment: Some(comment.get()).filter(|c| !c.is_empty()),
            config: cfg,
            default_group: Some(default_group.get()).filter(|g| !g.is_empty()),
            tfa_required: tfa.get(),
        };
        spawn_local(async move {
            match api::create_realm(&r).await {
                Ok(()) => {
                    set_notice.set(Some(format!("Realm {} created", r.name)));
                    reload();
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    view! {
        <div class="page realms-page">
            <h1>"Authentication Realms"</h1>
            <p class="help">
                "Users of a realm log in as " <code>"name@realm"</code> " (or pick the realm on the login page). "
                "Their account is created on first login and starts in the realm's default group, "
                "so grant that group a role on the Permissions page."
            </p>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="realm-error">{e}</div> })}
            {move || notice.get().map(|n| view! { <div class="alert alert-success" id="realm-notice">{n}</div> })}
            <table class="table" id="realms-table">
                <thead><tr><th>"Realm"</th><th>"Type"</th><th>"Enabled"</th><th>"2FA required"</th><th>"Default group"</th><th></th></tr></thead>
                <tbody>
                    <tr><td>"local"</td><td>"Horcrux users"</td><td>"yes"</td><td>"-"</td><td>"-"</td><td></td></tr>
                    {move || realms.get().into_iter().map(|r| {
                        let (n1, n2, n3) = (r.name.clone(), r.name.clone(), r.name.clone());
                        let toggled = Realm { enabled: !r.enabled, ..r.clone() };
                        view! {
                            <tr>
                                <td>{r.name.clone()}</td>
                                <td>{r.realm_type.clone()}</td>
                                <td>{if r.enabled { "yes" } else { "no" }}</td>
                                <td>{if r.tfa_required { "yes" } else { "no" }}</td>
                                <td>{r.default_group.clone().unwrap_or_default()}</td>
                                <td>
                                    <button class="btn btn-sm" on:click=move |_| {
                                        let n = n1.clone();
                                        let (u, p) = (test_user.get(), test_pw.get());
                                        spawn_local(async move {
                                            let creds = (!u.is_empty()).then_some((u, p));
                                            match api::test_realm(&n, creds.as_ref().map(|c| c.0.clone()), creds.map(|c| c.1)).await {
                                                Ok(v) => {
                                                    let msg = v["message"].as_str().unwrap_or_default().to_string();
                                                    if v["ok"].as_bool().unwrap_or(false) {
                                                        set_notice.set(Some(format!("{}: {}", n, msg)));
                                                    } else {
                                                        set_error.set(Some(format!("{}: {}", n, msg)));
                                                    }
                                                }
                                                Err(e) => set_error.set(Some(e.message)),
                                            }
                                        });
                                    }>"Test"</button>
                                    <button class="btn btn-sm" on:click=move |_| {
                                        let r = toggled.clone();
                                        let _ = &n2;
                                        spawn_local(async move {
                                            match api::update_realm(&r).await {
                                                Ok(()) => reload(),
                                                Err(e) => set_error.set(Some(e.message)),
                                            }
                                        });
                                    }>{if r.enabled { "Disable" } else { "Enable" }}</button>
                                    <button class="btn btn-sm btn-danger" on:click=move |_| {
                                        let n = n3.clone();
                                        spawn_local(async move {
                                            match api::delete_realm(&n).await {
                                                Ok(()) => reload(),
                                                Err(e) => set_error.set(Some(e.message)),
                                            }
                                        });
                                    }>"Delete"</button>
                                </td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
            <div class="inline-form">
                <label>"Test login (optional): "</label>
                <input type="text" placeholder="user" id="realm-test-user" on:input=move |ev| set_test_user.set(event_target_value(&ev))/>
                <input type="password" placeholder="password" id="realm-test-password" on:input=move |ev| set_test_pw.set(event_target_value(&ev))/>
            </div>

            <section class="card">
                <h2>"Add realm"</h2>
                <form id="realm-add" on:submit=create>
                    <div class="form-group">
                        <label>"Name"</label>
                        <input type="text" id="realm-name" required on:input=move |ev| set_name.set(event_target_value(&ev)) prop:value=name/>
                    </div>
                    <div class="form-group">
                        <label>"Type"</label>
                        <select id="realm-type" on:change=move |ev| {
                            let t = event_target_value(&ev);
                            set_config.set(realm_template(&t));
                            set_rtype.set(t);
                        }>
                            <option value="ldap">"LDAP"</option>
                            <option value="ad">"Active Directory"</option>
                            <option value="openid">"OpenID Connect"</option>
                            <option value="pam">"Linux PAM (host accounts)"</option>
                        </select>
                    </div>
                    <div class="form-group">
                        <label>"Settings (JSON)"</label>
                        <textarea id="realm-config" rows="10" on:input=move |ev| set_config.set(event_target_value(&ev)) prop:value=config></textarea>
                    </div>
                    <div class="form-group">
                        <label>"Comment (shown on the login page)"</label>
                        <input type="text" id="realm-comment" on:input=move |ev| set_comment.set(event_target_value(&ev)) prop:value=comment/>
                    </div>
                    <div class="form-group">
                        <label>"Default group for new users"</label>
                        <input type="text" id="realm-default-group" on:input=move |ev| set_default_group.set(event_target_value(&ev)) prop:value=default_group/>
                    </div>
                    <label class="checkbox">
                        <input type="checkbox" id="realm-tfa" prop:checked=tfa on:change=move |_| set_tfa.update(|v| *v = !*v)/>
                        " Require a Horcrux second factor for this realm's users (not for OpenID: the provider handles that)"
                    </label>
                    <button type="submit" class="btn btn-primary" id="realm-submit">"Add realm"</button>
                </form>
            </section>
        </div>
    }
}
