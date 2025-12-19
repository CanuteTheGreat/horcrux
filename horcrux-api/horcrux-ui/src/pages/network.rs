//! Network Management Page

use leptos::*;
use serde::{Deserialize, Serialize};
use crate::api::{fetch_json, delete_json, post_empty};

/// Network information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub network_type: String,
    pub bridge: Option<String>,
    pub vlan_id: Option<u16>,
    pub subnet: Option<String>,
    pub gateway: Option<String>,
    pub status: String,
}

/// Firewall rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallRule {
    pub id: String,
    pub name: String,
    pub action: String,
    pub direction: String,
    pub protocol: Option<String>,
    pub port: Option<u16>,
    pub source: Option<String>,
    pub destination: Option<String>,
    pub enabled: bool,
    pub priority: i32,
}

/// Network Management Page Component
#[component]
pub fn NetworkManagement() -> impl IntoView {
    let (networks, set_networks) = create_signal(Vec::<Network>::new());
    let (firewall_rules, set_firewall_rules) = create_signal(Vec::<FirewallRule>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("networks".to_string());
    let (show_create_network, set_show_create_network) = create_signal(false);
    let (show_create_rule, set_show_create_rule) = create_signal(false);

    // Fetch data on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            // Fetch networks
            if let Ok(data) = fetch_json::<Vec<Network>>("/api/networks").await {
                set_networks.set(data);
            }

            // Fetch firewall rules
            if let Ok(data) = fetch_json::<Vec<FirewallRule>>("/api/firewall/rules").await {
                set_firewall_rules.set(data);
            }

            set_loading.set(false);
        });
    });

    // Delete network
    let delete_network = move |network_id: String| {
        spawn_local(async move {
            let url = format!("/api/networks/{}", network_id);
            match delete_json(&url).await {
                Ok(()) => {
                    if let Ok(data) = fetch_json::<Vec<Network>>("/api/networks").await {
                        set_networks.set(data);
                    }
                }
                Err(e) => set_error.set(Some(format!("Delete failed: {}", e.message))),
            }
        });
    };

    // Delete firewall rule
    let delete_rule = move |rule_id: String| {
        spawn_local(async move {
            let url = format!("/api/firewall/rules/{}", rule_id);
            match delete_json(&url).await {
                Ok(()) => {
                    if let Ok(data) = fetch_json::<Vec<FirewallRule>>("/api/firewall/rules").await {
                        set_firewall_rules.set(data);
                    }
                }
                Err(e) => set_error.set(Some(format!("Delete failed: {}", e.message))),
            }
        });
    };

    // Apply firewall rules
    let apply_firewall = move |_| {
        spawn_local(async move {
            match post_empty("/api/firewall/apply").await {
                Ok(()) => {
                    leptos::logging::log!("Firewall rules applied successfully");
                }
                Err(e) => set_error.set(Some(format!("Apply failed: {}", e.message))),
            }
        });
    };

    view! {
        <div class="page network-management">
            <header class="page-header">
                <h2>"Networking"</h2>
            </header>

            // Tabs
            <div class="tabs">
                <button class={move || if active_tab.get() == "networks" { "tab active" } else { "tab" }}
                        on:click=move |_| set_active_tab.set("networks".to_string())>
                    "Networks"
                </button>
                <button class={move || if active_tab.get() == "firewall" { "tab active" } else { "tab" }}
                        on:click=move |_| set_active_tab.set("firewall".to_string())>
                    "Firewall"
                </button>
            </div>

            // Error display
            {move || error.get().map(|e| view! {
                <div class="alert alert-error">
                    <span>{e}</span>
                    <button class="btn-close" on:click=move |_| set_error.set(None)>"x"</button>
                </div>
            })}

            // Loading state
            {move || loading.get().then(|| view! {
                <div class="loading">
                    <div class="spinner"></div>
                    <span>"Loading..."</span>
                </div>
            })}

            // Networks Tab
            {move || if !loading.get() && active_tab.get() == "networks" {
                view! {
                    <div class="tab-content">
                        <div class="section-header">
                            <h3>"Virtual Networks"</h3>
                            <button class="btn btn-primary" on:click=move |_| set_show_create_network.set(true)>
                                "+ Create Network"
                            </button>
                        </div>

                        {move || if networks.get().is_empty() {
                            view! {
                                <div class="empty-state">
                                    <div class="icon">"🌐"</div>
                                    <h3>"No Networks"</h3>
                                    <p>"Create a virtual network for your VMs and containers."</p>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <div class="network-grid">
                                    {move || networks.get().into_iter().map(|network| {
                                        let network_id = network.id.clone();

                                        view! {
                                            <div class={format!("network-card status-{}", network.status.to_lowercase())}>
                                                <div class="network-header">
                                                    <h4>{&network.name}</h4>
                                                    <span class="type-badge">{&network.network_type}</span>
                                                </div>
                                                <div class="network-details">
                                                    {network.bridge.clone().map(|b| view! {
                                                        <div class="detail-row">
                                                            <span class="label">"Bridge:"</span>
                                                            <span>{b}</span>
                                                        </div>
                                                    })}
                                                    {network.subnet.clone().map(|s| view! {
                                                        <div class="detail-row">
                                                            <span class="label">"Subnet:"</span>
                                                            <code>{s}</code>
                                                        </div>
                                                    })}
                                                    {network.gateway.clone().map(|g| view! {
                                                        <div class="detail-row">
                                                            <span class="label">"Gateway:"</span>
                                                            <code>{g}</code>
                                                        </div>
                                                    })}
                                                    {network.vlan_id.map(|v| view! {
                                                        <div class="detail-row">
                                                            <span class="label">"VLAN ID:"</span>
                                                            <span>{v}</span>
                                                        </div>
                                                    })}
                                                </div>
                                                <div class="network-actions">
                                                    <button class="btn btn-secondary">"Edit"</button>
                                                    <button class="btn btn-danger"
                                                            on:click=move |_| delete_network(network_id.clone())>
                                                        "Delete"
                                                    </button>
                                                </div>
                                            </div>
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_view()
                        }}
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Firewall Tab
            {move || if !loading.get() && active_tab.get() == "firewall" {
                view! {
                    <div class="tab-content">
                        <div class="section-header">
                            <h3>"Firewall Rules"</h3>
                            <div class="header-actions">
                                <button class="btn btn-success" on:click=apply_firewall>
                                    "Apply Rules"
                                </button>
                                <button class="btn btn-primary" on:click=move |_| set_show_create_rule.set(true)>
                                    "+ Add Rule"
                                </button>
                            </div>
                        </div>

                        {move || if firewall_rules.get().is_empty() {
                            view! {
                                <div class="empty-state">
                                    <div class="icon">"🛡️"</div>
                                    <h3>"No Firewall Rules"</h3>
                                    <p>"Add firewall rules to secure your network."</p>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <table class="data-table firewall-table">
                                    <thead>
                                        <tr>
                                            <th>"Priority"</th>
                                            <th>"Name"</th>
                                            <th>"Action"</th>
                                            <th>"Direction"</th>
                                            <th>"Protocol"</th>
                                            <th>"Port"</th>
                                            <th>"Source"</th>
                                            <th>"Status"</th>
                                            <th>"Actions"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {move || firewall_rules.get().into_iter().map(|rule| {
                                            let rule_id = rule.id.clone();

                                            view! {
                                                <tr class={if !rule.enabled { "disabled" } else { "" }}>
                                                    <td>{rule.priority}</td>
                                                    <td>{&rule.name}</td>
                                                    <td>
                                                        <span class={format!("action-badge {}", rule.action.to_lowercase())}>
                                                            {&rule.action}
                                                        </span>
                                                    </td>
                                                    <td>{&rule.direction}</td>
                                                    <td>{rule.protocol.clone().unwrap_or_else(|| "Any".to_string())}</td>
                                                    <td>{rule.port.map(|p| p.to_string()).unwrap_or_else(|| "Any".to_string())}</td>
                                                    <td><code>{rule.source.clone().unwrap_or_else(|| "Any".to_string())}</code></td>
                                                    <td>
                                                        {if rule.enabled {
                                                            view! { <span class="status-enabled">"Enabled"</span> }.into_view()
                                                        } else {
                                                            view! { <span class="status-disabled">"Disabled"</span> }.into_view()
                                                        }}
                                                    </td>
                                                    <td class="actions">
                                                        <button class="btn btn-sm btn-secondary">"Edit"</button>
                                                        <button class="btn btn-sm btn-danger"
                                                                on:click=move |_| delete_rule(rule_id.clone())>
                                                            "Delete"
                                                        </button>
                                                    </td>
                                                </tr>
                                            }
                                        }).collect_view()}
                                    </tbody>
                                </table>
                            }.into_view()
                        }}
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Create Network Modal
            {move || show_create_network.get().then(|| view! {
                <div class="modal-overlay" on:click=move |_| set_show_create_network.set(false)>
                    <div class="modal" on:click=|e| e.stop_propagation()>
                        <div class="modal-header">
                            <h3>"Create Network"</h3>
                            <button class="btn-close" on:click=move |_| set_show_create_network.set(false)>
                                "x"
                            </button>
                        </div>
                        <div class="modal-body">
                            <div class="form-group">
                                <label>"Network Name"</label>
                                <input type="text" class="form-control" placeholder="vmbr1"/>
                            </div>
                            <div class="form-group">
                                <label>"Type"</label>
                                <select class="form-control">
                                    <option value="Bridge">"Bridge"</option>
                                    <option value="NAT">"NAT"</option>
                                    <option value="VLAN">"VLAN"</option>
                                    <option value="VXLAN">"VXLAN"</option>
                                </select>
                            </div>
                            <div class="form-group">
                                <label>"Subnet (CIDR)"</label>
                                <input type="text" class="form-control" placeholder="10.0.0.0/24"/>
                            </div>
                            <div class="form-group">
                                <label>"Gateway"</label>
                                <input type="text" class="form-control" placeholder="10.0.0.1"/>
                            </div>
                        </div>
                        <div class="modal-footer">
                            <button class="btn btn-secondary" on:click=move |_| set_show_create_network.set(false)>
                                "Cancel"
                            </button>
                            <button class="btn btn-primary">"Create"</button>
                        </div>
                    </div>
                </div>
            })}

            // Create Firewall Rule Modal
            {move || show_create_rule.get().then(|| view! {
                <div class="modal-overlay" on:click=move |_| set_show_create_rule.set(false)>
                    <div class="modal" on:click=|e| e.stop_propagation()>
                        <div class="modal-header">
                            <h3>"Add Firewall Rule"</h3>
                            <button class="btn-close" on:click=move |_| set_show_create_rule.set(false)>
                                "x"
                            </button>
                        </div>
                        <div class="modal-body">
                            <div class="form-row">
                                <div class="form-group">
                                    <label>"Name"</label>
                                    <input type="text" class="form-control" placeholder="allow-ssh"/>
                                </div>
                                <div class="form-group">
                                    <label>"Priority"</label>
                                    <input type="number" class="form-control" value="100"/>
                                </div>
                            </div>
                            <div class="form-row">
                                <div class="form-group">
                                    <label>"Action"</label>
                                    <select class="form-control">
                                        <option value="Accept">"Accept"</option>
                                        <option value="Drop">"Drop"</option>
                                        <option value="Reject">"Reject"</option>
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label>"Direction"</label>
                                    <select class="form-control">
                                        <option value="in">"Inbound"</option>
                                        <option value="out">"Outbound"</option>
                                    </select>
                                </div>
                            </div>
                            <div class="form-row">
                                <div class="form-group">
                                    <label>"Protocol"</label>
                                    <select class="form-control">
                                        <option value="">"Any"</option>
                                        <option value="Tcp">"TCP"</option>
                                        <option value="Udp">"UDP"</option>
                                        <option value="Icmp">"ICMP"</option>
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label>"Port"</label>
                                    <input type="number" class="form-control" placeholder="Any"/>
                                </div>
                            </div>
                            <div class="form-group">
                                <label>"Source CIDR"</label>
                                <input type="text" class="form-control" placeholder="0.0.0.0/0"/>
                            </div>
                        </div>
                        <div class="modal-footer">
                            <button class="btn btn-secondary" on:click=move |_| set_show_create_rule.set(false)>
                                "Cancel"
                            </button>
                            <button class="btn btn-primary">"Add Rule"</button>
                        </div>
                    </div>
                </div>
            })}
        </div>
    }
}
