use crate::api::*;
use leptos::*;
use wasm_bindgen::JsCast;
use web_sys::MouseEvent;

#[component]
pub fn SystemConfigurationPage() -> impl IntoView {
    let (system_config, set_system_config) = create_signal(None::<SystemConfiguration>);
    let (net_unavailable, set_net_unavailable) = create_signal(None::<crate::api::Unavailable>);
    let (network_interfaces, set_network_interfaces) =
        create_signal(Vec::<NetworkInterface>::new());
    let (dns_config, set_dns_config) = create_signal(None::<DnsConfiguration>);
    let (ntp_config, set_ntp_config) = create_signal(None::<NtpConfiguration>);
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("general".to_string());

    // General configuration
    let (form_hostname, set_form_hostname) = create_signal(String::new());
    let (form_domain, set_form_domain) = create_signal(String::new());
    let (form_timezone, set_form_timezone) = create_signal(String::new());
    let (form_locale, set_form_locale) = create_signal(String::new());
    // Zone/locale choices read from the node (zoneinfo, `locale -a`)
    let (tz_choices, set_tz_choices) = create_signal(Vec::<String>::new());
    let (locale_choices, set_locale_choices) = create_signal(Vec::<String>::new());

    // Network configuration
    let (selected_interface, set_selected_interface) = create_signal(None::<NetworkInterface>);
    let (show_interface_modal, set_show_interface_modal) = create_signal(false);
    let (form_interface_name, set_form_interface_name) = create_signal(String::new());
    let (form_interface_method, set_form_interface_method) = create_signal("dhcp".to_string());
    let (form_interface_address, set_form_interface_address) = create_signal(String::new());
    let (form_interface_netmask, set_form_interface_netmask) = create_signal(String::new());
    let (form_interface_gateway, set_form_interface_gateway) = create_signal(String::new());
    let (form_interface_mtu, set_form_interface_mtu) = create_signal(1500);

    // DNS configuration
    let (form_dns_servers, set_form_dns_servers) = create_signal(String::new());
    let (form_dns_search_domains, set_form_dns_search_domains) = create_signal(String::new());

    // NTP configuration
    let (form_ntp_servers, set_form_ntp_servers) = create_signal(String::new());
    let (form_ntp_timezone, set_form_ntp_timezone) = create_signal(String::new());

    // Load system configuration
    let load_data = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_system_configuration().await {
            Ok(config) => {
                set_form_hostname.set(config.hostname.clone());
                set_form_domain.set(config.domain.clone().unwrap_or_default());
                set_form_timezone.set(config.timezone.clone());
                set_form_locale.set(config.locale.clone());
                set_system_config.set(Some(config));
            }
            Err(e) => {
                set_error_message.set(Some(format!("Failed to load system configuration: {}", e)))
            }
        }

        match get_system_config_options().await {
            Ok(o) => {
                set_tz_choices.set(o.timezones);
                set_locale_choices.set(o.locales);
            }
            // the selects still show the node's current values
            Err(e) => set_error_message.set(Some(format!(
                "Failed to load the time zone and locale lists: {}",
                e
            ))),
        }

        match get_network_interfaces().await {
            Ok(interfaces) => set_network_interfaces.set(interfaces),
            // 424: the node can't list interfaces (no iproute2): an empty state in the section
            Err(e) if e.unavailable().is_some() => set_net_unavailable.set(e.unavailable()),
            Err(e) => {
                set_error_message.set(Some(format!("Failed to load network interfaces: {}", e)))
            }
        }

        match get_dns_configuration().await {
            Ok(dns) => {
                set_form_dns_servers.set(dns.servers.join("\n"));
                set_form_dns_search_domains.set(dns.search_domains.join(" "));
                set_dns_config.set(Some(dns));
            }
            Err(e) => {
                set_error_message.set(Some(format!("Failed to load DNS configuration: {}", e)))
            }
        }

        match get_ntp_configuration().await {
            Ok(ntp) => {
                set_form_ntp_servers.set(ntp.servers.join("\n"));
                set_form_ntp_timezone.set(ntp.timezone.clone());
                set_ntp_config.set(Some(ntp));
            }
            Err(e) => {
                set_error_message.set(Some(format!("Failed to load NTP configuration: {}", e)))
            }
        }

        set_loading.set(false);
    });

    // Save general configuration
    let save_general_config = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let config = SystemConfiguration {
            hostname: form_hostname.get(),
            domain: if form_domain.get().is_empty() {
                None
            } else {
                Some(form_domain.get())
            },
            timezone: form_timezone.get(),
            locale: form_locale.get(),
        };

        match update_system_configuration(config).await {
            Ok(_) => {
                set_success_message
                    .set(Some("General configuration saved successfully".to_string()));
                load_data.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to save configuration: {}", e))),
        }

        set_loading.set(false);
    });

    // Clear interface form
    let clear_interface_form = move || {
        set_form_interface_name.set(String::new());
        set_form_interface_method.set("dhcp".to_string());
        set_form_interface_address.set(String::new());
        set_form_interface_netmask.set(String::new());
        set_form_interface_gateway.set(String::new());
        set_form_interface_mtu.set(1500);
    };

    // Save network interface
    let save_interface = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let interface = NetworkInterface {
            name: form_interface_name.get(),
            method: form_interface_method.get(),
            address: if form_interface_address.get().is_empty() {
                None
            } else {
                Some(form_interface_address.get())
            },
            netmask: if form_interface_netmask.get().is_empty() {
                None
            } else {
                Some(form_interface_netmask.get())
            },
            gateway: if form_interface_gateway.get().is_empty() {
                None
            } else {
                Some(form_interface_gateway.get())
            },
            mtu: Some(form_interface_mtu.get()),
            auto: true,
            bridge: None,
            bridge_ports: None,
            vlan_id: None,
        };

        match update_network_interface(interface).await {
            Ok(_) => {
                set_success_message.set(Some("Network interface saved successfully".to_string()));
                set_show_interface_modal.set(false);
                clear_interface_form();
                load_data.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to save interface: {}", e))),
        }

        set_loading.set(false);
    });

    // Save DNS configuration
    let save_dns_config = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let dns = DnsConfiguration {
            servers: form_dns_servers
                .get()
                .lines()
                .map(|s| s.trim().to_string())
                .collect(),
            search_domains: form_dns_search_domains
                .get()
                .split_whitespace()
                .map(|s| s.to_string())
                .collect(),
        };

        match update_dns_configuration(dns).await {
            Ok(_) => {
                set_success_message.set(Some("DNS configuration saved successfully".to_string()));
                load_data.dispatch(());
            }
            Err(e) => {
                set_error_message.set(Some(format!("Failed to save DNS configuration: {}", e)))
            }
        }

        set_loading.set(false);
    });

    // Save NTP configuration
    let save_ntp_config = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let ntp = NtpConfiguration {
            servers: form_ntp_servers
                .get()
                .lines()
                .map(|s| s.trim().to_string())
                .collect(),
            timezone: form_ntp_timezone.get(),
        };

        match update_ntp_configuration(ntp).await {
            Ok(_) => {
                set_success_message.set(Some("NTP configuration saved successfully".to_string()));
                load_data.dispatch(());
            }
            Err(e) => {
                set_error_message.set(Some(format!("Failed to save NTP configuration: {}", e)))
            }
        }

        set_loading.set(false);
    });

    // Helper functions
    let clear_interface_form = move || {
        set_form_interface_name.set(String::new());
        set_form_interface_method.set("dhcp".to_string());
        set_form_interface_address.set(String::new());
        set_form_interface_netmask.set(String::new());
        set_form_interface_gateway.set(String::new());
        set_form_interface_mtu.set(1500);
    };

    let init_interface_form = move |interface: &NetworkInterface| {
        set_form_interface_name.set(interface.name.clone());
        set_form_interface_method.set(interface.method.clone());
        set_form_interface_address.set(interface.address.clone().unwrap_or_default());
        set_form_interface_netmask.set(interface.netmask.clone().unwrap_or_default());
        set_form_interface_gateway.set(interface.gateway.clone().unwrap_or_default());
        set_form_interface_mtu.set(interface.mtu.unwrap_or(1500));
    };

    let get_interface_status = |interface: &NetworkInterface| {
        if interface.auto {
            "Enabled"
        } else {
            "Disabled"
        }
    };

    let get_interface_status_color = |interface: &NetworkInterface| {
        if interface.auto {
            "text-green-600"
        } else {
            "text-gray-500"
        }
    };

    // Clear messages after delay
    let clear_messages = move || {
        set_timeout(
            move || {
                set_success_message.set(None);
                set_error_message.set(None);
            },
            std::time::Duration::from_secs(5),
        );
    };

    // Initial load
    create_effect(move |_| {
        load_data.dispatch(());
    });

    view! {
        <div class="system-configuration-page">
            <div class="page-header">
                <h1>"System Configuration"</h1>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_data.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                </div>
            </div>

            {move || error_message.get().map(|msg| {
                clear_messages();
                view! {
                    <div class="alert alert-error">{msg}</div>
                }
            })}

            {move || success_message.get().map(|msg| {
                clear_messages();
                view! {
                    <div class="alert alert-success">{msg}</div>
                }
            })}

            <div class="configuration-tabs">
                <div class="tab-buttons">
                    <button
                        class={move || if active_tab.get() == "general" { "tab-button active" } else { "tab-button" }}
                        on:click=move |_| set_active_tab.set("general".to_string())
                    >
                        "General"
                    </button>
                    <button
                        class={move || if active_tab.get() == "network" { "tab-button active" } else { "tab-button" }}
                        on:click=move |_| set_active_tab.set("network".to_string())
                    >
                        "Network"
                    </button>
                    <button
                        class={move || if active_tab.get() == "dns" { "tab-button active" } else { "tab-button" }}
                        on:click=move |_| set_active_tab.set("dns".to_string())
                    >
                        "DNS"
                    </button>
                    <button
                        class={move || if active_tab.get() == "time" { "tab-button active" } else { "tab-button" }}
                        on:click=move |_| set_active_tab.set("time".to_string())
                    >
                        "Time & NTP"
                    </button>
                </div>

                <div class="tab-content">
                    {move || match active_tab.get().as_str() {
                        "general" => view! {
                            <DefaultEditorCard/>
                            <div class="general-config-section">
                                <h2>"General System Settings"</h2>
                                <div class="config-form">
                                    <div class="form-group">
                                        <label for="cfg-hostname">"Hostname"</label>
                                        <input id="cfg-hostname"
                                            type="text"
                                            prop:value=form_hostname
                                            on:input=move |ev| set_form_hostname.set(event_target_value(&ev))
                                            placeholder="node-01"
                                        />
                                        <small>"System hostname (without domain)"</small>
                                    </div>

                                    <div class="form-group">
                                        <label for="cfg-domain">"Domain"</label>
                                        <input id="cfg-domain"
                                            type="text"
                                            prop:value=form_domain
                                            on:input=move |ev| set_form_domain.set(event_target_value(&ev))
                                            placeholder="example.com"
                                        />
                                        <small>"Domain name for this system"</small>
                                    </div>

                                    <div class="form-row">
                                        <SearchSelect id="cfg-timezone" label="Timezone"
                                            choices=Signal::derive(move || tz_choices.get())
                                            value=form_timezone set_value=set_form_timezone
                                            placeholder="Search time zones"/>
                                        <SearchSelect id="cfg-locale" label="Locale"
                                            choices=Signal::derive(move || locale_choices.get())
                                            value=form_locale set_value=set_form_locale
                                            placeholder="Search locales"/>
                                    </div>

                                    <div class="form-actions">
                                        <button
                                            class="btn btn-primary"
                                            on:click=move |_| save_general_config.dispatch(())
                                            disabled=move || form_hostname.get().is_empty() || loading.get()
                                        >
                                            "Save General Settings"
                                        </button>
                                    </div>
                                </div>
                            </div>
                        }.into_view(),

                        "network" => view! {
                            <VmDefaultNetworkCard/>
                            <div class="network-config-section">
                                <div class="section-header">
                                    <h2>"Network Interfaces"</h2>
                                    <button
                                        class="btn btn-primary"
                                        on:click=move |_| {
                                            clear_interface_form();
                                            set_show_interface_modal.set(true);
                                        }
                                    >
                                        "Add Interface"
                                    </button>
                                </div>

                                {move || if loading.get() {
                                    view! { <div class="loading">"Loading network interfaces..."</div> }.into_view()
                                } else if let Some(u) = net_unavailable.get() {
                                    view! { <crate::components::UnavailableState info=u on_retry=Callback::new(move |_: ()| { if let Some(w) = web_sys::window() { let _ = w.location().reload(); } })/> }.into_view()
                                } else if network_interfaces.get().is_empty() {
                                    view! { <div class="empty-state">"No network interfaces configured"</div> }.into_view()
                                } else {
                                    view! {
                                        <div class="interfaces-table">
                                            <table>
                                                <thead>
                                                    <tr>
                                                        <th>"Interface"</th>
                                                        <th>"Method"</th>
                                                        <th>"Address"</th>
                                                        <th>"Gateway"</th>
                                                        <th>"MTU"</th>
                                                        <th>"Status"</th>
                                                        <th>"Actions"</th>
                                                    </tr>
                                                </thead>
                                                <tbody>
                                                    {network_interfaces.get().into_iter().map(|interface| {
                                                        let interface_clone = interface.clone();
                                                        let status_color = get_interface_status_color(&interface);
                                                        let status_text = get_interface_status(&interface);
                                                        let name = interface.name.clone();
                                                        let method = interface.method.clone();
                                                        let address = interface.address.clone().unwrap_or_else(|| "-".to_string());
                                                        let gateway = interface.gateway.clone().unwrap_or_else(|| "-".to_string());
                                                        let mtu = interface.mtu.unwrap_or(1500);
                                                        view! {
                                                            <tr>
                                                                <td>{name}</td>
                                                                <td>
                                                                    <span class="method-badge">{method}</span>
                                                                </td>
                                                                <td>{address}</td>
                                                                <td>{gateway}</td>
                                                                <td>{mtu}</td>
                                                                <td>
                                                                    <span class={format!("status-text {}", status_color)}>
                                                                        {status_text}
                                                                    </span>
                                                                </td>
                                                                <td>
                                                                    <button
                                                                        class="btn btn-sm btn-secondary"
                                                                        on:click=move |_| {
                                                                            set_selected_interface.set(Some(interface_clone.clone()));
                                                                            init_interface_form(&interface_clone);
                                                                            set_show_interface_modal.set(true);
                                                                        }
                                                                    >
                                                                        "Edit"
                                                                    </button>
                                                                </td>
                                                            </tr>
                                                        }
                                                    }).collect::<Vec<_>>()}
                                                </tbody>
                                            </table>
                                        </div>
                                    }.into_view()
                                }}
                            </div>
                        }.into_view(),

                        "dns" => view! {
                            <div class="dns-config-section">
                                <h2>"DNS Configuration"</h2>
                                <div class="config-form">
                                    <div class="form-group">
                                        <label for="cfg-dns-servers">"DNS Servers"</label>
                                        <textarea id="cfg-dns-servers"
                                            prop:value=form_dns_servers
                                            on:input=move |ev| set_form_dns_servers.set(event_target_value(&ev))
                                            placeholder="8.8.8.8\n8.8.4.4\n1.1.1.1"
                                            rows="4"
                                        ></textarea>
                                        <small>"One DNS server per line"</small>
                                    </div>

                                    <div class="form-group">
                                        <label for="cfg-search-domains">"Search Domains"</label>
                                        <input id="cfg-search-domains"
                                            type="text"
                                            prop:value=form_dns_search_domains
                                            on:input=move |ev| set_form_dns_search_domains.set(event_target_value(&ev))
                                            placeholder="example.com local.domain"
                                        />
                                        <small>"Space-separated list of search domains"</small>
                                    </div>

                                    <div class="form-actions">
                                        <button
                                            class="btn btn-primary"
                                            on:click=move |_| save_dns_config.dispatch(())
                                            disabled=loading
                                        >
                                            "Save DNS Settings"
                                        </button>
                                    </div>
                                </div>
                            </div>
                        }.into_view(),

                        "time" => view! {
                            <div class="time-config-section">
                                <h2>"Time & NTP Configuration"</h2>
                                <div class="config-form">
                                    <div class="form-group">
                                        <label for="cfg-ntp-servers">"NTP Servers"</label>
                                        <textarea id="cfg-ntp-servers"
                                            prop:value=form_ntp_servers
                                            on:input=move |ev| set_form_ntp_servers.set(event_target_value(&ev))
                                            placeholder="pool.ntp.org\ntime.google.com\ntime.cloudflare.com"
                                            rows="4"
                                        ></textarea>
                                        <small>"One NTP server per line"</small>
                                    </div>

                                    <SearchSelect id="ntp-timezone" label="Timezone"
                                        choices=Signal::derive(move || tz_choices.get())
                                        value=form_ntp_timezone set_value=set_form_ntp_timezone
                                        placeholder="Search time zones"/>

                                    <div class="form-actions">
                                        <button
                                            class="btn btn-primary"
                                            on:click=move |_| save_ntp_config.dispatch(())
                                            disabled=loading
                                        >
                                            "Save Time Settings"
                                        </button>
                                    </div>
                                </div>
                            </div>
                        }.into_view(),

                        _ => view! {}.into_view(),
                    }}
                </div>
            </div>

            // Network Interface Modal
            {move || if show_interface_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |e: MouseEvent| {
                        if e.target().and_then(|t| t.dyn_ref::<web_sys::Element>().map(|el| el.class_name())).unwrap_or_default() == "modal-overlay" {
                            set_show_interface_modal.set(false);
                            set_selected_interface.set(None);
                            clear_interface_form();
                        }
                    }>
                        <div class="modal-content">
                            <div class="modal-header">
                                <h2>{if selected_interface.get().is_some() { "Edit Interface" } else { "Add Interface" }}</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| {
                                        set_show_interface_modal.set(false);
                                        set_selected_interface.set(None);
                                        clear_interface_form();
                                    }
                                >"x"</button>
                            </div>
                            <div class="modal-body">
                                <div class="form-group">
                                    <label for="cfg-interface-name">"Interface Name"</label>
                                    <input id="cfg-interface-name"
                                        type="text"
                                        prop:value=form_interface_name
                                        on:input=move |ev| set_form_interface_name.set(event_target_value(&ev))
                                        placeholder="eth0"
                                        disabled=selected_interface.get().is_some()
                                    />
                                </div>

                                <div class="form-group">
                                    <label for="cfg-configuration-method">"Configuration Method"</label>
                                    <select id="cfg-configuration-method"
                                        prop:value=form_interface_method
                                        on:change=move |ev| set_form_interface_method.set(event_target_value(&ev))
                                    >
                                        <option value="dhcp">"DHCP"</option>
                                        <option value="static">"Static"</option>
                                        <option value="manual">"Manual"</option>
                                    </select>
                                </div>

                                {move || if form_interface_method.get() == "static" {
                                    view! {
                                        <>
                                            <div class="form-group">
                                                <label for="cfg-ip-address">"IP Address"</label>
                                                <input id="cfg-ip-address"
                                                    type="text"
                                                    prop:value=form_interface_address
                                                    on:input=move |ev| set_form_interface_address.set(event_target_value(&ev))
                                                    placeholder="192.168.1.100"
                                                />
                                            </div>

                                            <div class="form-group">
                                                <label for="cfg-netmask">"Netmask"</label>
                                                <input id="cfg-netmask"
                                                    type="text"
                                                    prop:value=form_interface_netmask
                                                    on:input=move |ev| set_form_interface_netmask.set(event_target_value(&ev))
                                                    placeholder="255.255.255.0 or /24"
                                                />
                                            </div>

                                            <div class="form-group">
                                                <label for="cfg-gateway">"Gateway"</label>
                                                <input id="cfg-gateway"
                                                    type="text"
                                                    prop:value=form_interface_gateway
                                                    on:input=move |ev| set_form_interface_gateway.set(event_target_value(&ev))
                                                    placeholder="192.168.1.1"
                                                />
                                            </div>
                                        </>
                                    }.into_view()
                                } else {
                                    view! {}.into_view()
                                }}

                                <div class="form-group">
                                    <label for="cfg-mtu">"MTU"</label>
                                    <input id="cfg-mtu"
                                        type="number"
                                        prop:value=form_interface_mtu
                                        on:input=move |ev| set_form_interface_mtu.set(event_target_value(&ev).parse().unwrap_or(1500))
                                        min="68"
                                        max="9000"
                                    />
                                    <small>"Maximum Transmission Unit (typically 1500)"</small>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| {
                                        set_show_interface_modal.set(false);
                                        set_selected_interface.set(None);
                                        clear_interface_form();
                                    }
                                >"Cancel"</button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| save_interface.dispatch(())
                                    disabled=move || form_interface_name.get().is_empty() || loading.get()
                                >
                                    {if selected_interface.get().is_some() { "Update Interface" } else { "Add Interface" }}
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }}
        </div>
    }
}

