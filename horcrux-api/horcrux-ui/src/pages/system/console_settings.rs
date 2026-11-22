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

/// The relay as the node reports it (GET /api/console/relay).
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct RelayStatus {
    #[serde(default)]
    running: bool,
    #[serde(default)]
    listen: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    allocations: u64,
    #[serde(default)]
    port_mapping: serde_json::Value,
}

/// WAN access: the node's relay, port mapping and link adaptation.
#[component]
fn WanFields(sig: RwSignal<ConsoleSettings>) -> impl IntoView {
    let (relay, set_relay) = create_signal(None::<RelayStatus>);
    let refresh = move || {
        spawn_local(async move {
            if let Ok(r) = api::fetch_json::<RelayStatus>("/console/relay").await {
                set_relay.set(Some(r));
            }
        })
    };
    refresh();
    let status = move || {
        relay.get().map(|r| {
            let map = &r.port_mapping;
            let mapping = match map.get("method").and_then(|m| m.as_str()) {
                Some(m) => format!(
                    "; router mapping via {m}: {}:{}",
                    map.get("external_address")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?"),
                    map.get("external_port")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                ),
                None => match map.get("error").and_then(|e| e.as_str()) {
                    Some(e) => format!("; router mapping failed: {e}"),
                    None => String::new(),
                },
            };
            if r.running {
                format!(
                    "Relay running on {} ({} client(s) relayed now){mapping}",
                    r.listen.unwrap_or_default(),
                    r.allocations
                )
            } else if let Some(e) = r.error {
                format!("Relay not running: {e}")
            } else {
                "Relay off: clients connect directly (LAN only).".to_string()
            }
        })
    };
    let w = move || sig.get().wan;
    let set =
        move |f: &dyn Fn(&mut horcrux_common::console::WanSettings)| sig.update(|s| f(&mut s.wan));
    view! {
        <fieldset class="hxc-wan" id="hxc-wan">
            <legend>"WAN access (from anywhere)"</legend>
            <p class="help">
                "Clients outside this network reach console sessions through this node's own relay "
                "(TURN, built into Horcrux; it only forwards encrypted media and never decrypts it). "
                "The router must forward the relay port to this node, or let Horcrux ask the router "
                "(UPnP / NAT-PMP). Remote quality is limited by this site's upload speed."
            </p>
            <p id="hxc-relay-status" class="notice">{status}</p>
            <label class="checkbox">
                <input type="checkbox" name="wan-enabled" prop:checked=move || w().enabled
                    on:change=move |ev| { let v = event_target_checked(&ev); set(&|x| x.enabled = v) }/>
                "Allow WAN access (run the relay)"
            </label>
            <label>"Relay port (UDP and TCP)"
                <input type="number" min="1" max="65535" name="wan-relay_port" prop:value=move || w().relay_port.to_string()
                    on:change=move |ev| { let v = num(&ev, w().relay_port); set(&|x| x.relay_port = v) }/>
            </label>
            <label class="checkbox">
                <input type="checkbox" name="wan-relay_tcp" prop:checked=move || w().relay_tcp
                    on:change=move |ev| { let v = event_target_checked(&ev); set(&|x| x.relay_tcp = v) }/>
                "Also accept the relay over TCP (networks that block UDP)"
            </label>
            <label>"Public address clients use (empty: the address used for this web UI)"
                <input type="text" name="wan-relay_public_address" prop:value=move || w().relay_public_address
                    on:change=move |ev| { let v = event_target_value(&ev).trim().to_string(); set(&|x| x.relay_public_address = v.clone()) }/>
            </label>
            <label>"Public port, when the router forwards a different one (0: same)"
                <input type="number" min="0" max="65535" name="wan-relay_public_port" prop:value=move || w().relay_public_port.to_string()
                    on:change=move |ev| { let v = num(&ev, w().relay_public_port); set(&|x| x.relay_public_port = v) }/>
            </label>
            <label>"Address relayed media is sent from (empty: automatic)"
                <input type="text" name="wan-relay_peer_address" prop:value=move || w().relay_peer_address
                    on:change=move |ev| { let v = event_target_value(&ev).trim().to_string(); set(&|x| x.relay_peer_address = v.clone()) }/>
            </label>
            <label>"Relayed client ports, first"
                <input type="number" min="1" max="65535" name="wan-relay_port_min" prop:value=move || w().relay_port_min.to_string()
                    on:change=move |ev| { let v = num(&ev, w().relay_port_min); set(&|x| x.relay_port_min = v) }/>
            </label>
            <label>"Relayed client ports, last"
                <input type="number" min="1" max="65535" name="wan-relay_port_max" prop:value=move || w().relay_port_max.to_string()
                    on:change=move |ev| { let v = num(&ev, w().relay_port_max); set(&|x| x.relay_port_max = v) }/>
            </label>
            <label>"Relay realm"
                <input type="text" name="wan-relay_realm" prop:value=move || w().relay_realm
                    on:change=move |ev| { let v = event_target_value(&ev); set(&|x| x.relay_realm = v.clone()) }/>
            </label>
            <label>"Session relay credentials valid for (s)"
                <input type="number" min="10" max="86400" name="wan-relay_credential_lifetime_secs" prop:value=move || w().relay_credential_lifetime_secs.to_string()
                    on:change=move |ev| { let v = num(&ev, w().relay_credential_lifetime_secs); set(&|x| x.relay_credential_lifetime_secs = v) }/>
            </label>
            <label>"Most relayed clients at once"
                <input type="number" min="1" max="4096" name="wan-relay_max_allocations" prop:value=move || w().relay_max_allocations.to_string()
                    on:change=move |ev| { let v = num(&ev, w().relay_max_allocations); set(&|x| x.relay_max_allocations = v) }/>
            </label>
            <label class="checkbox">
                <input type="checkbox" name="wan-force_relay" prop:checked=move || w().force_relay
                    on:change=move |ev| { let v = event_target_checked(&ev); set(&|x| x.force_relay = v) }/>
                "Always use the relay (clients never connect directly)"
            </label>
            <label class="checkbox">
                <input type="checkbox" name="wan-upnp" prop:checked=move || w().upnp
                    on:change=move |ev| { let v = event_target_checked(&ev); set(&|x| x.upnp = v) }/>
                "Ask the router to forward the relay port (UPnP / NAT-PMP)"
            </label>
            <label>"Router mapping lease (s)"
                <input type="number" min="120" max="86400" name="wan-upnp_lease_secs" prop:value=move || w().upnp_lease_secs.to_string()
                    on:change=move |ev| { let v = num(&ev, w().upnp_lease_secs); set(&|x| x.upnp_lease_secs = v) }/>
            </label>
            <label>"Router address (empty: the default gateway)"
                <input type="text" name="wan-upnp_gateway" prop:value=move || w().upnp_gateway
                    on:change=move |ev| { let v = event_target_value(&ev).trim().to_string(); set(&|x| x.upnp_gateway = v.clone()) }/>
            </label>
            <label>"UPnP discovery address"
                <input type="text" name="wan-upnp_ssdp_address" prop:value=move || w().upnp_ssdp_address
                    on:change=move |ev| { let v = event_target_value(&ev).trim().to_string(); set(&|x| x.upnp_ssdp_address = v.clone()) }/>
            </label>
            <label>"This site's upload speed (kbit/s, 0: unknown) - remote sessions never exceed it"
                <input type="number" min="0" name="wan-uplink_kbps" prop:value=move || w().uplink_kbps.to_string()
                    on:change=move |ev| { let v = num(&ev, w().uplink_kbps); set(&|x| x.uplink_kbps = v) }/>
            </label>
            <label class="checkbox">
                <input type="checkbox" name="wan-adaptive" prop:checked=move || w().adaptive
                    on:change=move |ev| { let v = event_target_checked(&ev); set(&|x| x.adaptive = v) }/>
                "Fit bitrate, resolution and frame rate to the measured link"
            </label>
            <label>"Lowest bitrate (kbit/s)"
                <input type="number" min="50" name="wan-min_kbps" prop:value=move || w().min_kbps.to_string()
                    on:change=move |ev| { let v = num(&ev, w().min_kbps); set(&|x| x.min_kbps = v) }/>
            </label>
            <label>"Link headroom kept for audio and retransmissions (%)"
                <input type="number" min="0" max="50" name="wan-headroom_pct" prop:value=move || w().headroom_pct.to_string()
                    on:change=move |ev| { let v = num(&ev, w().headroom_pct); set(&|x| x.headroom_pct = v) }/>
            </label>
            <label>"Quality floor (thousandths of a bit per pixel)"
                <input type="number" min="1" max="2000" name="wan-min_millibits_per_pixel" prop:value=move || w().min_millibits_per_pixel.to_string()
                    on:change=move |ev| { let v = num(&ev, w().min_millibits_per_pixel); set(&|x| x.min_millibits_per_pixel = v) }/>
            </label>
            <label>"Lowest frame rate (desktop and console profiles)"
                <input type="number" min="1" max="120" name="wan-min_fps" prop:value=move || w().min_fps.to_string()
                    on:change=move |ev| { let v = num(&ev, w().min_fps); set(&|x| x.min_fps = v) }/>
            </label>
            <label>"Step down at most every (ms)"
                <input type="number" min="100" name="wan-down_interval_ms" prop:value=move || w().down_interval_ms.to_string()
                    on:change=move |ev| { let v = num(&ev, w().down_interval_ms); set(&|x| x.down_interval_ms = v) }/>
            </label>
            <label>"Step up after a better link for (ms)"
                <input type="number" min="100" name="wan-up_interval_ms" prop:value=move || w().up_interval_ms.to_string()
                    on:change=move |ev| { let v = num(&ev, w().up_interval_ms); set(&|x| x.up_interval_ms = v) }/>
            </label>
            <label>"Video packets kept for retransmission"
                <input type="number" min="100" max="65536" name="wan-nack_history_packets" prop:value=move || w().nack_history_packets.to_string()
                    on:change=move |ev| { let v = num(&ev, w().nack_history_packets); set(&|x| x.nack_history_packets = v) }/>
            </label>
            <label class="checkbox">
                <input type="checkbox" name="wan-fec" prop:checked=move || w().fec
                    on:change=move |ev| { let v = event_target_checked(&ev); set(&|x| x.fec = v) }/>
                "Forward error correction (FlexFEC) when the client supports it"
            </label>
            <label>"FEC repair packets per 100 video packets"
                <input type="number" min="1" max="100" name="wan-fec_overhead_percent" prop:value=move || w().fec_overhead_percent.to_string()
                    on:change=move |ev| { let v = num(&ev, w().fec_overhead_percent); set(&|x| x.fec_overhead_percent = v) }/>
            </label>
            <label>"Use FEC from this round trip (ms, 0: always)"
                <input type="number" min="0" max="10000" name="wan-fec_min_rtt_ms" prop:value=move || w().fec_min_rtt_ms.to_string()
                    on:change=move |ev| { let v = num(&ev, w().fec_min_rtt_ms); set(&|x| x.fec_min_rtt_ms = v) }/>
            </label>
            <label class="checkbox">
                <input type="checkbox" name="wan-pace_video" prop:checked=move || w().pace_video
                    on:change=move |ev| { let v = event_target_checked(&ev); set(&|x| x.pace_video = v) }/>
                "Pace video packets at the link estimate (off: each frame is sent at once)"
            </label>
            <button type="button" class="btn" id="hxc-relay-refresh" on:click=move |_| refresh()>"Refresh relay status"</button>
        </fieldset>
    }
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
            <label class="checkbox">
                <input type="checkbox" name=format!("{name}-low_latency_playout") prop:checked=move || get().low_latency_playout
                    on:change=move |ev| { let v = event_target_checked(&ev); set(&|p| p.low_latency_playout = v) }/>
                "Show each frame as soon as it arrives (no playout smoothing)"
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
                <label class="checkbox">
                    <input type="checkbox" name="x264_in_process" prop:checked=move || sig.get().x264_in_process
                        on:change=move |ev| { let v = event_target_checked(&ev); sig.update(|s| s.x264_in_process = v) }/>
                    "Software H.264 in-process (libx264): rate changes without an encoder restart"
                </label>
                <label>"libx264 names or paths (comma-separated, tried in order)"
                    <input type="text" name="x264_library" prop:value=move || list(&sig.get().x264_library)
                        on:change=move |ev| { let v = parse_list(event_target_value(&ev)); sig.update(|s| s.x264_library = v) }/>
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
                <label class="checkbox">
                    <input type="checkbox" name="input_via_guest_tools" prop:checked=move || sig.get().input_via_guest_tools
                        on:change=move |ev| { let v = event_target_checked(&ev); sig.update(|s| s.input_via_guest_tools = v) }/>
                    "Keyboard and pointer through the guest tools while they run (QEMU's input before they start)"
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
                <label>"Clipboard limit (bytes)"
                    <input type="number" min="1" name="clipboard_max_bytes" prop:value=move || sig.get().clipboard_max_bytes.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().clipboard_max_bytes); sig.update(|s| s.clipboard_max_bytes = v) }/>
                </label>
                <label>"Audio bitrate (kbit/s, Opus)"
                    <input type="number" min="6" max="510" name="audio_bitrate_kbps" prop:value=move || sig.get().audio_bitrate_kbps.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().audio_bitrate_kbps); sig.update(|s| s.audio_bitrate_kbps = v) }/>
                </label>
                <WanFields sig=sig/>
                <label>"Audio frame length (ms; 5, 10, 20, 40 or 60)"
                    <input type="number" min="5" max="60" name="audio_frame_ms" prop:value=move || sig.get().audio_frame_ms.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().audio_frame_ms); sig.update(|s| s.audio_frame_ms = v) }/>
                </label>
                <label class="checkbox">
                    <input type="checkbox" name="shm_frames" prop:checked=move || sig.get().shm_frames
                        on:change=move |ev| { let v = event_target_checked(&ev); sig.update(|s| s.shm_frames = v) }/>
                    "Frame ring for the native viewer on this node (raw frames, no encode; applies at the next VM start)"
                </label>
                <label>"Frame ring directory (tmpfs or hugetlbfs)"
                    <input type="text" name="shm_frames_dir" prop:value=move || sig.get().shm_frames_dir
                        on:change=move |ev| { let v = event_target_value(&ev); sig.update(|s| s.shm_frames_dir = v) }/>
                </label>
                <label>"Frame ring size (MiB, power of two; 128 fits three 4K frames)"
                    <input type="number" min="4" max="1024" name="shm_frames_size_mib" prop:value=move || sig.get().shm_frames_size_mib.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().shm_frames_size_mib); sig.update(|s| s.shm_frames_size_mib = v) }/>
                </label>
                <label>"Frame ring slots"
                    <input type="number" min="2" max="8" name="shm_frames_slots" prop:value=move || sig.get().shm_frames_slots.to_string()
                        on:change=move |ev| { let v = num(&ev, sig.get().shm_frames_slots); sig.update(|s| s.shm_frames_slots = v) }/>
                </label>
                <label>"Native viewer socket"
                    <input type="text" name="local_socket" prop:value=move || sig.get().local_socket
                        on:change=move |ev| { let v = event_target_value(&ev); sig.update(|s| s.local_socket = v) }/>
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
