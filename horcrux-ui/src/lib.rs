use leptos::*;
use leptos_meta::*;
use leptos_router::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VmConfig {
    pub id: String,
    pub name: String,
    pub memory: u64,
    pub cpus: u32,
    pub disk_size: u64,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoragePool {
    pub id: String,
    pub name: String,
    pub storage_type: String,
    pub available: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClusterNode {
    pub name: String,
    pub address: String,
    pub architecture: String,
    pub total_memory: u64,
    pub total_cpus: u32,
    pub online: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeMetrics {
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub memory_total: u64,
    pub disk_usage: f64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HaResource {
    pub vm_id: String,
    pub priority: u32,
    pub state: String,
    pub current_node: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HaStatus {
    pub enabled: bool,
    pub resources: Vec<HaResource>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DashboardStats {
    pub total_vms: usize,
    pub running_vms: usize,
    pub total_containers: usize,
    pub total_storage_gb: u64,
    pub used_storage_gb: u64,
    pub total_nodes: usize,
    pub online_nodes: usize,
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/horcrux-ui.css"/>
        <Title text="Horcrux - Gentoo Virtualization Platform"/>
        <Router>
            <nav class="top-nav">
                <div class="nav-brand">
                    <h2>"⚡ Horcrux"</h2>
                </div>
                <div class="nav-links">
                    <A href="/" class="nav-link">"Dashboard"</A>
                    <A href="/vms" class="nav-link">"VMs"</A>
                    <A href="/containers" class="nav-link">"Containers"</A>
                    <A href="/storage" class="nav-link">"Storage"</A>
                    <A href="/network" class="nav-link">"Network"</A>
                    <A href="/cluster" class="nav-link">"Cluster"</A>
                </div>
                <div class="nav-actions">
                    <button class="btn-icon" title="Settings">"⚙"</button>
                    <button class="btn-icon" title="Notifications">"🔔"</button>
                    <button class="btn-icon" title="User">"👤"</button>
                </div>
            </nav>
            <main class="main-content">
                <Routes>
                    <Route path="" view=Dashboard/>
                    <Route path="/vms" view=VirtualMachines/>
                    <Route path="/containers" view=Containers/>
                    <Route path="/storage" view=Storage/>
                    <Route path="/cluster" view=Cluster/>
                    <Route path="/network" view=Network/>
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn Dashboard() -> impl IntoView {
    let stats = create_resource(
        || (),
        |_| async move {
            DashboardStats {
                total_vms: 12,
                running_vms: 8,
                total_containers: 24,
                total_storage_gb: 2000,
                used_storage_gb: 1240,
                total_nodes: 3,
                online_nodes: 3,
            }
        },
    );

    let metrics = create_resource(
        || (),
        |_| async move {
            NodeMetrics {
                cpu_usage: 45.2,
                memory_usage: 62.8,
                memory_total: 65536,
                disk_usage: 68.5,
                network_rx_bytes: 1024000000,
                network_tx_bytes: 512000000,
            }
        },
    );

    let (show_advanced, set_show_advanced) = create_signal(false);

    view! {
        <div class="dashboard">
            <div class="page-header">
                <h1>"Dashboard"</h1>
                <div class="header-actions">
                    <button class="btn-secondary" on:click=move |_| set_show_advanced.update(|v| *v = !*v)>
                        {move || if show_advanced.get() { "▼ Hide Advanced" } else { "▶ Show Advanced" }}
                    </button>
                    <button class="btn-primary">"+ Quick Create"</button>
                </div>
            </div>

            <Suspense fallback=move || view! { <p>"Loading..."</p> }>
                {move || stats.get().map(|stats_data| {
                    view! {
                        <div class="stats-grid">
                            <StatCard
                                icon="🖥️"
                                title="Virtual Machines"
                                value=format!("{}/{}", stats_data.running_vms, stats_data.total_vms)
                                label="Running / Total"
                                link="/vms"
                            />
                            <StatCard
                                icon="📦"
                                title="Containers"
                                value=stats_data.total_containers.to_string()
                                label="Active Containers"
                                link="/containers"
                            />
                            <StatCard
                                icon="💾"
                                title="Storage"
                                value=format!("{}%", ((stats_data.used_storage_gb as f64 / stats_data.total_storage_gb as f64) * 100.0) as u32)
                                label=format!("{}/{} GB Used", stats_data.used_storage_gb, stats_data.total_storage_gb)
                                link="/storage"
                            />
                            <StatCard
                                icon="🌐"
                                title="Cluster"
                                value=format!("{}/{}", stats_data.online_nodes, stats_data.total_nodes)
                                label="Nodes Online"
                                link="/cluster"
                            />
                        </div>
                    }
                })}
            </Suspense>

            <Suspense fallback=move || view! { <p>"Loading metrics..."</p> }>
                {move || metrics.get().map(|m| {
                    view! {
                        <div class="metrics-section">
                            <h2>"System Overview"</h2>
                            <div class="metrics-grid">
                                <MetricCard label="CPU" value=m.cpu_usage unit="%" max=100.0/>
                                <MetricCard label="Memory" value=m.memory_usage unit="%" max=100.0/>
                                <MetricCard label="Disk" value=m.disk_usage unit="%" max=100.0/>
                                <div class="metric-card">
                                    <h4>"Network"</h4>
                                    <p class="metric-detail">"↓ "{format_bytes(m.network_rx_bytes)}</p>
                                    <p class="metric-detail">"↑ "{format_bytes(m.network_tx_bytes)}</p>
                                </div>
                            </div>
                        </div>
                    }
                })}
            </Suspense>

            <Show when=move || show_advanced.get()>
                <AdvancedDashboard/>
            </Show>
        </div>
    }
}

#[component]
fn AdvancedDashboard() -> impl IntoView {
    view! {
        <div class="advanced-section">
            <h2>"Advanced System Information"</h2>
            <div class="advanced-grid">
                <div class="advanced-card">
                    <h3>"Performance Trends"</h3>
                    <p>"CPU trend: ↗ +2.3% (24h)"</p>
                    <p>"Memory trend: → Stable"</p>
                    <p>"Network trend: ↗ +12% (24h)"</p>
                </div>
                <div class="advanced-card">
                    <h3>"Resource Allocation"</h3>
                    <p>"VMs: 45% of total capacity"</p>
                    <p>"Containers: 22% of total capacity"</p>
                    <p>"Available: 33%"</p>
                </div>
                <div class="advanced-card">
                    <h3>"Recent Events"</h3>
                    <p>"✓ VM migration completed (2m ago)"</p>
                    <p>"⚠ Storage pool usage >75% (15m ago)"</p>
                    <p>"ℹ Backup job started (1h ago)"</p>
                </div>
            </div>
        </div>
    }
}

#[component]
fn StatCard(
    icon: &'static str,
    title: &'static str,
    value: String,
    label: String,
    link: &'static str,
) -> impl IntoView {
    view! {
        <A href=link class="stat-card">
            <div class="stat-icon">{icon}</div>
            <div class="stat-content">
                <h3>{title}</h3>
                <p class="stat-value">{value}</p>
                <p class="stat-label">{label}</p>
            </div>
        </A>
    }
}

#[component]
fn MetricCard(label: &'static str, value: f64, unit: &'static str, max: f64) -> impl IntoView {
    let percentage = (value / max) * 100.0;
    let bar_class = if percentage > 90.0 {
        "metric-bar danger"
    } else if percentage > 75.0 {
        "metric-bar warning"
    } else {
        "metric-bar ok"
    };

    view! {
        <div class="metric-card">
            <h4>{label}</h4>
            <div class="metric-bar-container">
                <div class=bar_class style=format!("width: {}%", percentage)>
                    <span class="metric-value">{format!("{:.1}{}", value, unit)}</span>
                </div>
            </div>
        </div>
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.2} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.2} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{} B", bytes)
    }
}

#[component]
fn VirtualMachines() -> impl IntoView {
    let vms = create_resource(
        || (),
        |_| async move {
            vec![
                VmConfig {
                    id: "100".to_string(),
                    name: "web-server-01".to_string(),
                    memory: 4096,
                    cpus: 2,
                    disk_size: 50,
                    status: "running".to_string(),
                },
                VmConfig {
                    id: "101".to_string(),
                    name: "database-vm".to_string(),
                    memory: 8192,
                    cpus: 4,
                    disk_size: 100,
                    status: "running".to_string(),
                },
                VmConfig {
                    id: "102".to_string(),
                    name: "app-server".to_string(),
                    memory: 2048,
                    cpus: 2,
                    disk_size: 30,
                    status: "stopped".to_string(),
                },
            ]
        },
    );

    let (show_create_modal, set_show_create_modal) = create_signal(false);
    let (selected_vm, set_selected_vm) = create_signal::<Option<String>>(None);

    view! {
        <div class="page">
            <div class="page-header">
                <h1>"Virtual Machines"</h1>
                <div class="header-actions">
                    <input type="text" placeholder="Search VMs..." class="search-input"/>
                    <button class="btn-secondary">"⚙ Batch Actions"</button>
                    <button class="btn-primary" on:click=move |_| set_show_create_modal.set(true)>
                        "+ Create VM"
                    </button>
                </div>
            </div>

            <Suspense fallback=move || view! { <p>"Loading VMs..."</p> }>
                {move || vms.get().map(|vms_list| {
                    view! {
                        <div class="vm-list">
                            {vms_list.iter().map(|vm| {
                                let vm_id = vm.id.clone();
                                let is_selected = move || selected_vm.get() == Some(vm_id.clone());

                                view! {
                                    <VmCard
                                        vm=vm.clone()
                                        is_expanded=is_selected
                                        on_toggle=move || {
                                            if is_selected() {
                                                set_selected_vm.set(None)
                                            } else {
                                                set_selected_vm.set(Some(vm_id.clone()))
                                            }
                                        }
                                    />
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    }
                })}
            </Suspense>

            <Show when=move || show_create_modal.get()>
                <CreateVmModal on_close=move || set_show_create_modal.set(false)/>
            </Show>
        </div>
    }
}

#[component]
fn VmCard<F>(vm: VmConfig, is_expanded: F, on_toggle: F) -> impl IntoView
where
    F: Fn() -> bool + 'static + Copy,
{
    let status_class = match vm.status.as_str() {
        "running" => "status-badge running",
        "stopped" => "status-badge stopped",
        _ => "status-badge",
    };

    view! {
        <div class="vm-card" class:expanded=is_expanded>
            <div class="vm-card-header" on:click=move |_| on_toggle()>
                <div class="vm-info">
                    <h3>{&vm.name}</h3>
                    <span class=status_class>{&vm.status}</span>
                </div>
                <div class="vm-specs">
                    <span class="spec">"💾 "{vm.memory}" MB"</span>
                    <span class="spec">"⚙ "{vm.cpus}" vCPU"</span>
                    <span class="spec">"📀 "{vm.disk_size}" GB"</span>
                </div>
                <div class="vm-actions">
                    <button class="btn-icon" title="Start">"▶"</button>
                    <button class="btn-icon" title="Stop">"⏹"</button>
                    <button class="btn-icon" title="Console">"🖥"</button>
                    <button class="btn-icon" title="More">{if is_expanded() { "▼" } else { "▶" }}</button>
                </div>
            </div>

            <Show when=is_expanded>
                <div class="vm-card-details">
                    <div class="details-tabs">
                        <button class="tab-button active">"Overview"</button>
                        <button class="tab-button">"Hardware"</button>
                        <button class="tab-button">"Network"</button>
                        <button class="tab-button">"Snapshots"</button>
                        <button class="tab-button">"Backup"</button>
                    </div>
                    <div class="details-content">
                        <div class="detail-section">
                            <h4>"General Information"</h4>
                            <div class="detail-grid">
                                <div class="detail-item">
                                    <label>"VM ID:"</label>
                                    <span>{&vm.id}</span>
                                </div>
                                <div class="detail-item">
                                    <label>"OS Type:"</label>
                                    <span>"Linux (Gentoo)"</span>
                                </div>
                                <div class="detail-item">
                                    <label>"Boot Order:"</label>
                                    <span>"disk, network"</span>
                                </div>
                                <div class="detail-item">
                                    <label>"Node:"</label>
                                    <span>"node-01"</span>
                                </div>
                            </div>
                        </div>
                        <div class="detail-section">
                            <h4>"Performance"</h4>
                            <div class="detail-grid">
                                <MetricCard label="CPU" value=25.3 unit="%" max=100.0/>
                                <MetricCard label="Memory" value=45.8 unit="%" max=100.0/>
                                <MetricCard label="Disk I/O" value=12.5 unit="MB/s" max=100.0/>
                                <MetricCard label="Network" value=8.2 unit="Mb/s" max=100.0/>
                            </div>
                        </div>
                        <div class="detail-section">
                            <h4>"Advanced Actions"</h4>
                            <div class="action-grid">
                                <button class="btn-action">"🔄 Clone"</button>
                                <button class="btn-action">"📸 Snapshot"</button>
                                <button class="btn-action">"🔀 Migrate"</button>
                                <button class="btn-action">"💾 Backup"</button>
                                <button class="btn-action">"⚙ Configure"</button>
                                <button class="btn-action">"📊 Monitoring"</button>
                                <button class="btn-action">"🔒 Firewall"</button>
                                <button class="btn-action">"⚡ HA Settings"</button>
                            </div>
                        </div>
                    </div>
                </div>
            </Show>
        </div>
    }
}

#[component]
fn CreateVmModal<F>(on_close: F) -> impl IntoView
where
    F: Fn() + 'static,
{
    let (show_advanced, set_show_advanced) = create_signal(false);

    view! {
        <div class="modal-overlay" on:click=move |_| on_close()>
            <div class="modal-dialog" on:click=|e| e.stop_propagation()>
                <div class="modal-header">
                    <h2>"Create Virtual Machine"</h2>
                    <button class="btn-close" on:click=move |_| on_close()>"✕"</button>
                </div>
                <div class="modal-body">
                    <div class="form-section">
                        <h3>"Basic Configuration"</h3>
                        <div class="form-grid">
                            <div class="form-group">
                                <label>"VM Name"</label>
                                <input type="text" placeholder="my-vm" class="form-input"/>
                            </div>
                            <div class="form-group">
                                <label>"VM ID"</label>
                                <input type="number" placeholder="Auto" class="form-input"/>
                            </div>
                            <div class="form-group">
                                <label>"Memory (MB)"</label>
                                <input type="number" placeholder="2048" class="form-input"/>
                            </div>
                            <div class="form-group">
                                <label>"CPU Cores"</label>
                                <input type="number" placeholder="2" class="form-input"/>
                            </div>
                            <div class="form-group">
                                <label>"Disk Size (GB)"</label>
                                <input type="number" placeholder="32" class="form-input"/>
                            </div>
                            <div class="form-group">
                                <label>"OS Type"</label>
                                <select class="form-select">
                                    <option>"Linux"</option>
                                    <option>"Windows"</option>
                                    <option>"Other"</option>
                                </select>
                            </div>
                        </div>
                    </div>

                    <button
                        class="btn-expand"
                        on:click=move |_| set_show_advanced.update(|v| *v = !*v)
                    >
                        {move || if show_advanced.get() { "▼ Hide Advanced Options" } else { "▶ Show Advanced Options" }}
                    </button>

                    <Show when=move || show_advanced.get()>
                        <div class="form-section">
                            <h3>"Advanced Configuration"</h3>
                            <div class="form-grid">
                                <div class="form-group">
                                    <label>"Boot Order"</label>
                                    <input type="text" placeholder="disk,network" class="form-input"/>
                                </div>
                                <div class="form-group">
                                    <label>"BIOS Type"</label>
                                    <select class="form-select">
                                        <option>"SeaBIOS"</option>
                                        <option>"OVMF (UEFI)"</option>
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label>"Machine Type"</label>
                                    <select class="form-select">
                                        <option>"pc-q35"</option>
                                        <option>"pc-i440fx"</option>
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label>"CPU Type"</label>
                                    <select class="form-select">
                                        <option>"host"</option>
                                        <option>"kvm64"</option>
                                        <option>"Haswell"</option>
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label>"Network Bridge"</label>
                                    <input type="text" placeholder="vmbr0" class="form-input"/>
                                </div>
                                <div class="form-group">
                                    <label>"Storage Pool"</label>
                                    <select class="form-select">
                                        <option>"local"</option>
                                        <option>"fast-ssd"</option>
                                        <option>"ceph-pool"</option>
                                    </select>
                                </div>
                            </div>
                            <div class="form-group">
                                <label>
                                    <input type="checkbox"/>
                                    " Enable HA (High Availability)"
                                </label>
                            </div>
                            <div class="form-group">
                                <label>
                                    <input type="checkbox"/>
                                    " Start VM after creation"
                                </label>
                            </div>
                        </div>
                    </Show>
                </div>
                <div class="modal-footer">
                    <button class="btn-secondary" on:click=move |_| on_close()>"Cancel"</button>
                    <button class="btn-primary">"Create VM"</button>
                </div>
            </div>
        </div>
    }
}

#[component]
fn Storage() -> impl IntoView {
    let pools = create_resource(
        || (),
        |_| async move {
            vec![
                StoragePool {
                    id: "local".to_string(),
                    name: "local".to_string(),
                    storage_type: "dir".to_string(),
                    available: 800,
                    total: 1000,
                },
                StoragePool {
                    id: "fast-ssd".to_string(),
                    name: "fast-ssd".to_string(),
                    storage_type: "lvm".to_string(),
                    available: 1560,
                    total: 2000,
                },
                StoragePool {
                    id: "ceph-pool".to_string(),
                    name: "ceph-pool".to_string(),
                    storage_type: "rbd".to_string(),
                    available: 3200,
                    total: 5000,
                },
            ]
        },
    );

    let (expanded_pool, set_expanded_pool) = create_signal::<Option<String>>(None);

    view! {
        <div class="page">
            <div class="page-header">
                <h1>"Storage Pools"</h1>
                <button class="btn-primary">"+ Add Storage Pool"</button>
            </div>

            <Suspense fallback=move || view! { <p>"Loading storage pools..."</p> }>
                {move || pools.get().map(|pools_list| {
                    view! {
                        <div class="storage-list">
                            {pools_list.iter().map(|pool| {
                                let pool_id = pool.id.clone();
                                let is_expanded = move || expanded_pool.get() == Some(pool_id.clone());

                                view! {
                                    <StorageCard
                                        pool=pool.clone()
                                        is_expanded=is_expanded
                                        on_toggle=move || {
                                            if is_expanded() {
                                                set_expanded_pool.set(None)
                                            } else {
                                                set_expanded_pool.set(Some(pool_id.clone()))
                                            }
                                        }
                                    />
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    }
                })}
            </Suspense>
        </div>
    }
}

#[component]
fn StorageCard<F>(pool: StoragePool, is_expanded: F, on_toggle: F) -> impl IntoView
where
    F: Fn() -> bool + 'static + Copy,
{
    let used = pool.total - pool.available;
    let usage_percent = (used as f64 / pool.total as f64) * 100.0;

    view! {
        <div class="storage-card" class:expanded=is_expanded>
            <div class="storage-card-header" on:click=move |_| on_toggle()>
                <div class="storage-info">
                    <h3>{&pool.name}</h3>
                    <span class="type-badge">{&pool.storage_type}</span>
                </div>
                <div class="storage-usage">
                    <div class="usage-bar">
                        <div
                            class="usage-fill"
                            style=format!("width: {}%", usage_percent)
                            class:danger=move || usage_percent > 90.0
                            class:warning=move || usage_percent > 75.0 && usage_percent <= 90.0
                        ></div>
                    </div>
                    <span class="usage-text">{format!("{:.0}%", usage_percent)}" used"</span>
                </div>
                <div class="storage-stats">
                    <span>{used}" GB / "{pool.total}" GB"</span>
                </div>
                <button class="btn-icon">{if is_expanded() { "▼" } else { "▶" }}</button>
            </div>

            <Show when=is_expanded>
                <div class="storage-card-details">
                    <div class="details-tabs">
                        <button class="tab-button active">"Overview"</button>
                        <button class="tab-button">"Volumes"</button>
                        <button class="tab-button">"Permissions"</button>
                        <button class="tab-button">"Settings"</button>
                    </div>
                    <div class="details-content">
                        <div class="detail-section">
                            <h4>"Storage Details"</h4>
                            <div class="detail-grid">
                                <div class="detail-item">
                                    <label>"Type:"</label>
                                    <span>{&pool.storage_type}</span>
                                </div>
                                <div class="detail-item">
                                    <label>"Path:"</label>
                                    <span>"/mnt/storage/"{&pool.name}</span>
                                </div>
                                <div class="detail-item">
                                    <label>"Available:"</label>
                                    <span>{pool.available}" GB"</span>
                                </div>
                                <div class="detail-item">
                                    <label>"Used:"</label>
                                    <span>{used}" GB"</span>
                                </div>
                            </div>
                        </div>
                        <div class="detail-section">
                            <h4>"Advanced Options"</h4>
                            <div class="action-grid">
                                <button class="btn-action">"📝 Edit Configuration"</button>
                                <button class="btn-action">"➕ Add Volume"</button>
                                <button class="btn-action">"📊 Performance Stats"</button>
                                <button class="btn-action">"🔄 Resize Pool"</button>
                                <button class="btn-action">"🗑️ Remove Pool"</button>
                                <button class="btn-action">"⚡ Enable Quotas"</button>
                            </div>
                        </div>
                    </div>
                </div>
            </Show>
        </div>
    }
}

#[component]
fn Cluster() -> impl IntoView {
    view! {
        <div class="page">
            <h1>"Cluster Management"</h1>
            <p>"Cluster nodes and configuration"</p>
        </div>
    }
}

#[component]
fn Containers() -> impl IntoView {
    view! {
        <div class="page">
            <h1>"Containers"</h1>
            <p>"LXC container management"</p>
        </div>
    }
}

#[component]
fn Network() -> impl IntoView {
    view! {
        <div class="page">
            <h1>"Network"</h1>
            <p>"SDN and network configuration"</p>
        </div>
    }
}
