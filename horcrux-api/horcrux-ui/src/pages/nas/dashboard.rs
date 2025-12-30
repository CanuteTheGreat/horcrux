//! NAS Dashboard Page
//!
//! Overview of NAS services, shares, and storage.

use leptos::*;

#[component]
pub fn NasDashboard() -> impl IntoView {
    let (health, set_health) = create_signal(None::<serde_json::Value>);
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // Load NAS health status
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match reqwasm::http::Request::get("/api/nas/health")
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<serde_json::Value>().await {
                            set_health.set(Some(data));
                            set_error.set(None);
                        }
                    } else {
                        set_error.set(Some("Failed to load NAS health".to_string()));
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Network error: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    view! {
        <div class="nas-dashboard">
            <div class="page-header">
                <h1>"NAS Dashboard"</h1>
                <p class="subtitle">"Network Attached Storage Overview"</p>
            </div>

            {move || {
                if loading.get() {
                    view! { <div class="loading">"Loading NAS status..."</div> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <div class="error">"Error: " {err}</div> }.into_view()
                } else {
                    view! {
                        <div class="dashboard-grid">
                            // Services Status Card
                            <div class="dashboard-card">
                                <h3>"Services"</h3>
                                <div class="service-grid">
                                    <ServiceStatus name="SMB/Samba" icon="folder-shared" />
                                    <ServiceStatus name="NFS" icon="folder" />
                                    <ServiceStatus name="AFP" icon="apple" />
                                    <ServiceStatus name="iSCSI" icon="storage" />
                                    <ServiceStatus name="S3 Gateway" icon="cloud" />
                                    <ServiceStatus name="Rsync" icon="sync" />
                                </div>
                            </div>

                            // Storage Overview Card
                            <div class="dashboard-card">
                                <h3>"Storage Pools"</h3>
                                <div class="storage-overview">
                                    <div class="stat-item">
                                        <span class="stat-value">"0"</span>
                                        <span class="stat-label">"Pools"</span>
                                    </div>
                                    <div class="stat-item">
                                        <span class="stat-value">"0"</span>
                                        <span class="stat-label">"Datasets"</span>
                                    </div>
                                    <div class="stat-item">
                                        <span class="stat-value">"0"</span>
                                        <span class="stat-label">"Snapshots"</span>
                                    </div>
                                </div>
                            </div>

                            // Shares Overview Card
                            <div class="dashboard-card">
                                <h3>"Shares"</h3>
                                <div class="shares-overview">
                                    <div class="stat-item">
                                        <span class="stat-value">"0"</span>
                                        <span class="stat-label">"Total Shares"</span>
                                    </div>
                                    <div class="stat-item">
                                        <span class="stat-value">"0"</span>
                                        <span class="stat-label">"Active"</span>
                                    </div>
                                </div>
                                <a href="/nas/shares" class="btn btn-link">"Manage Shares"</a>
                            </div>

                            // Users & Groups Card
                            <div class="dashboard-card">
                                <h3>"Users & Groups"</h3>
                                <div class="users-overview">
                                    <div class="stat-item">
                                        <span class="stat-value">"0"</span>
                                        <span class="stat-label">"Users"</span>
                                    </div>
                                    <div class="stat-item">
                                        <span class="stat-value">"0"</span>
                                        <span class="stat-label">"Groups"</span>
                                    </div>
                                </div>
                                <div class="card-actions">
                                    <a href="/nas/users" class="btn btn-link">"Users"</a>
                                    <a href="/nas/groups" class="btn btn-link">"Groups"</a>
                                </div>
                            </div>

                            // Quick Actions Card
                            <div class="dashboard-card">
                                <h3>"Quick Actions"</h3>
                                <div class="quick-actions">
                                    <a href="/nas/shares/create" class="action-btn">"Create Share"</a>
                                    <a href="/nas/users/create" class="action-btn">"Add User"</a>
                                    <a href="/nas/pools" class="action-btn">"Manage Pools"</a>
                                    <a href="/nas/iscsi" class="action-btn">"iSCSI Targets"</a>
                                </div>
                            </div>

                            // Recent Activity Card
                            <div class="dashboard-card wide">
                                <h3>"Recent Activity"</h3>
                                <div class="activity-list">
                                    <p class="no-activity">"No recent activity"</p>
                                </div>
                            </div>
                        </div>
                    }.into_view()
                }
            }}
        </div>
    }
}

#[component]
fn ServiceStatus(name: &'static str, icon: &'static str) -> impl IntoView {
    view! {
        <div class="service-status">
            <span class="service-icon">{icon}</span>
            <span class="service-name">{name}</span>
            <span class="status-indicator unknown">"Unknown"</span>
        </div>
    }
}
