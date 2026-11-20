//! Node settings of the Horcrux Console (`[console]`, GET/PUT
//! /api/console/settings). Route: `/system/console`.
use crate::api;
use horcrux_common::console::{
    ConsoleProfile, ConsoleSettings, EncoderPreference, ProfileSettings,
};
use leptos::*;

fn num<T: std::str::FromStr + Copy>(ev: &leptos::ev::Event, old: T) -> T {
    event_target_value(ev).trim().parse().unwrap_or(old)
}

#[component]
fn ProfileFields(name: &'static str, sig: RwSignal<ConsoleSettings>) -> impl IntoView {
    let get = move || {
        let s = sig.get();
        match name {
            "gaming" => s.gaming,
            "console" => s.console,
            _ => s.desktop,
        }
    };
    let set = move |f: &dyn Fn(&mut ProfileSettings)| {
        sig.update(|s| {
            let p = match name {
                "gaming" => &mut s.gaming,
                "console" => &mut s.console,
                _ => &mut s.desktop,
            };
            f(p)
        })
    };
    view! {
        <fieldset class="hxc-profile" id=format!("hxc-profile-{name}")>
            <legend>{format!("Profile: {name}")}</legend>
            <label>"Max frames per second"
                <input type="number" min="1" max="360" name=format!("{name}-max_fps")
                    prop:value=move || get().max_fps.to_string()
                    on:change=move |ev| { let v = num(&ev, get().max_fps); set(&|p| p.max_fps = v) }/>
            </label>
            <label>"Bitrate (kbit/s)"
                <input type="number" min="100" name=format!("{name}-bitrate_kbps")
                    prop:value=move || get().bitrate_kbps.to_string()
                    on:change=move |ev| { let v = num(&ev, get().bitrate_kbps); set(&|p| p.bitrate_kbps = v) }/>
            </label>
            <label>"Max bitrate (kbit/s)"
                <input type="number" min="100" name=format!("{name}-max_bitrate_kbps")
                    prop:value=move || get().max_bitrate_kbps.to_string()
                    on:change=move |ev| { let v = num(&ev, get().max_bitrate_kbps); set(&|p| p.max_bitrate_kbps = v) }/>
            </label>
            <label>"Keyframe interval (s)"
                <input type="number" min="1" max="600" name=format!("{name}-keyframe_interval_secs")
                    prop:value=move || get().keyframe_interval_secs.to_string()
                    on:change=move |ev| { let v = num(&ev, get().keyframe_interval_secs); set(&|p| p.keyframe_interval_secs = v) }/>
            </label>
            <label>"Idle refresh (frames/s, 0 = off)"
                <input type="number" min="0" name=format!("{name}-idle_refresh_fps")
                    prop:value=move || get().idle_refresh_fps.to_string()
                    on:change=move |ev| { let v = num(&ev, get().idle_refresh_fps); set(&|p| p.idle_refresh_fps = v) }/>
            </label>
            <label class="checkbox">
                <input type="checkbox" name=format!("{name}-fixed_rate") prop:checked=move || get().fixed_rate
                    on:change=move |ev| { let v = event_target_checked(&ev); set(&|p| p.fixed_rate = v) }/>
                "Fixed frame rate (send every frame, not only changes)"
            </label>
            <label>"Chroma"
                <select name=format!("{name}-chroma")
                    on:change=move |ev| { let v = event_target_value(&ev); set(&|p| p.chroma = v.clone()) }>
                    {["420", "444"].into_iter().map(|c| view! {
                        <option value=c selected=move || get().chroma == c>{if c == "444" { "4:4:4 (crisp text, needs client support)" } else { "4:2:0 (every client)" }}</option>
                    }).collect_view()}
                </select>
            </label>
            <label class="checkbox">
                <input type="checkbox" name=format!("{name}-intra_refresh") prop:checked=move || get().intra_refresh
                    on:change=move |ev| { let v = event_target_checked(&ev); set(&|p| p.intra_refresh = v) }/>
                "Intra refresh instead of large keyframes"
            </label>
            <label class="checkbox">
                <input type="checkbox" name=format!("{name}-hdr") prop:checked=move || get().hdr
                    on:change=move |ev| { let v = event_target_checked(&ev); set(&|p| p.hdr = v) }/>
                "HDR where the guest GPU and the client support it"
            </label>
            <label class="checkbox">
                <input type="checkbox" name=format!("{name}-guest_capture") prop:checked=move || get().guest_capture
                    on:change=move |ev| { let v = event_target_checked(&ev); set(&|p| p.guest_capture = v) }/>
                "Capture and encode inside the guest when Horcrux guest tools run"
            </label>
        </fieldset>
    }
}