/// Global default network for new VMs (Create VM attaches the first NIC to
/// it instead of the hypervisor's own default).
#[component]
fn VmDefaultNetworkCard() -> impl IntoView {
    let (bridges, set_bridges) = create_signal(Vec::<String>::new());
    let (current, set_current) = create_signal(String::new());
    let (hv_default, set_hv_default) = create_signal(String::new());
    let (msg, set_msg) = create_signal(None::<(bool, String)>);
    // The select stays disabled until both the bridges and the saved value
    // are in: a choice made in between was overwritten by the late saved
    // value, so Save stored "per hypervisor" instead of the picked bridge.
    let (loaded, set_loaded) = create_signal(false);
    spawn_local(async move {
        if let Ok(d) = crate::api::get_vm_defaults().await {
            set_current.set(d.default_network.unwrap_or_default());
        }
        if let Ok(o) = crate::api::get_vm_host_options().await {
            set_bridges.set(o.bridges.iter().map(|b| b.name.clone()).collect());
            set_hv_default.set(o.default_networks.qemu.clone().unwrap_or_default());
        }
        set_loaded.set(true);
    });
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let v = current.get_untracked();
        spawn_local(async move {
            let d = crate::api::VmDefaults {
                default_network: (!v.is_empty()).then_some(v),
            };
            match crate::api::put_vm_defaults(&d).await {
                Ok(_) => set_msg.set(Some((true, "Default VM network saved".into()))),
                Err(e) => set_msg.set(Some((false, e.message))),
            }
        });
    };
    view! {
        <div class="network-config-section" id="vm-default-network">
            <div class="section-header"><h2>"Default VM network"</h2></div>
            <form on:submit=save>
                <div class="form-row">
                    <select id="vm-default-network-select" aria-label="Default network for new VMs" disabled=move || !loaded.get() on:change=move |ev| set_current.set(event_target_value(&ev))>
                        <option value="" selected=move || current.get().is_empty()>
                            {move || format!("Per hypervisor (QEMU: {})", if hv_default.get().is_empty() { "no bridge found".to_string() } else { hv_default.get() })}
                        </option>
                        {move || bridges.get().into_iter().map(|b| {
                            let sel = b == current.get();
                            view! { <option value=b.clone() selected=sel>{b.clone()}</option> }
                        }).collect_view()}
                    </select>
                    <button type="submit" id="vm-default-network-save" class="btn btn-primary" disabled=move || !loaded.get()>"Save"</button>
                </div>
                <small class="form-help">"New VMs get a network card on this bridge. Leave on \"Per hypervisor\" to use each hypervisor's own network."</small>
                {move || msg.get().map(|(ok, m)| view! { <div class=if ok { "alert alert-success" } else { "alert alert-error" } id="vm-default-network-msg">{m}</div> })}
            </form>
        </div>
    }
}

