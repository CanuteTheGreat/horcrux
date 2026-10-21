//! Network Management Page

use crate::api::{delete_json, fetch_json, post_empty, post_json, post_json_no_response};
use leptos::*;
use serde::{Deserialize, Serialize};

/// Firewall rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallRule {
    pub id: String,
    pub enabled: bool,
    pub action: String,
    pub direction: String,
    pub protocol: Option<String>,
    pub source: Option<String>,
    pub dest: Option<String>,
    pub sport: Option<String>,
    pub dport: Option<String>,
    pub comment: Option<String>,
    pub log: bool,
    pub position: i32,
}

// Horcrux SDN CNI networks and their attachments: types and calls live in
// api/sdn.rs (checked against the live API by tests/api_contract.rs).
use crate::api::sdn as sdn_api;
use crate::api::sdn::{CniAttachment, CniNetwork};

#[derive(Debug, Clone, Serialize)]
struct CreateCniNetworkReq {
    name: String,
    plugin_type: String,
    bridge: Option<String>,
    subnet: Option<String>,
    gateway: Option<String>,
}

/// Horcrux SDN network policy (backed by /api/network-policies — real
/// backend in horcrux-api/src/sdn/policy.rs, same "implemented but never
/// surfaced" gap as the CNI networks above).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicy {
    pub id: String,
    pub name: String,
    pub namespace: String,
    pub policy_types: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