#[component]
pub fn ConsoleSettingsPage() -> impl IntoView {
    let sig = create_rw_signal(ConsoleSettings::default());
    let (loaded, set_loaded) = create_signal(false);
    let (notice, set_notice) = create_signal(None::<String>);
    create_effect(move |_| {
        spawn_local(async move {
            match api::fetch_json::<ConsoleSettings>("/console/settings").await {
                Ok(s) => {
                    sig.set(s);
                    set_loaded.set(true);
                }
                Err(e) => set_notice.set(Some(format!("Could not load: {}", e.message))),
            }
        });
    });
    let save = move |_| {
        let s = sig.get();
        spawn_local(async move {
            match api::put_json::<ConsoleSettings, _>("/console/settings", &s).await {
                Ok(saved) => {
                    sig.set(saved);
                    set_notice.set(Some(
                        "Saved. Encoder and session settings apply to new sessions; \
                         turning the console on or off applies at each VM's next start."
                            .into(),
                    ));
                }
                Err(e) => set_notice.set(Some(format!("Not saved: {}", e.message))),
            }
        });
    };
    let list = |v: &[String]| v.join(", ");
    let parse_list = |s: String| -> Vec<String> {
        s.split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect()
    };
    view! {
        <div class="page system-console-settings">
            <div class="page-header"><h1>"Horcrux Console"</h1></div>
            <p class="help">
                "One remote display for every VM: the node reads the guest's screen through QEMU's "
                "D-Bus display (no guest tools, works in firmware and installers), encodes it "
                "(hardware encoder when one works, otherwise software) and streams it to the browser "
                "over WebRTC. The classic consoles (VNC, SPICE, serial) stay available on every VM."
            </p>
            {move || notice.get().map(|n| view! { <p id="hxc-settings-notice" class="notice">{n}</p> })}
            {move || loaded.get().then(|| view! {
            <form class="settings-form" id="hxc-settings" on:submit=|ev| ev.prevent_default()>
                <label class="checkbox">
                    <input type="checkbox" name="enabled" prop:checked=move || sig.get().enabled
                        on:change=move |ev| { let v = event_target_checked(&ev); sig.update(|s| s.enabled = v) }/>
                    "Enabled (VMs start with the D-Bus display)"
                </label>
                <label class="checkbox">
                    <input type="checkbox" name="prefer_classic" prop:checked=move || sig.get().prefer_classic
                        on:change=move |ev| { let v = event_target_checked(&ev); sig.update(|s| s.prefer_classic = v) }/>
                    "Open the classic console first"
                </label>
                <label>"Default profile"
                    <select name="default_profile" on:change=move |ev| {
                        if let Some(p) = ConsoleProfile::parse(&event_target_value(&ev)) { sig.update(|s| s.default_profile = p) }
                    }>
                        {ConsoleProfile::ALL.into_iter().map(|p| view! {
                            <option value=p.name() selected=move || sig.get().default_profile == p>{p.name()}</option>
                        }).collect_view()}
                    </select>
                </label>
                <label>"Video codecs, in order of preference (h264, vp8)"
                    <input type="text" name="codecs" prop:value=move || list(&sig.get().codecs)
                        on:change=move |ev| { let v = parse_list(event_target_value(&ev)); sig.update(|s| s.codecs = v) }/>
                </label>
                <label>"Encoder"
                    <select name="encoder" on:change=move |ev| {
                        let e = match event_target_value(&ev).as_str() {
                            "software" => EncoderPreference::Software,
                            "vaapi" => EncoderPreference::Vaapi,
                            "nvenc" => EncoderPreference::Nvenc,
                            _ => EncoderPreference::Auto,
                        };
                        sig.update(|s| s.encoder = e)
                    }>
                        {[("auto", EncoderPreference::Auto), ("software", EncoderPreference::Software),
                          ("vaapi", EncoderPreference::Vaapi), ("nvenc", EncoderPreference::Nvenc)]
                            .into_iter().map(|(n, e)| view! {
                                <option value=n selected=move || sig.get().encoder == e>{n}</option>
                            }).collect_view()}
                    </select>
                </label>
                <label>"VA-API render node"
                    <input type="text" name="vaapi_device" prop:value=move || sig.get().vaapi_device
                        on:change=move |ev| { let v = event_target_value(&ev); sig.update(|s| s.vaapi_device = v) }/>
                </label>
                <label>"FFmpeg program"
                    <input type="text" name="ffmpeg_path" prop:value=move || sig.get().ffmpeg_path
                        on:change=move |ev| { let v = event_target_value(&ev); sig.update(|s| s.ffmpeg_path = v) }/>
                </label>
                <label>"x264 preset (software H.264)"
                    <input type="text" name="x264_preset" prop:value=move || sig.get().x264_preset
                        on:change=move |ev| { let v = event_target_value(&ev); sig.update(|s| s.x264_preset = v) }/>
                </label>
                <label>"Ticket lifetime (s)"
                    <input type="number" name="ticket_lifetime_secs" prop:value=move || sig.get().ticket_lifetime_secs.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().ticket_lifetime_secs); sig.update(|s| s.ticket_lifetime_secs = v) }/>
                </label>
                <label>"Viewers per guest"
                    <input type="number" name="max_sessions_per_guest" prop:value=move || sig.get().max_sessions_per_guest.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().max_sessions_per_guest); sig.update(|s| s.max_sessions_per_guest = v) }/>
                </label>
                <label>"Media UDP ports (from)"
                    <input type="number" name="udp_port_min" prop:value=move || sig.get().udp_port_min.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().udp_port_min); sig.update(|s| s.udp_port_min = v) }/>
                </label>
                <label>"Media UDP ports (to)"
                    <input type="number" name="udp_port_max" prop:value=move || sig.get().udp_port_max.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().udp_port_max); sig.update(|s| s.udp_port_max = v) }/>
                </label>
                <label>"Addresses offered to clients (empty: automatic)"
                    <input type="text" name="candidate_addresses" prop:value=move || list(&sig.get().candidate_addresses)
                        on:change=move |ev| { let v = parse_list(event_target_value(&ev)); sig.update(|s| s.candidate_addresses = v) }/>
                </label>
                <label>"Largest encoded size (width x height)"
                    <span class="inline">
                        <input type="number" name="max_width" prop:value=move || sig.get().max_width.to_string()
                            on:change=move |ev| { let v = num(&ev, sig.get().max_width); sig.update(|s| s.max_width = v) }/>
                        <input type="number" name="max_height" prop:value=move || sig.get().max_height.to_string()
                            on:change=move |ev| { let v = num(&ev, sig.get().max_height); sig.update(|s| s.max_height = v) }/>
                    </span>
                </label>
                <label>"End a session after this many silent seconds"
                    <input type="number" name="session_idle_timeout_secs" prop:value=move || sig.get().session_idle_timeout_secs.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().session_idle_timeout_secs); sig.update(|s| s.session_idle_timeout_secs = v) }/>
                </label>
                <label class="checkbox">
                    <input type="checkbox" name="audit_sessions" prop:checked=move || sig.get().audit_sessions
                        on:change=move |ev| { let v = event_target_checked(&ev); sig.update(|s| s.audit_sessions = v) }/>
                    "Audit session start/stop and refused tickets"
                </label>
                <label class="checkbox">
                    <input type="checkbox" name="guest_tools" prop:checked=move || sig.get().guest_tools
                        on:change=move |ev| { let v = event_target_checked(&ev); sig.update(|s| s.guest_tools = v) }/>
                    "Horcrux guest tools port on console VMs (applies at the next VM start)"
                </label>
                <label>"Guest tools port name"
                    <input type="text" name="guest_tools_port" prop:value=move || sig.get().guest_tools_port
                        on:change=move |ev| { let v = event_target_value(&ev); sig.update(|s| s.guest_tools_port = v) }/>
                </label>
                <label>"Wait for the guest tools (ms)"
                    <input type="number" min="0" max="30000" name="guest_tools_wait_ms" prop:value=move || sig.get().guest_tools_wait_ms.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().guest_tools_wait_ms); sig.update(|s| s.guest_tools_wait_ms = v) }/>
                </label>
                <label>"Check again for the guest tools during a session (s, 0 = off)"
                    <input type="number" min="0" max="3600" name="guest_tools_retry_secs" prop:value=move || sig.get().guest_tools_retry_secs.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().guest_tools_retry_secs); sig.update(|s| s.guest_tools_retry_secs = v) }/>
                </label>
                <label>"Longest wait for a guest tools install (s)"
                    <input type="number" min="30" max="7200" name="guest_tools_install_timeout_secs" prop:value=move || sig.get().guest_tools_install_timeout_secs.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().guest_tools_install_timeout_secs); sig.update(|s| s.guest_tools_install_timeout_secs = v) }/>
                </label>
                <label>"Wait before another automatic install attempt on the same VM (s)"
                    <input type="number" min="0" max="86400" name="guest_tools_auto_install_retry_secs" prop:value=move || sig.get().guest_tools_auto_install_retry_secs.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().guest_tools_auto_install_retry_secs); sig.update(|s| s.guest_tools_auto_install_retry_secs = v) }/>
                </label>
                <label>"Guest tools CD image"
                    <input type="text" name="guest_tools_iso" prop:value=move || sig.get().guest_tools_iso
                        on:change=move |ev| { let v = event_target_value(&ev); sig.update(|s| s.guest_tools_iso = v) }/>
                </label>
                <label class="checkbox">
                    <input type="checkbox" name="guest_tools_auto_install" prop:checked=move || sig.get().guest_tools_auto_install
                        on:change=move |ev| { let v = event_target_checked(&ev); sig.update(|s| s.guest_tools_auto_install = v) }/>
                    "Install the guest tools automatically through the QEMU guest agent (Windows MSI; Linux .deb/.rpm/.apk)"
                </label>
                <label>"Virtual display (Windows guests)"
                    <select name="virtual_display" prop:value=move || sig.get().virtual_display
                        on:change=move |ev| { let v = event_target_value(&ev); sig.update(|s| s.virtual_display = v) }>
                        <option value="auto">"Automatic: when the passed-through GPU has no monitor or no outputs"</option>
                        <option value="always">"Always"</option>
                        <option value="never">"Never"</option>
                    </select>
                </label>
                <label>"Clipboard limit (bytes)"
                    <input type="number" min="1" name="clipboard_max_bytes" prop:value=move || sig.get().clipboard_max_bytes.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().clipboard_max_bytes); sig.update(|s| s.clipboard_max_bytes = v) }/>
                </label>
                <label>"Audio bitrate (kbit/s, Opus)"
                    <input type="number" min="6" max="510" name="audio_bitrate_kbps" prop:value=move || sig.get().audio_bitrate_kbps.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().audio_bitrate_kbps); sig.update(|s| s.audio_bitrate_kbps = v) }/>
                </label>
                <ProfileFields name="desktop" sig=sig/>
                <ProfileFields name="console" sig=sig/>
                <ProfileFields name="gaming" sig=sig/>
                <button id="hxc-settings-save" class="btn btn-primary" on:click=save>"Save"</button>
            </form>
            })}
        </div>
    }
}