/// System default text editor (`EDITOR` / `VISUAL`): vim by default, neovim
/// as the one-setting alternative (also the declarative key `system.editor`).
#[component]
fn DefaultEditorCard() -> impl IntoView {
    let (state, set_state) = create_signal(None::<EditorState>);
    let (unavailable, set_unavailable) = create_signal(None::<crate::api::Unavailable>);
    let (choice, set_choice) = create_signal(String::new());
    let (msg, set_msg) = create_signal(None::<(bool, String)>);
    let (loaded, set_loaded) = create_signal(false);
    let load = move || {
        spawn_local(async move {
            match crate::api::get_editor().await {
                Ok(s) => {
                    set_choice.set(s.choice.clone());
                    set_unavailable.set(None);
                    set_state.set(Some(s));
                }
                // 424: no default-editor tool on this node: an empty state, never a bare error
                Err(e) if e.unavailable().is_some() => set_unavailable.set(e.unavailable()),
                Err(e) => set_msg.set(Some((false, e.message))),
            }
            set_loaded.set(true);
        });
    };
    load();
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let v = choice.get_untracked();
        spawn_local(async move {
            match crate::api::put_editor(&v).await {
                Ok(s) => {
                    set_msg.set(Some((true, format!("Default editor saved: {}", s.choice))));
                    set_choice.set(s.choice.clone());
                    set_state.set(Some(s));
                }
                Err(e) => set_msg.set(Some((false, e.message))),
            }
        });
    };
    let options = move || {
        let s = state.get().unwrap_or_default();
        let cur = choice.get();
        let mut names: Vec<(String, bool)> = s
            .installed
            .iter()
            .map(|i| (i.choice.clone(), i.installed))
            .collect();
        // a custom absolute path stays selectable
        if !cur.is_empty() && !names.iter().any(|(n, _)| *n == cur) {
            names.push((cur.clone(), true));
        }
        names
            .into_iter()
            .map(|(n, inst)| {
                let sel = n == cur;
                let label = match (n.as_str(), inst) {
                    ("vim", _) => "vim (default)".to_string(),
                    ("neovim", true) => "neovim (nvim)".to_string(),
                    (_, false) => format!("{n} (not installed)"),
                    _ => n.clone(),
                };
                view! { <option value=n.clone() selected=sel>{label}</option> }
            })
            .collect_view()
    };
    let missing = move || {
        state.get().map_or(false, |s| {
            s.installed
                .iter()
                .any(|i| i.choice == choice.get() && !i.installed)
        })
    };
    view! {
        <div class="general-config-section" id="default-editor">
            <h2>"Default editor"</h2>
            {move || if let Some(u) = unavailable.get() {
                view! { <crate::components::UnavailableState info=u on_retry=Callback::new(move |_: ()| load())/> }.into_view()
            } else if !loaded.get() {
                view! { <div class="loading">"Loading the default editor..."</div> }.into_view()
            } else {
                view! {
                    <form on:submit=save>
                        <div class="form-row">
                            <select id="default-editor-select" aria-label="Default text editor"
                                on:change=move |ev| set_choice.set(event_target_value(&ev))>
                                {options}
                            </select>
                            <button type="submit" id="default-editor-save" class="btn btn-primary">"Save"</button>
                        </div>
                        <small class="form-help">
                            "EDITOR and VISUAL for login shells, git, crontab -e, visudo and sudoedit. vim is the default; neovim is the one-setting alternative. Now: "
                            {move || state.get().and_then(|s| s.effective).unwrap_or_else(|| "no editor installed".into())}
                        </small>
                        {move || missing().then(|| view! {
                            <div class="alert alert-warning" id="default-editor-missing">
                                "That editor is not installed, so the node falls back to vim. "
                                <a href="/system/packages">"Install it from Packages"</a>
                            </div>
                        })}
                        {move || msg.get().map(|(ok, m)| view! { <div class=if ok { "alert alert-success" } else { "alert alert-error" } id="default-editor-msg">{m}</div> })}
                    </form>
                }.into_view()
            }}
        </div>
    }
}