struct CreateNetworkPolicyReq {
    id: String,
    name: String,
    namespace: String,
    pod_selector: PolicySelector,
    policy_types: Vec<String>,
    ingress: Vec<serde_json::Value>,
    egress: Vec<serde_json::Value>,
    enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
struct PolicySelector {
    match_labels: std::collections::HashMap<String, String>,
    match_expressions: Vec<serde_json::Value>,
}

/// Network Management Page Component
#[component]
pub fn NetworkManagement() -> impl IntoView {
    let (firewall_rules, set_firewall_rules) = create_signal(Vec::<FirewallRule>::new());
    let (cni_networks, set_cni_networks) = create_signal(Vec::<CniNetwork>::new());
    let (net_policies, set_net_policies) = create_signal(Vec::<NetworkPolicy>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("sdn-cni".to_string());
    let (show_create_rule, set_show_create_rule) = create_signal(false);
    let (show_create_cni, set_show_create_cni) = create_signal(false);
    let (show_create_policy, set_show_create_policy) = create_signal(false);
    let (cni_attachments, set_cni_attachments) = create_signal(Vec::<CniAttachment>::new());
    // Network the "Attach container" dialog is open for.
    let (attach_target, set_attach_target) = create_signal(None::<String>);
    let (attach_candidates, set_attach_candidates) =
        create_signal(Vec::<crate::api::Container>::new());
    let attach_ct_ref: NodeRef<html::Select> = create_node_ref();
    let attach_if_ref: NodeRef<html::Input> = create_node_ref();
    let attach_ns_ref: NodeRef<html::Input> = create_node_ref();
    let attach_labels_ref: NodeRef<html::Input> = create_node_ref();

    let rule_name_ref: NodeRef<html::Input> = create_node_ref();
    let rule_priority_ref: NodeRef<html::Input> = create_node_ref();
    let rule_action_ref: NodeRef<html::Select> = create_node_ref();
    let rule_direction_ref: NodeRef<html::Select> = create_node_ref();
    let rule_protocol_ref: NodeRef<html::Select> = create_node_ref();
    let rule_port_ref: NodeRef<html::Input> = create_node_ref();
    let rule_source_ref: NodeRef<html::Input> = create_node_ref();

    let cni_name_ref: NodeRef<html::Input> = create_node_ref();
    let cni_plugin_ref: NodeRef<html::Select> = create_node_ref();
    let cni_bridge_ref: NodeRef<html::Input> = create_node_ref();
    let cni_subnet_ref: NodeRef<html::Input> = create_node_ref();
    let cni_gateway_ref: NodeRef<html::Input> = create_node_ref();

    let policy_name_ref: NodeRef<html::Input> = create_node_ref();
    let policy_namespace_ref: NodeRef<html::Input> = create_node_ref();
    let policy_ingress_ref: NodeRef<html::Input> = create_node_ref();
    let policy_egress_ref: NodeRef<html::Input> = create_node_ref();

    // Fetch data on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            // Fetch firewall rules
            if let Ok(data) = fetch_json::<Vec<FirewallRule>>("/api/firewall/rules").await {
                set_firewall_rules.set(data);
            }

            // Fetch Horcrux SDN CNI networks (real backend, previously unused by any page)
            if let Ok(data) = fetch_json::<Vec<CniNetwork>>("/api/cni/networks").await {
                set_cni_networks.set(data);
            }
            if let Ok(data) = sdn_api::list_attachments().await {
                set_cni_attachments.set(data);
            }

            // Fetch Horcrux SDN network policies (real backend, previously unused by any page)
            if let Ok(data) = fetch_json::<Vec<NetworkPolicy>>("/api/network-policies").await {
                set_net_policies.set(data);
            }

            set_loading.set(false);
        });
    });

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
            match post_empty("/api/firewall/datacenter/apply").await {
                Ok(()) => set_notice.set(Some("Firewall rules applied".to_string())),
                Err(e) => set_error.set(Some(format!("Apply failed: {}", e.message))),
            }
        });
    };

    // Create firewall rule (previously a dead button — modal opened but the
    // "Add Rule" submit never called the API, even though POST
    // /api/firewall/rules is a real, working endpoint)
    let create_rule = move |_| {
        let comment = rule_name_ref.get().map(|el| el.value()).unwrap_or_default();
        if comment.is_empty() {
            set_error.set(Some("Rule name is required".to_string()));
            return;
        }
        let priority = rule_priority_ref
            .get()
            .and_then(|el| el.value().parse::<i32>().ok())
            .unwrap_or(100);
        let action = rule_action_ref
            .get()
            .map(|el| el.value().to_uppercase())
            .unwrap_or_else(|| "ACCEPT".to_string());
        let direction = rule_direction_ref
            .get()
            .map(|el| el.value())
            .unwrap_or_else(|| "in".to_string());
        let protocol = rule_protocol_ref
            .get()
            .map(|el| el.value().to_lowercase())
            .filter(|v| !v.is_empty());
        let dport = rule_port_ref
            .get()
            .map(|el| el.value())
            .filter(|v| !v.is_empty());
        let source = rule_source_ref
            .get()
            .map(|el| el.value())
            .filter(|v| !v.is_empty());

        let new_rule = FirewallRule {
            id: String::new(),
            enabled: true,
            action,
            direction,
            protocol,
            source,
            dest: None,
            sport: None,
            dport,
            comment: Some(comment),
            log: false,
            position: priority,
        };

        spawn_local(async move {
            match post_json_no_response("/api/firewall/rules", &new_rule).await {
                Ok(()) => {
                    set_show_create_rule.set(false);
                    if let Ok(data) = fetch_json::<Vec<FirewallRule>>("/api/firewall/rules").await {
                        set_firewall_rules.set(data);
                    }
                }
                Err(e) => set_error.set(Some(format!("Create rule failed: {}", e.message))),
            }
        });
    };

    // Delete SDN CNI network
    let delete_cni_network = move |name: String| {
        spawn_local(async move {
            let url = format!("/api/cni/networks/{}", name);
            match delete_json(&url).await {
                Ok(()) => {
                    if let Ok(data) = fetch_json::<Vec<CniNetwork>>("/api/cni/networks").await {
                        set_cni_networks.set(data);
                    }
                }
                Err(e) => set_error.set(Some(format!("Delete failed: {}", e.message))),
            }
        });
    };

    // Create SDN CNI network (surfaces horcrux-api/src/sdn/cni.rs, which has
    // a real, tested, routed API but no UI anywhere in the app until now)
    let refresh_cni = move || {
        spawn_local(async move {
            if let Ok(data) = sdn_api::list_networks().await {
                set_cni_networks.set(data);
            }
            if let Ok(data) = sdn_api::list_attachments().await {
                set_cni_attachments.set(data);
            }
        });
    };

    // "Attach container" dialog: running Horcrux containers to pick from.
    let open_attach = move |network: String| {
        set_attach_target.set(Some(network));
        spawn_local(async move {
            match crate::api::get_containers().await {
                Ok(list) => set_attach_candidates.set(
                    list.into_iter()
                        .filter(|c| c.status.eq_ignore_ascii_case("running"))
                        .collect(),
                ),
                Err(e) => set_error.set(Some(format!("Could not list containers: {}", e.message))),
            }
        });
    };

    let attach_container = move |_| {
        let Some(network_name) = attach_target.get_untracked() else {
            return;
        };
        let container_id = attach_ct_ref.get().map(|el| el.value()).unwrap_or_default();
        if container_id.is_empty() {
            set_error.set(Some("Pick a running container to attach".to_string()));
            return;
        }
        let opt = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let req = sdn_api::AttachRequest {
            container_id,
            network_name,
            interface_name: opt(attach_if_ref.get().map(|el| el.value())),
            namespace: opt(attach_ns_ref.get().map(|el| el.value())),
            labels: sdn_api::parse_labels(
                &attach_labels_ref
                    .get()
                    .map(|el| el.value())
                    .unwrap_or_default(),
            ),
        };
        spawn_local(async move {
            match sdn_api::attach(&req).await {
                Ok(a) => {
                    set_attach_target.set(None);
                    set_notice.set(Some(format!(
                        "Attached {} to {} as {} ({})",
                        a.container_id, a.network_name, a.interface_name, a.ip_address
                    )));
                    refresh_cni();
                }
                Err(e) => set_error.set(Some(format!("Attach failed: {}", e.message))),
            }
        });
    };

    let detach_container = move |container_id: String, network_name: String| {
        spawn_local(async move {
            let req = sdn_api::DetachRequest {
                container_id,
                network_name,
            };
            match sdn_api::detach(&req).await {
                Ok(_) => {
                    set_notice.set(Some(format!(
                        "Detached {} from {}",
                        req.container_id, req.network_name
                    )));
                    refresh_cni();
                }
                Err(e) => set_error.set(Some(format!("Detach failed: {}", e.message))),
            }
        });
    };

    let create_cni_network = move |_| {
        let name = cni_name_ref.get().map(|el| el.value()).unwrap_or_default();
        if name.is_empty() {
            set_error.set(Some("Network name is required".to_string()));
            return;
        }
        let plugin_type = cni_plugin_ref
            .get()
            .map(|el| el.value())
            .unwrap_or_else(|| "bridge".to_string());
        let bridge = cni_bridge_ref
            .get()
            .map(|el| el.value())
            .filter(|v| !v.is_empty());
        let subnet = cni_subnet_ref
            .get()
            .map(|el| el.value())
            .filter(|v| !v.is_empty());
        let gateway = cni_gateway_ref
            .get()
            .map(|el| el.value())
            .filter(|v| !v.is_empty());

        let req = CreateCniNetworkReq {
            name,
            plugin_type,
            bridge,
            subnet,
            gateway,
        };

        spawn_local(async move {
            match post_json::<CniNetwork, CreateCniNetworkReq>("/api/cni/networks", &req).await {
                Ok(_) => {
                    set_show_create_cni.set(false);
                    if let Ok(data) = fetch_json::<Vec<CniNetwork>>("/api/cni/networks").await {
                        set_cni_networks.set(data);
                    }
                }
                Err(e) => set_error.set(Some(format!("Create network failed: {}", e.message))),
            }
        });
    };

    // Delete SDN network policy
    let delete_policy = move |id: String| {
        spawn_local(async move {
            let url = format!("/api/network-policies/{}", id);
            match delete_json(&url).await {
                Ok(()) => {
                    if let Ok(data) =
                        fetch_json::<Vec<NetworkPolicy>>("/api/network-policies").await
                    {
                        set_net_policies.set(data);
                    }
                }
                Err(e) => set_error.set(Some(format!("Delete failed: {}", e.message))),
            }
        });
    };

    // Create SDN network policy (surfaces horcrux-api/src/sdn/policy.rs,
    // same previously-stranded-code gap as the CNI networks above)
    let create_policy = move |_| {
        let name = policy_name_ref
            .get()
            .map(|el| el.value())
            .unwrap_or_default();
        if name.is_empty() {
            set_error.set(Some("Policy name is required".to_string()));
            return;
        }
        let namespace = policy_namespace_ref
            .get()
            .map(|el| el.value())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "default".to_string());
        let mut policy_types = Vec::new();
        if policy_ingress_ref
            .get()
            .map(|el| el.checked())
            .unwrap_or(false)
        {
            policy_types.push("Ingress".to_string());
        }
        if policy_egress_ref
            .get()
            .map(|el| el.checked())
            .unwrap_or(false)
        {
            policy_types.push("Egress".to_string());
        }
        if policy_types.is_empty() {
            policy_types.push("Ingress".to_string());
        }

        let req = CreateNetworkPolicyReq {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            namespace,
            pod_selector: PolicySelector {
                match_labels: std::collections::HashMap::new(),
                match_expressions: Vec::new(),
            },
            policy_types,
            ingress: Vec::new(),
            egress: Vec::new(),
            enabled: true,
        };

        spawn_local(async move {
            match post_json::<serde_json::Value, CreateNetworkPolicyReq>(
                "/api/network-policies",
                &req,
            )
            .await
            {
                Ok(_) => {
                    set_show_create_policy.set(false);
                    if let Ok(data) =
                        fetch_json::<Vec<NetworkPolicy>>("/api/network-policies").await
                    {
                        set_net_policies.set(data);
                    }
                }
                Err(e) => set_error.set(Some(format!("Create policy failed: {}", e.message))),
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
                <button class={move || if active_tab.get() == "firewall" { "tab active" } else { "tab" }}
                        on:click=move |_| set_active_tab.set("firewall".to_string())>
                    "Firewall"
                </button>
                <button class={move || if active_tab.get() == "sdn-cni" { "tab active" } else { "tab" }}
                        on:click=move |_| set_active_tab.set("sdn-cni".to_string())>
                    "SDN Networks (CNI)"
                </button>
                <button class={move || if active_tab.get() == "sdn-policies" { "tab active" } else { "tab" }}
                        on:click=move |_| set_active_tab.set("sdn-policies".to_string())>
                    "SDN Policies"
                </button>
            </div>

            {move || notice.get().map(|n| view! {
                <div class="alert alert-success" id="network-notice">
                    <span>{n}</span>
                    <button class="btn-close" on:click=move |_| set_notice.set(None)>"x"</button>
                </div>
            })}

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
                                                    <td>{rule.position}</td>
                                                    <td>{rule.comment.clone().unwrap_or_default()}</td>
                                                    <td>
                                                        <span class={format!("action-badge {}", rule.action.to_lowercase())}>
                                                            {&rule.action}
                                                        </span>
                                                    </td>
                                                    <td>{&rule.direction}</td>
                                                    <td>{rule.protocol.clone().unwrap_or_else(|| "Any".to_string())}</td>
                                                    <td>{rule.dport.clone().unwrap_or_else(|| "Any".to_string())}</td>
                                                    <td><code>{rule.source.clone().unwrap_or_else(|| "Any".to_string())}</code></td>
                                                    <td>
                                                        {if rule.enabled {
                                                            view! { <span class="status-enabled">"Enabled"</span> }.into_view()
                                                        } else {
                                                            view! { <span class="status-disabled">"Disabled"</span> }.into_view()
                                                        }}
                                                    </td>
                                                    <td class="actions">
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
                                    <input node_ref=rule_name_ref type="text" class="form-control" placeholder="allow-ssh"/>
                                </div>
                                <div class="form-group">
                                    <label>"Priority"</label>
                                    <input node_ref=rule_priority_ref type="number" class="form-control" value="100"/>
                                </div>
                            </div>
                            <div class="form-row">
                                <div class="form-group">
                                    <label>"Action"</label>
                                    <select node_ref=rule_action_ref class="form-control">
                                        <option value="Accept">"Accept"</option>
                                        <option value="Drop">"Drop"</option>
                                        <option value="Reject">"Reject"</option>
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label>"Direction"</label>
                                    <select node_ref=rule_direction_ref class="form-control">
                                        <option value="in">"Inbound"</option>
                                        <option value="out">"Outbound"</option>
                                    </select>
                                </div>
                            </div>
                            <div class="form-row">
                                <div class="form-group">
                                    <label>"Protocol"</label>
                                    <select node_ref=rule_protocol_ref class="form-control">
                                        <option value="">"Any"</option>
                                        <option value="Tcp">"TCP"</option>
                                        <option value="Udp">"UDP"</option>
                                        <option value="Icmp">"ICMP"</option>
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label>"Port"</label>
                                    <input node_ref=rule_port_ref type="number" class="form-control" placeholder="Any"/>
                                </div>
                            </div>
                            <div class="form-group">
                                <label>"Source CIDR"</label>
                                <input node_ref=rule_source_ref type="text" class="form-control" placeholder="0.0.0.0/0"/>
                            </div>
                        </div>
                        <div class="modal-footer">
                            <button class="btn btn-secondary" on:click=move |_| set_show_create_rule.set(false)>
                                "Cancel"
                            </button>
                            <button class="btn btn-primary" on:click=create_rule>"Add Rule"</button>
                        </div>
                    </div>
                </div>
            })}

            // SDN CNI Networks Tab (horcrux-api/src/sdn/cni.rs — real,
            // routed, tested backend that had no UI page anywhere before
            // this fix)
            {move || if !loading.get() && active_tab.get() == "sdn-cni" {
                view! {
                    <div class="tab-content">
                        <div class="section-header">
                            <h3>"SDN CNI Networks"</h3>
                            <button class="btn btn-primary" on:click=move |_| set_show_create_cni.set(true)>
                                "+ Create CNI Network"
                            </button>
                        </div>

                        {move || if cni_networks.get().is_empty() {
                            view! {
                                <div class="empty-state">
                                    <div class="icon">"🔌"</div>
                                    <h3>"No SDN CNI Networks"</h3>
                                    <p>"Create a CNI network to attach containers/pods to real Horcrux SDN networking."</p>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <table class="data-table">
                                    <thead>
                                        <tr>
                                            <th>"Name"</th>
                                            <th>"Plugin"</th>
                                            <th>"Bridge"</th>
                                            <th>"Subnet"</th>
                                            <th>"Gateway"</th>
                                            <th>"Attached"</th>
                                            <th>"Actions"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {move || cni_networks.get().into_iter().map(|net| {
                                            let name_del = net.name.clone();
                                            let name_att = net.name.clone();
                                            let attached = cni_attachments
                                                .get()
                                                .iter()
                                                .filter(|a| a.network_name == net.name)
                                                .count();
                                            view! {
                                                <tr>
                                                    <td>{&net.name}</td>
                                                    <td><span class="type-badge">{&net.plugin_type}</span></td>
                                                    <td>{net.bridge.clone().unwrap_or_else(|| "-".to_string())}</td>
                                                    <td><code>{net.ipam.subnet.clone().unwrap_or_else(|| "-".to_string())}</code></td>
                                                    <td><code>{net.ipam.gateway.clone().unwrap_or_else(|| "-".to_string())}</code></td>
                                                    <td class="cni-attached-count">{attached}</td>
                                                    <td class="actions">
                                                        <button class="btn btn-sm btn-primary cni-attach-btn"
                                                                on:click=move |_| open_attach(name_att.clone())>
                                                            "Attach container"
                                                        </button>
                                                        <button class="btn btn-sm btn-danger"
                                                                on:click=move |_| delete_cni_network(name_del.clone())>
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

                        <h3>"Attached containers"</h3>
                        {move || if cni_attachments.get().is_empty() {
                            view! {
                                <p class="empty-hint">"No container is attached to an SDN network. Use \"Attach container\" on a network above."</p>
                            }.into_view()
                        } else {
                            view! {
                                <table class="data-table cni-attachments-table">
                                    <thead>
                                        <tr>
                                            <th>"Container"</th>
                                            <th>"Network"</th>
                                            <th>"Interface"</th>
                                            <th>"IP"</th>
                                            <th>"MAC"</th>
                                            <th>"Policy namespace / labels"</th>
                                            <th>"Actions"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {move || cni_attachments.get().into_iter().map(|a| {
                                            let ct = a.container_id.clone();
                                            let nw = a.network_name.clone();
                                            let mut labels: Vec<String> = a.labels.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
                                            labels.sort();
                                            view! {
                                                <tr>
                                                    <td>{&a.container_id}</td>
                                                    <td>{&a.network_name}</td>
                                                    <td>{&a.interface_name}</td>
                                                    <td><code>{&a.ip_address}</code></td>
                                                    <td><code>{&a.mac_address}</code></td>
                                                    <td>{format!("{} {}", a.namespace, labels.join(", "))}</td>
                                                    <td class="actions">
                                                        <button class="btn btn-sm btn-danger cni-detach-btn"
                                                                on:click=move |_| detach_container(ct.clone(), nw.clone())>
                                                            "Detach"
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

            // Attach a container to an SDN network
            {move || attach_target.get().map(|network| view! {
                <div class="modal-overlay" on:click=move |_| set_attach_target.set(None)>
                    <div class="modal" id="cni-attach-modal" on:click=|e| e.stop_propagation()>
                        <div class="modal-header">
                            <h3>{format!("Attach a container to {}", network)}</h3>
                            <button class="btn-close" on:click=move |_| set_attach_target.set(None)>"x"</button>
                        </div>
                        <div class="modal-body">
                            <div class="form-group">
                                <label>"Container (running)"</label>
                                <select node_ref=attach_ct_ref class="form-control" id="cni-attach-container">
                                    {move || attach_candidates.get().into_iter().map(|c| {
                                        let label = format!("{} ({}, {})", c.name, c.id, c.runtime);
                                        view! { <option value={c.id.clone()}>{label}</option> }
                                    }).collect_view()}
                                </select>
                                {move || attach_candidates.get().is_empty().then(|| view! {
                                    <p class="form-hint">"No running container. Start one on the Containers page first."</p>
                                })}
                            </div>
                            <div class="form-group">
                                <label>"Interface name in the container"</label>
                                <input node_ref=attach_if_ref type="text" class="form-control" placeholder="net0 (first free netN)"/>
                            </div>
                            <div class="form-group">
                                <label>"Policy namespace"</label>
                                <input node_ref=attach_ns_ref type="text" class="form-control" placeholder="default"/>
                            </div>
                            <div class="form-group">
                                <label>"Labels (for network policy selectors)"</label>
                                <input node_ref=attach_labels_ref type="text" class="form-control" placeholder="app=web, tier=frontend"/>
                            </div>
                            <p class="form-hint">"VMs are not CNI endpoints: once a container has brought this network's bridge up, give a VM a NIC on that bridge (VM > Hardware > Network)."</p>
                        </div>
                        <div class="modal-footer">
                            <button class="btn btn-secondary" on:click=move |_| set_attach_target.set(None)>"Cancel"</button>
                            <button class="btn btn-primary" id="cni-attach-submit" on:click=attach_container>"Attach"</button>
                        </div>
                    </div>
                </div>
            })}

            // SDN Network Policies Tab (horcrux-api/src/sdn/policy.rs — same
            // previously-stranded-code gap as the CNI tab above)
            {move || if !loading.get() && active_tab.get() == "sdn-policies" {
                view! {
                    <div class="tab-content">
                        <div class="section-header">
                            <h3>"SDN Network Policies"</h3>
                            <button class="btn btn-primary" on:click=move |_| set_show_create_policy.set(true)>
                                "+ Create Policy"
                            </button>
                        </div>

                        {move || if net_policies.get().is_empty() {
                            view! {
                                <div class="empty-state">
                                    <div class="icon">"📜"</div>
                                    <h3>"No Network Policies"</h3>
                                    <p>"Create a Kubernetes-style network policy to filter real traffic between workloads."</p>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <table class="data-table">
                                    <thead>
                                        <tr>
                                            <th>"Name"</th>
                                            <th>"Namespace"</th>
                                            <th>"Types"</th>
                                            <th>"Enabled"</th>
                                            <th>"Actions"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {move || net_policies.get().into_iter().map(|policy| {
                                            let id_del = policy.id.clone();
                                            view! {
                                                <tr>
                                                    <td>{&policy.name}</td>
                                                    <td>{&policy.namespace}</td>
                                                    <td>{policy.policy_types.join(", ")}</td>
                                                    <td>{if policy.enabled { "Yes" } else { "No" }}</td>
                                                    <td class="actions">
                                                        <button class="btn btn-sm btn-danger"
                                                                on:click=move |_| delete_policy(id_del.clone())>
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

            // Create SDN CNI Network Modal
            {move || show_create_cni.get().then(|| view! {
                <div class="modal-overlay" on:click=move |_| set_show_create_cni.set(false)>
                    <div class="modal" on:click=|e| e.stop_propagation()>
                        <div class="modal-header">
                            <h3>"Create SDN CNI Network"</h3>
                            <button class="btn-close" on:click=move |_| set_show_create_cni.set(false)>
                                "x"
                            </button>
                        </div>
                        <div class="modal-body">
                            <div class="form-group">
                                <label>"Network Name"</label>
                                <input node_ref=cni_name_ref type="text" class="form-control" placeholder="cni0"/>
                            </div>
                            <div class="form-group">
                                <label>"Plugin Type"</label>
                                <select node_ref=cni_plugin_ref class="form-control">
                                    <option value="bridge">"Bridge"</option>
                                    <option value="macvlan">"Macvlan"</option>
                                    <option value="ipvlan">"Ipvlan"</option>
                                    <option value="vlan">"VLAN"</option>
                                    <option value="vxlan">"VXLAN"</option>
                                    <option value="ptp">"Point-to-Point"</option>
                                    <option value="host">"Host"</option>
                                    <option value="loopback">"Loopback"</option>
                                </select>
                            </div>
                            <div class="form-group">
                                <label>"Bridge Device"</label>
                                <input node_ref=cni_bridge_ref type="text" class="form-control" placeholder="cni-br0"/>
                            </div>
                            <div class="form-group">
                                <label>"Subnet (CIDR)"</label>
                                <input node_ref=cni_subnet_ref type="text" class="form-control" placeholder="10.244.0.0/24"/>
                            </div>
                            <div class="form-group">
                                <label>"Gateway"</label>
                                <input node_ref=cni_gateway_ref type="text" class="form-control" placeholder="10.244.0.1"/>
                            </div>
                        </div>
                        <div class="modal-footer">
                            <button class="btn btn-secondary" on:click=move |_| set_show_create_cni.set(false)>
                                "Cancel"
                            </button>
                            <button class="btn btn-primary" on:click=create_cni_network>"Create"</button>
                        </div>
                    </div>
                </div>
            })}

            // Create SDN Network Policy Modal
            {move || show_create_policy.get().then(|| view! {
                <div class="modal-overlay" on:click=move |_| set_show_create_policy.set(false)>
                    <div class="modal" on:click=|e| e.stop_propagation()>
                        <div class="modal-header">
                            <h3>"Create Network Policy"</h3>
                            <button class="btn-close" on:click=move |_| set_show_create_policy.set(false)>
                                "x"
                            </button>
                        </div>
                        <div class="modal-body">
                            <div class="form-group">
                                <label>"Policy Name"</label>
                                <input node_ref=policy_name_ref type="text" class="form-control" placeholder="deny-all-ingress"/>
                            </div>
                            <div class="form-group">
                                <label>"Namespace"</label>
                                <input node_ref=policy_namespace_ref type="text" class="form-control" placeholder="default"/>
                            </div>
                            <div class="form-group">
                                <label>
                                    <input node_ref=policy_ingress_ref type="checkbox" checked/>
                                    " Applies to Ingress"
                                </label>
                            </div>
                            <div class="form-group">
                                <label>
                                    <input node_ref=policy_egress_ref type="checkbox"/>
                                    " Applies to Egress"
                                </label>
                            </div>
                        </div>
                        <div class="modal-footer">
                            <button class="btn btn-secondary" on:click=move |_| set_show_create_policy.set(false)>
                                "Cancel"
                            </button>
                            <button class="btn btn-primary" on:click=create_policy>"Create"</button>
                        </div>
                    </div>
                </div>
            })}
        </div>
    }
}
