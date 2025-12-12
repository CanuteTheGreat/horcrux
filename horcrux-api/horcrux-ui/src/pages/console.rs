//! VM Console page with noVNC integration
//!
//! Provides browser-based VNC console access to virtual machines

use leptos::*;
use crate::api;

/// Console access types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConsoleType {
    VNC,
    SPICE,
    Serial,
}

impl ConsoleType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConsoleType::VNC => "vnc",
            ConsoleType::SPICE => "spice",
            ConsoleType::Serial => "serial",
        }
    }
}

/// VM Console component
#[component]
pub fn VmConsole() -> impl IntoView {
    let params = leptos_router::use_params_map();
    let vm_id = move || params.get().get("id").cloned().unwrap_or_default();

    let (console_type, set_console_type) = create_signal(ConsoleType::VNC);
    let (is_connected, set_is_connected) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (vm_name, set_vm_name) = create_signal(String::new());
    let (console_url, set_console_url) = create_signal(None::<String>);

    // Load VM info and console URL
    create_effect(move |_| {
        let id = vm_id();
        if id.is_empty() {
            return;
        }

        spawn_local(async move {
            // Get VM info
            match api::get_vm(&id).await {
                Ok(vm) => {
                    set_vm_name.set(vm.name.clone());
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to load VM: {}", e)));
                }
            }

            // Get console URL
            match api::get_console_url(&id, console_type.get().as_str()).await {
                Ok(url) => {
                    set_console_url.set(Some(url));
                    set_is_connected.set(true);
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to get console URL: {}", e)));
                }
            }
        });
    });

    // Handle console type change
    let on_type_change = move |new_type: ConsoleType| {
        set_console_type.set(new_type);
        set_is_connected.set(false);
        set_console_url.set(None);
    };

    // Full screen handler
    let toggle_fullscreen = move |_| {
        let document = web_sys::window().unwrap().document().unwrap();
        if let Some(elem) = document.get_element_by_id("console-frame") {
            let _ = elem.request_fullscreen();
        }
    };

    // Send Ctrl+Alt+Del
    let send_cad = move |_| {
        if let Some(url) = console_url.get() {
            // Send key sequence via API
            let vm = vm_id();
            spawn_local(async move {
                let _ = api::send_console_keys(&vm, &["ctrl", "alt", "delete"]).await;
            });
        }
    };

    view! {
        <div class="console-page">
            <div class="console-header">
                <h1>"VM Console: " {move || vm_name.get()}</h1>

                <div class="console-controls">
                    <div class="console-type-selector">
                        <label>"Console Type:"</label>
                        <select on:change=move |ev| {
                            let value = event_target_value(&ev);
                            match value.as_str() {
                                "vnc" => on_type_change(ConsoleType::VNC),
                                "spice" => on_type_change(ConsoleType::SPICE),
                                "serial" => on_type_change(ConsoleType::Serial),
                                _ => {}
                            }
                        }>
                            <option value="vnc" selected=move || console_type.get() == ConsoleType::VNC>
                                "VNC"
                            </option>
                            <option value="spice" selected=move || console_type.get() == ConsoleType::SPICE>
                                "SPICE"
                            </option>
                            <option value="serial" selected=move || console_type.get() == ConsoleType::Serial>
                                "Serial"
                            </option>
                        </select>
                    </div>

                    <button class="btn btn-secondary" on:click=send_cad>
                        "Ctrl+Alt+Del"
                    </button>

                    <button class="btn btn-secondary" on:click=toggle_fullscreen>
                        "Fullscreen"
                    </button>

                    <span class=move || {
                        if is_connected.get() {
                            "connection-status connected"
                        } else {
                            "connection-status disconnected"
                        }
                    }>
                        {move || if is_connected.get() { "Connected" } else { "Disconnected" }}
                    </span>
                </div>
            </div>

            {move || error_message.get().map(|msg| view! {
                <div class="error-banner">
                    <span class="error-icon">"!"</span>
                    {msg}
                </div>
            })}

            <div class="console-container">
                {move || {
                    match console_type.get() {
                        ConsoleType::VNC => view! {
                            <NoVncConsole url=console_url.get() />
                        }.into_view(),
                        ConsoleType::SPICE => view! {
                            <SpiceConsole url=console_url.get() />
                        }.into_view(),
                        ConsoleType::Serial => view! {
                            <SerialConsole vm_id=vm_id() />
                        }.into_view(),
                    }
                }}
            </div>
        </div>
    }
}

/// noVNC Console component
#[component]
fn NoVncConsole(url: Option<String>) -> impl IntoView {
    view! {
        <div class="novnc-container">
            {match url {
                Some(url) => view! {
                    <iframe
                        id="console-frame"
                        class="console-iframe"
                        src=format!("/novnc/vnc.html?host=localhost&port=8006&path=api/vms/{}/vnc&autoconnect=true&resize=scale",
                            url.split('/').last().unwrap_or(""))
                        allowfullscreen=true
                    />
                }.into_view(),
                None => view! {
                    <div class="console-loading">
                        <div class="spinner"></div>
                        <p>"Connecting to VNC console..."</p>
                    </div>
                }.into_view(),
            }}
        </div>
    }
}

