use leptos::*;
use std::collections::HashMap;
use base64::Engine;
use crate::api::{KubernetesSecret, CreateSecretRequest, get_kubernetes_secrets, create_kubernetes_secret, update_kubernetes_secret, delete_kubernetes_secret};

#[component]
pub fn SecretsPage() -> impl IntoView {
    let (secrets, set_secrets) = create_signal(Vec::<KubernetesSecret>::new());
    let (loading, set_loading) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    let (show_create_modal, set_show_create_modal) = create_signal(false);
    let (show_edit_modal, set_show_edit_modal) = create_signal(false);
    let (selected_secret, set_selected_secret) = create_signal(None::<KubernetesSecret>);
    let (search_query, set_search_query) = create_signal(String::new());
    let (selected_namespace, set_selected_namespace) = create_signal("default".to_string());
    let (cluster_id, set_cluster_id) = create_signal("main".to_string());

    // Form state
    let (name, set_name) = create_signal(String::new());
    let (namespace, set_namespace) = create_signal("default".to_string());
    let (secret_type, set_secret_type) = create_signal("Opaque".to_string());
    let (data_key, set_data_key) = create_signal(String::new());
    let (data_value, set_data_value) = create_signal(String::new());
    let (secret_data, set_secret_data) = create_signal(HashMap::<String, String>::new());
    let (labels, set_labels) = create_signal(HashMap::<String, String>::new());
    let (annotations, set_annotations) = create_signal(HashMap::<String, String>::new());

    // TLS specific fields
    let (tls_cert, set_tls_cert) = create_signal(String::new());
    let (tls_key, set_tls_key) = create_signal(String::new());

    // Docker config fields
    let (docker_server, set_docker_server) = create_signal(String::new());
    let (docker_username, set_docker_username) = create_signal(String::new());
    let (docker_password, set_docker_password) = create_signal(String::new());
    let (docker_email, set_docker_email) = create_signal(String::new());

    let load_secrets = move || {
        set_loading.set(true);
        set_error.set(None);

        spawn_local(async move {
            match get_kubernetes_secrets(&cluster_id.get(), Some(&selected_namespace.get())).await {
                Ok(secs) => {
                    set_secrets.set(secs);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(format!("Failed to load Secrets: {}", e))),
            }
            set_loading.set(false);
        });
    };

    // Load secrets on mount and when dependencies change
    create_effect(move |_| {
        if !cluster_id.get().is_empty() && !selected_namespace.get().is_empty() {
            load_secrets();
        }
    });

    // Auto-refresh every 30 seconds
    use leptos::set_interval;
    set_interval(
        move || load_secrets(),
        std::time::Duration::from_secs(30),
    );

    let filtered_secrets = move || {
        let query = search_query.get().to_lowercase();
        if query.is_empty() {
            secrets.get()
        } else {
            secrets
                .get()
                .into_iter()
                .filter(|secret| {
                    secret.name.to_lowercase().contains(&query) ||
                    secret.namespace.to_lowercase().contains(&query) ||
                    secret.secret_type.as_deref().unwrap_or("").to_lowercase().contains(&query)
                })
                .collect()
        }
    };

    let add_data_pair = move || {
        let key = data_key.get().trim().to_string();
        let value = data_value.get();

        if !key.is_empty() {
            let mut current_data = secret_data.get();
            current_data.insert(key, value);
            set_secret_data.set(current_data);
            set_data_key.set(String::new());
            set_data_value.set(String::new());
        }
    };

    let remove_data_pair = move |key: String| {
        let mut current_data = secret_data.get();
        current_data.remove(&key);
        set_secret_data.set(current_data);
    };

    let setup_tls_secret = move || {
        let mut data = HashMap::new();
        if !tls_cert.get().is_empty() {
            data.insert("tls.crt".to_string(), tls_cert.get());
        }
        if !tls_key.get().is_empty() {
            data.insert("tls.key".to_string(), tls_key.get());
        }
        set_secret_data.set(data);
    };

    let setup_docker_secret = move || {
        let mut data = HashMap::new();
        let config = format!(
            r#"{{"auths":{{"{}":{{"username":"{}","password":"{}","email":"{}","auth":"{}"}}}}}}"#,
            docker_server.get(),
            docker_username.get(),
            docker_password.get(),
            docker_email.get(),
            base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", docker_username.get(), docker_password.get()))
        );
        data.insert(".dockerconfigjson".to_string(), config);
        set_secret_data.set(data);
    };

    let reset_form = move || {
        set_name.set(String::new());
        set_namespace.set("default".to_string());
        set_secret_type.set("Opaque".to_string());
        set_secret_data.set(HashMap::new());
        set_labels.set(HashMap::new());
        set_annotations.set(HashMap::new());
        set_data_key.set(String::new());
        set_data_value.set(String::new());
        set_tls_cert.set(String::new());
        set_tls_key.set(String::new());
        set_docker_server.set(String::new());
        set_docker_username.set(String::new());
        set_docker_password.set(String::new());
        set_docker_email.set(String::new());
    };

    let create_secret = move || {
        let request = CreateSecretRequest {
            name: name.get(),
            secret_type: secret_type.get(),
            data: secret_data.get(),
            string_data: None,
            labels: if labels.get().is_empty() { None } else { Some(labels.get()) },
            annotations: if annotations.get().is_empty() { None } else { Some(annotations.get()) },
        };

        let ns = namespace.get();
        spawn_local(async move {
            match create_kubernetes_secret(&cluster_id.get(), &ns, request).await {
                Ok(_) => {
                    set_show_create_modal.set(false);
                    reset_form();
                    load_secrets();
                }
                Err(e) => set_error.set(Some(format!("Failed to create Secret: {}", e))),
            }
        });
    };

    let edit_secret = move |secret: KubernetesSecret| {
        set_selected_secret.set(Some(secret.clone()));
        set_name.set(secret.name);
        set_namespace.set(secret.namespace);
        set_secret_type.set(secret.secret_type.unwrap_or("Opaque".to_string()));
        set_secret_data.set(secret.data);
        set_labels.set(secret.labels.unwrap_or_default());
        set_annotations.set(secret.annotations.unwrap_or_default());
        set_show_edit_modal.set(true);
    };

    let update_secret = move || {
        if let Some(secret) = selected_secret.get() {
            let request = CreateSecretRequest {
                name: name.get(),
                secret_type: secret_type.get(),
                data: secret_data.get(),
                string_data: None,
                labels: if labels.get().is_empty() { None } else { Some(labels.get()) },
                annotations: if annotations.get().is_empty() { None } else { Some(annotations.get()) },
            };

            spawn_local(async move {
                match update_kubernetes_secret(&cluster_id.get(), &secret.namespace, &secret.name, request).await {
                    Ok(_) => {
                        set_show_edit_modal.set(false);
                        reset_form();
                        load_secrets();
                    }
                    Err(e) => set_error.set(Some(format!("Failed to update Secret: {}", e))),
                }
            });
        }
    };

    let delete_secret = move |secret: KubernetesSecret| {
        if web_sys::window()
            .unwrap()
            .confirm_with_message(&format!("Are you sure you want to delete Secret '{}'?", secret.name))
            .unwrap()
        {
            spawn_local(async move {
                match delete_kubernetes_secret(&cluster_id.get(), &secret.namespace, &secret.name).await {
                    Ok(_) => load_secrets(),
                    Err(e) => set_error.set(Some(format!("Failed to delete Secret: {}", e))),
                }
            });
        }
    };

    let get_secret_type_color = move |secret_type: &Option<String>| {
        match secret_type.as_deref() {
            Some("Opaque") => "bg-gray-100 text-gray-800",
            Some("kubernetes.io/tls") => "bg-green-100 text-green-800",
            Some("kubernetes.io/dockerconfigjson") => "bg-blue-100 text-blue-800",
            Some("kubernetes.io/service-account-token") => "bg-purple-100 text-purple-800",
            _ => "bg-gray-100 text-gray-800",
        }
    };

    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-2xl font-bold">Secrets</h1>
                <button
                    on:click=move |_| {
                        reset_form();
                        set_show_create_modal.set(true);
                    }
                    class="bg-blue-500 hover:bg-blue-600 text-white px-4 py-2 rounded-lg flex items-center gap-2"
                >
                    <i class="fas fa-plus"></i>
                    "Create Secret"
                </button>
            </div>

            // Controls
            <div class="bg-white rounded-lg shadow p-4 mb-6">
                <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">
                            "Cluster"
                        </label>
                        <select
                            on:change=move |ev| set_cluster_id.set(event_target_value(&ev))
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                        >
                            <option value="main">"Main Cluster"</option>
                            <option value="staging">"Staging Cluster"</option>
                        </select>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">
                            "Namespace"
                        </label>
                        <select
                            on:change=move |ev| set_selected_namespace.set(event_target_value(&ev))
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                        >
                            <option value="default">"default"</option>
                            <option value="kube-system">"kube-system"</option>
                            <option value="monitoring">"monitoring"</option>
                            <option value="logging">"logging"</option>
                        </select>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">
                            "Search Secrets"
                        </label>
                        <input
                            type="text"
                            placeholder="Search by name, namespace, or type..."
                            on:input=move |ev| set_search_query.set(event_target_value(&ev))
                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                        />
                    </div>
                </div>
            </div>

            // Error display
            {move || error.get().map(|e| view! {
                <div class="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded mb-4">
                    {e}
                </div>
            })}

            // Loading state
            {move || if loading.get() {
                view! {
                    <div class="bg-white rounded-lg shadow p-8 text-center">
                        <i class="fas fa-spinner fa-spin text-2xl text-gray-400 mb-2"></i>
                        <p class="text-gray-600">"Loading Secrets..."</p>
                    </div>
                }
            } else {
                view! {
                    <div class="bg-white rounded-lg shadow overflow-hidden">
                        <div class="overflow-x-auto">
                            <table class="min-w-full divide-y divide-gray-200">
                                <thead class="bg-gray-50">
                                    <tr>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Name"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Namespace"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Type"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Data Keys"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Age"
                                        </th>
                                        <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                            "Actions"
                                        </th>
                                    </tr>
                                </thead>
                                <tbody class="bg-white divide-y divide-gray-200">
                                    {filtered_secrets().into_iter().map(|secret| {
                                        let secret_edit = secret.clone();
                                        let secret_delete = secret.clone();
                                        let name = secret.name.clone();
                                        let namespace = secret.namespace.clone();
                                        let data_keys: Vec<String> = secret.data.keys().cloned().collect();
                                        let keys_len = data_keys.len();
                                        let keys_display = if data_keys.len() > 3 {
                                            format!("{}, ... (+{})", data_keys[..3].join(", "), data_keys.len() - 3)
                                        } else {
                                            data_keys.join(", ")
                                        };
                                        let type_color = get_secret_type_color(&secret.secret_type);
                                        let secret_type_display = secret.secret_type.clone().unwrap_or_else(|| "Opaque".to_string());
                                        let created_at = secret.created_at.map(|dt| dt.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_else(|| "Unknown".to_string());

                                        view! {
                                            <tr>
                                                <td class="px-6 py-4 whitespace-nowrap">
                                                    <div class="text-sm font-medium text-gray-900">{name}</div>
                                                </td>
                                                <td class="px-6 py-4 whitespace-nowrap">
                                                    <span class="px-2 py-1 text-xs font-medium bg-blue-100 text-blue-800 rounded">
                                                        {namespace}
                                                    </span>
                                                </td>
                                                <td class="px-6 py-4 whitespace-nowrap">
                                                    <span class=format!("px-2 py-1 text-xs font-medium rounded {}", type_color)>
                                                        {secret_type_display}
                                                    </span>
                                                </td>
                                                <td class="px-6 py-4 whitespace-nowrap">
                                                    <div class="text-sm text-gray-900">{keys_display}</div>
                                                    <div class="text-xs text-gray-500">{format!("{} keys total", keys_len)}</div>
                                                </td>
                                                <td class="px-6 py-4 whitespace-nowrap">
                                                    <div class="text-sm text-gray-900">
                                                        {created_at}
                                                    </div>
                                                </td>
                                                <td class="px-6 py-4 whitespace-nowrap text-sm font-medium">
                                                    <div class="flex space-x-2">
                                                        <button
                                                            on:click=move |_| edit_secret(secret_edit.clone())
                                                            class="text-blue-600 hover:text-blue-900 px-2 py-1 rounded hover:bg-blue-50"
                                                            title="Edit Secret"
                                                        >
                                                            <i class="fas fa-edit"></i>
                                                        </button>
                                                        <button
                                                            on:click=move |_| delete_secret(secret_delete.clone())
                                                            class="text-red-600 hover:text-red-900 px-2 py-1 rounded hover:bg-red-50"
                                                            title="Delete Secret"
                                                        >
                                                            <i class="fas fa-trash"></i>
                                                        </button>
                                                    </div>
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>
                    </div>
                }
            }}

            // Create Modal
            {move || if show_create_modal.get() {
                view! {
                    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                        <div class="bg-white rounded-lg w-full max-w-4xl max-h-[90vh] overflow-y-auto">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-xl font-semibold">"Create Secret"</h2>
                            </div>

                            <div class="p-6 space-y-6">
                                <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Name"
                                        </label>
                                        <input
                                            type="text"
                                            placeholder="my-secret"
                                            prop:value=move || name.get()
                                            on:input=move |ev| set_name.set(event_target_value(&ev))
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Namespace"
                                        </label>
                                        <input
                                            type="text"
                                            placeholder="default"
                                            prop:value=move || namespace.get()
                                            on:input=move |ev| set_namespace.set(event_target_value(&ev))
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Type"
                                        </label>
                                        <select
                                            on:change=move |ev| set_secret_type.set(event_target_value(&ev))
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                        >
                                            <option value="Opaque">"Opaque"</option>
                                            <option value="kubernetes.io/tls">"TLS"</option>
                                            <option value="kubernetes.io/dockerconfigjson">"Docker Config"</option>
                                        </select>
                                    </div>
                                </div>

                                // Type-specific forms
                                {move || match secret_type.get().as_str() {
                                    "kubernetes.io/tls" => view! {
                                        <div>
                                            <h3 class="text-lg font-medium mb-4">"TLS Configuration"</h3>
                                            <div class="space-y-4">
                                                <div>
                                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                                        "Certificate (tls.crt)"
                                                    </label>
                                                    <textarea
                                                        placeholder="-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----"
                                                        prop:value=move || tls_cert.get()
                                                        on:input=move |ev| set_tls_cert.set(event_target_value(&ev))
                                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg font-mono"
                                                        rows="8"
                                                    />
                                                </div>
                                                <div>
                                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                                        "Private Key (tls.key)"
                                                    </label>
                                                    <textarea
                                                        placeholder="-----BEGIN PRIVATE KEY-----\n...\n-----END PRIVATE KEY-----"
                                                        prop:value=move || tls_key.get()
                                                        on:input=move |ev| set_tls_key.set(event_target_value(&ev))
                                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg font-mono"
                                                        rows="8"
                                                    />
                                                </div>
                                                <button
                                                    on:click=move |_| setup_tls_secret()
                                                    class="bg-green-500 hover:bg-green-600 text-white px-4 py-2 rounded-lg"
                                                >
                                                    "Setup TLS Secret Data"
                                                </button>
                                            </div>
                                        </div>
                                    },
                                    "kubernetes.io/dockerconfigjson" => view! {
                                        <div>
                                            <h3 class="text-lg font-medium mb-4">"Docker Registry Configuration"</h3>
                                            <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-4">
                                                <div>
                                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                                        "Registry Server"
                                                    </label>
                                                    <input
                                                        type="text"
                                                        placeholder="docker.io"
                                                        prop:value=move || docker_server.get()
                                                        on:input=move |ev| set_docker_server.set(event_target_value(&ev))
                                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                                    />
                                                </div>
                                                <div>
                                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                                        "Username"
                                                    </label>
                                                    <input
                                                        type="text"
                                                        placeholder="username"
                                                        prop:value=move || docker_username.get()
                                                        on:input=move |ev| set_docker_username.set(event_target_value(&ev))
                                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                                    />
                                                </div>
                                                <div>
                                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                                        "Password"
                                                    </label>
                                                    <input
                                                        type="password"
                                                        placeholder="password"
                                                        prop:value=move || docker_password.get()
                                                        on:input=move |ev| set_docker_password.set(event_target_value(&ev))
                                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                                    />
                                                </div>
                                                <div>
                                                    <label class="block text-sm font-medium text-gray-700 mb-1">
                                                        "Email"
                                                    </label>
                                                    <input
                                                        type="email"
                                                        placeholder="email@example.com"
                                                        prop:value=move || docker_email.get()
                                                        on:input=move |ev| set_docker_email.set(event_target_value(&ev))
                                                        class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                                    />
                                                </div>
                                            </div>
                                            <button
                                                on:click=move |_| setup_docker_secret()
                                                class="bg-green-500 hover:bg-green-600 text-white px-4 py-2 rounded-lg"
                                            >
                                                "Setup Docker Config"
                                            </button>
                                        </div>
                                    },
                                    _ => view! {
                                        <div>
                                            <h3 class="text-lg font-medium mb-4">"Secret Data"</h3>

                                            <div class="bg-gray-50 p-4 rounded-lg mb-4">
                                                <div class="grid grid-cols-1 md:grid-cols-3 gap-4 items-end">
                                                    <div>
                                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                                            "Key"
                                                        </label>
                                                        <input
                                                            type="text"
                                                            placeholder="username"
                                                            prop:value=move || data_key.get()
                                                            on:input=move |ev| set_data_key.set(event_target_value(&ev))
                                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                                        />
                                                    </div>
                                                    <div>
                                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                                            "Value"
                                                        </label>
                                                        <input
                                                            type="password"
                                                            placeholder="password"
                                                            prop:value=move || data_value.get()
                                                            on:input=move |ev| set_data_value.set(event_target_value(&ev))
                                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                                        />
                                                    </div>
                                                    <div>
                                                        <button
                                                            on:click=move |_| add_data_pair()
                                                            class="w-full bg-green-500 hover:bg-green-600 text-white px-4 py-2 rounded-lg"
                                                        >
                                                            "Add Data"
                                                        </button>
                                                    </div>
                                                </div>
                                            </div>
                                        </div>
                                    }
                                }}

                                // Current Data
                                <div class="space-y-2">
                                    <h4 class="text-md font-medium">"Current Secret Data"</h4>
                                    {move || secret_data.get().into_iter().map(|(key, value)| {
                                        let key_clone = key.clone();
                                        let key_display = key.clone();
                                        let value_len = value.len();
                                        view! {
                                            <div class="bg-white border border-gray-200 rounded-lg p-3">
                                                <div class="flex justify-between items-start">
                                                    <div class="flex-1 min-w-0">
                                                        <div class="text-sm font-medium text-gray-900 mb-1">{key_display}</div>
                                                        <div class="text-xs text-gray-600 font-mono bg-gray-50 p-2 rounded">
                                                            {format!("******** ({} characters)", value_len)}
                                                        </div>
                                                    </div>
                                                    <button
                                                        on:click=move |_| remove_data_pair(key_clone.clone())
                                                        class="ml-2 text-red-600 hover:text-red-900 p-1"
                                                    >
                                                        <i class="fas fa-times"></i>
                                                    </button>
                                                </div>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
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
                                    on:click=move |_| create_secret()
                                    class="px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600"
                                    disabled=move || name.get().is_empty() || secret_data.get().is_empty()
                                >
                                    "Create Secret"
                                </button>
                            </div>
                        </div>
                    </div>
                }
            } else {
                view! { <div></div> }
            }}

            // Edit Modal (similar structure to create modal)
            {move || if show_edit_modal.get() {
                view! {
                    <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
                        <div class="bg-white rounded-lg w-full max-w-4xl max-h-[90vh] overflow-y-auto">
                            <div class="p-6 border-b border-gray-200">
                                <h2 class="text-xl font-semibold">"Edit Secret"</h2>
                            </div>

                            <div class="p-6 space-y-6">
                                <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Name"
                                        </label>
                                        <input
                                            type="text"
                                            prop:value=move || name.get()
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            disabled=true
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Namespace"
                                        </label>
                                        <input
                                            type="text"
                                            prop:value=move || namespace.get()
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            disabled=true
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 mb-1">
                                            "Type"
                                        </label>
                                        <input
                                            type="text"
                                            prop:value=move || secret_type.get()
                                            class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                            disabled=true
                                        />
                                    </div>
                                </div>

                                // Edit data section
                                <div>
                                    <h3 class="text-lg font-medium mb-4">"Secret Data"</h3>

                                    <div class="bg-gray-50 p-4 rounded-lg mb-4">
                                        <div class="grid grid-cols-1 md:grid-cols-3 gap-4 items-end">
                                            <div>
                                                <label class="block text-sm font-medium text-gray-700 mb-1">
                                                    "Key"
                                                </label>
                                                <input
                                                    type="text"
                                                    placeholder="key"
                                                    prop:value=move || data_key.get()
                                                    on:input=move |ev| set_data_key.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                                />
                                            </div>
                                            <div>
                                                <label class="block text-sm font-medium text-gray-700 mb-1">
                                                    "Value"
                                                </label>
                                                <input
                                                    type="password"
                                                    placeholder="value"
                                                    prop:value=move || data_value.get()
                                                    on:input=move |ev| set_data_value.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 border border-gray-300 rounded-lg"
                                                />
                                            </div>
                                            <div>
                                                <button
                                                    on:click=move |_| add_data_pair()
                                                    class="w-full bg-green-500 hover:bg-green-600 text-white px-4 py-2 rounded-lg"
                                                >
                                                    "Add Data"
                                                </button>
                                            </div>
                                        </div>
                                    </div>

                                    // Current data
                                    <div class="space-y-2">
                                        {move || secret_data.get().into_iter().map(|(key, value)| {
                                            let key_clone = key.clone();
                                            let key_display = key.clone();
                                            let value_len = value.len();
                                            view! {
                                                <div class="bg-white border border-gray-200 rounded-lg p-3">
                                                    <div class="flex justify-between items-start">
                                                        <div class="flex-1 min-w-0">
                                                            <div class="text-sm font-medium text-gray-900 mb-1">{key_display}</div>
                                                            <div class="text-xs text-gray-600 font-mono bg-gray-50 p-2 rounded">
                                                                {format!("******** ({} characters)", value_len)}
                                                            </div>
                                                        </div>
                                                        <button
                                                            on:click=move |_| remove_data_pair(key_clone.clone())
                                                            class="ml-2 text-red-600 hover:text-red-900 p-1"
                                                        >
                                                            <i class="fas fa-times"></i>
                                                        </button>
                                                    </div>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>
                            </div>

                            <div class="p-6 border-t border-gray-200 flex justify-end space-x-3">
                                <button
                                    on:click=move |_| set_show_edit_modal.set(false)
                                    class="px-4 py-2 text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50"
                                >
                                    "Cancel"
                                </button>
                                <button
                                    on:click=move |_| update_secret()
                                    class="px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600"
                                >
                                    "Update Secret"
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