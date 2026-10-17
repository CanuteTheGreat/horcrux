//! Per-VM display settings (protocol, video card, sound card), shared by the
//! Create VM form and the console page's "Display settings" panel.
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

/// Three selects bound to `display`. Sound is only offered with SPICE (the
/// audio travels over SPICE's playback/record channels).
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
        (VideoDevice::None, "None"),
    ];
    let sounds = [
        (SoundDevice::None, "None"),
        (SoundDevice::IntelHda, "Intel HDA (ich6)"),
        (SoundDevice::Ich9IntelHda, "Intel HDA (ich9)"),
        (SoundDevice::Ac97, "AC97"),
    ];
    view! {
        <div class="form-group">
            <label for="disp-protocol">"Display"</label>
            <select id="disp-protocol" on:change=move |ev| {
                if let Some(p) = parse::<DisplayProtocol>(&event_target_value(&ev)) {
                    display.update(|d| {
                        d.protocol = p;
                        if p != DisplayProtocol::Spice {
                            d.sound = SoundDevice::None;
                        }
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
        <div class="form-group">
            <label for="disp-sound">"Sound card (over SPICE)"</label>
            <select id="disp-sound"
                disabled=move || display.get().protocol != DisplayProtocol::Spice
                on:change=move |ev| {
                    if let Some(s) = parse::<SoundDevice>(&event_target_value(&ev)) {
                        display.update(|d| d.sound = s);
                    }
                }>
                {sounds.into_iter().map(|(s, label)| view! {
                    <option value=wire(&s) selected=move || display.get().sound == s>{label}</option>
                }).collect_view()}
            </select>
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
        assert_eq!(parse::<SoundDevice>("ac97"), Some(SoundDevice::Ac97));
    }
}