/// The choices a search box narrows `all` to: case-insensitive substring
/// match (a space also matches `_`, so "new york" finds America/New_York),
/// with `current` always kept so the select never shows a blank value.
pub fn filter_choices(all: &[String], query: &str, current: &str) -> Vec<String> {
    let q = query.trim().to_lowercase().replace(' ', "_");
    let mut out: Vec<String> = all
        .iter()
        .filter(|c| q.is_empty() || c.to_lowercase().contains(&q))
        .cloned()
        .collect();
    if !current.is_empty() && !out.iter().any(|c| c == current) {
        out.insert(0, current.to_string());
    }
    out
}

/// A select over a long list (hundreds of time zones) with a search box.
#[component]
fn SearchSelect(
    id: &'static str,
    label: &'static str,
    #[prop(into)] choices: Signal<Vec<String>>,
    value: ReadSignal<String>,
    set_value: WriteSignal<String>,
    placeholder: &'static str,
) -> impl IntoView {
    let (query, set_query) = create_signal(String::new());
    let shown = create_memo(move |_| filter_choices(&choices.get(), &query.get(), &value.get()));
    view! {
        <div class="form-group search-select">
            <label for=id>{label}</label>
            <input type="search" id=format!("{id}-search") placeholder=placeholder
                prop:value=query
                on:input=move |ev| {
                    let q = event_target_value(&ev);
                    set_query.set(q.clone());
                    // one match left: take it (typing a full zone name selects it)
                    let m = filter_choices(&choices.get_untracked(), &q, "");
                    if m.len() == 1 {
                        set_value.set(m[0].clone());
                    }
                }/>
            <select id=id on:change=move |ev| set_value.set(event_target_value(&ev))>
                {move || shown.get().into_iter().map(|c| {
                    let c2 = c.clone();
                    view! { <option value=c.clone() selected=move || value.get() == c2>{c}</option> }
                }).collect_view()}
            </select>
            <small class="form-help">{move || {
                let total = choices.with(|c| c.len());
                if query.get().trim().is_empty() {
                    format!("{total} available on this node")
                } else {
                    format!("{} of {total} match", shown.with(|s| s.len()))
                }
            }}</small>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_state_reads_the_api_answer() {
        let s: EditorState = serde_json::from_str(
            r#"{"choice":"neovim","effective":"/usr/bin/nvim","installed":[{"choice":"vim","installed":true},{"choice":"neovim","installed":true}]}"#,
        )
        .unwrap();
        assert_eq!(s.choice, "neovim");
        assert_eq!(s.effective.as_deref(), Some("/usr/bin/nvim"));
        assert_eq!(s.installed.len(), 2);
    }

    #[test]
    fn search_narrows_the_list_but_keeps_the_current_value() {
        let all: Vec<String> = ["America/Chicago", "America/New_York", "Etc/UTC", "UTC"]
            .map(String::from)
            .to_vec();
        assert_eq!(filter_choices(&all, "", "Etc/UTC"), all);
        assert_eq!(
            filter_choices(&all, "new york", "Etc/UTC"),
            ["Etc/UTC", "America/New_York"]
        );
        assert_eq!(filter_choices(&all, "CHICAGO", ""), ["America/Chicago"]);
        // a value the node has but the list lacks is still shown
        assert_eq!(filter_choices(&all, "", "Asia/Tokyo")[0], "Asia/Tokyo");
    }
}
