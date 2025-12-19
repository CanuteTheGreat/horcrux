use leptos::*;
use wasm_bindgen::JsCast;
use crate::api::*;
use web_sys::MouseEvent;

#[component]
pub fn SystemConfigurationPage() -> impl IntoView {
    let (system_config, set_system_config) = create_signal(None::<SystemConfiguration>);
    let (network_interfaces, set_network_interfaces) = create_signal(Vec::<NetworkInterface>::new());
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
            Err(e) => set_error_message.set(Some(format!("Failed to load system configuration: {}", e))),
        }

        match get_network_interfaces().await {
            Ok(interfaces) => set_network_interfaces.set(interfaces),
            Err(e) => set_error_message.set(Some(format!("Failed to load network interfaces: {}", e))),
        }

        match get_dns_configuration().await {
            Ok(dns) => {
                set_form_dns_servers.set(dns.servers.join("\n"));
                set_form_dns_search_domains.set(dns.search_domains.join(" "));
                set_dns_config.set(Some(dns));
            }
            Err(e) => set_error_message.set(Some(format!("Failed to load DNS configuration: {}", e))),
        }

        match get_ntp_configuration().await {
            Ok(ntp) => {
                set_form_ntp_servers.set(ntp.servers.join("\n"));
                set_form_ntp_timezone.set(ntp.timezone.clone());
                set_ntp_config.set(Some(ntp));
            }
            Err(e) => set_error_message.set(Some(format!("Failed to load NTP configuration: {}", e))),
        }

        set_loading.set(false);
    });

    // Save general configuration
    let save_general_config = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let config = SystemConfiguration {
            hostname: form_hostname.get(),
            domain: if form_domain.get().is_empty() { None } else { Some(form_domain.get()) },
            timezone: form_timezone.get(),
            locale: form_locale.get(),
        };

        match update_system_configuration(config).await {
            Ok(_) => {
                set_success_message.set(Some("General configuration saved successfully".to_string()));
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
            address: if form_interface_address.get().is_empty() { None } else { Some(form_interface_address.get()) },
            netmask: if form_interface_netmask.get().is_empty() { None } else { Some(form_interface_netmask.get()) },
            gateway: if form_interface_gateway.get().is_empty() { None } else { Some(form_interface_gateway.get()) },
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
            servers: form_dns_servers.get().lines().map(|s| s.trim().to_string()).collect(),
            search_domains: form_dns_search_domains.get().split_whitespace().map(|s| s.to_string()).collect(),
        };

        match update_dns_configuration(dns).await {
            Ok(_) => {
                set_success_message.set(Some("DNS configuration saved successfully".to_string()));
                load_data.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to save DNS configuration: {}", e))),
        }

        set_loading.set(false);
    });

    // Save NTP configuration
    let save_ntp_config = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        let ntp = NtpConfiguration {
            servers: form_ntp_servers.get().lines().map(|s| s.trim().to_string()).collect(),
            timezone: form_ntp_timezone.get(),
        };

        match update_ntp_configuration(ntp).await {
            Ok(_) => {
                set_success_message.set(Some("NTP configuration saved successfully".to_string()));
                load_data.dispatch(());
            }
            Err(e) => set_error_message.set(Some(format!("Failed to save NTP configuration: {}", e))),
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
        if interface.auto { "Enabled" } else { "Disabled" }
    };

    let get_interface_status_color = |interface: &NetworkInterface| {
        if interface.auto { "text-green-600" } else { "text-gray-500" }
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
                            <div class="general-config-section">
                                <h2>"General System Settings"</h2>
                                <div class="config-form">
                                    <div class="form-group">
                                        <label>"Hostname"</label>
                                        <input
                                            type="text"
                                            prop:value=form_hostname
                                            on:input=move |ev| set_form_hostname.set(event_target_value(&ev))
                                            placeholder="hostname"
                                        />
                                        <small>"System hostname (without domain)"</small>
                                    </div>

                                    <div class="form-group">
                                        <label>"Domain"</label>
                                        <input
                                            type="text"
                                            prop:value=form_domain
                                            on:input=move |ev| set_form_domain.set(event_target_value(&ev))
                                            placeholder="example.com"
                                        />
                                        <small>"Domain name for this system"</small>
                                    </div>

                                    <div class="form-row">
                                        <div class="form-group">
                                            <label>"Timezone"</label>
                                            <select
                                                prop:value=form_timezone
                                                on:change=move |ev| set_form_timezone.set(event_target_value(&ev))
                                            >
                                                <option value="UTC">"UTC"</option>
                                                <option value="America/New_York">"America/New_York"</option>
                                                <option value="America/Los_Angeles">"America/Los_Angeles"</option>
                                                <option value="Europe/London">"Europe/London"</option>
                                                <option value="Europe/Berlin">"Europe/Berlin"</option>
                                                <option value="Asia/Tokyo">"Asia/Tokyo"</option>
                                                <option value="Asia/Shanghai">"Asia/Shanghai"</option>
                                                <option value="Australia/Sydney">"Australia/Sydney"</option>
                                            </select>
                                        </div>
                                        <div class="form-group">
                                            <label>"Locale"</label>
                                            <select
                                                prop:value=form_locale
                                                on:change=move |ev| set_form_locale.set(event_target_value(&ev))
                                            >
                                                <option value="en_US.UTF-8">"English (US)"</option>
                                                <option value="en_GB.UTF-8">"English (GB)"</option>
                                                <option value="de_DE.UTF-8">"German"</option>
                                                <option value="fr_FR.UTF-8">"French"</option>
                                                <option value="es_ES.UTF-8">"Spanish"</option>
                                                <option value="it_IT.UTF-8">"Italian"</option>
                                                <option value="ja_JP.UTF-8">"Japanese"</option>
                                                <option value="zh_CN.UTF-8">"Chinese (Simplified)"</option>
                                            </select>
                                        </div>
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
                                        <label>"DNS Servers"</label>
                                        <textarea
                                            prop:value=form_dns_servers
                                            on:input=move |ev| set_form_dns_servers.set(event_target_value(&ev))
                                            placeholder="8.8.8.8\n8.8.4.4\n1.1.1.1"
                                            rows="4"
                                        ></textarea>
                                        <small>"One DNS server per line"</small>
                                    </div>

                                    <div class="form-group">
                                        <label>"Search Domains"</label>
                                        <input
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
                                        <label>"NTP Servers"</label>
                                        <textarea
                                            prop:value=form_ntp_servers
                                            on:input=move |ev| set_form_ntp_servers.set(event_target_value(&ev))
                                            placeholder="pool.ntp.org\ntime.google.com\ntime.cloudflare.com"
                                            rows="4"
                                        ></textarea>
                                        <small>"One NTP server per line"</small>
                                    </div>

                                    <div class="form-group">
                                        <label>"Timezone"</label>
                                        <select
                                            prop:value=form_ntp_timezone
                                            on:change=move |ev| set_form_ntp_timezone.set(event_target_value(&ev))
                                        >
                                            <option value="UTC">"UTC"</option>
                                            <option value="America/New_York">"America/New_York"</option>
                                            <option value="America/Los_Angeles">"America/Los_Angeles"</option>
                                            <option value="Europe/London">"Europe/London"</option>
                                            <option value="Europe/Berlin">"Europe/Berlin"</option>
                                            <option value="Asia/Tokyo">"Asia/Tokyo"</option>
                                            <option value="Asia/Shanghai">"Asia/Shanghai"</option>
                                            <option value="Australia/Sydney">"Australia/Sydney"</option>
                                        </select>
                                    </div>

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
                                    <label>"Interface Name"</label>
                                    <input
                                        type="text"
                                        prop:value=form_interface_name
                                        on:input=move |ev| set_form_interface_name.set(event_target_value(&ev))
                                        placeholder="eth0"
                                        disabled=selected_interface.get().is_some()
                                    />
                                </div>

                                <div class="form-group">
                                    <label>"Configuration Method"</label>
                                    <select
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
                                                <label>"IP Address"</label>
                                                <input
                                                    type="text"
                                                    prop:value=form_interface_address
                                                    on:input=move |ev| set_form_interface_address.set(event_target_value(&ev))
                                                    placeholder="192.168.1.100"
                                                />
                                            </div>

                                            <div class="form-group">
                                                <label>"Netmask"</label>
                                                <input
                                                    type="text"
                                                    prop:value=form_interface_netmask
                                                    on:input=move |ev| set_form_interface_netmask.set(event_target_value(&ev))
                                                    placeholder="255.255.255.0 or /24"
                                                />
                                            </div>

                                            <div class="form-group">
                                                <label>"Gateway"</label>
                                                <input
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
                                    <label>"MTU"</label>
                                    <input
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