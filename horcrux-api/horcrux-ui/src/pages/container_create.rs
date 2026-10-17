//! "Create Container" form (/containers/create), reached from the
//! Containers page's "Create Container" button.
use crate::api;
use leptos::*;
use leptos_router::*;

/// Split a textarea into non-empty trimmed lines.
pub fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Build the API request from the raw form values. Kept free of any
/// browser APIs so it can be unit-tested natively.
#[allow(clippy::too_many_arguments)]
pub fn build_request(
    name: &str,
    runtime: &str,
    image: &str,
    memory: &str,
    cpus: &str,
    env: &str,
    ports: &str,
    volumes: &str,
    command: &str,
) -> Result<api::CreateContainerRequest, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Name is required".to_string());
    }
    let image = image.trim();
    if image.is_empty() {
        return Err("Image is required".to_string());
    }
    let memory: u64 = memory
        .trim()
        .parse()
        .map_err(|_| "Memory must be a whole number of MB".to_string())?;
    let cpus: u32 = cpus
        .trim()
        .parse()
        .map_err(|_| "CPUs must be a whole number".to_string())?;
    if memory == 0 || cpus == 0 {
        return Err("Memory and CPUs must be greater than zero".to_string());
    }
    let oci = matches!(runtime, "docker" | "podman");
    let (env, ports, volumes, command) = (
        lines(env),
        lines(ports),
        lines(volumes),
        command
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>(),
    );
    if !oci && (!env.is_empty() || !ports.is_empty() || !volumes.is_empty() || !command.is_empty())
    {
        return Err(
            "Environment, ports, volumes and command are only supported for Docker and Podman"
                .to_string(),
        );
    }
    Ok(api::CreateContainerRequest {
        id: name.to_string(),
        name: name.to_string(),
        runtime: runtime.to_string(),
        memory,
        cpus,
        rootfs: image.to_string(),
        status: "stopped".to_string(),
        env,
        ports,
        volumes,
        command,
    })
}

/// (value, label) of every container runtime, in menu order.
const RUNTIMES: [(&str, &str); 5] = [
    ("docker", "Docker"),
    ("podman", "Podman"),
    ("lxc", "LXC"),
    ("lxd", "LXD"),
    ("incus", "Incus"),
];

