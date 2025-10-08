//! VM pages for mobile UI

use yew::prelude::*;
use yew_router::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::api::{ApiClient, VmInfo};
use crate::components::{Header, Card, StatusBadge, Loading};
use crate::router::Route;

/// VM list page
#[function_component(VMList)]
pub fn vm_list() -> Html {
    let vms = use_state(|| Vec::<VmInfo>::new());
    let loading = use_state(|| true);

    // Fetch VMs on mount
    {
        let vms = vms.clone();
        let loading = loading.clone();

        use_effect_with((), move |_| {
            spawn_local(async move {
                match ApiClient::list_vms().await {
                    Ok(vm_list) => vms.set(vm_list),
                    Err(_) => {}
                }
                loading.set(false);
            });

            || ()
        });
    }

    if *loading {
        return html! { <Loading /> };
    }

    html! {
        <div class="vms-page">
            <Header title="Virtual Machines" />

            <div class="page-content">
                {if vms.is_empty() {
                    html! {
                        <div class="empty-state">
                            <p>{"No virtual machines found"}</p>
                        </div>
                    }
                } else {
                    vms.iter().map(|vm| html! {
                        <VMCard vm={vm.clone()} />
                    }).collect::<Html>()
                }}
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct VMCardProps {
    vm: VmInfo,
}

#[function_component(VMCard)]
fn vm_card(props: &VMCardProps) -> Html {
    let navigator = use_navigator().unwrap();
    let vm = &props.vm;

    let onclick = {
        let navigator = navigator.clone();
        let vm_id = vm.id.clone();
        Callback::from(move |_| {
            navigator.push(&Route::VMDetail { id: vm_id.clone() });
        })
    };

    html! {
        <Card onclick={Some(onclick)}>
            <div class="vm-card">
                <div class="vm-header">
                    <h3 class="vm-name">{&vm.name}</h3>
                    <StatusBadge status={vm.status.clone()} />
                </div>

                <div class="vm-details">
                    <div class="vm-detail-item">
                        <span class="label">{"ID:"}</span>
                        <span class="value">{&vm.id}</span>
                    </div>
                    <div class="vm-detail-item">
                        <span class="label">{"CPU:"}</span>
                        <span class="value">{format!("{} cores", vm.cpu_cores)}</span>
                    </div>
                    <div class="vm-detail-item">
                        <span class="label">{"Memory:"}</span>
                        <span class="value">{format!("{} MB", vm.memory_mb)}</span>
                    </div>
                    <div class="vm-detail-item">
                        <span class="label">{"Arch:"}</span>
                        <span class="value">{&vm.architecture}</span>
                    </div>
                    {if let Some(ref node) = vm.node {
                        html! {
                            <div class="vm-detail-item">
                                <span class="label">{"Node:"}</span>
                                <span class="value">{node}</span>
                            </div>
                        }
                    } else {
                        html! {}
                    }}
                </div>
            </div>
        </Card>
    }
}

/// VM detail page
#[derive(Properties, PartialEq)]
pub struct VMDetailProps {
    pub vm_id: String,
}

#[function_component(VMDetail)]
pub fn vm_detail(props: &VMDetailProps) -> Html {
    let vm = use_state(|| None::<VmInfo>);
    let loading = use_state(|| true);
    let action_loading = use_state(|| false);

    // Fetch VM details
    {
        let vm = vm.clone();
        let loading = loading.clone();
        let vm_id = props.vm_id.clone();

        use_effect_with((), move |_| {
            spawn_local(async move {
                match ApiClient::get_vm(&vm_id).await {
                    Ok(vm_info) => vm.set(Some(vm_info)),
                    Err(_) => {}
                }
                loading.set(false);
            });

            || ()
        });
    }

    let start_vm = {
        let vm_id = props.vm_id.clone();
        let vm = vm.clone();
        let action_loading = action_loading.clone();

        Callback::from(move |_| {
            let vm_id = vm_id.clone();
            let vm = vm.clone();
            let action_loading = action_loading.clone();

            action_loading.set(true);

            spawn_local(async move {
                let _ = ApiClient::start_vm(&vm_id).await;

                // Refresh VM info
                if let Ok(vm_info) = ApiClient::get_vm(&vm_id).await {
                    vm.set(Some(vm_info));
                }

                action_loading.set(false);
            });
        })
    };

    let stop_vm = {
        let vm_id = props.vm_id.clone();
        let vm = vm.clone();
        let action_loading = action_loading.clone();

        Callback::from(move |_| {
            let vm_id = vm_id.clone();
            let vm = vm.clone();
            let action_loading = action_loading.clone();

            action_loading.set(true);

            spawn_local(async move {
                let _ = ApiClient::stop_vm(&vm_id).await;

                // Refresh VM info
                if let Ok(vm_info) = ApiClient::get_vm(&vm_id).await {
                    vm.set(Some(vm_info));
                }

                action_loading.set(false);
            });
        })
    };

    if *loading {
        return html! { <Loading /> };
    }

    html! {
        <div class="vm-detail-page">
            <Header title="VM Details" show_back={true} />

            <div class="page-content">
                {if let Some(ref vm_info) = *vm {
                    html! {
                        <>
                            <Card title="Information">
                                <div class="detail-grid">
                                    <div class="detail-row">
                                        <span class="label">{"Name:"}</span>
                                        <span class="value">{&vm_info.name}</span>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">{"ID:"}</span>
                                        <span class="value">{&vm_info.id}</span>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">{"Status:"}</span>
                                        <StatusBadge status={vm_info.status.clone()} />
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">{"CPU Cores:"}</span>
                                        <span class="value">{vm_info.cpu_cores}</span>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">{"Memory:"}</span>
                                        <span class="value">{format!("{} MB", vm_info.memory_mb)}</span>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">{"Architecture:"}</span>
                                        <span class="value">{&vm_info.architecture}</span>
                                    </div>
                                    {if let Some(ref node) = vm_info.node {
                                        html! {
                                            <div class="detail-row">
                                                <span class="label">{"Node:"}</span>
                                                <span class="value">{node}</span>
                                            </div>
                                        }
                                    } else {
                                        html! {}
                                    }}
                                </div>
                            </Card>

                            <Card title="Actions">
                                <div class="action-buttons">
                                    <button
                                        class="mobile-button success"
                                        onclick={start_vm}
                                        disabled={*action_loading || vm_info.status == "running"}
                                    >
                                        {"▶️ Start"}
                                    </button>
                                    <button
                                        class="mobile-button danger"
                                        onclick={stop_vm}
                                        disabled={*action_loading || vm_info.status == "stopped"}
                                    >
                                        {"⏹️ Stop"}
                                    </button>
                                </div>
                            </Card>
                        </>
                    }
                } else {
                    html! {
                        <div class="empty-state">
                            <p>{"VM not found"}</p>
                        </div>
                    }
                }}
            </div>
        </div>
    }
}
