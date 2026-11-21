//! VM Remote Play page (`/vms/:id/remote-play`; docs/design/gaming-vm.md
//! section 16): one toggle for Steam Remote Play / Steam Link (and, if
//! chosen, Moonlight), the options, the prerequisite checklist with each
//! item's status, the rules Horcrux keeps for it (port forwards, firewall
//! allow rules, discovery relay) and the guest setup (Steam install,
//! automatic logon, Big Picture, no sleep). Node-wide ports and listeners
//! are in the collapsible node settings at the bottom.
use crate::api::remote_play as rpapi;
use horcrux_common::remote_play::{
    GuestSetupRequest, RemotePlayClient, RemotePlayNetwork, RemotePlaySettings, RemotePlayStatus,
    VmRemotePlay,
};
use leptos::*;
use leptos_router::*;

#[derive(Params, PartialEq, Clone)]
struct VmParams {
    id: Option<String>,
}

fn status_icon(s: &str) -> &'static str {
    match s {
        "ok" => "OK",
        "warn" => "Check",
        "fail" => "Missing",
        "unknown" => "Unknown",
        _ => "Note",
    }
}

#[component]
pub fn VmRemotePlayPage() -> impl IntoView {
    let params = use_params::<VmParams>();
    let id = move || params.get().ok().and_then(|p| p.id).unwrap_or_default();
    let cfg = create_rw_signal(VmRemotePlay::default());
    let sources_text = create_rw_signal(String::new());
    let (st, set_st) = create_signal(None::<RemotePlayStatus>);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    let install_steam = create_rw_signal(true);
    let al_user = create_rw_signal(String::new());
    let al_pass = create_rw_signal(String::new());
    let al_off = create_rw_signal(false);
    let show_node = create_rw_signal(false);
    let node = create_rw_signal(RemotePlaySettings::default());
    let wol_text = create_rw_signal(String::new());

    let load_status = move || {
        let vid = id();
        if vid.is_empty() {
            return;
        }
        spawn_local(async move {
            match rpapi::status(&vid).await {
                Ok(s) => {
                    cfg.set(s.config.clone());
                    sources_text.set(s.config.lan_sources.join(", "));
                    set_st.set(Some(s));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    create_effect(move |_| {
        if !id().is_empty() {
            crate::titles::set_subject(&id());
            load_status();
        }
    });
    spawn_local(async move {
        if let Ok(s) = rpapi::get_settings().await {
            wol_text.set(
                s.wol_ports
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            node.set(s);
        }
    });

    let save = move |_| {
        let vid = id();
        let mut c = cfg.get_untracked();
        c.lan_sources = sources_text
            .get_untracked()
            .split([',', ' ', '\n'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        set_busy.set(true);
        spawn_local(async move {
            match rpapi::put(&vid, &c).await {
                Ok(_) => {
                    set_notice.set(Some("Saved: the node applies it at once.".into()));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
            // the service reconciles right after a change
            gloo_timers::future::TimeoutFuture::new(1500).await;
            load_status();
        });
    };

    let lan_nic = move |_| {
        let vid = id();
        set_busy.set(true);
        spawn_local(async move {
            match rpapi::lan_nic(&vid).await {
                Ok(v) => {
                    let n = v
                        .hardware
                        .nets
                        .first()
                        .map(|n| n.bridge.clone())
                        .unwrap_or_default();
                    set_notice.set(Some(format!(
                        "The network card is now on {n} (takes effect at the next start)."
                    )));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
            load_status();
        });
    };

    let guest_setup = move |_| {
        let vid = id();
        let req = GuestSetupRequest {
            install_steam: install_steam.get_untracked()
                && cfg
                    .get_untracked()
                    .clients
                    .contains(&RemotePlayClient::Steam),
            auto_login_user: Some(al_user.get_untracked().trim().to_string())
                .filter(|u| !u.is_empty()),
            password: Some(al_pass.get_untracked()).filter(|p| !p.is_empty()),
            auto_login_off: al_off.get_untracked(),
        };
        set_busy.set(true);
        set_notice.set(Some(
            "Setting up the guest (installing Steam can take a few minutes)...".into(),
        ));
        spawn_local(async move {
            match rpapi::guest_setup(&vid, &req).await {
                Ok(r) => {
                    al_pass.set(String::new());
                    set_notice.set(Some(if r.messages.is_empty() {
                        "Guest checked.".into()
                    } else {
                        format!("Guest: {}", r.messages.join("; "))
                    }));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            set_busy.set(false);
            load_status();
        });
    };

    let save_node = move |_| {
        let mut s = node.get_untracked();
        s.wol_ports = wol_text
            .get_untracked()
            .split([',', ' '])
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        spawn_local(async move {
            match rpapi::put_settings(&s).await {
                Ok(s) => {
                    node.set(s);
                    set_notice.set(Some("Node Remote Play settings saved.".into()));
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
            load_status();
        });
    };

    let client_box = move |c: RemotePlayClient, elem_id: &'static str| {
        view! {
            <label class="checkbox">
                <input type="checkbox" id=elem_id
                    prop:checked=move || cfg.get().clients.contains(&c)
                    on:change=move |ev| {
                        let on = event_target_checked(&ev);
                        cfg.update(|x| {
                            x.clients.retain(|y| *y != c);
                            if on {
                                x.clients.push(c);
                                x.clients.sort();
                            }
                        });
                    }/>
                {c.label()}
            </label>
        }
    };
    let flag = move |elem_id: &'static str,
                     label: &'static str,
                     get: fn(&VmRemotePlay) -> bool,
                     set: fn(&mut VmRemotePlay, bool)| {
        view! {
            <label class="checkbox">
                <input type="checkbox" id=elem_id
                    prop:checked=move || get(&cfg.get())
                    on:change=move |ev| {
                        let on = event_target_checked(&ev);
                        cfg.update(|x| set(x, on));
                    }/>
                {label}
            </label>
        }
    };
    let net_value = |n: RemotePlayNetwork| match n {
        RemotePlayNetwork::Auto => "auto",
        RemotePlayNetwork::Lan => "lan",
        RemotePlayNetwork::Nat => "nat",
    };

    view! {
        <div class="vm-remote-play">
            <div class="page-header"><h1>"Remote Play: " {id}</h1></div>
            <nav class="tabs" id="vm-tabs">
                <A href=move || format!("/vms/{}", id()) class="tab">"Summary"</A>
                <A href=move || format!("/vms/{}/hardware", id()) class="tab">"Hardware"</A>
                <A href=move || format!("/vms/{}/console", id()) class="tab">"Console"</A>
                <A href=move || format!("/vms/{}/remote-play", id()) class="tab tab-active">"Remote Play"</A>
            </nav>
            <p class="form-help">
                "Stream games from this VM with Steam Remote Play / Steam Link (TVs, phones, tablets, other PCs, Remote Play Together) or, if you choose it, Moonlight. Steam (or Sunshine) runs inside the VM; Horcrux makes the network reach it, wakes the VM when a client asks, and sets the guest up so Steam is ready after a start. The Horcrux Console keeps working alongside."
            </p>
            {move || notice.get().map(|n| view! { <div class="alert alert-info" id="rp-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="rp-error">{e}</div> })}

            <section class="card" id="rp-settings">
                {flag("rp-enabled", "Remote Play ready", |c| c.enabled, |c, v| c.enabled = v)}
                {client_box(RemotePlayClient::Steam, "rp-steam")}
                {client_box(RemotePlayClient::Moonlight, "rp-moonlight")}
                <div class="form-group">
                    <label for="rp-network">"Network"</label>
                    <select id="rp-network" on:change=move |ev| {
                        let v = event_target_value(&ev);
                        cfg.update(|c| c.network = match v.as_str() {
                            "lan" => RemotePlayNetwork::Lan,
                            "nat" => RemotePlayNetwork::Nat,
                            _ => RemotePlayNetwork::Auto,
                        });
                    }>
                        {[RemotePlayNetwork::Auto, RemotePlayNetwork::Lan, RemotePlayNetwork::Nat].into_iter().map(|n| view! {
                            <option value=net_value(n) selected=move || cfg.get().network == n>{match n {
                                RemotePlayNetwork::Auto => "Automatic (LAN if the network card is on the LAN, else NAT with relay)",
                                RemotePlayNetwork::Lan => "LAN: the VM's card is on the LAN (recommended)",
                                RemotePlayNetwork::Nat => "NAT: Horcrux relays discovery and forwards the ports",
                            }}</option>
                        }).collect_view()}
                    </select>
                </div>
                <div class="form-group">
                    <label for="rp-sources">"Allowed networks (CIDR, comma-separated; empty = this host's LAN networks)"</label>
                    <input id="rp-sources" type="text" prop:value=sources_text placeholder="192.168.1.0/24"
                        on:input=move |ev| sources_text.set(event_target_value(&ev))/>
                </div>
                {flag("rp-wol", "Wake-on-LAN: a magic packet for the VM's MAC starts (or resumes) it", |c| c.wake_on_lan, |c, v| c.wake_on_lan = v)}
                {flag("rp-wod", "Also start it when a Steam client looks for it (every open Steam client does, all the time)", |c| c.wake_on_discovery, |c, v| c.wake_on_discovery = v)}
                {flag("rp-bigpicture", "Open Steam in Big Picture at logon (guest setup)", |c| c.big_picture, |c, v| c.big_picture = v)}
                {flag("rp-keepawake", "Keep the guest from sleeping (guest setup)", |c| c.keep_awake, |c, v| c.keep_awake = v)}
                <button class="btn btn-primary" id="rp-save" disabled=busy on:click=save>"Save"</button>
            </section>

            <section class="card" id="rp-checklist">
                <h2>"Checklist"</h2>
                {move || st.get().map(|s| view! {
                    <p class="form-help" id="rp-summary">{format!(
                        "Network: {} | card: {} | MAC: {} | address: {}",
                        if s.network.is_empty() { "-".to_string() } else { s.network.clone() },
                        if s.nic.is_empty() { "-".to_string() } else { s.nic.clone() },
                        s.mac.clone().unwrap_or_else(|| "-".into()),
                        s.address.clone().unwrap_or_else(|| "-".into()),
                    )}</p>
                    <table class="vm-table" id="rp-checks">
                        <tr><th>"Status"</th><th>"Prerequisite"</th><th>"Details"</th><th>"What to do"</th></tr>
                        {s.checks.into_iter().map(|c| view! {
                            <tr class=format!("rp-check rp-status-{}", c.status) data-id=c.id.clone()>
                                <td class="rp-check-status">{status_icon(&c.status)}</td>
                                <td>{c.title}</td>
                                <td>{c.detail}</td>
                                <td>{c.fix}</td>
                            </tr>
                        }).collect_view()}
                    </table>
                })}
                <button class="btn" id="rp-refresh" on:click=move |_| load_status()>"Refresh"</button>
                <button class="btn" id="rp-lan-nic" disabled=busy on:click=lan_nic
                    title="Puts the VM's first network card on a bridge with a physical port, or directly on the wired uplink (macvtap). The VM must be stopped.">
                    "Use a LAN network card"
                </button>
            </section>

            <section class="card" id="rp-rules-card">
                <h2>"Rules Horcrux keeps for this VM"</h2>
                {move || st.get().map(|s| if s.rules.is_empty() {
                    view! { <p class="form-help" id="rp-no-rules">"None (off, or the VM is on the LAN without a Horcrux firewall)."</p> }.into_view()
                } else {
                    view! {
                        <table class="vm-table" id="rp-rules">
                            <tr><th>"Kind"</th><th>"Client"</th><th>"Protocol"</th><th>"Ports"</th><th>"From"</th><th>"To"</th><th>"Applied"</th></tr>
                            {s.rules.into_iter().map(|r| view! {
                                <tr class="rp-rule">
                                    <td>{match r.kind.as_str() {
                                        "forward" => "Port forward (NAT)",
                                        "allow" => "VM firewall allow",
                                        "relay" => "Discovery relay",
                                        _ => "Rule",
                                    }}</td>
                                    <td>{format!("{:?}", r.client)}</td>
                                    <td>{r.protocol}</td>
                                    <td>{r.ports}</td>
                                    <td>{r.sources.join(", ")}</td>
                                    <td>{if r.to.is_empty() { "(VM address not known yet)".to_string() } else { r.to }}</td>
                                    <td class="rp-applied">{if r.applied { "yes" } else { "no" }}</td>
                                </tr>
                            }).collect_view()}
                        </table>
                    }.into_view()
                })}
                {move || st.get().filter(|s| !s.nft.is_empty()).map(|s| view! {
                    <details><summary>"nftables"</summary><pre id="rp-nft">{s.nft}</pre></details>
                })}
            </section>

            <section class="card" id="rp-guest">
                <h2>"Guest setup"</h2>
                <p class="form-help">"Runs inside the running VM through the QEMU guest agent. Steam's installer is downloaded from Valve and only run when its Valve signature checks out. Automatic logon stores the password with Windows (as an LSA secret, like Microsoft's Autologon); Horcrux never stores it."</p>
                <label class="checkbox"><input type="checkbox" id="rp-install-steam" prop:checked=install_steam on:change=move |ev| install_steam.set(event_target_checked(&ev))/>"Install Steam if it is missing"</label>
                <div class="form-group">
                    <label for="rp-al-user">"Automatic logon: Windows account"</label>
                    <input id="rp-al-user" type="text" prop:value=al_user on:input=move |ev| al_user.set(event_target_value(&ev))/>
                </div>
                <div class="form-group">
                    <label for="rp-al-pass">"Password"</label>
                    <input id="rp-al-pass" type="password" autocomplete="off" prop:value=al_pass on:input=move |ev| al_pass.set(event_target_value(&ev))/>
                </div>
                <label class="checkbox"><input type="checkbox" id="rp-al-off" prop:checked=al_off on:change=move |ev| al_off.set(event_target_checked(&ev))/>"Turn automatic logon off"</label>
                <button class="btn btn-primary" id="rp-guest-setup" disabled=busy on:click=guest_setup>"Check / set up the guest"</button>
            </section>

            <section class="card" id="rp-node">
                <h2><a href="#" id="rp-node-toggle" on:click=move |ev| { ev.prevent_default(); show_node.update(|v| *v = !*v); }>"Node settings (all VMs)"</a></h2>
                {move || show_node.get().then(|| view! {
                    <div class="form-group">
                        <label for="rp-steam-tcp">"Steam TCP ports"</label>
                        <input id="rp-steam-tcp" type="text" prop:value=move || node.get().steam.tcp on:input=move |ev| { let v = event_target_value(&ev); node.update(|s| s.steam.tcp = v); }/>
                        <label for="rp-steam-udp">"Steam UDP ports"</label>
                        <input id="rp-steam-udp" type="text" prop:value=move || node.get().steam.udp on:input=move |ev| { let v = event_target_value(&ev); node.update(|s| s.steam.udp = v); }/>
                        <label for="rp-moon-tcp">"Moonlight TCP ports"</label>
                        <input id="rp-moon-tcp" type="text" prop:value=move || node.get().moonlight.tcp on:input=move |ev| { let v = event_target_value(&ev); node.update(|s| s.moonlight.tcp = v); }/>
                        <label for="rp-moon-udp">"Moonlight UDP ports"</label>
                        <input id="rp-moon-udp" type="text" prop:value=move || node.get().moonlight.udp on:input=move |ev| { let v = event_target_value(&ev); node.update(|s| s.moonlight.udp = v); }/>
                        <label for="rp-wol-ports">"Wake-on-LAN UDP ports"</label>
                        <input id="rp-wol-ports" type="text" prop:value=wol_text on:input=move |ev| wol_text.set(event_target_value(&ev))/>
                        <label class="checkbox"><input type="checkbox" id="rp-relay" prop:checked=move || node.get().discovery_relay on:change=move |ev| { let on = event_target_checked(&ev); node.update(|s| s.discovery_relay = on); }/>"Relay Steam discovery for VMs behind NAT"</label>
                    </div>
                    <button class="btn" id="rp-node-save" on:click=save_node>"Save node settings"</button>
                })}
            </section>
        </div>
    }
}