#[component]
pub fn ContainerCreate() -> impl IntoView {
    let (name, set_name) = create_signal(String::new());
    let (runtime, set_runtime) = create_signal("docker".to_string());
    let (image, set_image) = create_signal(String::new());
    let (memory, set_memory) = create_signal("512".to_string());
    let (cpus, set_cpus) = create_signal("1".to_string());
    let (env, set_env) = create_signal(String::new());
    let (ports, set_ports) = create_signal(String::new());
    let (volumes, set_volumes) = create_signal(String::new());
    let (command, set_command) = create_signal(String::new());
    let (start_after, set_start_after) = create_signal(true);
    let (creating, set_creating) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);

    let navigate = use_navigate();

    // Default to the first runtime this server was built with (docker is
    // not there with USE=-docker).
    let features = crate::features::use_features();
    create_effect(move |_| {
        if features.loaded() && !features.has(&runtime.get_untracked()) {
            if let Some((v, _)) = RUNTIMES.iter().find(|(v, _)| features.has(v)) {
                set_runtime.set(v.to_string());
            }
        }
    });

    let submit = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        set_error.set(None);
        let req = match build_request(
            &name.get(),
            &runtime.get(),
            &image.get(),
            &memory.get(),
            &cpus.get(),
            &env.get(),
            &ports.get(),
            &volumes.get(),
            &command.get(),
        ) {
            Ok(r) => r,
            Err(e) => {
                set_error.set(Some(e));
                return;
            }
        };
        set_creating.set(true);
        let start = start_after.get();
        let navigate = navigate.clone();
        spawn_local(async move {
            match api::create_container(&req).await {
                Ok(created) => {
                    if start {
                        if let Err(e) = api::start_container(&created.id).await {
                            set_creating.set(false);
                            set_error.set(Some(format!(
                                "Container created, but starting it failed: {}",
                                e.message
                            )));
                            return;
                        }
                    }
                    navigate("/containers", Default::default());
                }
                Err(e) => {
                    set_creating.set(false);
                    set_error.set(Some(e.message));
                }
            }
        });
    };

    let oci = move || matches!(runtime.get().as_str(), "docker" | "podman");

    view! {
        <div class="container-create">
            <div class="page-header">
                <h1>"Create Container"</h1>
                <div class="header-actions">
                    <a href="/containers" class="btn">"Cancel"</a>
                </div>
            </div>

            {move || error.get().map(|e| view! { <div class="alert alert-error" id="create-container-error">{e}</div> })}

            <form id="create-container-form" on:submit=submit class="form">
                <div class="form-group">
                    <label for="ct-name">"Name"</label>
                    <input id="ct-name" name="name" type="text" required placeholder="web-1"
                        prop:value=name
                        on:input=move |ev| set_name.set(event_target_value(&ev))/>
                </div>
                <div class="form-group">
                    <label for="ct-runtime">"Runtime"</label>
                    // Only the runtimes compiled into the server (USE flags).
                    <select id="ct-runtime" name="runtime"
                        prop:value=runtime
                        on:change=move |ev| set_runtime.set(event_target_value(&ev))>
                        {move || {
                            RUNTIMES
                                .iter()
                                .filter(|(v, _)| features.has(v))
                                .map(|(v, label)| {
                                    view! { <option value=*v selected=move || runtime.get() == *v>{*label}</option> }
                                })
                                .collect_view()
                        }}
                    </select>
                </div>
                <div class="form-group">
                    <label for="ct-image">{move || if oci() { "Image" } else { "Template / rootfs" }}</label>
                    <input id="ct-image" name="image" type="text" required placeholder="nginx:alpine"
                        prop:value=image
                        on:input=move |ev| set_image.set(event_target_value(&ev))/>
                </div>
                <div class="form-row">
                    <div class="form-group">
                        <label for="ct-memory">"Memory (MB)"</label>
                        <input id="ct-memory" name="memory" type="number" min="1"
                            prop:value=memory
                            on:input=move |ev| set_memory.set(event_target_value(&ev))/>
                    </div>
                    <div class="form-group">
                        <label for="ct-cpus">"CPUs"</label>
                        <input id="ct-cpus" name="cpus" type="number" min="1"
                            prop:value=cpus
                            on:input=move |ev| set_cpus.set(event_target_value(&ev))/>
                    </div>
                </div>
                <Show when=oci>
                    <div class="form-group">
                        <label for="ct-ports">"Published ports (one per line, host:container)"</label>
                        <textarea id="ct-ports" name="ports" rows="2" placeholder="8080:80"
                            prop:value=ports
                            on:input=move |ev| set_ports.set(event_target_value(&ev))></textarea>
                    </div>
                    <div class="form-group">
                        <label for="ct-env">"Environment (one KEY=value per line)"</label>
                        <textarea id="ct-env" name="env" rows="3"
                            prop:value=env
                            on:input=move |ev| set_env.set(event_target_value(&ev))></textarea>
                    </div>
                    <div class="form-group">
                        <label for="ct-volumes">"Volumes (one source:/target per line)"</label>
                        <textarea id="ct-volumes" name="volumes" rows="2"
                            prop:value=volumes
                            on:input=move |ev| set_volumes.set(event_target_value(&ev))></textarea>
                    </div>
                    <div class="form-group">
                        <label for="ct-command">"Command (optional, overrides the image CMD)"</label>
                        <input id="ct-command" name="command" type="text" placeholder="sleep 3600"
                            prop:value=command
                            on:input=move |ev| set_command.set(event_target_value(&ev))/>
                    </div>
                </Show>
                <div class="form-group">
                    <label>
                        <input id="ct-start" type="checkbox" prop:checked=start_after
                            on:change=move |ev| set_start_after.set(event_target_checked(&ev))/>
                        " Start after creating"
                    </label>
                </div>
                <div class="form-actions">
                    <button id="ct-submit" type="submit" class="btn btn-primary" disabled=creating>
                        {move || if creating.get() { "Creating..." } else { "Create Container" }}
                    </button>
                </div>
            </form>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_docker_request_from_form_values() {
        let r = build_request(
            " web-1 ",
            "docker",
            "nginx:alpine",
            "256",
            "2",
            "A=1\n\n B=2 ",
            "8080:80",
            "data:/data",
            "sleep 60",
        )
        .unwrap();
        assert_eq!(r.id, "web-1");
        assert_eq!(r.name, "web-1");
        assert_eq!(r.rootfs, "nginx:alpine");
        assert_eq!((r.memory, r.cpus), (256, 2));
        assert_eq!(r.env, vec!["A=1", "B=2"]);
        assert_eq!(r.ports, vec!["8080:80"]);
        assert_eq!(r.volumes, vec!["data:/data"]);
        assert_eq!(r.command, vec!["sleep", "60"]);
        // Serialized shape is exactly what POST /api/containers accepts.
        let json = serde_json::to_value(&r).unwrap();
        let cfg: horcrux_common::ContainerConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.runtime, horcrux_common::ContainerRuntime::Docker);
        assert_eq!(cfg.options.ports, vec!["8080:80"]);
        assert_eq!(cfg.options.command, vec!["sleep", "60"]);
    }

    #[test]
    fn rejects_missing_or_invalid_fields() {
        let ok = |n: &str, img: &str, mem: &str, cpu: &str| {
            build_request(n, "docker", img, mem, cpu, "", "", "", "")
        };
        assert!(ok("", "nginx", "1", "1").is_err());
        assert!(ok("a", " ", "1", "1").is_err());
        assert!(ok("a", "nginx", "lots", "1").is_err());
        assert!(ok("a", "nginx", "0", "1").is_err());
        assert!(build_request("a", "lxc", "ubuntu", "512", "1", "A=1", "", "", "").is_err());
        assert!(build_request("a", "lxc", "ubuntu", "512", "1", "", "", "", "").is_ok());
    }
}
