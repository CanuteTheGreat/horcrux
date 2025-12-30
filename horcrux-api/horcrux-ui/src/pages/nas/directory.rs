//! Directory Services Management Page
//!
//! Manages LDAP, Kerberos, and Active Directory integration

use leptos::*;

#[allow(dead_code)]
#[derive(Clone, Debug, serde::Deserialize)]
pub struct LdapStatus {
    pub configured: bool,
    pub connected: bool,
    pub uri: Option<String>,
    pub base_dn: Option<String>,
    pub user_count: u32,
    pub group_count: u32,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct KerberosStatus {
    pub configured: bool,
    pub default_realm: String,
    pub keytab_exists: bool,
    pub active_tickets: u32,
    pub realms: Vec<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct AdStatus {
    pub joined: bool,
    pub domain: Option<String>,
    pub domain_controller: Option<String>,
    pub winbind_running: bool,
    pub kerberos_realm: String,
    pub idmap_backend: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct LdapConfigForm {
    pub uri: String,
    pub base_dn: String,
    pub bind_dn: String,
    pub bind_password: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct KerberosConfigForm {
    pub realm: String,
    pub kdc: String,
    pub admin_server: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct AdJoinForm {
    pub domain: String,
    pub username: String,
    pub password: String,
    pub computer_ou: String,
    pub register_dns: bool,
}

#[component]
pub fn DirectoryPage() -> impl IntoView {
    let (active_tab, set_active_tab) = create_signal("ldap".to_string());

    view! {
        <div class="directory-page">
            <div class="page-header">
                <h1>"Directory Services"</h1>
            </div>

            <div class="tabs">
                <button
                    class=move || if active_tab.get() == "ldap" { "tab active" } else { "tab" }
                    on:click=move |_| set_active_tab.set("ldap".to_string())
                >"LDAP"</button>
                <button
                    class=move || if active_tab.get() == "kerberos" { "tab active" } else { "tab" }
                    on:click=move |_| set_active_tab.set("kerberos".to_string())
                >"Kerberos"</button>
                <button
                    class=move || if active_tab.get() == "ad" { "tab active" } else { "tab" }
                    on:click=move |_| set_active_tab.set("ad".to_string())
                >"Active Directory"</button>
            </div>

            <div class="tab-content">
                {move || match active_tab.get().as_str() {
                    "ldap" => view! { <LdapPanel /> }.into_view(),
                    "kerberos" => view! { <KerberosPanel /> }.into_view(),
                    "ad" => view! { <AdPanel /> }.into_view(),
                    _ => view! { <LdapPanel /> }.into_view(),
                }}
            </div>
        </div>
    }
}

#[component]
fn LdapPanel() -> impl IntoView {
    let (status, set_status) = create_signal(None::<LdapStatus>);
    let (loading, set_loading) = create_signal(true);
    let (show_config, set_show_config) = create_signal(false);
    let (message, set_message) = create_signal(None::<(String, bool)>);

    // Form fields
    let (uri, set_uri) = create_signal(String::new());
    let (base_dn, set_base_dn) = create_signal(String::new());
    let (bind_dn, set_bind_dn) = create_signal(String::new());
    let (bind_password, set_bind_password) = create_signal(String::new());

    // Load status
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            if let Ok(resp) = reqwasm::http::Request::get("/api/nas/directory/ldap/status")
                .send()
                .await
            {
                if resp.ok() {
                    if let Ok(data) = resp.json::<LdapStatus>().await {
                        set_status.set(Some(data));
                    }
                }
            }
            set_loading.set(false);
        });
    });

    let save_config = move |_| {
        let config = LdapConfigForm {
            uri: uri.get(),
            base_dn: base_dn.get(),
            bind_dn: bind_dn.get(),
            bind_password: bind_password.get(),
        };

        spawn_local(async move {
            match reqwasm::http::Request::post("/api/nas/directory/ldap/configure")
                .header("Content-Type", "application/json")
                .body(serde_json::to_string(&config).unwrap())
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    set_message.set(Some(("LDAP configured successfully".to_string(), true)));
                    set_show_config.set(false);
                }
                _ => {
                    set_message.set(Some(("Failed to configure LDAP".to_string(), false)));
                }
            }
        });
    };

    let test_connection = move |_| {
        spawn_local(async move {
            match reqwasm::http::Request::get("/api/nas/directory/ldap/test")
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    set_message.set(Some(("LDAP connection test successful".to_string(), true)));
                }
                _ => {
                    set_message.set(Some(("LDAP connection test failed".to_string(), false)));
                }
            }
        });
    };

    let sync_users = move |_| {
        spawn_local(async move {
            match reqwasm::http::Request::post("/api/nas/directory/ldap/sync")
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    set_message.set(Some(("LDAP sync completed".to_string(), true)));
                }
                _ => {
                    set_message.set(Some(("LDAP sync failed".to_string(), false)));
                }
            }
        });
    };

    view! {
        <div class="panel ldap-panel">
            <h2>"LDAP Directory"</h2>

            {move || message.get().map(|(msg, success)| {
                let class = if success { "alert alert-success" } else { "alert alert-error" };
                view! { <div class={class}>{msg}</div> }
            })}

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading LDAP status..."</p> }.into_view()
                } else if let Some(s) = status.get() {
                    view! {
                        <div class="status-card">
                            <div class="status-row">
                                <span class="label">"Status:"</span>
                                <span class={if s.connected { "status-active" } else { "status-inactive" }}>
                                    {if s.connected { "Connected" } else { "Not Connected" }}
                                </span>
                            </div>
                            {s.uri.map(|u| view! {
                                <div class="status-row">
                                    <span class="label">"Server:"</span>
                                    <span>{u}</span>
                                </div>
                            })}
                            {s.base_dn.map(|b| view! {
                                <div class="status-row">
                                    <span class="label">"Base DN:"</span>
                                    <span>{b}</span>
                                </div>
                            })}
                            <div class="status-row">
                                <span class="label">"Users:"</span>
                                <span>{s.user_count}</span>
                            </div>
                            <div class="status-row">
                                <span class="label">"Groups:"</span>
                                <span>{s.group_count}</span>
                            </div>
                        </div>

                        <div class="button-group">
                            <button class="btn btn-primary" on:click=move |_| set_show_config.set(true)>
                                "Configure"
                            </button>
                            <button class="btn" on:click=test_connection>
                                "Test Connection"
                            </button>
                            <button class="btn" on:click=sync_users>
                                "Sync Users"
                            </button>
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <p>"LDAP not configured"</p>
                        <button class="btn btn-primary" on:click=move |_| set_show_config.set(true)>
                            "Configure LDAP"
                        </button>
                    }.into_view()
                }
            }}

            {move || show_config.get().then(|| view! {
                <div class="modal-overlay">
                    <div class="modal">
                        <h3>"Configure LDAP"</h3>
                        <form class="config-form">
                            <div class="form-group">
                                <label>"Server URI"</label>
                                <input
                                    type="text"
                                    placeholder="ldap://ldap.example.com"
                                    prop:value=move || uri.get()
                                    on:input=move |e| set_uri.set(event_target_value(&e))
                                />
                            </div>
                            <div class="form-group">
                                <label>"Base DN"</label>
                                <input
                                    type="text"
                                    placeholder="dc=example,dc=com"
                                    prop:value=move || base_dn.get()
                                    on:input=move |e| set_base_dn.set(event_target_value(&e))
                                />
                            </div>
                            <div class="form-group">
                                <label>"Bind DN"</label>
                                <input
                                    type="text"
                                    placeholder="cn=admin,dc=example,dc=com"
                                    prop:value=move || bind_dn.get()
                                    on:input=move |e| set_bind_dn.set(event_target_value(&e))
                                />
                            </div>
                            <div class="form-group">
                                <label>"Bind Password"</label>
                                <input
                                    type="password"
                                    prop:value=move || bind_password.get()
                                    on:input=move |e| set_bind_password.set(event_target_value(&e))
                                />
                            </div>
                            <div class="button-group">
                                <button type="button" class="btn btn-primary" on:click=save_config>
                                    "Save"
                                </button>
                                <button type="button" class="btn" on:click=move |_| set_show_config.set(false)>
                                    "Cancel"
                                </button>
                            </div>
                        </form>
                    </div>
                </div>
            })}
        </div>
    }
}

