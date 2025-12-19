use leptos::*;
use crate::websocket;

#[component]
pub fn Monitoring() -> impl IntoView {
    // Subscribe to node and VM metrics via WebSocket
    let (ws_event, ws_connected) = websocket::use_websocket(vec![
        websocket::TOPIC_NODE_METRICS.to_string(),
        websocket::TOPIC_VM_METRICS.to_string(),
    ]);

    // Store latest metrics
    let (node_metrics, set_node_metrics) = create_signal(None::<NodeMetricsData>);
    let (vm_metrics_list, set_vm_metrics_list) = create_signal(Vec::<VmMetricsData>::new());

    // Update metrics when WebSocket events arrive
    create_effect(move |_| {
        if let Some(event) = ws_event.get() {
            match event {
                websocket::WsEvent::NodeMetrics {
                    hostname,
                    cpu_usage,
                    memory_usage,
                    disk_usage,
                    load_average,
                    timestamp,
                } => {
                    set_node_metrics.set(Some(NodeMetricsData {
                        hostname,
                        cpu_usage,
                        memory_usage,
                        disk_usage,
                        load_average,
                        timestamp,
                    }));
                }
                websocket::WsEvent::VmMetrics {
                    vm_id,
                    cpu_usage,
                    memory_usage,
                    disk_read,
                    disk_write,
                    network_rx,
                    network_tx,
                    timestamp,
                } => {
                    set_vm_metrics_list.update(|list| {
                        // Update or add VM metrics
                        if let Some(existing) = list.iter_mut().find(|vm| vm.vm_id == vm_id) {
                            existing.cpu_usage = cpu_usage;
                            existing.memory_usage = memory_usage;
                            existing.disk_read = disk_read;
                            existing.disk_write = disk_write;
                            existing.network_rx = network_rx;
                            existing.network_tx = network_tx;
                            existing.timestamp = timestamp;
                        } else {
                            list.push(VmMetricsData {
                                vm_id,
                                cpu_usage,
                                memory_usage,
                                disk_read,
                                disk_write,
                                network_rx,
                                network_tx,
                                timestamp,
                            });
                        }
                    });
                }
                _ => {}
            }
        }
    });

    view! {
        <div class="monitoring-page">
            <div class="page-header">
                <h1>"System Monitoring"</h1>
                <div class="header-actions">
                    <div class="connection-status">
                        {move || if ws_connected.get() {
                            view! { <span class="badge badge-success">"● Connected"</span> }.into_view()
                        } else {
                            view! { <span class="badge badge-danger">"○ Disconnected"</span> }.into_view()
                        }}
                    </div>
                    <button class="btn btn-secondary">"⚙ Configure Alerts"</button>
                    <button class="btn btn-primary">"📊 Export Data"</button>
                </div>
            </div>

            // Node Metrics Section
            <div class="metrics-section">
                <h2>"Node Metrics"</h2>
                {move || {
                    if let Some(metrics) = node_metrics.get() {
                        view! {
                            <div class="node-metrics-grid">
                                <MetricCard
                                    title="CPU Usage"
                                    value=metrics.cpu_usage
                                    unit="%"
                                    max=100.0
                                    icon="⚙️"
                                />
                                <MetricCard
                                    title="Memory Usage"
                                    value=metrics.memory_usage
                                    unit="%"
                                    max=100.0
                                    icon="💾"
                                />
                                <MetricCard
                                    title="Disk Usage"
                                    value=metrics.disk_usage
                                    unit="%"
                                    max=100.0
                                    icon="📀"
                                />
                                <LoadAverageCard load_average=metrics.load_average/>
                            </div>
                            <div class="metric-details">
                                <p class="metric-timestamp">"Last updated: "{&metrics.timestamp}</p>
                                <p class="metric-hostname">"Node: "{&metrics.hostname}</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="no-data">
                                <p>"Waiting for node metrics..."</p>
                                <p class="hint">"Make sure the monitoring service is running."</p>
                            </div>
                        }.into_view()
                    }
                }}
            </div>

            // VM Metrics Section
            <div class="metrics-section">
                <h2>"Virtual Machine Metrics"</h2>
                {move || {
                    let vms = vm_metrics_list.get();
                    if vms.is_empty() {
                        view! {
                            <div class="no-data">
                                <p>"No VM metrics available yet."</p>
                                <p class="hint">"Metrics will appear here when VMs are running."</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="vm-metrics-list">
                                {vms.into_iter().map(|vm| {
                                    view! {
                                        <VmMetricsCard vm=vm/>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_view()
                    }
                }}
            </div>

            // Real-time Graph Section (placeholder for future charts)
            <div class="metrics-section">
                <h2>"Performance Trends"</h2>
                <div class="chart-container">
                    <p class="chart-placeholder">"📈 Real-time graphs coming soon..."</p>
                    <p class="hint">"CPU, Memory, Disk I/O, and Network trends over time"</p>
                </div>
            </div>
        </div>
    }
}

#[derive(Clone, Debug)]
struct NodeMetricsData {
    hostname: String,
    cpu_usage: f64,
    memory_usage: f64,
    disk_usage: f64,
    load_average: [f64; 3],
    timestamp: String,
}

#[derive(Clone, Debug)]
struct VmMetricsData {
    vm_id: String,
    cpu_usage: f64,
    memory_usage: f64,
    disk_read: u64,
    disk_write: u64,
    network_rx: u64,
    network_tx: u64,
    timestamp: String,
}

#[component]
fn MetricCard(
    title: &'static str,
    value: f64,
    unit: &'static str,
    max: f64,
    icon: &'static str,
) -> impl IntoView {
    let percentage = (value / max) * 100.0;
    let status_class = if percentage > 90.0 {
        "metric-card critical"
    } else if percentage > 75.0 {
        "metric-card warning"
    } else {
        "metric-card ok"
    };

    view! {
        <div class=status_class>
            <div class="metric-header">
                <span class="metric-icon">{icon}</span>
                <h3>{title}</h3>
            </div>
            <div class="metric-value-large">
                {format!("{:.1}{}", value, unit)}
            </div>
            <div class="metric-bar-container">
                <div
                    class="metric-bar-fill"
                    style=format!("width: {}%", percentage.min(100.0))
                ></div>
            </div>
            <div class="metric-status">
                {if percentage > 90.0 {
                    "Critical"
                } else if percentage > 75.0 {
                    "Warning"
                } else if percentage > 50.0 {
                    "Moderate"
                } else {
                    "Good"
                }}
            </div>
        </div>
    }
}

#[component]
fn LoadAverageCard(load_average: [f64; 3]) -> impl IntoView {
    view! {
        <div class="metric-card load-average">
            <div class="metric-header">
                <span class="metric-icon">"⚖️"</span>
                <h3>"Load Average"</h3>
            </div>
            <div class="load-values">
                <div class="load-item">
                    <span class="load-label">"1 min"</span>
                    <span class="load-value">{format!("{:.2}", load_average[0])}</span>
                </div>
                <div class="load-item">
                    <span class="load-label">"5 min"</span>
                    <span class="load-value">{format!("{:.2}", load_average[1])}</span>
                </div>
                <div class="load-item">
                    <span class="load-label">"15 min"</span>
                    <span class="load-value">{format!("{:.2}", load_average[2])}</span>
                </div>
            </div>
        </div>
    }
}

#[component]
fn VmMetricsCard(vm: VmMetricsData) -> impl IntoView {
    view! {
        <div class="vm-metric-card">
            <div class="vm-metric-header">
                <h4>"VM "{&vm.vm_id}</h4>
                <span class="timestamp">{&vm.timestamp}</span>
            </div>
            <div class="vm-metric-grid">
                <div class="vm-metric-item">
                    <span class="metric-label">"CPU"</span>
                    <span class="metric-value">{format!("{:.1}%", vm.cpu_usage)}</span>
                    <div class="mini-bar">
                        <div
                            class="mini-bar-fill"
                            style=format!("width: {}%", vm.cpu_usage.min(100.0))
                        ></div>
                    </div>
                </div>
                <div class="vm-metric-item">
                    <span class="metric-label">"Memory"</span>
                    <span class="metric-value">{format!("{:.1}%", vm.memory_usage)}</span>
                    <div class="mini-bar">
                        <div
                            class="mini-bar-fill"
                            style=format!("width: {}%", vm.memory_usage.min(100.0))
                        ></div>
                    </div>
                </div>
                <div class="vm-metric-item">
                    <span class="metric-label">"Disk Read"</span>
                    <span class="metric-value">{format_bytes(vm.disk_read)}</span>
                </div>
                <div class="vm-metric-item">
                    <span class="metric-label">"Disk Write"</span>
                    <span class="metric-value">{format_bytes(vm.disk_write)}</span>
                </div>
                <div class="vm-metric-item">
                    <span class="metric-label">"Network RX"</span>
                    <span class="metric-value">{format_bytes(vm.network_rx)}</span>
                </div>
                <div class="vm-metric-item">
                    <span class="metric-label">"Network TX"</span>
                    <span class="metric-value">{format_bytes(vm.network_tx)}</span>
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
