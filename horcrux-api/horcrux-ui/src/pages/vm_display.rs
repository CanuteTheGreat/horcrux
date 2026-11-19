//! Per-VM display settings (protocol, video card, sound card), shared by the
//! Create VM form and the console page's "Display settings" panel.
use crate::api::passthrough as pt;
use horcrux_common::{DisplayProtocol, SoundDevice, VideoDevice, VmDisplay};
use leptos::*;

fn parse<T: serde::de::DeserializeOwned>(v: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(v.to_string())).ok()
}

fn wire<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Three selects bound to `display`. A sound card can be added with any
/// display; only SPICE carries its audio to Horcrux's console, so other
/// displays get a (non-blocking) warning.
#[component]
pub fn DisplaySettingsFields(display: RwSignal<VmDisplay>) -> impl IntoView {
    let protocols = [
        (DisplayProtocol::Vnc, "VNC (browser console via noVNC)"),
        (
            DisplayProtocol::Spice,
            "SPICE (remote-viewer: audio, clipboard, USB redirection)",
        ),
        (
            DisplayProtocol::None,
            "None (headless, serial console only)",
        ),
    ];
    let videos = [
        (VideoDevice::Std, "Standard VGA"),
        (VideoDevice::Virtio, "virtio-gpu"),
        (VideoDevice::Qxl, "QXL (best with SPICE, x86 only)"),
        (
            VideoDevice::Virtio3d,
            "virtio-gpu 3D (Vulkan + OpenGL on the host GPU, Linux guests)",
        ),
        (VideoDevice::None, "None"),
    ];
    let sounds = [
        (SoundDevice::None, "None"),
        (SoundDevice::IntelHda, "Intel HDA (ich6)"),
        (SoundDevice::Ich9IntelHda, "Intel HDA (ich9)"),
        (SoundDevice::Ac97, "AC97"),
    ];
    // What this host can do for 3D guests (loaded when the card is picked).
    let host3d = create_rw_signal(None::<Result<pt::Host3d, String>>);
    create_effect(move |_| {
        if display.get().video == VideoDevice::Virtio3d && host3d.get_untracked().is_none() {
            spawn_local(async move {
                host3d.set(Some(pt::host_3d().await.map_err(|e| e.message)));
            });
        }
    });
    view! {
        <div class="form-group">
            <label for="disp-protocol">"Display"</label>
            <select id="disp-protocol" on:change=move |ev| {
                if let Some(p) = parse::<DisplayProtocol>(&event_target_value(&ev)) {
                    display.update(|d| {
                        d.protocol = p;
                        if p == DisplayProtocol::None {
                            d.video = VideoDevice::None;
                        } else if d.video == VideoDevice::None {
                            d.video = VideoDevice::Std;
                        }
                    });
                }
            }>
                {protocols.into_iter().map(|(p, label)| view! {
                    <option value=wire(&p) selected=move || display.get().protocol == p>{label}</option>
                }).collect_view()}
            </select>
        </div>
        <div class="form-group">
            <label for="disp-video">"Video card"</label>
            <select id="disp-video" on:change=move |ev| {
                if let Some(v) = parse::<VideoDevice>(&event_target_value(&ev)) {
                    display.update(|d| d.video = v);
                }
            }>
                {videos.into_iter().map(|(v, label)| view! {
                    <option value=wire(&v) selected=move || display.get().video == v>{label}</option>
                }).collect_view()}
            </select>
        </div>
        {move || (display.get().video == VideoDevice::Virtio3d).then(|| view! {
            <fieldset class="form-group" id="disp-3d">
                <legend>"3D virtio-gpu"</legend>
                {move || match host3d.get() {
                    None => view! { <p class="form-help">"Checking this host..."</p> }.into_view(),
                    Some(Err(e)) => view! { <p class="form-help warning">"Host check failed: " {e}</p> }.into_view(),
                    Some(Ok(h)) => view! {
                        <div id="disp-3d-host">
                            {h.ready.then(|| view! {
                                <p class="form-help" id="disp-3d-ready">
                                    "This host can run 3D guests (QEMU " {h.qemu.version.clone().unwrap_or_default()} ")."
                                </p>
                            })}
                            {h.issues.iter().map(|i| view! {
                                <p class="form-help warning" data-reason=i.reason.clone()>
                                    {i.message.clone()} " " {i.hint.clone()}
                                </p>
                            }).collect_view()}
                            {h.warnings.iter().map(|w| view! { <p class="form-help">{w.clone()}</p> }).collect_view()}
                        </div>
                    }.into_view(),
                }}
                <label>
                    <input id="disp-3d-venus" type="checkbox"
                        prop:checked=move || display.get().gpu3d.venus
                        on:change=move |ev| {
                            let on = event_target_checked(&ev);
                            display.update(|d| d.gpu3d.venus = on);
                        }/>
                    " Vulkan (Venus)"
                </label>
                <label>
                    <input id="disp-3d-virgl" type="checkbox"
                        prop:checked=move || display.get().gpu3d.virgl
                        on:change=move |ev| {
                            let on = event_target_checked(&ev);
                            display.update(|d| d.gpu3d.virgl = on);
                        }/>
                    " OpenGL (virgl)"
                </label>
                <label for="disp-3d-gpu">"Host GPU"</label>
                <select id="disp-3d-gpu" on:change=move |ev| {
                    let v = event_target_value(&ev);
                    display.update(|d| d.gpu3d.render_node = (!v.is_empty()).then_some(v));
                }>
                    <option value="" selected=move || display.get().gpu3d.render_node.is_none()>
                        "Automatic (first suitable GPU)"
                    </option>
                    {move || host3d.get().and_then(|h| h.ok()).map(|h| h.gpus.into_iter().map(|g| {
                        let value = g.path.clone();
                        let label = format!(
                            "{} {}{}",
                            g.name.clone().or(g.driver.clone()).unwrap_or_else(|| g.path.clone()),
                            g.pci_address.clone().unwrap_or_default(),
                            g.reason.clone().map(|r| format!(" (not usable: {r})")).unwrap_or_default()
                        );
                        let v2 = value.clone();
                        view! {
                            <option value=value disabled=!g.suitable
                                selected=move || display.get().gpu3d.render_node.as_deref() == Some(v2.as_str())>
                                {label}
                            </option>
                        }
                    }).collect_view())}
                </select>
                <label for="disp-3d-hostmem">"Host-visible GPU memory (MiB)"</label>
                <input id="disp-3d-hostmem" type="number" min="64" max="65536"
                    placeholder=move || host3d.get().and_then(|h| h.ok()).map(|h| h.default_hostmem_mib.to_string()).unwrap_or_default()
                    prop:value=move || display.get().gpu3d.hostmem_mib.map(|m| m.to_string()).unwrap_or_default()
                    on:input=move |ev| {
                        let v = event_target_value(&ev);
                        display.update(|d| d.gpu3d.hostmem_mib = v.trim().parse::<u32>().ok());
                    }/>
                <p class="form-help" id="disp-3d-save-note">
                    "A VM with a 3D virtio-gpu cannot be saved or live-migrated: QEMU cannot save 3D virtio-gpu state and the guest driver refuses hibernation, so it shuts down cleanly when the host stops. Shown through VNC (the Horcrux console); not with SPICE."
                </p>
            </fieldset>
        })}
        <div class="form-group">
            <label for="disp-sound">"Sound card"</label>
            <select id="disp-sound"
                on:change=move |ev| {
                    if let Some(s) = parse::<SoundDevice>(&event_target_value(&ev)) {
                        display.update(|d| d.sound = s);
                    }
                }>
                {sounds.into_iter().map(|(s, label)| view! {
                    <option value=wire(&s) selected=move || display.get().sound == s>{label}</option>
                }).collect_view()}
            </select>
            {move || {
                let d = display.get();
                (d.sound != SoundDevice::None && d.protocol != DisplayProtocol::Spice).then(|| view! {
                    <p class="form-help warning" id="disp-sound-warning">
                        "Horcrux's console only carries audio over SPICE: with this display the sound card exists in the guest, but you won't hear it in the console. Use SPICE, or RDP or another remote-access tool inside the guest, to get the audio."
                    </p>
                })
            }}
        </div>
        <div class="form-group">
            <label>
                <input id="disp-relative-mouse" type="checkbox"
                    prop:checked=move || display.get().relative_mouse
                    on:change=move |ev| {
                        let on = event_target_checked(&ev);
                        display.update(|d| d.relative_mouse = on);
                    }/>
                " Relative mouse only (no USB tablet; for old guests without USB support)"
            </label>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_values_are_the_api_wire_names() {
        assert_eq!(wire(&SoundDevice::Ich9IntelHda), "ich9-intel-hda");
        assert_eq!(wire(&DisplayProtocol::Spice), "spice");
        assert_eq!(parse::<VideoDevice>("qxl"), Some(VideoDevice::Qxl));
        assert_eq!(wire(&VideoDevice::Virtio3d), "virtio-3d");
        assert_eq!(
            parse::<VideoDevice>("virtio-3d"),
            Some(VideoDevice::Virtio3d)
        );
        assert_eq!(parse::<SoundDevice>("ac97"), Some(SoundDevice::Ac97));
    }
}