#[component]
fn KerberosPanel() -> impl IntoView {
    let (status, set_status) = create_signal(None::<KerberosStatus>);
    let (loading, set_loading) = create_signal(true);
    let (show_config, set_show_config) = create_signal(false);
    let (message, set_message) = create_signal(None::<(String, bool)>);

    // Form fields
    let (realm, set_realm) = create_signal(String::new());
    let (kdc, set_kdc) = create_signal(String::new());
    let (admin_server, set_admin_server) = create_signal(String::new());

    // Load status
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            if let Ok(resp) = reqwasm::http::Request::get("/api/nas/directory/kerberos/status")
                .send()
                .await
            {
                if resp.ok() {
                    if let Ok(data) = resp.json::<KerberosStatus>().await {
                        set_status.set(Some(data));
                    }
                }
            }
            set_loading.set(false);
        });
    });

    let save_config = move |_| {
        let config = KerberosConfigForm {
            realm: realm.get(),
            kdc: kdc.get(),
            admin_server: admin_server.get(),
        };

        spawn_local(async move {
            match reqwasm::http::Request::post("/api/nas/directory/kerberos/configure")
                .header("Content-Type", "application/json")
                .body(serde_json::to_string(&config).unwrap())
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    set_message.set(Some(("Kerberos configured successfully".to_string(), true)));
                    set_show_config.set(false);
                }
                _ => {
                    set_message.set(Some(("Failed to configure Kerberos".to_string(), false)));
                }
            }
        });
    };

    let kdestroy = move |_| {
        spawn_local(async move {
            match reqwasm::http::Request::post("/api/nas/directory/kerberos/kdestroy")
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    set_message.set(Some(("Tickets destroyed".to_string(), true)));
                }
                _ => {
                    set_message.set(Some(("Failed to destroy tickets".to_string(), false)));
                }
            }
        });
    };

    view! {
        <div class="panel kerberos-panel">
            <h2>"Kerberos Authentication"</h2>

            {move || message.get().map(|(msg, success)| {
                let class = if success { "alert alert-success" } else { "alert alert-error" };
                view! { <div class={class}>{msg}</div> }
            })}

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading Kerberos status..."</p> }.into_view()
                } else if let Some(s) = status.get() {
                    view! {
                        <div class="status-card">
                            <div class="status-row">
                                <span class="label">"Configured:"</span>
                                <span class={if s.configured { "status-active" } else { "status-inactive" }}>
                                    {if s.configured { "Yes" } else { "No" }}
                                </span>
                            </div>
                            <div class="status-row">
                                <span class="label">"Default Realm:"</span>
                                <span>{s.default_realm.clone()}</span>
                            </div>
                            <div class="status-row">
                                <span class="label">"Keytab Exists:"</span>
                                <span>{if s.keytab_exists { "Yes" } else { "No" }}</span>
                            </div>
                            <div class="status-row">
                                <span class="label">"Active Tickets:"</span>
                                <span>{s.active_tickets}</span>
                            </div>
                            {(!s.realms.is_empty()).then(|| view! {
                                <div class="status-row">
                                    <span class="label">"Realms:"</span>
                                    <span>{s.realms.join(", ")}</span>
                                </div>
                            })}
                        </div>

                        <div class="button-group">
                            <button class="btn btn-primary" on:click=move |_| set_show_config.set(true)>
                                "Configure"
                            </button>
                            <button class="btn" on:click=kdestroy>
                                "Destroy Tickets"
                            </button>
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <p>"Kerberos not configured"</p>
                        <button class="btn btn-primary" on:click=move |_| set_show_config.set(true)>
                            "Configure Kerberos"
                        </button>
                    }.into_view()
                }
            }}

            {move || show_config.get().then(|| view! {
                <div class="modal-overlay">
                    <div class="modal">
                        <h3>"Configure Kerberos"</h3>
                        <form class="config-form">
                            <div class="form-group">
                                <label>"Realm"</label>
                                <input
                                    type="text"
                                    placeholder="EXAMPLE.COM"
                                    prop:value=move || realm.get()
                                    on:input=move |e| set_realm.set(event_target_value(&e))
                                />
                            </div>
                            <div class="form-group">
                                <label>"KDC Server"</label>
                                <input
                                    type="text"
                                    placeholder="kdc.example.com"
                                    prop:value=move || kdc.get()
                                    on:input=move |e| set_kdc.set(event_target_value(&e))
                                />
                            </div>
                            <div class="form-group">
                                <label>"Admin Server (optional)"</label>
                                <input
                                    type="text"
                                    placeholder="admin.example.com"
                                    prop:value=move || admin_server.get()
                                    on:input=move |e| set_admin_server.set(event_target_value(&e))
                                />
                            </div>
                            <div class="button-group">
                                <button type="button" class="btn btn-primary" on:click=save_config>
                                    "Save"
                                </button>
                                <button type="button" class="btn" on:click=move |_| set_show_config.set(false)>
                                    "Cancel"
                                </button>
                            </div>
                        </form>
                    </div>
                </div>
            })}
        </div>
    }
}

