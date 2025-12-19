use leptos::*;
use crate::api::*;

#[component]
pub fn SmartMonitoringPage() -> impl IntoView {
    let (disks, set_disks) = create_signal(Vec::<SmartDiskInfo>::new());
    let (alerts, set_alerts) = create_signal(Vec::<SmartAlert>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (selected_disk, set_selected_disk) = create_signal(None::<SmartDiskInfo>);
    let (show_details_modal, set_show_details_modal) = create_signal(false);
    let (show_test_modal, set_show_test_modal) = create_signal(false);
    let (test_type, set_test_type) = create_signal("short".to_string());
    let (running_test, set_running_test) = create_signal(false);
    let (filter_status, set_filter_status) = create_signal("all".to_string());

    // Load SMART data
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            match get_smart_disk_info().await {
                Ok(disk_list) => {
                    set_disks.set(disk_list);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load SMART data: {}", e)));
                }
            }

            match get_smart_alerts().await {
                Ok(alert_list) => {
                    set_alerts.set(alert_list);
                }
                Err(_) => {}
            }

            set_loading.set(false);
        });
    });

    let filtered_disks = move || {
        let status = filter_status.get();
        if status == "all" {
            disks.get()
        } else {
            disks.get().into_iter().filter(|d| {
                d.smart_status.as_str() == status
            }).collect()
        }
    };

    let disk_counts = move || {
        let all = disks.get();
        let healthy = all.iter().filter(|d| d.smart_status == SmartStatus::Healthy).count();
        let warning = all.iter().filter(|d| d.smart_status == SmartStatus::Warning).count();
        let critical = all.iter().filter(|d| d.smart_status == SmartStatus::Critical).count();
        (all.len(), healthy, warning, critical)
    };

    let unacknowledged_alerts = move || {
        alerts.get().into_iter().filter(|a| !a.acknowledged).count()
    };

    let start_test = move |_| {
        if let Some(disk) = selected_disk.get() {
            let disk_id = disk.id.clone();
            let test = test_type.get();
            set_running_test.set(true);

            spawn_local(async move {
                match start_smart_test(&disk_id, &test).await {
                    Ok(_) => {
                        set_show_test_modal.set(false);
                        // Refresh disk data
                        if let Ok(disk_list) = get_smart_disk_info().await {
                            set_disks.set(disk_list);
                        }
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to start test: {}", e)));
                    }
                }
                set_running_test.set(false);
            });
        }
    };

    let acknowledge_alert = move |alert_id: String| {
        spawn_local(async move {
            match acknowledge_smart_alert(&alert_id).await {
                Ok(_) => {
                    if let Ok(alert_list) = get_smart_alerts().await {
                        set_alerts.set(alert_list);
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to acknowledge alert: {}", e)));
                }
            }
        });
    };

    view! {
        <div class="smart-monitoring-page">
            <div class="page-header">
                <h1 class="page-title">SMART Monitoring</h1>
                <p class="page-description">
                    Monitor disk health using SMART (Self-Monitoring, Analysis and Reporting Technology)
                </p>
            </div>

            // Overview Statistics
            <div class="smart-overview">
                <div class="overview-stats">
                    <div class="stat-card total">
                        <span class="stat-icon">"[D]"</span>
                        <div class="stat-content">
                            <span class="stat-value">{move || disk_counts().0}</span>
                            <span class="stat-label">"Total Disks"</span>
                        </div>
                    </div>
                    <div class="stat-card healthy">
                        <span class="stat-icon">"[OK]"</span>
                        <div class="stat-content">
                            <span class="stat-value">{move || disk_counts().1}</span>
                            <span class="stat-label">"Healthy"</span>
                        </div>
                    </div>
                    <div class="stat-card warning">
                        <span class="stat-icon">"[!]"</span>
                        <div class="stat-content">
                            <span class="stat-value">{move || disk_counts().2}</span>
                            <span class="stat-label">"Warning"</span>
                        </div>
                    </div>
                    <div class="stat-card critical">
                        <span class="stat-icon">"[X]"</span>
                        <div class="stat-content">
                            <span class="stat-value">{move || disk_counts().3}</span>
                            <span class="stat-label">"Critical"</span>
                        </div>
                    </div>
                </div>
            </div>

            // Alerts Section
            {move || {
                let unack = unacknowledged_alerts();
                if unack > 0 {
                    view! {
                        <div class="alerts-section">
                            <h2 class="section-title">Active Alerts ({unack})</h2>
                            <div class="alerts-list">
                                {alerts.get().into_iter().filter(|a| !a.acknowledged).take(5).map(|alert| {
                                    let alert_id = alert.id.clone();
                                    view! {
                                        <div class={format!("alert-item severity-{}", alert.severity)}>
                                            <div class="alert-icon">
                                                {match alert.severity.as_str() {
                                                    "critical" => "🔴",
                                                    "warning" => "🟡",
                                                    _ => "🔵"
                                                }}
                                            </div>
                                            <div class="alert-content">
                                                <div class="alert-header">
                                                    <span class="alert-disk">{alert.disk_name}</span>
                                                    <span class="alert-type">{alert.alert_type}</span>
                                                </div>
                                                <p class="alert-message">{alert.message}</p>
                                                <span class="alert-time">{alert.created_at}</span>
                                            </div>
                                            <button
                                                class="btn btn-sm btn-secondary"
                                                on:click=move |_| acknowledge_alert(alert_id.clone())
                                            >
                                                Acknowledge
                                            </button>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! {}.into_view()
                }
            }}

            // Filter Bar
            <div class="filter-bar">
                <div class="filter-buttons">
                    <button
                        class={move || if filter_status.get() == "all" { "btn btn-primary" } else { "btn btn-secondary" }}
                        on:click=move |_| set_filter_status.set("all".to_string())
                    >
                        All
                    </button>
                    <button
                        class={move || if filter_status.get() == "healthy" { "btn btn-success" } else { "btn btn-secondary" }}
                        on:click=move |_| set_filter_status.set("healthy".to_string())
                    >
                        Healthy
                    </button>
                    <button
                        class={move || if filter_status.get() == "warning" { "btn btn-warning" } else { "btn btn-secondary" }}
                        on:click=move |_| set_filter_status.set("warning".to_string())
                    >
                        Warning
                    </button>
                    <button
                        class={move || if filter_status.get() == "critical" { "btn btn-danger" } else { "btn btn-secondary" }}
                        on:click=move |_| set_filter_status.set("critical".to_string())
                    >
                        Critical
                    </button>
                </div>
            </div>

            {move || if loading.get() {
                view! {
                    <div class="loading-container">
                        <div class="spinner"></div>
                        <p>Loading SMART data...</p>
                    </div>
                }.into_view()
            } else if let Some(err) = error.get() {
                view! {
                    <div class="error-container">
                        <div class="error-message">
                            <h3>Error</h3>
                            <p>{err}</p>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {
                    <div class="disks-grid">
                        {filtered_disks().into_iter().map(|disk| {
                            let disk_clone = disk.clone();
                            let disk_clone_2 = disk.clone();

                            view! {
                                <div class={format!("smart-disk-card status-{}", disk.smart_status.as_str())}>
                                    <div class="disk-header">
                                        <div class="disk-title">
                                            <h3>{disk.name.clone()}</h3>
                                            <span class="disk-path">{disk.path.clone()}</span>
                                        </div>
                                        <div class="health-score-ring">
                                            <svg viewBox="0 0 36 36" class="health-ring">
                                                <path
                                                    d="M18 2.0845
                                                        a 15.9155 15.9155 0 0 1 0 31.831
                                                        a 15.9155 15.9155 0 0 1 0 -31.831"
                                                    fill="none"
                                                    stroke="#eee"
                                                    stroke-width="3"
                                                />
                                                <path
                                                    d="M18 2.0845
                                                        a 15.9155 15.9155 0 0 1 0 31.831
                                                        a 15.9155 15.9155 0 0 1 0 -31.831"
                                                    fill="none"
                                                    stroke={get_health_color(disk.health_score)}
                                                    stroke-width="3"
                                                    stroke-dasharray={format!("{}, 100", disk.health_score)}
                                                />
                                            </svg>
                                            <span class="health-value">{disk.health_score}%</span>
                                        </div>
                                    </div>

                                    <div class="disk-info-grid">
                                        <div class="info-item">
                                            <span class="label">Model</span>
                                            <span class="value">{disk.model.clone()}</span>
                                        </div>
                                        <div class="info-item">
                                            <span class="label">Serial</span>
                                            <span class="value">{disk.serial.clone()}</span>
                                        </div>
                                        <div class="info-item">
                                            <span class="label">Type</span>
                                            <span class="value">{disk.disk_type.to_uppercase()} ({disk.interface.clone()})</span>
                                        </div>
                                        <div class="info-item">
                                            <span class="label">Capacity</span>
                                            <span class="value">{format_bytes(disk.capacity_bytes)}</span>
                                        </div>
                                        {disk.temperature.map(|temp| view! {
                                            <div class="info-item">
                                                <span class="label">"Temperature"</span>
                                                <span class={format!("value {}", if temp > 50 { "hot" } else if temp > 40 { "warm" } else { "" })}>
                                                    {format!("{}C", temp)}
                                                </span>
                                            </div>
                                        })}
                                        {disk.power_on_hours.map(|hours| view! {
                                            <div class="info-item">
                                                <span class="label">Power On</span>
                                                <span class="value">{format_hours(hours)}</span>
                                            </div>
                                        })}
                                    </div>

                                    // Key SMART Attributes
                                    <div class="smart-attributes-preview">
                                        <h4>Key Attributes</h4>
                                        <div class="attributes-list">
                                            {disk.attributes.iter().take(4).map(|attr| {
                                                view! {
                                                    <div class={format!("attribute-item status-{}", attr.status)}>
                                                        <span class="attr-name">{attr.name.clone()}</span>
                                                        <span class="attr-value">{attr.raw_value.clone()}</span>
                                                    </div>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    </div>

                                    // Last Test Result
                                    {disk.last_test.as_ref().map(|test| view! {
                                        <div class="last-test-info">
                                            <span class="test-label">Last Test:</span>
                                            <span class={format!("test-result status-{}", if test.status == "passed" { "ok" } else { "fail" })}>
                                                {test.test_type.clone()} - {test.status.clone()}
                                            </span>
                                        </div>
                                    })}

                                    <div class="disk-actions">
                                        <button
                                            class="btn btn-secondary"
                                            on:click=move |_| {
                                                set_selected_disk.set(Some(disk_clone.clone()));
                                                set_show_details_modal.set(true);
                                            }
                                        >
                                            View Details
                                        </button>
                                        <button
                                            class="btn btn-primary"
                                            on:click=move |_| {
                                                set_selected_disk.set(Some(disk_clone_2.clone()));
                                                set_show_test_modal.set(true);
                                            }
                                        >
                                            Run Test
                                        </button>
                                    </div>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_view()
            }}

            // Disk Details Modal
            {move || if show_details_modal.get() {
                if let Some(disk) = selected_disk.get() {
                    view! {
                        <div class="modal-overlay">
                            <div class="modal modal-xl">
                                <div class="modal-header">
                                    <h2>SMART Details: {disk.name.clone()}</h2>
                                    <button
                                        class="modal-close"
                                        on:click=move |_| set_show_details_modal.set(false)
                                    >
                                        x
                                    </button>
                                </div>
                                <div class="modal-body">
                                    <div class="details-grid">
                                        <div class="detail-section">
                                            <h3>Device Information</h3>
                                            <table class="info-table">
                                                <tbody>
                                                    <tr><td>Model</td><td>{disk.model.clone()}</td></tr>
                                                    <tr><td>Serial</td><td>{disk.serial.clone()}</td></tr>
                                                    <tr><td>Firmware</td><td>{disk.firmware.clone()}</td></tr>
                                                    <tr><td>Type</td><td>{disk.disk_type.to_uppercase()}</td></tr>
                                                    <tr><td>Interface</td><td>{disk.interface.clone()}</td></tr>
                                                    <tr><td>Capacity</td><td>{format_bytes(disk.capacity_bytes)}</td></tr>
                                                    <tr><td>SMART Enabled</td><td>{if disk.smart_enabled { "Yes" } else { "No" }}</td></tr>
                                                </tbody>
                                            </table>
                                        </div>

                                        <div class="detail-section">
                                            <h3>Health Status</h3>
                                            <div class="health-status-large">
                                                <div class={format!("status-indicator status-{}", disk.smart_status.as_str())}>
                                                    {match disk.smart_status {
                                                        SmartStatus::Healthy => "HEALTHY",
                                                        SmartStatus::Warning => "WARNING",
                                                        SmartStatus::Critical => "CRITICAL",
                                                        SmartStatus::Unknown => "UNKNOWN",
                                                    }}
                                                </div>
                                                <div class="health-score-large">
                                                    <span class="score">{disk.health_score}%</span>
                                                    <span class="label">Health Score</span>
                                                </div>
                                            </div>
                                        </div>
                                    </div>

                                    <div class="attributes-section">
                                        <h3>All SMART Attributes</h3>
                                        <table class="attributes-table">
                                            <thead>
                                                <tr>
                                                    <th>ID</th>
                                                    <th>Attribute</th>
                                                    <th>Value</th>
                                                    <th>Worst</th>
                                                    <th>Threshold</th>
                                                    <th>Raw</th>
                                                    <th>Status</th>
                                                </tr>
                                            </thead>
                                            <tbody>
                                                {disk.attributes.iter().map(|attr| {
                                                    view! {
                                                        <tr class={format!("attr-row status-{}", attr.status)}>
                                                            <td>{attr.id}</td>
                                                            <td>
                                                                <span class="attr-name">{attr.name.clone()}</span>
                                                                <span class="attr-desc">{attr.description.clone()}</span>
                                                            </td>
                                                            <td>{attr.value}</td>
                                                            <td>{attr.worst}</td>
                                                            <td>{attr.threshold}</td>
                                                            <td>{attr.raw_value.clone()}</td>
                                                            <td>
                                                                <span class={format!("status-badge status-{}", attr.status)}>
                                                                    {attr.status.to_uppercase()}
                                                                </span>
                                                            </td>
                                                        </tr>
                                                    }
                                                }).collect::<Vec<_>>()}
                                            </tbody>
                                        </table>
                                    </div>
                                </div>
                                <div class="modal-footer">
                                    <button
                                        class="btn btn-secondary"
                                        on:click=move |_| set_show_details_modal.set(false)
                                    >
                                        Close
                                    </button>
                                </div>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! {}.into_view()
                }
            } else {
                view! {}.into_view()
            }}

            // Run Test Modal
            {move || if show_test_modal.get() {
                if let Some(disk) = selected_disk.get() {
                    view! {
                        <div class="modal-overlay">
                            <div class="modal">
                                <div class="modal-header">
                                    <h2>Run SMART Test</h2>
                                    <button
                                        class="modal-close"
                                        on:click=move |_| set_show_test_modal.set(false)
                                    >
                                        x
                                    </button>
                                </div>
                                <div class="modal-body">
                                    <p><strong>Disk:</strong> {disk.name} ({disk.path})</p>

                                    <div class="form-group">
                                        <label>Test Type</label>
                                        <div class="test-type-options">
                                            <label class="radio-option">
                                                <input
                                                    type="radio"
                                                    name="test-type"
                                                    value="short"
                                                    checked=move || test_type.get() == "short"
                                                    on:change=move |_| set_test_type.set("short".to_string())
                                                />
                                                <div class="option-content">
                                                    <span class="option-title">Short Test</span>
                                                    <span class="option-desc">Quick test (~2 minutes)</span>
                                                </div>
                                            </label>
                                            <label class="radio-option">
                                                <input
                                                    type="radio"
                                                    name="test-type"
                                                    value="long"
                                                    checked=move || test_type.get() == "long"
                                                    on:change=move |_| set_test_type.set("long".to_string())
                                                />
                                                <div class="option-content">
                                                    <span class="option-title">Long Test</span>
                                                    <span class="option-desc">Comprehensive test (1-2 hours)</span>
                                                </div>
                                            </label>
                                            <label class="radio-option">
                                                <input
                                                    type="radio"
                                                    name="test-type"
                                                    value="conveyance"
                                                    checked=move || test_type.get() == "conveyance"
                                                    on:change=move |_| set_test_type.set("conveyance".to_string())
                                                />
                                                <div class="option-content">
                                                    <span class="option-title">Conveyance Test</span>
                                                    <span class="option-desc">Test for transport damage (~5 minutes)</span>
                                                </div>
                                            </label>
                                        </div>
                                    </div>

                                    <div class="test-warning">
                                        <p>The test will run in the background. You can continue using the system while the test runs.</p>
                                    </div>
                                </div>
                                <div class="modal-footer">
                                    <button
                                        class="btn btn-secondary"
                                        on:click=move |_| set_show_test_modal.set(false)
                                    >
                                        Cancel
                                    </button>
                                    <button
                                        class="btn btn-primary"
                                        disabled=running_test
                                        on:click=start_test
                                    >
                                        {move || if running_test.get() { "Starting..." } else { "Start Test" }}
                                    </button>
                                </div>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! {}.into_view()
                }
            } else {
                view! {}.into_view()
            }}
        </div>
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_hours(hours: u64) -> String {
    let days = hours / 24;
    let years = days / 365;

    if years > 0 {
        format!("{:.1} years", years as f64 + (days % 365) as f64 / 365.0)
    } else if days > 0 {
        format!("{} days", days)
    } else {
        format!("{} hours", hours)
    }
}

fn get_health_color(score: u32) -> &'static str {
    if score >= 80 {
        "#22c55e" // green
    } else if score >= 60 {
        "#eab308" // yellow
    } else if score >= 40 {
        "#f97316" // orange
    } else {
        "#ef4444" // red
    }
}