/// SPICE Console component (placeholder)
#[component]
fn SpiceConsole(url: Option<String>) -> impl IntoView {
    view! {
        <div class="spice-container">
            {match url {
                Some(_) => view! {
                    <div class="spice-info">
                        <h3>"SPICE Console"</h3>
                        <p>"SPICE console requires the virt-viewer application."</p>
                        <a href=format!("/api/vms/{}/spice", url.as_deref().unwrap_or(""))
                           class="btn btn-primary"
                           download="console.vv">
                            "Download .vv file"
                        </a>
                        <p class="hint">"Open the downloaded file with virt-viewer"</p>
                    </div>
                }.into_view(),
                None => view! {
                    <div class="console-loading">
                        <div class="spinner"></div>
                        <p>"Loading SPICE console..."</p>
                    </div>
                }.into_view(),
            }}
        </div>
    }
}

/// Serial Console component
#[component]
fn SerialConsole(vm_id: String) -> impl IntoView {
    let (output, set_output) = create_signal(String::new());
    let (input, set_input) = create_signal(String::new());

    // Connect to serial console WebSocket
    create_effect(move |_| {
        let vm = vm_id.clone();
        spawn_local(async move {
            // In production, this would connect to a WebSocket for serial I/O
            set_output.set("Serial console connected.\r\nPress Enter to activate console.\r\n".to_string());
        });
    });

    let on_input = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" {
            let cmd = input.get();
            set_output.update(|o| {
                o.push_str(&format!("\r\n$ {}\r\n", cmd));
            });
            set_input.set(String::new());

            // Send command to serial console
            let vm = vm_id.clone();
            let command = cmd.clone();
            spawn_local(async move {
                match api::send_serial_input(&vm, &command).await {
                    Ok(response) => {
                        set_output.update(|o| {
                            o.push_str(&response);
                        });
                    }
                    Err(e) => {
                        set_output.update(|o| {
                            o.push_str(&format!("Error: {}\r\n", e));
                        });
                    }
                }
            });
        }
    };

    view! {
        <div class="serial-container">
            <pre class="serial-output">{move || output.get()}</pre>
            <div class="serial-input-container">
                <span class="prompt">"$ "</span>
                <input
                    type="text"
                    class="serial-input"
                    prop:value=move || input.get()
                    on:input=move |ev| set_input.set(event_target_value(&ev))
                    on:keydown=on_input
                    placeholder="Enter command..."
                />
            </div>
        </div>
    }
}

/// Console page for multiple VMs
#[component]
pub fn ConsolePage() -> impl IntoView {
    let (vms, set_vms) = create_signal(Vec::<api::Vm>::new());
    let (selected_vm, set_selected_vm) = create_signal(None::<String>);

    // Load VMs
    create_effect(move |_| {
        spawn_local(async move {
            if let Ok(vm_list) = api::get_vms().await {
                set_vms.set(vm_list);
            }
        });
    });

    view! {
        <div class="console-list-page">
            <h1>"VM Console Access"</h1>

            <div class="vm-console-grid">
                {move || {
                    let vm_list = vms.get();
                    if vm_list.is_empty() {
                        view! {
                            <p class="no-vms">"No virtual machines found"</p>
                        }.into_view()
                    } else {
                        view! {
                            <div class="vm-cards">
                                {vm_list.into_iter().map(|vm| {
                                    let vm_id = vm.id.clone();
                                    let status_class = match vm.status.as_str() {
                                        "running" => "status-running",
                                        "stopped" => "status-stopped",
                                        _ => "status-unknown",
                                    };
                                    view! {
                                        <div class="vm-card">
                                            <div class="vm-card-header">
                                                <h3>{&vm.name}</h3>
                                                <span class=format!("vm-status {}", status_class)>
                                                    {&vm.status}
                                                </span>
                                            </div>
                                            <div class="vm-card-body">
                                                <p>"ID: " {&vm.id}</p>
                                                <p>"CPUs: " {vm.cpus} " | Memory: " {vm.memory / 1024} " GB"</p>
                                            </div>
                                            <div class="vm-card-actions">
                                                {if vm.status == "running" {
                                                    view! {
                                                        <a href=format!("/console/{}", vm_id) class="btn btn-primary">
                                                            "Open Console"
                                                        </a>
                                                    }.into_view()
                                                } else {
                                                    view! {
                                                        <button class="btn btn-disabled" disabled=true>
                                                            "VM not running"
                                                        </button>
                                                    }.into_view()
                                                }}
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_view()
                    }
                }}
            </div>
        </div>
    }
}
