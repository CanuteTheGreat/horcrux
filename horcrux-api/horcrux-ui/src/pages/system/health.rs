use leptos::*;
use crate::api::*;

#[component]
pub fn SystemHealthPage() -> impl IntoView {
    let (system_health, set_system_health) = create_signal(None::<SystemHealth>);
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);

    // Load system health data
    let load_health = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_system_health().await {
            Ok(health) => set_system_health.set(Some(health)),
            Err(e) => set_error_message.set(Some(format!("Failed to load system health: {}", e))),
        }

        set_loading.set(false);
    });

    // Auto-refresh every 30 seconds
    let refresh_interval = 30000; // 30 seconds
    create_effect(move |_| {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::JsCast;

        let closure = Closure::wrap(Box::new(move || {
            load_health.dispatch(());
        }) as Box<dyn Fn()>);

        // Initial load
        load_health.dispatch(());

        // Set up interval
        web_sys::window()
            .unwrap()
            .set_interval_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                refresh_interval,
            )
            .unwrap();

        closure.forget();
    });

    // Helper functions
    let format_bytes = |bytes: u64| -> String {
        if bytes >= 1024_u64.pow(4) {
            format!("{:.2} TB", bytes as f64 / 1024_f64.powi(4))
        } else if bytes >= 1024_u64.pow(3) {
            format!("{:.2} GB", bytes as f64 / 1024_f64.powi(3))
        } else if bytes >= 1024_u64.pow(2) {
            format!("{:.2} MB", bytes as f64 / 1024_f64.powi(2))
        } else if bytes >= 1024 {
            format!("{:.2} KB", bytes as f64 / 1024.0)
        } else {
            format!("{} B", bytes)
        }
    };

    let format_uptime = |uptime_seconds: u64| -> String {
        let days = uptime_seconds / 86400;
        let hours = (uptime_seconds % 86400) / 3600;
        let minutes = (uptime_seconds % 3600) / 60;
        format!("{}d {}h {}m", days, hours, minutes)
    };

    let get_usage_color = |percentage: f64| -> &'static str {
        if percentage >= 90.0 { "danger" }
        else if percentage >= 75.0 { "warning" }
        else { "success" }
    };

    let calculate_percentage = |used: u64, total: u64| -> f64 {
        if total == 0 { 0.0 } else { (used as f64 / total as f64) * 100.0 }
    };

    view! {
        <div class="system-health-page">
            <div class="page-header">
                <h1>"System Health"</h1>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_health.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                </div>
            </div>

            {move || error_message.get().map(|msg| view! {
                <div class="alert alert-error">{msg}</div>
            })}

            {move || if loading.get() && system_health.get().is_none() {
                view! { <div class="loading">"Loading system health data..."</div> }.into_view()
            } else if let Some(health) = system_health.get() {
                let memory_percentage = calculate_percentage(health.memory_usage.used, health.memory_usage.total);
                let memory_color = get_usage_color(memory_percentage);

                view! {
                    <div class="health-dashboard">
                        // System Overview Cards
                        <div class="health-cards">
                            <div class="health-card">
                                <div class="card-icon system-icon">"🖥️"</div>
                                <div class="card-content">
                                    <h3>"System Info"</h3>
                                    <div class="info-details">
                                        <div class="detail-row">
                                            <span class="label">"Hostname:"</span>
                                            <span class="value">{&health.hostname}</span>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Uptime:"</span>
                                            <span class="value">{format_uptime(health.uptime)}</span>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Architecture:"</span>
                                            <span class="value">{&health.system_info.arch}</span>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Kernel:"</span>
                                            <span class="value">{&health.system_info.kernel_version}</span>
                                        </div>
                                    </div>
                                </div>
                            </div>

                            <div class="health-card">
                                <div class="card-icon cpu-icon">"⚡"</div>
                                <div class="card-content">
                                    <h3>"CPU"</h3>
                                    <div class="info-details">
                                        <div class="detail-row">
                                            <span class="label">"Model:"</span>
                                            <span class="value cpu-model">{&health.system_info.cpu_model}</span>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Cores:"</span>
                                            <span class="value">{health.system_info.cpu_cores}</span>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Load Average:"</span>
                                            <span class="value load-avg">
                                                {format!("{:.2}, {:.2}, {:.2}",
                                                    health.load_average[0],
                                                    health.load_average[1],
                                                    health.load_average[2])}
                                            </span>
                                        </div>
                                    </div>
                                </div>
                            </div>

                            <div class="health-card">
                                <div class="card-icon memory-icon">"💾"</div>
                                <div class="card-content">
                                    <h3>"Memory"</h3>
                                    <div class="info-details">
                                        <div class="detail-row">
                                            <span class="label">"Total:"</span>
                                            <span class="value">{format_bytes(health.memory_usage.total)}</span>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Used:"</span>
                                            <span class="value">{format_bytes(health.memory_usage.used)}</span>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Available:"</span>
                                            <span class="value">{format_bytes(health.memory_usage.available)}</span>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Usage:"</span>
                                            <span class={format!("value usage-{}", memory_color)}>
                                                {format!("{:.1}%", memory_percentage)}
                                            </span>
                                        </div>
                                    </div>
                                    <div class="progress-bar">
                                        <div
                                            class={format!("progress-fill progress-{}", memory_color)}
                                            style=format!("width: {}%", memory_percentage)
                                        ></div>
                                    </div>
                                </div>
                            </div>

                            <div class="health-card">
                                <div class="card-icon network-icon">"🌐"</div>
                                <div class="card-content">
                                    <h3>"Network"</h3>
                                    <div class="network-interfaces">
                                        {health.network_stats.clone().into_iter().map(|stats| {
                                            let interface = stats.interface.clone();
                                            let rx_bytes = stats.rx_bytes;
                                            let tx_bytes = stats.tx_bytes;
                                            view! {
                                                <div class="interface-stats">
                                                    <div class="interface-name">{interface}</div>
                                                    <div class="interface-data">
                                                        <span class="rx">
                                                            "↓ "{format_bytes(rx_bytes)}
                                                        </span>
                                                        <span class="tx">
                                                            "↑ "{format_bytes(tx_bytes)}
                                                        </span>
                                                    </div>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>
                            </div>
                        </div>

                        // Disk Usage Section
                        <div class="disk-usage-section">
                            <h2>"Storage"</h2>
                            <div class="disk-grid">
                                {health.disk_usage.clone().into_iter().map(|disk| {
                                    let disk_percentage = calculate_percentage(disk.used, disk.total);
                                    let disk_color = get_usage_color(disk_percentage);
                                    let mount_point = disk.mount_point.clone();
                                    let filesystem = disk.filesystem.clone();
                                    let device = disk.device.clone();
                                    let total = disk.total;
                                    let used = disk.used;
                                    let available = disk.available;

                                    view! {
                                        <div class="disk-card">
                                            <div class="disk-header">
                                                <h4>{mount_point}</h4>
                                                <span class="filesystem-type">{filesystem}</span>
                                            </div>
                                            <div class="disk-details">
                                                <div class="detail-row">
                                                    <span class="label">"Device:"</span>
                                                    <span class="value">{device}</span>
                                                </div>
                                                <div class="detail-row">
                                                    <span class="label">"Total:"</span>
                                                    <span class="value">{format_bytes(total)}</span>
                                                </div>
                                                <div class="detail-row">
                                                    <span class="label">"Used:"</span>
                                                    <span class="value">{format_bytes(used)}</span>
                                                </div>
                                                <div class="detail-row">
                                                    <span class="label">"Available:"</span>
                                                    <span class="value">{format_bytes(available)}</span>
                                                </div>
                                                <div class="detail-row">
                                                    <span class="label">"Usage:"</span>
                                                    <span class={format!("value usage-{}", disk_color)}>
                                                        {format!("{:.1}%", disk_percentage)}
                                                    </span>
                                                </div>
                                            </div>
                                            <div class="progress-bar">
                                                <div
                                                    class={format!("progress-fill progress-{}", disk_color)}
                                                    style=format!("width: {}%", disk_percentage)
                                                ></div>
                                            </div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        </div>

                        // Network Statistics
                        <div class="network-stats-section">
                            <h2>"Network Statistics"</h2>
                            <div class="network-table">
                                <table>
                                    <thead>
                                        <tr>
                                            <th>"Interface"</th>
                                            <th>"RX Bytes"</th>
                                            <th>"TX Bytes"</th>
                                            <th>"RX Packets"</th>
                                            <th>"TX Packets"</th>
                                            <th>"RX Errors"</th>
                                            <th>"TX Errors"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {health.network_stats.clone().into_iter().map(|stats| {
                                            let interface = stats.interface.clone();
                                            let rx_bytes = stats.rx_bytes;
                                            let tx_bytes = stats.tx_bytes;
                                            let rx_packets = stats.rx_packets;
                                            let tx_packets = stats.tx_packets;
                                            let rx_errors = stats.rx_errors;
                                            let tx_errors = stats.tx_errors;
                                            let rx_errors_class = if stats.rx_errors > 0 { "text-danger" } else { "text-success" };
                                            let tx_errors_class = if stats.tx_errors > 0 { "text-danger" } else { "text-success" };
                                            view! {
                                                <tr>
                                                    <td class="interface-name">{interface}</td>
                                                    <td>{format_bytes(rx_bytes)}</td>
                                                    <td>{format_bytes(tx_bytes)}</td>
                                                    <td>{rx_packets.to_string()}</td>
                                                    <td>{tx_packets.to_string()}</td>
                                                    <td class=rx_errors_class>
                                                        {rx_errors.to_string()}
                                                    </td>
                                                    <td class=tx_errors_class>
                                                        {tx_errors.to_string()}
                                                    </td>
                                                </tr>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </tbody>
                                </table>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! { <div class="empty-state">"No system health data available"</div> }.into_view()
            }}
        </div>
    }
}