//! The per-VM tab bar, the same on every VM page so the user moves between a
//! VM's views without a trip back to the VM list (Proxmox keeps the guest's
//! tabs on every guest view). `active` names the tab of the current page.
use leptos::*;
use leptos_router::*;

/// (tab label, path below /vms/<id>, or an absolute path with {id})
const TABS: &[(&str, &str)] = &[
    ("Summary", ""),
    ("Hardware", "/hardware"),
    ("Console", "/console"),
    ("Remote Play", "/remote-play"),
    ("Snapshots", "/snapshots?vm={id}"),
];

pub fn tab_href(id: &str, path: &str) -> String {
    if path.starts_with('/') && path.contains("{id}") {
        path.replace("{id}", &urlencoding::encode(id))
    } else {
        format!("/vms/{}{}", urlencoding::encode(id), path)
    }
}

#[component]
pub fn VmTabs(#[prop(into)] vm_id: Signal<String>, active: &'static str) -> impl IntoView {
    view! {
        <nav class="tabs" id="vm-tabs" aria-label="VM views">
            {TABS
                .iter()
                .map(|&(label, path)| {
                    let cls = if label == active { "tab tab-active" } else { "tab" };
                    view! {
                        <A href=move || tab_href(&vm_id.get(), path) class=cls
                            attr:aria-current=move || (label == active).then_some("page")>{label}</A>
                    }
                })
                .collect_view()}
        </nav>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_stay_on_the_same_vm() {
        assert_eq!(tab_href("web 1", "/hardware"), "/vms/web%201/hardware");
        assert_eq!(tab_href("web", ""), "/vms/web");
        assert_eq!(tab_href("web", "/snapshots?vm={id}"), "/snapshots?vm=web");
    }
}
