//! The Podman/Docker picker shared by the app-container pages (Images,
//! Volumes, Container Networks, Stacks, Events).
//!
//! Those pages used to start on Podman unconditionally, so a node with only
//! Docker opened every one of them on "Podman isn't installed" although
//! Docker was right there. The picker asks the node which runtimes work
//! (`GET /api/containers/runtimes`) and starts on the first available one
//! (Podman first: the product prefers it when both exist). Runtimes that
//! don't work stay selectable (the page then shows why and what to do) but
//! are labelled "not installed".
use crate::api;
use leptos::*;

/// The runtimes these pages can manage, in preference order.
pub const OCI_RUNTIMES: [(&str, &str); 2] = [("podman", "Podman"), ("docker", "Docker")];

/// Pick the runtime a page starts on: the first runtime of `OCI_RUNTIMES`
/// the node reports available, else the first one that is at least
/// compiled in, else Podman (the page then explains what is missing).
pub fn pick_default(probes: &[api::ContainerRuntimeProbe]) -> String {
    let ok = |rt: &str, need_available: bool| {
        probes.iter().any(|p| {
            p.runtime == rt
                && if need_available {
                    p.available
                } else {
                    p.compiled
                }
        })
    };
    OCI_RUNTIMES
        .iter()
        .find(|(rt, _)| ok(rt, true))
        .or_else(|| OCI_RUNTIMES.iter().find(|(rt, _)| ok(rt, false)))
        .map(|(rt, _)| rt.to_string())
        .unwrap_or_else(|| "podman".to_string())
}

/// Label for an option: "Docker" or "Podman (not installed)".
pub fn option_label(rt: &str, label: &str, probes: &[api::ContainerRuntimeProbe]) -> String {
    match probes.iter().find(|p| p.runtime == rt) {
        Some(p) if !p.available => format!("{label} (not installed)"),
        _ => label.to_string(),
    }
}

/// The runtime signal of a page. It stays empty until the node answered,
/// so a page must not load anything while `runtime.get()` is empty.
pub fn use_oci_runtime() -> (
    ReadSignal<String>,
    WriteSignal<String>,
    ReadSignal<Vec<api::ContainerRuntimeProbe>>,
) {
    let (runtime, set_runtime) = create_signal(String::new());
    let (probes, set_probes) = create_signal(Vec::<api::ContainerRuntimeProbe>::new());
    spawn_local(async move {
        let list = api::get_container_runtimes(false)
            .await
            .map(|r| r.runtimes)
            .unwrap_or_default();
        set_runtime.set(pick_default(&list));
        set_probes.set(list);
    });
    (runtime, set_runtime, probes)
}

/// `<select>` bound to the runtime signal.
#[component]
pub fn OciRuntimeSelect(
    runtime: ReadSignal<String>,
    set_runtime: WriteSignal<String>,
    probes: ReadSignal<Vec<api::ContainerRuntimeProbe>>,
    #[prop(optional, into)] id: Option<String>,
) -> impl IntoView {
    view! {
        <select id=id aria-label="Container runtime" class="oci-runtime-select"
            prop:value=runtime
            on:change=move |ev| set_runtime.set(event_target_value(&ev))>
            {move || {
                let p = probes.get();
                let cur = runtime.get();
                OCI_RUNTIMES
                    .iter()
                    .map(|(rt, label)| {
                        let text = option_label(rt, label, &p);
                        view! { <option value=*rt selected=cur == *rt>{text}</option> }
                    })
                    .collect_view()
            }}
        </select>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(rt: &str, compiled: bool, available: bool) -> api::ContainerRuntimeProbe {
        api::ContainerRuntimeProbe {
            runtime: rt.into(),
            label: rt.into(),
            compiled,
            available,
            ..Default::default()
        }
    }

    #[test]
    fn starts_on_docker_when_podman_is_missing() {
        let p = vec![probe("podman", true, false), probe("docker", true, true)];
        assert_eq!(pick_default(&p), "docker");
        assert_eq!(
            option_label("podman", "Podman", &p),
            "Podman (not installed)"
        );
        assert_eq!(option_label("docker", "Docker", &p), "Docker");
    }

    #[test]
    fn prefers_podman_when_both_work() {
        let p = vec![probe("docker", true, true), probe("podman", true, true)];
        assert_eq!(pick_default(&p), "podman");
    }

    #[test]
    fn falls_back_to_podman_when_nothing_answers() {
        assert_eq!(pick_default(&[]), "podman");
        let p = vec![probe("podman", false, false), probe("docker", true, false)];
        assert_eq!(pick_default(&p), "docker");
    }
}