#[component]
fn AdPanel() -> impl IntoView {
    let (status, set_status) = create_signal(None::<AdStatus>);
    let (loading, set_loading) = create_signal(true);
    let (show_join, set_show_join) = create_signal(false);
    let (message, set_message) = create_signal(None::<(String, bool)>);

    // Form fields
    let (domain, set_domain) = create_signal(String::new());
    let (username, set_username) = create_signal(String::new());
    let (password, set_password) = create_signal(String::new());
    let (computer_ou, set_computer_ou) = create_signal(String::new());
    let (register_dns, set_register_dns) = create_signal(true);

    // Load status
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            if let Ok(resp) = reqwasm::http::Request::get("/api/nas/directory/ad/status")
                .send()
                .await
            {
                if resp.ok() {
                    if let Ok(data) = resp.json::<AdStatus>().await {
                        set_status.set(Some(data));
                    }
                }
            }
            set_loading.set(false);
        });
    });

    let join_domain = move |_| {
        let form = AdJoinForm {
            domain: domain.get(),
            username: username.get(),
            password: password.get(),
            computer_ou: computer_ou.get(),
            register_dns: register_dns.get(),
        };

        spawn_local(async move {
            match reqwasm::http::Request::post("/api/nas/directory/ad/join")
                .header("Content-Type", "application/json")
                .body(serde_json::to_string(&form).unwrap())
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    set_message.set(Some(("Successfully joined domain".to_string(), true)));
                    set_show_join.set(false);
                }
                _ => {
                    set_message.set(Some(("Failed to join domain".to_string(), false)));
                }
            }
        });
    };

    let leave_domain = move |_| {
        if !web_sys::window()
            .and_then(|w| w.confirm_with_message("Are you sure you want to leave the domain?").ok())
            .unwrap_or(false)
        {
            return;
        }

        spawn_local(async move {
            match reqwasm::http::Request::post("/api/nas/directory/ad/leave")
                .header("Content-Type", "application/json")
                .body("{}")
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    set_message.set(Some(("Successfully left domain".to_string(), true)));
                }
                _ => {
                    set_message.set(Some(("Failed to leave domain".to_string(), false)));
                }
            }
        });
    };

    let test_trust = move |_| {
        spawn_local(async move {
            match reqwasm::http::Request::get("/api/nas/directory/ad/trust/test")
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    set_message.set(Some(("Trust relationship is healthy".to_string(), true)));
                }
                _ => {
                    set_message.set(Some(("Trust test failed".to_string(), false)));
                }
            }
        });
    };

    view! {
        <div class="panel ad-panel">
            <h2>"Active Directory"</h2>

            {move || message.get().map(|(msg, success)| {
                let class = if success { "alert alert-success" } else { "alert alert-error" };
                view! { <div class={class}>{msg}</div> }
            })}

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading AD status..."</p> }.into_view()
                } else if let Some(s) = status.get() {
                    view! {
                        <div class="status-card">
                            <div class="status-row">
                                <span class="label">"Domain Joined:"</span>
                                <span class={if s.joined { "status-active" } else { "status-inactive" }}>
                                    {if s.joined { "Yes" } else { "No" }}
                                </span>
                            </div>
                            {s.domain.map(|d| view! {
                                <div class="status-row">
                                    <span class="label">"Domain:"</span>
                                    <span>{d}</span>
                                </div>
                            })}
                            {s.domain_controller.map(|dc| view! {
                                <div class="status-row">
                                    <span class="label">"Domain Controller:"</span>
                                    <span>{dc}</span>
                                </div>
                            })}
                            <div class="status-row">
                                <span class="label">"Winbind Running:"</span>
                                <span class={if s.winbind_running { "status-active" } else { "status-inactive" }}>
                                    {if s.winbind_running { "Yes" } else { "No" }}
                                </span>
                            </div>
                            <div class="status-row">
                                <span class="label">"Kerberos Realm:"</span>
                                <span>{s.kerberos_realm.clone()}</span>
                            </div>
                            <div class="status-row">
                                <span class="label">"ID Map Backend:"</span>
                                <span>{s.idmap_backend.clone()}</span>
                            </div>
                        </div>

                        <div class="button-group">
                            {(!s.joined).then(|| view! {
                                <button class="btn btn-primary" on:click=move |_| set_show_join.set(true)>
                                    "Join Domain"
                                </button>
                            })}
                            {s.joined.then(|| view! {
                                <>
                                    <button class="btn" on:click=test_trust>
                                        "Test Trust"
                                    </button>
                                    <button class="btn btn-danger" on:click=leave_domain>
                                        "Leave Domain"
                                    </button>
                                </>
                            })}
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <p>"Active Directory not configured"</p>
                        <button class="btn btn-primary" on:click=move |_| set_show_join.set(true)>
                            "Join Domain"
                        </button>
                    }.into_view()
                }
            }}

            {move || show_join.get().then(|| view! {
                <div class="modal-overlay">
                    <div class="modal">
                        <h3>"Join Active Directory Domain"</h3>
                        <form class="config-form">
                            <div class="form-group">
                                <label>"Domain"</label>
                                <input
                                    type="text"
                                    placeholder="CORP.EXAMPLE.COM"
                                    prop:value=move || domain.get()
                                    on:input=move |e| set_domain.set(event_target_value(&e))
                                />
                            </div>
                            <div class="form-group">
                                <label>"Admin Username"</label>
                                <input
                                    type="text"
                                    placeholder="Administrator"
                                    prop:value=move || username.get()
                                    on:input=move |e| set_username.set(event_target_value(&e))
                                />
                            </div>
                            <div class="form-group">
                                <label>"Password"</label>
                                <input
                                    type="password"
                                    prop:value=move || password.get()
                                    on:input=move |e| set_password.set(event_target_value(&e))
                                />
                            </div>
                            <div class="form-group">
                                <label>"Computer OU (optional)"</label>
                                <input
                                    type="text"
                                    placeholder="OU=Computers,DC=corp,DC=example,DC=com"
                                    prop:value=move || computer_ou.get()
                                    on:input=move |e| set_computer_ou.set(event_target_value(&e))
                                />
                            </div>
                            <div class="form-group checkbox">
                                <label>
                                    <input
                                        type="checkbox"
                                        prop:checked=move || register_dns.get()
                                        on:change=move |e| set_register_dns.set(event_target_checked(&e))
                                    />
                                    " Register DNS record"
                                </label>
                            </div>
                            <div class="button-group">
                                <button type="button" class="btn btn-primary" on:click=join_domain>
                                    "Join Domain"
                                </button>
                                <button type="button" class="btn" on:click=move |_| set_show_join.set(false)>
                                    "Cancel"
                                </button>
                            </div>
                        </form>
                    </div>
                </div>
            })}
        </div>
    }
}
