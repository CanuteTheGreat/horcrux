//! VM templates: a stopped VM's disk + hardware saved as a base for new VMs
//! (Proxmox: "Convert to template", then "Clone"). Templates are made from
//! stopped VMs only (a disk copied while the guest writes is torn), deployed
//! as full or linked clones, and deleted when no linked clone needs them.
use crate::api::{
    clone_template, create_template, delete_template, get_templates, get_vms,
    CreateTemplateRequest, VmTemplate,
};
use horcrux_common::{VmConfig, VmStatus};
use leptos::*;
use leptos_router::use_query_map;

/// Why a VM can't become a template right now (`None` = it can).
pub fn template_blocker(vm: &VmConfig) -> Option<&'static str> {
    match vm.status {
        VmStatus::Running | VmStatus::Paused => Some("running: shut it down first"),
        _ if vm.disks.is_empty() => Some("has no disk"),
        _ => None,
    }
}

/// "1 template", "3 templates", "2 of 5 templates".
pub fn count_label(shown: usize, total: usize) -> String {
    let noun = if total == 1 { "template" } else { "templates" };
    if shown == total {
        format!("{total} {noun}")
    } else {
        format!("{shown} of {total} {noun}")
    }
}

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

#[component]
pub fn TemplateManagerPage() -> impl IntoView {
    let (templates, set_templates) = create_signal(Vec::<VmTemplate>::new());
    let (vms, set_vms) = create_signal(Vec::<VmConfig>::new());
    let (loaded, set_loaded) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    // (message, link to the new VM)
    let (notice, set_notice) = create_signal(None::<(String, Option<String>)>);
    let (search_query, set_search_query) = create_signal(String::new());
    let (show_create, set_show_create) = create_signal(false);
    let (selected_template, set_selected_template) = create_signal(None::<VmTemplate>);

    let (template_name, set_template_name) = create_signal(String::new());
    let (template_description, set_template_description) = create_signal(String::new());
    let (source_vm_id, set_source_vm_id) = create_signal(String::new());
    let (clone_name, set_clone_name) = create_signal(String::new());
    let (clone_mode, set_clone_mode) = create_signal("full".to_string());
    let (busy, set_busy) = create_signal(false);
    let (dialog_error, set_dialog_error) = create_signal(None::<String>);

    let load_templates = move || {
        spawn_local(async move {
            match get_templates().await {
                Ok(list) => {
                    set_templates.set(list);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(format!("Could not load templates: {}", e.message))),
            }
            set_loaded.set(true);
        });
    };
    let load_vms = move || {
        spawn_local(async move {
            match get_vms().await {
                Ok(list) => set_vms.set(list),
                Err(e) => set_error.set(Some(format!("Could not load VMs: {}", e.message))),
            }
        });
    };
    load_templates();
    load_vms();

    let open_create = move |vm_id: String| {
        set_template_name.set(String::new());
        set_template_description.set(String::new());
        set_source_vm_id.set(vm_id);
        set_dialog_error.set(None);
        set_show_create.set(true);
    };

    // `?vm=<id>` (the VM list's "Make template"): open the dialog on that VM.
    let query = use_query_map();
    create_effect(move |_| {
        if let Some(v) = query.get().get("vm").cloned() {
            if !v.is_empty() {
                open_create(v);
            }
        }
    });

    let vm_name = move |vm_id: &str| -> String {
        vms.with(|v| {
            v.iter()
                .find(|vm| vm.id == vm_id)
                .map(|vm| vm.name.clone())
                .unwrap_or_else(|| vm_id.to_string())
        })
    };

    let filtered = move || {
        let q = search_query.get().to_lowercase();
        templates
            .get()
            .into_iter()
            .filter(|t| {
                q.is_empty()
                    || t.name.to_lowercase().contains(&q)
                    || t.description
                        .as_ref()
                        .is_some_and(|d| d.to_lowercase().contains(&q))
                    || t.source_vm_id.to_lowercase().contains(&q)
            })
            .collect::<Vec<_>>()
    };

    let submit_create = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let name = template_name.get().trim().to_string();
        let vm_id = source_vm_id.get();
        if vm_id.is_empty() {
            set_dialog_error.set(Some("Choose the VM to make a template of".into()));
            return;
        }
        if name.is_empty() {
            set_dialog_error.set(Some("The template needs a name".into()));
            return;
        }
        if let Some(why) = vms.with(|v| v.iter().find(|x| x.id == vm_id).and_then(template_blocker))
        {
            set_dialog_error.set(Some(format!("{} is {why}", vm_name(&vm_id))));
            return;
        }
        let desc = template_description.get();
        let request = CreateTemplateRequest {
            name: name.clone(),
            description: (!desc.trim().is_empty()).then_some(desc),
            source_vm_id: vm_id.clone(),
        };
        set_busy.set(true);
        spawn_local(async move {
            match create_template(request).await {
                Ok(_) => {
                    set_show_create.set(false);
                    set_notice.set(Some((
                        format!("Template {name} created from {}", vm_name(&vm_id)),
                        None,
                    )));
                    load_templates();
                }
                Err(e) => set_dialog_error.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };

    let delete_action = move |t: VmTemplate| {
        if confirm(&format!(
            "Delete template {}? Its disk image is removed; VMs already deployed from it as full clones keep working.",
            t.name
        )) {
            set_notice.set(None);
            spawn_local(async move {
                match delete_template(&t.id).await {
                    Ok(_) => {
                        set_notice.set(Some((format!("Template {} deleted", t.name), None)));
                        load_templates();
                    }
                    Err(e) => set_error.set(Some(format!(
                        "Deleting template {} failed: {}",
                        t.name, e.message
                    ))),
                }
            });
        }
    };

    let open_clone = move |t: VmTemplate| {
        set_clone_name.set(format!("{}-vm", t.name));
        set_clone_mode.set("full".to_string());
        set_dialog_error.set(None);
        set_selected_template.set(Some(t));
    };

    let submit_clone = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let Some(t) = selected_template.get() else {
            return;
        };
        let name = clone_name.get().trim().to_string();
        if name.is_empty() {
            set_dialog_error.set(Some("The new VM needs a name".into()));
            return;
        }
        let mode = clone_mode.get();
        set_busy.set(true);
        spawn_local(async move {
            match clone_template(&t.id, name.clone(), &mode).await {
                Ok(v) => {
                    set_selected_template.set(None);
                    let link = v["new_vm_id"].as_str().map(|id| format!("/vms/{id}"));
                    set_notice.set(Some((
                        format!("VM {name} created from template {}", t.name),
                        link,
                    )));
                    load_vms();
                }
                Err(e) => set_dialog_error.set(Some(e.message)),
            }
            set_busy.set(false);
        });
    };

    view! {
        <div class="template-manager">
            <div class="page-header">
                <h1>"VM Templates"</h1>
                <div class="header-actions actions">
                    <a class="btn btn-secondary" href="/clones">"Clone jobs"</a>
                    <button id="template-create-btn" class="btn btn-primary"
                        on:click=move |_| open_create(String::new())>"Create Template"</button>
                </div>
            </div>
            <p class="hint">
                "A template is a stopped VM's disk and hardware saved as a base for new VMs. "
                "Deploy it as a full clone (independent copy) or a linked clone (stores only its own changes)."
            </p>

            {move || notice.get().map(|(msg, link)| view! {
                <div class="alert alert-success" id="template-notice">
                    {msg}
                    {link.map(|href| view! { " " <a href=href id="template-notice-link">"Open the VM"</a> })}
                </div>
            })}
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="template-error">{e}</div> })}

            <div class="form-group" style="max-width:32rem">
                <label for="template-search">"Search"</label>
                <input id="template-search" type="search" placeholder="Name, description or source VM"
                    on:input=move |ev| set_search_query.set(event_target_value(&ev))/>
            </div>

            {move || {
                let list = filtered();
                let total = templates.with(|t| t.len());
                if !loaded.get() {
                    view! { <p class="loading">"Loading templates..."</p> }.into_view()
                } else if total == 0 {
                    view! {
                        <div class="no-data empty-state" id="templates-empty">
                            <p>"No templates yet."</p>
                            <p>"Shut down a VM that is set up the way new VMs should start, then make a template of it."</p>
                            <button class="btn btn-primary" on:click=move |_| open_create(String::new())>"Create Template"</button>
                        </div>
                    }.into_view()
                } else {
                    view! {
                        <p class="hint" id="template-count">{count_label(list.len(), total)}</p>
                        <table class="data-table" id="template-table">
                            <thead>
                                <tr>
                                    <th>"Name"</th>
                                    <th>"Source VM"</th>
                                    <th>"Hardware"</th>
                                    <th>"Disk"</th>
                                    <th>"Created"</th>
                                    <th>"Actions"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {list.into_iter().map(|t| {
                                    let t_clone = t.clone();
                                    let t_delete = t.clone();
                                    let src = t.source_vm_id.clone();
                                    let src_label = vm_name(&src);
                                    let src_exists = vms.with(|v| v.iter().any(|x| x.id == src));
                                    let disk = if t.virtual_size_bytes > 0 {
                                        format!(
                                            "{} ({} used)",
                                            crate::api::human_bytes(t.virtual_size_bytes),
                                            crate::api::human_bytes(t.size_bytes)
                                        )
                                    } else {
                                        crate::api::human_bytes(t.size_bytes)
                                    };
                                    let hw = format!(
                                        "{} vCPU, {} MB",
                                        t.config["cpus"].as_u64().unwrap_or(0),
                                        t.config["memory"].as_u64().unwrap_or(0)
                                    );
                                    view! {
                                        <tr class="template-row" data-template-id=t.id.clone() data-template-name=t.name.clone()>
                                            <td title=t.storage_location.clone()>
                                                <strong>{t.name.clone()}</strong>
                                                {t.description.clone().map(|d| view! { <div class="hint">{d}</div> })}
                                            </td>
                                            <td>{if src_exists {
                                                view! { <a href=format!("/vms/{src}")>{src_label}</a> }.into_view()
                                            } else {
                                                view! { <span class="hint">{src_label} " (deleted)"</span> }.into_view()
                                            }}</td>
                                            <td>{hw}</td>
                                            <td class="template-size">{disk}</td>
                                            <td>{t.created_at.clone()}</td>
                                            <td class="actions">
                                                <button class="btn btn-sm btn-primary template-deploy"
                                                    title="Create a new VM from this template"
                                                    on:click=move |_| open_clone(t_clone.clone())>"New VM"</button>
                                                <button class="btn btn-sm btn-danger template-delete"
                                                    title="Delete this template"
                                                    on:click=move |_| delete_action(t_delete.clone())>"Delete"</button>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_view()
                }
            }}

            // Create Template dialog
            {move || show_create.get().then(|| view! {
                <div class="modal-overlay" id="template-create-dialog">
                    <div class="modal-content">
                        <form on:submit=submit_create>
                            <div class="modal-header"><h2>"Create Template"</h2></div>
                            <div class="modal-body">
                                {move || dialog_error.get().map(|e| view! { <div class="alert alert-error" id="template-create-error">{e}</div> })}
                                <div class="form-group">
                                    <label for="template-source-vm">"From VM"</label>
                                    <select id="template-source-vm"
                                        on:change=move |ev| set_source_vm_id.set(event_target_value(&ev))>
                                        <option value="" selected=move || source_vm_id.get().is_empty()>"Choose a stopped VM..."</option>
                                        {move || vms.get().into_iter().map(|vm| {
                                            let blocker = template_blocker(&vm);
                                            let id = vm.id.clone();
                                            let label = match blocker {
                                                Some(why) => format!("{} ({})", vm.name, why),
                                                None => vm.name.clone(),
                                            };
                                            view! {
                                                <option value=vm.id.clone() disabled=blocker.is_some()
                                                    selected=move || source_vm_id.get() == id>{label}</option>
                                            }
                                        }).collect_view()}
                                    </select>
                                    <p class="hint">"The VM is not changed; its disk is copied into the template."</p>
                                </div>
                                <div class="form-group">
                                    <label for="template-name">"Template name"</label>
                                    <input id="template-name" type="text" required placeholder="debian-13-base"
                                        prop:value=template_name
                                        on:input=move |ev| set_template_name.set(event_target_value(&ev))/>
                                </div>
                                <div class="form-group">
                                    <label for="template-description">"Description"</label>
                                    <input id="template-description" type="text"
                                        prop:value=template_description
                                        on:input=move |ev| set_template_description.set(event_target_value(&ev))/>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button type="button" class="btn btn-secondary"
                                    on:click=move |_| set_show_create.set(false)>"Cancel"</button>
                                <button id="template-create-submit" type="submit" class="btn btn-primary" disabled=busy>
                                    {move || if busy.get() { "Creating..." } else { "Create Template" }}
                                </button>
                            </div>
                        </form>
                    </div>
                </div>
            })}

            // New VM from template dialog
            {move || selected_template.get().map(|t| view! {
                <div class="modal-overlay" id="template-clone-dialog">
                    <div class="modal-content">
                        <form on:submit=submit_clone>
                            <div class="modal-header"><h2>{format!("New VM from {}", t.name)}</h2></div>
                            <div class="modal-body">
                                {move || dialog_error.get().map(|e| view! { <div class="alert alert-error" id="template-clone-error">{e}</div> })}
                                <div class="form-group">
                                    <label for="template-clone-name">"VM name"</label>
                                    <input id="template-clone-name" type="text" required
                                        prop:value=clone_name
                                        on:input=move |ev| set_clone_name.set(event_target_value(&ev))/>
                                </div>
                                <div class="form-group">
                                    <label for="template-clone-mode">"Mode"</label>
                                    <select id="template-clone-mode"
                                        on:change=move |ev| set_clone_mode.set(event_target_value(&ev))>
                                        <option value="full" selected=move || clone_mode.get() == "full">"Full clone (independent copy of the template disk)"</option>
                                        <option value="linked" selected=move || clone_mode.get() == "linked">"Linked clone (stores only its own changes)"</option>
                                    </select>
                                    {move || (clone_mode.get() == "linked").then(|| view! {
                                        <p class="hint" id="template-clone-linked-help">
                                            "A linked clone reads unchanged data from the template disk, so the template can't be deleted while the clone exists."
                                        </p>
                                    })}
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button type="button" class="btn btn-secondary"
                                    on:click=move |_| set_selected_template.set(None)>"Cancel"</button>
                                <button id="template-clone-submit" type="submit" class="btn btn-primary" disabled=busy>
                                    {move || if busy.get() { "Creating..." } else { "Create VM" }}
                                </button>
                            </div>
                        </form>
                    </div>
                </div>
            })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm(status: VmStatus, disks: usize) -> VmConfig {
        let mut v: VmConfig = serde_json::from_value(serde_json::json!({
            "id": "v1", "name": "web", "hypervisor": "qemu", "memory": 512, "cpus": 1,
            "disk_size": 1, "status": "stopped", "architecture": "x86_64"
        }))
        .unwrap();
        v.status = status;
        v.disks = (0..disks)
            .map(|i| {
                serde_json::from_value(serde_json::json!({
                    "path": format!("/d{i}.qcow2"), "size_gb": 1, "disk_type": "virtio", "cache": ""
                }))
                .unwrap()
            })
            .collect();
        v
    }

    #[test]
    fn only_stopped_vms_with_a_disk_become_templates() {
        assert_eq!(template_blocker(&vm(VmStatus::Stopped, 1)), None);
        assert!(template_blocker(&vm(VmStatus::Running, 1))
            .unwrap()
            .contains("shut it down"));
        assert!(template_blocker(&vm(VmStatus::Paused, 1)).is_some());
        assert_eq!(
            template_blocker(&vm(VmStatus::Stopped, 0)),
            Some("has no disk")
        );
    }

    #[test]
    fn counts_are_pluralised() {
        assert_eq!(count_label(1, 1), "1 template");
        assert_eq!(count_label(3, 3), "3 templates");
        assert_eq!(count_label(1, 4), "1 of 4 templates");
        assert_eq!(count_label(0, 1), "0 of 1 template");
    }
}
