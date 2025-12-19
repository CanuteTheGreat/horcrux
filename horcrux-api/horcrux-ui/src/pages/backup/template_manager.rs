use leptos::*;
use crate::api::{
    VmTemplate, CreateTemplateRequest,
    get_templates, create_template, delete_template, clone_template,
    get_vms
};
use horcrux_common::VmConfig;

#[component]
pub fn TemplateManagerPage() -> impl IntoView {
    let (templates, set_templates) = create_signal(Vec::<VmTemplate>::new());
    let (vms, set_vms) = create_signal(Vec::<VmConfig>::new());
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);
    let (search_query, set_search_query) = create_signal(String::new());
    let (show_create_modal, set_show_create_modal) = create_signal(false);
    let (show_clone_modal, set_show_clone_modal) = create_signal(false);
    let (selected_template, set_selected_template) = create_signal(None::<VmTemplate>);

    // Form state
    let (template_name, set_template_name) = create_signal(String::new());
    let (template_description, set_template_description) = create_signal(String::new());
    let (source_vm_id, set_source_vm_id) = create_signal(String::new());
    let (clone_name, set_clone_name) = create_signal(String::new());

    let load_templates = move || {
        set_loading.set(true);
        set_error.set(None);

        spawn_local(async move {
            match get_templates().await {
                Ok(template_list) => set_templates.set(template_list),
                Err(e) => set_error.set(Some(format!("Failed to load templates: {}", e))),
            }
            set_loading.set(false);
        });
    };

    let load_vms = move || {
        spawn_local(async move {
            match get_vms().await {
                Ok(vm_list) => set_vms.set(vm_list),
                Err(e) => set_error.set(Some(format!("Failed to load VMs: {}", e))),
            }
        });
    };

    // Load data on mount
    create_effect(move |_| {
        load_templates();
        load_vms();
    });

    // Auto-refresh every 60 seconds
    use leptos::set_interval;
    set_interval(
        move || load_templates(),
        std::time::Duration::from_secs(60),
    );

    let filtered_templates = move || {
        let query = search_query.get().to_lowercase();
        if query.is_empty() {
            templates.get()
        } else {
            templates
                .get()
                .into_iter()
                .filter(|template| {
                    template.name.to_lowercase().contains(&query) ||
                    template.description.as_ref().map_or(false, |d| d.to_lowercase().contains(&query)) ||
                    template.source_vm_id.to_lowercase().contains(&query)
                })
                .collect()
        }
    };

    let reset_form = move || {
        set_template_name.set(String::new());
        set_template_description.set(String::new());
        set_source_vm_id.set(String::new());
        set_clone_name.set(String::new());
    };

    let create_template = move || {
        let request = CreateTemplateRequest {
            name: template_name.get(),
            description: if template_description.get().is_empty() {
                None
            } else {
                Some(template_description.get())
            },
            source_vm_id: source_vm_id.get(),
        };

        spawn_local(async move {
            match create_template(request).await {
                Ok(_) => {
                    set_show_create_modal.set(false);
                    reset_form();
                    load_templates();
                    set_success_message.set(Some("Template created successfully".to_string()));
                    // Clear success message after 3 seconds
                    set_timeout(
                        move || set_success_message.set(None),
                        std::time::Duration::from_secs(3),
                    );
                }
                Err(e) => set_error.set(Some(format!("Failed to create template: {}", e))),
            }
        });
    };

    let delete_template_action = move |template_id: String, template_name: String| {
        if web_sys::window()
            .unwrap()
            .confirm_with_message(&format!("Are you sure you want to delete template '{}'? This action cannot be undone.", template_name))
            .unwrap()
        {
            spawn_local(async move {
                match delete_template(&template_id).await {
                    Ok(_) => {
                        load_templates();
                        set_success_message.set(Some("Template deleted successfully".to_string()));
                        set_timeout(
                            move || set_success_message.set(None),
                            std::time::Duration::from_secs(3),
                        );
                    }
                    Err(e) => set_error.set(Some(format!("Failed to delete template: {}", e))),
                }
            });
        }
    };

    let start_clone = move |template: VmTemplate| {
        set_selected_template.set(Some(template.clone()));
        set_clone_name.set(format!("{}-clone", template.name));
        set_show_clone_modal.set(true);
    };

    let clone_template_action = move || {
        if let Some(template) = selected_template.get() {
            let template_id = template.id.clone();
            let new_name = clone_name.get();

            spawn_local(async move {
                match clone_template(&template_id, new_name.clone()).await {
                    Ok(_) => {
                        set_show_clone_modal.set(false);
                        reset_form();
                        set_success_message.set(Some(format!("Template cloned as '{}' successfully", new_name)));
                        set_timeout(
                            move || set_success_message.set(None),
                            std::time::Duration::from_secs(3),
                        );
                    }
                    Err(e) => set_error.set(Some(format!("Failed to clone template: {}", e))),
                }
            });
        }
    };

    let format_bytes = move |bytes: u64| {
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    };

    let get_vm_name = move |vm_id: &str| -> String {
        vms.get()
            .iter()
            .find(|vm| vm.id == vm_id)
            .map(|vm| vm.name.clone())
            .unwrap_or_else(|| vm_id.to_string())
    };

    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <div>
                    <h1 class="text-2xl font-bold">VM Templates</h1>
                    <p class="text-gray-600">
                        "Create and manage virtual machine templates for rapid deployment"
                    </p>
                </div>
                <div class="flex space-x-3">
                    <button
                        on:click=move |_| load_templates()
                        class="bg-gray-500 hover:bg-gray-600 text-white px-4 py-2 rounded-lg"
                    >
                        <i class="fas fa-sync mr-2"></i>
                        "Refresh"
                    </button>
                    <button
                        on:click=move |_| {
                            reset_form();
                            set_show_create_modal.set(true);
                        }
                        class="bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg flex items-center gap-2"
                    >
                        <i class="fas fa-plus"></i>
                        "Create Template"
                    </button>
                </div>
            </div>

            // Success message
            {move || success_message.get().map(|msg| view! {
                <div class="bg-green-50 border border-green-200 text-green-700 px-4 py-3 rounded mb-6">
                    <i class="fas fa-check-circle mr-2"></i>
                    {msg}
                </div>
            })}

            // Error display
            {move || error.get().map(|e| view! {
                <div class="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded mb-6">
                    <i class="fas fa-exclamation-triangle mr-2"></i>
                    {e}
                </div>
            })}

            // Template Info Card
            <div class="bg-blue-50 border border-blue-200 rounded-lg p-4 mb-6">
                <h3 class="text-lg font-medium text-blue-900 mb-2">
                    <i class="fas fa-info-circle mr-2"></i>
                    "About VM Templates"
                </h3>
                <div class="text-sm text-blue-800 space-y-2">
                    <p>
                        <strong>"Templates"</strong>
                        " are pre-configured virtual machine images that can be used to quickly deploy new VMs with identical configurations."
                    </p>
                    <ul class="list-disc list-inside ml-4 space-y-1">
                        <li>"Create templates from existing VMs to standardize deployments"</li>
                        <li>"Clone templates to create new VMs with the same configuration"</li>
                        <li>"Templates include the complete VM disk image and configuration"</li>
                        <li>"Use templates for development environments, testing, or production standardization"</li>
                    </ul>
                </div>
            </div>

            // Search and Controls
            <div class="bg-white rounded-lg shadow p-4 mb-6">
                <div class="flex items-center space-x-4">
                    <div class="flex-1">
                        <label class="block text-sm font-medium text-gray-700 mb-1">
                            "Search Templates"
                        </label>
                        <input
                            type="text"
                            placeholder="Search by name, description, or source VM..."
                            on:input=move |ev| set_search_query.set(event_target_value(&ev))
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                        />
                    </div>
                    <div class="text-sm text-gray-600">
                        {move || {
                            let filtered = filtered_templates();
                            let total = templates.get().len();
                            if filtered.len() == total {
                                format!("{} templates", total)
                            } else {
                                format!("{} of {} templates", filtered.len(), total)
                            }
                        }}
                    </div>
                </div>
            </div>

            // Templates Grid
            {move || if loading.get() {
                view! {
                    <div class="bg-white rounded-lg shadow p-8 text-center">
                        <i class="fas fa-spinner fa-spin text-2xl text-gray-400 mb-2"></i>
                        <p class="text-gray-600">"Loading templates..."</p>
                    </div>
                }
            } else if filtered_templates().is_empty() {
                if templates.get().is_empty() {
                    view! {
                        <div class="bg-white rounded-lg shadow p-8 text-center">
                            <i class="fas fa-file-archive text-6xl text-gray-300 mb-4"></i>
                            <h3 class="text-xl font-medium text-gray-900 mb-2">"No Templates Found"</h3>
                            <p class="text-gray-600 mb-6">
                                "Create your first VM template to enable rapid virtual machine deployment"
                            </p>
                            <button
                                on:click=move |_| {
                                    reset_form();
                                    set_show_create_modal.set(true);
                                }
                                class="bg-blue-500 hover:bg-blue-600 text-white px-6 py-3 rounded-lg"
                            >
                                <i class="fas fa-plus mr-2"></i>
                                "Create First Template"
                            </button>
                        </div>
                    }
                } else {
                    view! {
                        <div class="bg-white rounded-lg shadow p-8 text-center">
                            <i class="fas fa-search text-4xl text-gray-300 mb-4"></i>
                            <h3 class="text-lg font-medium text-gray-900 mb-2">"No Templates Found"</h3>
                            <p class="text-gray-600">"No templates match your search criteria"</p>
                        </div>
                    }
                }
            } else {
                view! {
                    <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
                        {filtered_templates().into_iter().map(|template| {
                            let template_id = template.id.clone();
                            let template_name = template.name.clone();
                            let template_name2 = template.name.clone();
                            let template_clone = template.clone();
                            let template_desc = template.description.clone();
                            let source_vm_name = get_vm_name(&template.source_vm_id);
                            let size = format_bytes(template.size_mb * 1024 * 1024);
                            let created_at = template.created_at.clone();
                            let storage_location = template.storage_location.clone();

                            view! {
                                <div class="bg-white rounded-lg shadow hover:shadow-lg transition-shadow">
                                    <div class="p-6">
                                        <div class="flex items-start justify-between mb-4">
                                            <div class="flex-1">
                                                <h3 class="text-lg font-medium text-gray-900 mb-1">{template_name}</h3>
                                                {template_desc.map(|desc| view! {
                                                    <p class="text-sm text-gray-600 mb-2 line-clamp-2">{desc}</p>
                                                })}
                                            </div>
                                            <i class="fas fa-file-archive text-2xl text-purple-500"></i>
                                        </div>

                                        <div class="space-y-2 text-sm text-gray-600">
                                            <div class="flex items-center">
                                                <i class="fas fa-server w-4 mr-2 text-gray-400"></i>
                                                <span class="font-medium">"Source VM: "</span>
                                                <span class="ml-1">{source_vm_name}</span>
                                            </div>
                                            <div class="flex items-center">
                                                <i class="fas fa-hdd w-4 mr-2 text-gray-400"></i>
                                                <span class="font-medium">"Size: "</span>
                                                <span class="ml-1">{size}</span>
                                            </div>
                                            <div class="flex items-center">
                                                <i class="fas fa-calendar w-4 mr-2 text-gray-400"></i>
                                                <span class="font-medium">"Created: "</span>
                                                <span class="ml-1">{created_at}</span>
                                            </div>
                                            <div class="flex items-center">
                                                <i class="fas fa-folder w-4 mr-2 text-gray-400"></i>
                                                <span class="font-medium">"Location: "</span>
                                                <span class="ml-1 font-mono text-xs">{storage_location}</span>
                                            </div>
                                        </div>

                                        <div class="mt-6 flex space-x-2">
                                            <button
                                                on:click=move |_| start_clone(template_clone.clone())
                                                class="flex-1 bg-green-500 hover:bg-green-600 text-white px-3 py-2 rounded text-sm"
                                            >
                                                <i class="fas fa-copy mr-1"></i>
                                                "Clone"
                                            </button>
                                            <button
                                                class="bg-gray-500 hover:bg-gray-600 text-white px-3 py-2 rounded text-sm"
                                                title="View Details"
                                            >
                                                <i class="fas fa-eye"></i>
                                            </button>
                                            <button
                                                on:click=move |_| delete_template_action(template_id.clone(), template_name2.clone())
                                                class="bg-red-500 hover:bg-red-600 text-white px-3 py-2 rounded text-sm"
                                                title="Delete Template"
                                            >
                                                <i class="fas fa-trash"></i>
                                            </button>
                                        </div>
                                    </div>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                }
            }}

            // Create Template Modal
            {move || if show_create_modal.get() {
                view! {
                    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                        <div class="bg-white rounded-lg w-full max-w-lg">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-xl font-semibold">"Create VM Template"</h2>
                            </div>

                            <div class="p-6 space-y-4">
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Template Name"
                                    </label>
                                    <input
                                        type="text"
                                        placeholder="ubuntu-22.04-template"
                                        prop:value=move || template_name.get()
                                        on:input=move |ev| set_template_name.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    />
                                </div>

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Source Virtual Machine"
                                    </label>
                                    <select
                                        on:change=move |ev| set_source_vm_id.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    >
                                        <option value="">"Select a VM to template..."</option>
                                        {move || vms.get().into_iter().map(|vm| {
                                            let vm_id = vm.id.clone();
                                            let vm_display = format!("{} ({}) - {:?}", vm.name, vm.id, vm.status);
                                            view! {
                                                <option value={vm_id}>{vm_display}</option>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </select>
                                    <p class="text-xs text-gray-500 mt-1">
                                        "Note: The VM will be stopped during template creation"
                                    </p>
                                </div>

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "Description"
                                    </label>
                                    <textarea
                                        placeholder="Describe this template and its intended use..."
                                        prop:value=move || template_description.get()
                                        on:input=move |ev| set_template_description.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        rows="3"
                                    />
                                </div>

                                <div class="bg-yellow-50 border border-yellow-200 rounded p-3">
                                    <div class="flex items-start">
                                        <i class="fas fa-exclamation-triangle text-yellow-600 mt-1 mr-2"></i>
                                        <div class="text-sm text-yellow-800">
                                            <p class="font-medium">"Important Notes:"</p>
                                            <ul class="list-disc list-inside mt-1 space-y-1">
                                                <li>"The source VM will be temporarily stopped during template creation"</li>
                                                <li>"Template creation may take several minutes depending on VM size"</li>
                                                <li>"Ensure the source VM is in a clean, deployable state"</li>
                                            </ul>
                                        </div>
                                    </div>
                                </div>
                            </div>

                            <div class="p-6 border-t border-gray-200 flex justify-end space-x-3">
                                <button
                                    on:click=move |_| set_show_create_modal.set(false)
                                    class="px-4 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50"
                                >
                                    "Cancel"
                                </button>
                                <button
                                    on:click=move |_| create_template()
                                    class="px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600"
                                    disabled=move || template_name.get().is_empty() || source_vm_id.get().is_empty()
                                >
                                    "Create Template"
                                </button>
                            </div>
                        </div>
                    </div>
                }
            } else {
                view! { <div></div> }
            }}

            // Clone Template Modal
            {move || if show_clone_modal.get() {
                view! {
                    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                        <div class="bg-white rounded-lg w-full max-w-md">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-xl font-semibold">"Clone Template"</h2>
                            </div>

                            <div class="p-6 space-y-4">
                                {move || selected_template.get().map(|template| view! {
                                    <div class="bg-gray-50 rounded-lg p-4 mb-4">
                                        <div class="text-sm">
                                            <div class="font-medium text-gray-900">"Source Template:"</div>
                                            <div class="text-gray-600">{&template.name}</div>
                                            {template.description.as_ref().map(|desc| view! {
                                                <div class="text-xs text-gray-500 mt-1">{desc}</div>
                                            })}
                                        </div>
                                    </div>
                                })}

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                        "New VM Name"
                                    </label>
                                    <input
                                        type="text"
                                        placeholder="new-vm-name"
                                        prop:value=move || clone_name.get()
                                        on:input=move |ev| set_clone_name.set(event_target_value(&ev))
                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                    />
                                    <p class="text-xs text-gray-500 mt-1">
                                        "This will create a new VM from the template"
                                    </p>
                                </div>

                                <div class="bg-blue-50 border border-blue-200 rounded p-3">
                                    <div class="flex items-start">
                                        <i class="fas fa-info-circle text-blue-600 mt-1 mr-2"></i>
                                        <div class="text-sm text-blue-800">
                                            <p>"The new VM will be created with the same configuration as the template but can be modified independently."</p>
                                        </div>
                                    </div>
                                </div>
                            </div>

                            <div class="p-6 border-t border-gray-200 flex justify-end space-x-3">
                                <button
                                    on:click=move |_| set_show_clone_modal.set(false)
                                    class="px-4 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50"
                                >
                                    "Cancel"
                                </button>
                                <button
                                    on:click=move |_| clone_template_action()
                                    class="px-4 py-2 bg-green-500 text-white rounded-lg hover:bg-green-600"
                                    disabled=move || clone_name.get().is_empty()
                                >
                                    "Clone Template"
                                </button>
                            </div>
                        </div>
                    </div>
                }
            } else {
                view! { <div></div> }
            }}
        </div>
    }
}