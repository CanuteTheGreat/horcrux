//! Role and Permission Management
//!
//! Complete RBAC management interface for roles and permissions.
//! Provides visual permission matrix and role customization.

use leptos::*;
use crate::api;
use std::collections::HashMap;

/// Available privilege types for the permission system
const AVAILABLE_PRIVILEGES: &[&str] = &[
    "VmAudit", "VmConsole", "VmConfig", "VmPowerMgmt", "VmAllocate",
    "VmMigrate", "VmSnapshot", "VmBackup", "DatastoreAudit", "DatastoreAllocate",
    "DatastoreAllocateSpace", "PoolAudit", "PoolAllocate", "SysAudit",
    "SysModify", "SysConsole", "UserModify", "PermissionsModify"
];

/// Common resource paths for permission assignment
const COMMON_PATHS: &[&str] = &[
    "/", "/vms", "/vms/*", "/storage", "/storage/*",
    "/pools", "/pools/*", "/cluster", "/users", "/system"
];

/// Role management page component
#[component]
pub fn RolesPage() -> impl IntoView {
    let (roles, set_roles) = create_signal(Vec::<api::Role>::new());
    let (loading, set_loading) = create_signal(true);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (selected_role, set_selected_role) = create_signal(None::<api::Role>);
    let (show_create_form, set_show_create_form) = create_signal(false);
    let (show_permission_matrix, set_show_permission_matrix) = create_signal(false);

    // Load roles on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            set_error_message.set(None);

            match api::get_roles().await {
                Ok(roles_data) => {
                    set_roles.set(roles_data);
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to load roles: {}", e)));
                }
            }

            set_loading.set(false);
        });
    });

    // Delete role
    let delete_role = move |role_name: String| {
        spawn_local(async move {
            match api::delete_role(&role_name).await {
                Ok(()) => {
                    // Refresh roles list
                    if let Ok(roles_data) = api::get_roles().await {
                        set_roles.set(roles_data);
                    }
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Failed to delete role: {}", e)));
                }
            }
        });
    };

    // Check if role is built-in and cannot be deleted
    let is_builtin_role = |role_name: &str| {
        matches!(role_name, "Administrator" | "PVEAdmin" | "PVEVMUser")
    };

    view! {
        <div class="roles-management-page">
            <div class="page-header">
                <div class="header-content">
                    <h1>"Role & Permission Management"</h1>
                    <p class="description">
                        "Configure roles and assign granular permissions for resource access control"
                    </p>
                </div>

                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| set_show_permission_matrix.set(!show_permission_matrix.get())
                    >
                        <span class="icon">"📊"</span>
                        {move || if show_permission_matrix.get() { "Hide Matrix" } else { "Permission Matrix" }}
                    </button>
                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_create_form.set(!show_create_form.get())
                    >
                        <span class="icon">"+"</span>
                        {move || if show_create_form.get() { "Cancel" } else { "Create Role" }}
                    </button>
                </div>
            </div>

            {move || error_message.get().map(|msg| view! {
                <div class="alert alert-error">
                    <span class="alert-icon">"!"</span>
                    <span class="alert-message">{msg}</span>
                    <button
                        class="alert-close"
                        on:click=move |_| set_error_message.set(None)
                    >"x"</button>
                </div>
            })}

            {move || if show_create_form.get() {
                view! {
                    <CreateRoleForm
                        on_success={
                            let set_show_create_form = set_show_create_form.clone();
                            let set_roles = set_roles.clone();
                            move || {
                                set_show_create_form.set(false);
                                // Refresh roles list
                                spawn_local(async move {
                                    if let Ok(roles_data) = api::get_roles().await {
                                        set_roles.set(roles_data);
                                    }
                                });
                            }
                        }
                        on_error={
                            let set_error_message = set_error_message.clone();
                            move |msg| set_error_message.set(Some(msg))
                        }
                    />
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            {move || if show_permission_matrix.get() {
                view! {
                    <PermissionMatrix roles=roles.get() />
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            <div class="roles-container">
                {move || if loading.get() {
                    view! {
                        <div class="loading-container">
                            <div class="spinner"></div>
                            <p>"Loading roles..."</p>
                        </div>
                    }.into_view()
                } else {
                    let roles_list = roles.get();
                    if roles_list.is_empty() {
                        view! {
                            <div class="empty-state">
                                <div class="empty-icon">"🔐"</div>
                                <h3>"No roles found"</h3>
                                <p>"Create your first role to get started with access control"</p>
                            </div>
                        }.into_view()
                    } else {
                        view! {
                            <div class="roles-grid">
                                {roles_list.into_iter().map(|role| {
                                    let role_name = role.name.clone();
                                    let role_name_delete = role.name.clone();
                                    let role_description = role.description.clone();
                                    let role_for_edit = role.clone();
                                    let permissions_list = role.permissions.clone();
                                    let is_builtin = is_builtin_role(&role.name);
                                    let permission_count = permissions_list.len();

                                    view! {
                                        <div class="role-card" class:builtin=is_builtin>
                                            <div class="role-header">
                                                <div class="role-title">
                                                    <h3>{&role_name}</h3>
                                                    {if is_builtin {
                                                        view! {
                                                            <span class="builtin-badge">"Built-in"</span>
                                                        }.into_view()
                                                    } else {
                                                        view! {}.into_view()
                                                    }}
                                                </div>
                                                <div class="role-actions">
                                                    <button
                                                        class="btn-icon"
                                                        title="Edit Role"
                                                        on:click=move |_| set_selected_role.set(Some(role_for_edit.clone()))
                                                    >
                                                        "✏️"
                                                    </button>
                                                    {if !is_builtin {
                                                        view! {
                                                            <button
                                                                class="btn-icon delete-btn"
                                                                title="Delete Role"
                                                                on:click=move |_| {
                                                                    if web_sys::window()
                                                                        .unwrap()
                                                                        .confirm_with_message(&format!("Delete role '{}'? This action cannot be undone.", &role_name_delete))
                                                                        .unwrap_or(false)
                                                                    {
                                                                        delete_role(role_name_delete.clone());
                                                                    }
                                                                }
                                                            >
                                                                "🗑"
                                                            </button>
                                                        }.into_view()
                                                    } else {
                                                        view! {}.into_view()
                                                    }}
                                                </div>
                                            </div>

                                            <div class="role-description">
                                                <p>{&role_description}</p>
                                            </div>

                                            <div class="role-stats">
                                                <div class="stat">
                                                    <span class="stat-value">{permission_count}</span>
                                                    <span class="stat-label">"Permissions"</span>
                                                </div>
                                            </div>

                                            <div class="role-permissions-preview">
                                                <h4>"Permissions:"</h4>
                                                {if permissions_list.is_empty() {
                                                    view! {
                                                        <p class="no-permissions">"No permissions assigned"</p>
                                                    }.into_view()
                                                } else {
                                                    view! {
                                                        <div class="permissions-list">
                                                            {permissions_list.into_iter().map(|permission| {
                                                                view! {
                                                                    <div class="permission-item">
                                                                        <span class="permission-path">{&permission.path}</span>
                                                                        <div class="privileges-list">
                                                                            {permission.privileges.into_iter().map(|privilege| {
                                                                                view! {
                                                                                    <span class="privilege-badge">{privilege}</span>
                                                                                }
                                                                            }).collect_view()}
                                                                        </div>
                                                                    </div>
                                                                }
                                                            }).collect_view()}
                                                        </div>
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

            {move || selected_role.get().map(|role| view! {
                <RoleEditModal
                    role=role
                    on_close={
                        let set_selected_role = set_selected_role.clone();
                        move || set_selected_role.set(None)
                    }
                    on_save={
                        let set_selected_role = set_selected_role.clone();
                        let set_roles = set_roles.clone();
                        move || {
                            set_selected_role.set(None);
                            // Refresh roles list
                            spawn_local(async move {
                                if let Ok(roles_data) = api::get_roles().await {
                                    set_roles.set(roles_data);
                                }
                            });
                        }
                    }
                    on_error={
                        let set_error_message = set_error_message.clone();
                        move |msg| set_error_message.set(Some(msg))
                    }
                />
            })}
        </div>
    }
}

/// Create role form component
#[component]
pub fn CreateRoleForm<F, G>(
    on_success: F,
    on_error: G,
) -> impl IntoView
where
    F: Fn() + 'static,
    G: Fn(String) + 'static,
{
    let (role_name, set_role_name) = create_signal(String::new());
    let (description, set_description) = create_signal(String::new());
    let (creating, set_creating) = create_signal(false);

    // Wrap callbacks in Rc early so they can be cloned into closures
    let on_success_rc = std::rc::Rc::new(on_success);
    let on_error_rc = std::rc::Rc::new(on_error);

    let is_valid = move || {
        !role_name.get().trim().is_empty() && !description.get().trim().is_empty()
    };

    let on_success_clone = on_success_rc.clone();
    let on_error_clone = on_error_rc.clone();
    let submit_form = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();

        if !is_valid() {
            return;
        }

        let role = api::Role {
            name: role_name.get().trim().to_string(),
            description: description.get().trim().to_string(),
            permissions: Vec::new(),
        };

        set_creating.set(true);

        let on_success_inner = on_success_clone.clone();
        let on_error_inner = on_error_clone.clone();

        spawn_local(async move {
            match api::create_role(role).await {
                Ok(_) => {
                    on_success_inner();
                }
                Err(e) => {
                    on_error_inner(format!("Failed to create role: {}", e));
                }
            }
            set_creating.set(false);
        });
    };

    view! {
        <div class="create-role-form-container">
            <form class="create-role-form" on:submit=submit_form>
                <h2>"Create New Role"</h2>

                <div class="form-group">
                    <label for="role-name">"Role Name *"</label>
                    <input
                        type="text"
                        id="role-name"
                        prop:value=move || role_name.get()
                        on:input=move |ev| set_role_name.set(event_target_value(&ev))
                        placeholder="e.g., VMOperator, BackupManager"
                        required
                    />
                    <small>"Use a descriptive name that indicates the role's purpose"</small>
                </div>

                <div class="form-group">
                    <label for="description">"Description *"</label>
                    <textarea
                        id="description"
                        prop:value=move || description.get()
                        on:input=move |ev| set_description.set(event_target_value(&ev))
                        placeholder="Describe what this role is allowed to do..."
                        rows="3"
                        required
                    ></textarea>
                </div>

                <div class="info-box">
                    <p>
                        <strong>"Note:"</strong>
                        " After creating the role, you can assign specific permissions using the edit functionality."
                    </p>
                </div>

                <div class="form-actions">
                    <button
                        type="submit"
                        class="btn btn-primary"
                        disabled=move || creating.get() || !is_valid()
                    >
                        {move || if creating.get() { "Creating..." } else { "Create Role" }}
                    </button>
                </div>
            </form>
        </div>
    }
}

/// Permission matrix component showing all roles and their permissions
#[component]
pub fn PermissionMatrix(roles: Vec<api::Role>) -> impl IntoView {
    let roles_for_matrix = roles.clone();
    let roles_for_header = roles.clone();

    // Build a matrix of roles vs privileges
    let build_matrix = move || {
        let mut matrix: HashMap<String, HashMap<String, bool>> = HashMap::new();

        for role in &roles_for_matrix {
            let mut role_permissions: HashMap<String, bool> = HashMap::new();

            // Initialize all privileges to false
            for privilege in AVAILABLE_PRIVILEGES {
                role_permissions.insert(privilege.to_string(), false);
            }

            // Mark privileges that this role has
            for permission in &role.permissions {
                for privilege in &permission.privileges {
                    role_permissions.insert(privilege.clone(), true);
                }
            }

            matrix.insert(role.name.clone(), role_permissions);
        }

        matrix
    };

    view! {
        <div class="permission-matrix-container">
            <div class="matrix-header">
                <h3>"Permission Matrix"</h3>
                <p>"Visual overview of all roles and their assigned privileges"</p>
            </div>

            <div class="matrix-table-container">
                <table class="permission-matrix">
                    <thead>
                        <tr>
                            <th class="privilege-header">"Privilege"</th>
                            {roles_for_header.iter().map(|role| {
                                view! {
                                    <th class="role-header">
                                        <div class="role-header-content">
                                            <span>{&role.name}</span>
                                        </div>
                                    </th>
                                }
                            }).collect_view()}
                        </tr>
                    </thead>
                    <tbody>
                        {AVAILABLE_PRIVILEGES.iter().map(|privilege| {
                            let matrix = build_matrix();
                            view! {
                                <tr>
                                    <td class="privilege-name">
                                        <div class="privilege-info">
                                            <strong>{privilege.to_string()}</strong>
                                            <small>{get_privilege_description(privilege)}</small>
                                        </div>
                                    </td>
                                    {roles.iter().map(|role| {
                                        let has_privilege = matrix
                                            .get(&role.name)
                                            .map(|perms| perms.get(*privilege).copied().unwrap_or(false))
                                            .unwrap_or(false);

                                        view! {
                                            <td class="privilege-cell">
                                                <span class={format!("privilege-indicator {}", if has_privilege { "granted" } else { "denied" })}>
                                                    {if has_privilege { "[OK]" } else { "[X]" }}
                                                </span>
                                            </td>
                                        }
                                    }).collect_view()}
                                </tr>
                            }
                        }).collect_view()}
                    </tbody>
                </table>
            </div>
        </div>
    }
}

/// Role edit modal component
#[component]
pub fn RoleEditModal<F, G, H>(
    role: api::Role,
    on_close: F,
    on_save: G,
    on_error: H,
) -> impl IntoView
where
    F: Fn() + Clone + 'static,
    G: Fn() + 'static,
    H: Fn(String) + 'static,
{
    let (role_name, set_role_name) = create_signal(role.name.clone());
    let (description, set_description) = create_signal(role.description.clone());
    let (permissions, set_permissions) = create_signal(role.permissions.clone());
    let (saving, set_saving) = create_signal(false);

    // Wrap callbacks in Rc early
    let on_save_rc = std::rc::Rc::new(on_save);
    let on_error_rc = std::rc::Rc::new(on_error);

    // Add new permission
    let add_permission = move |_| {
        set_permissions.update(|perms| {
            perms.push(api::Permission {
                path: "/".to_string(),
                privileges: Vec::new(),
            });
        });
    };

    // Remove permission
    let remove_permission = move |index: usize| {
        set_permissions.update(|perms| {
            if index < perms.len() {
                perms.remove(index);
            }
        });
    };

    // Update permission path
    let update_permission_path = move |index: usize, path: String| {
        set_permissions.update(|perms| {
            if let Some(permission) = perms.get_mut(index) {
                permission.path = path;
            }
        });
    };

    // Toggle privilege for permission
    let toggle_privilege = move |perm_index: usize, privilege: String| {
        set_permissions.update(|perms| {
            if let Some(permission) = perms.get_mut(perm_index) {
                if let Some(pos) = permission.privileges.iter().position(|p| p == &privilege) {
                    permission.privileges.remove(pos);
                } else {
                    permission.privileges.push(privilege);
                }
            }
        });
    };

    let on_save_clone = on_save_rc.clone();
    let on_error_clone = on_error_rc.clone();
    let role_name_for_save = role.name.clone();
    let save_role = move |_| {
        let updated_role = api::Role {
            name: role_name.get(),
            description: description.get(),
            permissions: permissions.get(),
        };

        set_saving.set(true);

        let on_save_inner = on_save_clone.clone();
        let on_error_inner = on_error_clone.clone();
        let role_name_inner = role_name_for_save.clone();

        spawn_local(async move {
            match api::update_role(&role_name_inner, updated_role).await {
                Ok(_) => {
                    on_save_inner();
                }
                Err(e) => {
                    on_error_inner(format!("Failed to update role: {}", e));
                }
            }
            set_saving.set(false);
        });
    };

    let on_close_clone1 = on_close.clone();
    let on_close_clone2 = on_close.clone();

    view! {
        <div class="modal-overlay" on:click=move |_| on_close_clone1()>
            <div class="modal-content role-edit-modal" on:click=move |ev| ev.stop_propagation()>
                <div class="modal-header">
                    <h2>{format!("Edit Role: {}", role.name)}</h2>
                    <button class="modal-close" on:click=move |_| on_close_clone2()>"x"</button>
                </div>

                <div class="modal-body">
                    <div class="form-group">
                        <label>"Role Name"</label>
                        <input
                            type="text"
                            prop:value=move || role_name.get()
                            on:input=move |ev| set_role_name.set(event_target_value(&ev))
                            disabled=true
                        />
                        <small>"Role name cannot be changed after creation"</small>
                    </div>

                    <div class="form-group">
                        <label>"Description"</label>
                        <textarea
                            prop:value=move || description.get()
                            on:input=move |ev| set_description.set(event_target_value(&ev))
                            rows="3"
                        ></textarea>
                    </div>

                    <div class="permissions-section">
                        <div class="section-header">
                            <h3>"Permissions"</h3>
                            <button class="btn btn-secondary" on:click=add_permission>
                                "Add Permission"
                            </button>
                        </div>

                        <div class="permissions-list">
                            {move || permissions.get().into_iter().enumerate().map(|(index, permission)| {
                                let path_for_value = permission.path.clone();
                                let path_for_options = permission.path.clone();
                                let privileges_for_check = permission.privileges.clone();

                                view! {
                                    <div class="permission-editor">
                                        <div class="permission-header">
                                            <select
                                                prop:value=move || path_for_value.clone()
                                                on:change=move |ev| update_permission_path(index, event_target_value(&ev))
                                            >
                                                {COMMON_PATHS.iter().map(|path| {
                                                    let current_path = path_for_options.clone();
                                                    view! {
                                                        <option value={path.to_string()} selected=current_path == *path>
                                                            {path.to_string()}
                                                        </option>
                                                    }
                                                }).collect_view()}
                                            </select>
                                            <button
                                                class="btn-icon delete-btn"
                                                on:click=move |_| remove_permission(index)
                                            >
                                                "🗑"
                                            </button>
                                        </div>

                                        <div class="privileges-grid">
                                            {AVAILABLE_PRIVILEGES.iter().map(|privilege| {
                                                let privilege_str = privilege.to_string();
                                                let privilege_for_change = privilege_str.clone();
                                                let privilege_for_label = privilege_str.clone();
                                                let is_selected = privileges_for_check.contains(&privilege_str);

                                                view! {
                                                    <label class="privilege-checkbox">
                                                        <input
                                                            type="checkbox"
                                                            checked=is_selected
                                                            on:change=move |_| toggle_privilege(index, privilege_for_change.clone())
                                                        />
                                                        <span class="checkmark"></span>
                                                        <span class="privilege-label">{privilege_for_label}</span>
                                                    </label>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>
                                }
                            }).collect_view()}
                        </div>

                        {move || if permissions.get().is_empty() {
                            view! {
                                <div class="empty-permissions">
                                    <p>"No permissions assigned. Add permissions to grant access to resources."</p>
                                </div>
                            }.into_view()
                        } else {
                            view! {}.into_view()
                        }}
                    </div>
                </div>

                <div class="modal-footer">
                    <button class="btn btn-secondary" on:click={
                        let on_close = on_close.clone();
                        move |_| on_close()
                    }>
                        "Cancel"
                    </button>
                    <button
                        class="btn btn-primary"
                        on:click=save_role
                        disabled=move || saving.get()
                    >
                        {move || if saving.get() { "Saving..." } else { "Save Changes" }}
                    </button>
                </div>
            </div>
        </div>
    }
}

/// Get human-readable description for a privilege
fn get_privilege_description(privilege: &str) -> &'static str {
    match privilege {
        "VmAudit" => "View VM details and status",
        "VmConsole" => "Access VM console",
        "VmConfig" => "Modify VM configuration",
        "VmPowerMgmt" => "Start, stop, and restart VMs",
        "VmAllocate" => "Create and delete VMs",
        "VmMigrate" => "Migrate VMs between nodes",
        "VmSnapshot" => "Create and manage VM snapshots",
        "VmBackup" => "Create and restore VM backups",
        "DatastoreAudit" => "View datastore information",
        "DatastoreAllocate" => "Create and delete datastores",
        "DatastoreAllocateSpace" => "Allocate space in datastores",
        "PoolAudit" => "View pool information",
        "PoolAllocate" => "Create and manage pools",
        "SysAudit" => "View system information",
        "SysModify" => "Modify system configuration",
        "SysConsole" => "Access system console",
        "UserModify" => "Manage user accounts",
        "PermissionsModify" => "Modify permissions and roles",
        _ => "Unknown privilege",
    }
}