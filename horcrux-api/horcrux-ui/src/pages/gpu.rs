//! GPU Passthrough Management Page

use leptos::*;
use serde::{Deserialize, Serialize};
use crate::api::{fetch_json, post_empty};

/// GPU device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuDevice {
    pub pci_address: String,
    pub vendor_id: String,
    pub device_id: String,
    pub vendor_name: String,
    pub device_name: String,
    pub driver: Option<String>,
    pub iommu_group: Option<String>,
    pub in_use: bool,
}

/// IOMMU status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IommuStatus {
    pub enabled: bool,
    pub message: String,
}

/// GPU Management Page Component
#[component]
pub fn GpuManagement() -> impl IntoView {
    let (devices, set_devices) = create_signal(Vec::<GpuDevice>::new());
    let (iommu_status, set_iommu_status) = create_signal(None::<IommuStatus>);
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (selected_device, set_selected_device) = create_signal(None::<String>);

    // Fetch GPU devices on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            set_error.set(None);

            // Fetch IOMMU status
            match fetch_json::<IommuStatus>("/api/gpu/iommu-status").await {
                Ok(status) => set_iommu_status.set(Some(status)),
                Err(e) => leptos::logging::error!("Failed to fetch IOMMU status: {}", e.message),
            }

            // Fetch GPU devices
            match fetch_json::<Vec<GpuDevice>>("/api/gpu/devices").await {
                Ok(gpus) => {
                    set_devices.set(gpus);
                    set_loading.set(false);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load GPU devices: {}", e.message)));
                    set_loading.set(false);
                }
            }
        });
    });

    // Scan for GPUs
    let scan_gpus = move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match fetch_json::<Vec<GpuDevice>>("/api/gpu/devices/scan").await {
                Ok(gpus) => {
                    set_devices.set(gpus);
                    set_loading.set(false);
                }
                Err(e) => {
                    set_error.set(Some(format!("Scan failed: {}", e.message)));
                    set_loading.set(false);
                }
            }
        });
    };

    // Bind GPU to VFIO
    let bind_vfio = move |pci_address: String| {
        spawn_local(async move {
            let url = format!("/api/gpu/devices/{}/bind-vfio", pci_address);
            match post_empty(&url).await {
                Ok(()) => {
                    leptos::logging::log!("GPU {} bound to vfio-pci", pci_address);
                    // Refresh device list
                    if let Ok(gpus) = fetch_json::<Vec<GpuDevice>>("/api/gpu/devices").await {
                        set_devices.set(gpus);
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Bind failed: {}", e.message)));
                }
            }
        });
    };

    // Unbind GPU from VFIO
    let unbind_vfio = move |pci_address: String| {
        spawn_local(async move {
            let url = format!("/api/gpu/devices/{}/unbind-vfio", pci_address);
            match post_empty(&url).await {
                Ok(()) => {
                    leptos::logging::log!("GPU {} unbound from vfio-pci", pci_address);
                    // Refresh device list
                    if let Ok(gpus) = fetch_json::<Vec<GpuDevice>>("/api/gpu/devices").await {
                        set_devices.set(gpus);
                    }
                }
                Err(e) => {
                    set_error.set(Some(format!("Unbind failed: {}", e.message)));
                }
            }
        });
    };

    view! {
        <div class="page gpu-management">
            <header class="page-header">
                <h2>"GPU Passthrough Management"</h2>
                <button class="btn btn-primary" on:click=scan_gpus>
                    "Scan for GPUs"
                </button>
            </header>

            // IOMMU Status Card
            <div class="status-card">
                {move || match iommu_status.get() {
                    Some(status) => view! {
                        <div class={format!("iommu-status {}", if status.enabled { "enabled" } else { "disabled" })}>
                            <span class="icon">{if status.enabled { "✓" } else { "✗" }}</span>
                            <span class="message">{status.message}</span>
                        </div>
                    }.into_view(),
                    None => view! { <span>"Checking IOMMU..."</span> }.into_view(),
                }}
            </div>

            // Error display
            {move || error.get().map(|e| view! {
                <div class="alert alert-error">
                    <span>{e}</span>
                    <button class="btn-close" on:click=move |_| set_error.set(None)>"×"</button>
                </div>
            })}

            // Loading state
            {move || loading.get().then(|| view! {
                <div class="loading">
                    <div class="spinner"></div>
                    <span>"Loading GPU devices..."</span>
                </div>
            })}

            // GPU Device List
            {move || if !loading.get() {
                view! {
                    <div class="gpu-grid">
                        {move || devices.get().into_iter().map(|gpu| {
                            let pci_addr = gpu.pci_address.clone();
                            let pci_addr2 = gpu.pci_address.clone();
                            let pci_addr3 = gpu.pci_address.clone();
                            let is_vfio = gpu.driver.as_ref().map(|d| d == "vfio-pci").unwrap_or(false);

                            view! {
                                <div class={format!("gpu-card {}", if gpu.in_use { "in-use" } else { "" })}
                                     on:click=move |_| set_selected_device.set(Some(pci_addr3.clone()))>
                                    <div class="gpu-header">
                                        <span class="vendor-badge">{&gpu.vendor_name}</span>
                                        {gpu.in_use.then(|| view! {
                                            <span class="status-badge in-use">"In Use"</span>
                                        })}
                                    </div>
                                    <h3 class="device-name">{&gpu.device_name}</h3>
                                    <div class="gpu-details">
                                        <div class="detail-row">
                                            <span class="label">"PCI Address:"</span>
                                            <code>{&gpu.pci_address}</code>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Vendor/Device ID:"</span>
                                            <code>{format!("{}:{}", &gpu.vendor_id, &gpu.device_id)}</code>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"Driver:"</span>
                                            <span class={format!("driver {}", if is_vfio { "vfio" } else { "" })}>
                                                {gpu.driver.as_ref().unwrap_or(&"none".to_string()).clone()}
                                            </span>
                                        </div>
                                        <div class="detail-row">
                                            <span class="label">"IOMMU Group:"</span>
                                            <span>{gpu.iommu_group.as_ref().unwrap_or(&"N/A".to_string()).clone()}</span>
                                        </div>
                                    </div>
                                    <div class="gpu-actions">
                                        {if !gpu.in_use {
                                            if is_vfio {
                                                view! {
                                                    <button class="btn btn-warning"
                                                            on:click=move |e| {
                                                                e.stop_propagation();
                                                                unbind_vfio(pci_addr.clone());
                                                            }>
                                                        "Unbind from VFIO"
                                                    </button>
                                                }.into_view()
                                            } else {
                                                view! {
                                                    <button class="btn btn-success"
                                                            on:click=move |e| {
                                                                e.stop_propagation();
                                                                bind_vfio(pci_addr2.clone());
                                                            }>
                                                        "Bind to VFIO"
                                                    </button>
                                                }.into_view()
                                            }
                                        } else {
                                            view! { <span class="text-muted">"Assigned to VM"</span> }.into_view()
                                        }}
                                    </div>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Empty state
            {move || if !loading.get() && devices.get().is_empty() {
                view! {
                    <div class="empty-state">
                        <div class="icon">"🖥️"</div>
                        <h3>"No GPU Devices Found"</h3>
                        <p>"Click 'Scan for GPUs' to detect available graphics cards."</p>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Instructions
            <div class="info-box">
                <h4>"GPU Passthrough Setup"</h4>
                <ol>
                    <li>"Enable IOMMU in BIOS (Intel VT-d or AMD-Vi)"</li>
                    <li>"Add kernel parameter: "<code>"intel_iommu=on"</code>" or "<code>"amd_iommu=on"</code></li>
                    <li>"Click 'Bind to VFIO' on the GPU you want to passthrough"</li>
                    <li>"Assign the GPU when creating a new VM"</li>
                </ol>
            </div>
        </div>
    }
}
