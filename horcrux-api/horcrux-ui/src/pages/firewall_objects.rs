//! Network > Firewall Objects (`/network/firewall-objects`): IP sets, aliases,
//! macros, host default policies and the firewall log. Use them in rules as
//! `+ipset` / alias in source and destination, `macro:SSH` as the port.
use crate::api::{self, FwAlias, FwIpSet, FwLogEntry, FwMacro, FwOptions};
use leptos::*;

#[component]
pub fn FirewallObjectsPage() -> impl IntoView {
    let (sets, set_sets) = create_signal(Vec::<FwIpSet>::new());
    let (aliases, set_aliases) = create_signal(Vec::<FwAlias>::new());
    let (macros, set_macros) = create_signal(Vec::<FwMacro>::new());
    let (opts, set_opts) = create_signal(None::<FwOptions>);
    let (log, set_log) = create_signal(Vec::<FwLogEntry>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (set_name, set_set_name) = create_signal(String::new());
    let (alias_name, set_alias_name) = create_signal(String::new());
    let (alias_cidr, set_alias_cidr) = create_signal(String::new());

    let reload = move || {
        spawn_local(async move {
            if let Ok(v) = api::list_fw_ipsets().await {
                set_sets.set(v);
            }
            if let Ok(v) = api::list_fw_aliases().await {
                set_aliases.set(v);
            }
            if let Ok(o) = api::get_fw_options().await {
                set_opts.set(Some(o));
            }
        });
    };
    let reload_log = move || {
        spawn_local(async move {
            match api::get_fw_log(200).await {
                Ok(v) => set_log.set(v),
                Err(e) => {
                    set_log.set(Vec::new());
                    set_error.set(Some(e.message));
                }
            }
        });
    };
    create_effect(move |_| {
        reload();
        reload_log();
        spawn_local(async move {
            if let Ok(m) = api::list_fw_macros().await {
                set_macros.set(m);
            }
        });
    });

    let report = move |r: Result<String, String>| {
        match r {
            Ok(m) => {
                set_notice.set(Some(m));
                set_error.set(None);
            }
            Err(e) => set_error.set(Some(e)),
        }
        reload();
    };

    let create_set = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let name = set_name.get().trim().to_string();
        spawn_local(async move {
            let r = api::create_fw_ipset(&FwIpSet {
                name: name.clone(),
                ..Default::default()
            })
            .await
            .map(|_| format!("IP set {name} created"))
            .map_err(|e| e.message);
            if r.is_ok() {
                set_set_name.set(String::new());
            }
            report(r);
        });
    };
    let create_alias = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let a = FwAlias {
            name: alias_name.get().trim().to_string(),
            cidr: alias_cidr.get().trim().to_string(),
            comment: String::new(),
        };
        spawn_local(async move {
            let r = api::create_fw_alias(&a)
                .await
                .map(|a| format!("Alias {} created", a.name))
                .map_err(|e| e.message);
            if r.is_ok() {
                set_alias_name.set(String::new());
                set_alias_cidr.set(String::new());
            }
            report(r);
        });
    };
    let save_policy = move |field: &'static str, value: String| {
        let Some(mut o) = opts.get_untracked() else {
            return;
        };
        if field == "in" {
            o.policy_in = value;
        } else {
            o.policy_out = value;
        }
        spawn_local(async move {
            let r = api::set_fw_options(&o)
                .await
                .map(|_| "Policy saved; press Apply to install it on the host".to_string())
                .map_err(|e| e.message);
            report(r);
        });
    };
    let apply = move |_| {
        spawn_local(async move {
            let r = api::apply_datacenter_firewall()
                .await
                .map(|_| "Datacenter firewall applied".to_string())
                .map_err(|e| e.message);
            report(r);
        });
    };

    view! {
        <div class="firewall-objects">
            <div class="page-header"><h1>"Firewall Objects"</h1></div>
            <p class="form-help">
                "In a rule use "<code>"+ipset"</code>" or an alias name as source/destination and "
                <code>"macro:SSH"</code>" as the destination port. Changes reach the kernel immediately."
            </p>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="fwo-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="fwo-error">{e}</div> })}

            <h2>"Default policy (host)"</h2>
            {move || opts.get().map(|o| view! {
                <div class="form-row" id="fwo-policy">
                    <label>"Input "
                        <select id="fwo-policy-in" on:change=move |ev| save_policy("in", event_target_value(&ev))>
                            <option value="accept" selected=o.policy_in == "accept">"accept"</option>
                            <option value="drop" selected=o.policy_in == "drop">"drop"</option>
                        </select></label>
                    <label>"Output "
                        <select id="fwo-policy-out" on:change=move |ev| save_policy("out", event_target_value(&ev))>
                            <option value="accept" selected=o.policy_out == "accept">"accept"</option>
                            <option value="drop" selected=o.policy_out == "drop">"drop"</option>
                        </select></label>
                    <button id="fwo-apply" class="btn btn-primary" on:click=apply>"Apply"</button>
                    <span class="form-help">"With drop, replies, loopback, SSH (22) and the API (8006) stay allowed."</span>
                </div>
            })}

            <h2>"IP sets"</h2>
            <form id="fwo-set-form" class="form-row" on:submit=create_set>
                <input id="fwo-set-name" type="text" required placeholder="name" prop:value=set_name
                    on:input=move |ev| set_set_name.set(event_target_value(&ev))/>
                <button type="submit" class="btn btn-primary">"Create IP set"</button>
            </form>
            {move || sets.get().into_iter().map(|s| {
                let name = s.name.clone();
                let (entry, set_entry) = create_signal(String::new());
                let add_name = name.clone();
                let del_name = name.clone();
                view! {
                    <div class="card ipset" data-ipset=s.name.clone()>
                        <h3>{format!("+{}", s.name)}</h3>
                        <ul>
                            {s.entries.iter().map(|e| {
                                let (n, c) = (name.clone(), e.clone());
                                view! { <li><code>{e.clone()}</code>" "
                                    <button class="btn btn-sm btn-danger ipset-entry-delete" on:click=move |_| {
                                        let (n, c) = (n.clone(), c.clone());
                                        spawn_local(async move {
                                            let r = api::remove_fw_ipset_entry(&n, &c).await.map(|_| format!("{c} removed")).map_err(|e| e.message);
                                            report(r);
                                        });
                                    }>"Remove"</button></li> }
                            }).collect_view()}
                        </ul>
                        <div class="form-row">
                            <input class="ipset-entry" type="text" placeholder="10.0.0.0/24" prop:value=entry
                                on:input=move |ev| set_entry.set(event_target_value(&ev))/>
                            <button class="btn btn-sm ipset-entry-add" on:click=move |_| {
                                let (n, c) = (add_name.clone(), entry.get_untracked());
                                spawn_local(async move {
                                    let r = api::add_fw_ipset_entry(&n, c.trim()).await.map(|_| format!("{c} added")).map_err(|e| e.message);
                                    if r.is_ok() { set_entry.set(String::new()); }
                                    report(r);
                                });
                            }>"Add"</button>
                            <button class="btn btn-sm btn-danger ipset-delete" on:click=move |_| {
                                let n = del_name.clone();
                                spawn_local(async move {
                                    let r = api::delete_fw_ipset(&n).await.map(|_| format!("IP set {n} deleted")).map_err(|e| e.message);
                                    report(r);
                                });
                            }>"Delete set"</button>
                        </div>
                    </div>
                }
            }).collect_view()}

            <h2>"Aliases"</h2>
            <form id="fwo-alias-form" class="form-row" on:submit=create_alias>
                <input id="fwo-alias-name" type="text" required placeholder="name" prop:value=alias_name
                    on:input=move |ev| set_alias_name.set(event_target_value(&ev))/>
                <input id="fwo-alias-cidr" type="text" required placeholder="10.0.0.5 or 10.0.0.0/24" prop:value=alias_cidr
                    on:input=move |ev| set_alias_cidr.set(event_target_value(&ev))/>
                <button type="submit" class="btn btn-primary">"Create alias"</button>
            </form>
            <table class="vm-table" id="fwo-alias-table">
                <thead><tr><th>"Name"</th><th>"Address"</th><th>"Actions"</th></tr></thead>
                <tbody>
                    {move || aliases.get().into_iter().map(|a| {
                        let n = a.name.clone();
                        view! { <tr class="alias-row"><td>{a.name.clone()}</td><td><code>{a.cidr.clone()}</code></td>
                            <td><button class="btn btn-sm btn-danger alias-delete" on:click=move |_| {
                                let n = n.clone();
                                spawn_local(async move {
                                    let r = api::delete_fw_alias(&n).await.map(|_| format!("Alias {n} deleted")).map_err(|e| e.message);
                                    report(r);
                                });
                            }>"Delete"</button></td></tr> }
                    }).collect_view()}
                </tbody>
            </table>

            <h2>"Macros"</h2>
            <table class="vm-table" id="fwo-macro-table">
                <thead><tr><th>"Macro"</th><th>"Service"</th><th>"Ports"</th></tr></thead>
                <tbody>
                    {move || macros.get().into_iter().map(|m| view! {
                        <tr><td><code>{format!("macro:{}", m.name)}</code></td><td>{m.description.clone()}</td>
                            <td>{m.rules.iter().map(|(p, port)| format!("{p}/{port}")).collect::<Vec<_>>().join(", ")}</td></tr>
                    }).collect_view()}
                </tbody>
            </table>

            <h2>"Log" <button class="btn btn-sm" on:click=move |_| reload_log()>"Refresh"</button></h2>
            <p class="form-help">"Packets matched by rules with logging on (kernel log, newest first)."</p>
            <table class="vm-table" id="fwo-log-table">
                <thead><tr><th>"Time"</th><th>"In"</th><th>"Out"</th><th>"Source"</th><th>"Destination"</th><th>"Proto"</th><th>"Ports"</th></tr></thead>
                <tbody>
                    {move || {
                        let l = log.get();
                        if l.is_empty() {
                            view! { <tr><td colspan="7" class="no-data">"No firewall log entries."</td></tr> }.into_view()
                        } else {
                            l.into_iter().map(|e| view! {
                                <tr><td>{e.time.clone()}</td><td>{e.iface_in.clone()}</td><td>{e.iface_out.clone()}</td>
                                    <td>{e.src.clone()}</td><td>{e.dst.clone()}</td><td>{e.proto.clone()}</td>
                                    <td>{format!("{} → {}", e.sport, e.dport)}</td></tr>
                            }).collect_view()
                        }
                    }}
                </tbody>
            </table>
        </div>
    }
}
