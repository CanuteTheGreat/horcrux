# Horcrux Console: one remote display for every VM, container and host

Status: C1 and the first part of C2 are built (2026-11-19); section 11 lists
what each phase delivered and what is still open.

## 1. Why one component

The gaming VM (docs/design/gaming-vm.md) needs a low-latency viewer and
streaming. Every other VM and container needs a good console too, and today
Horcrux has several unrelated ones (noVNC, a SPICE relay, a serial WebSocket,
the LXD console). Horcrux Console is a single remote-display stack used for all
of them, with profiles on the same core:

| Profile | For | Priorities |
|---|---|---|
| `desktop` | office/desktop VMs, remote host desktop | crisp text (4:4:4, lossless refinement of static areas), damage-based updates, low bandwidth when idle, clipboard/files/USB/audio, resize and multi-monitor |
| `gaming` | gaming VMs | fixed high frame rate (up to 240-360 Hz), lowest latency rate control, raw/relative mouse, gamepads, HDR |
| `console` | servers, installers, firmware/BIOS screens, guests without tools | text/serial channel plus framebuffer; works before any OS runs |

The profile is a per-session setting (default chosen from the guest: gaming VMs
`gaming`, VMs with a desktop `desktop`, everything else `console`), and every
knob inside a profile is a typed config key with a default (section 9).

Scope, in order of phases: VMs (Linux and Windows guests), the gaming profile,
WAN access, system containers with a desktop, the Horcrux host's own desktop
(remote into a Horcrux desktop machine), then mobile/TV clients. A standalone
Horcrux machine (one box, no cluster, no shared storage, no separate relay) is
a first-class target in every phase: it runs the broker, the relay and the
clients' endpoint itself. A cluster adds placement and failover, nothing more.

Nothing regresses: VNC, SPICE and serial stay as fallbacks for guests without
tools and for pre-OS screens (installers, OVMF setup, boot menus), reachable
from the same viewer.

## 2. Prior art (ideas only, clean-room)

Horcrux is GPL-3.0. Licences below were read from each project's own
licence file or source headers on 2026-11-18. No code is copied from any of
them; where a library is used as a dependency it is listed in section 10.

| Project | Licence | Best at | Weak at | Borrow (Horcrux feature, phase) | Avoid |
|---|---|---|---|---|---|
| SPICE (server, spice-gtk) + spice-vdagent | LGPL-2.1 (server); vdagent GPL-3.0 | guest agent model: clipboard, auto-resize to the window, multi-monitor, USB redirection (usbredir), works pre-OS through QXL/VGA | 3D and high frame rates; video detection heuristics; TCP only | guest agent over virtio-serial for clipboard/resize/monitor layout (C2); USB redirection over a channel (C5); one viewer that falls back to the emulated display pre-OS (C1) | QXL as the main path; per-region lossy heuristics |
| RDP (MS-RDPBCGR, MS-RDPEGFX, MS-RDPEDYC, MS-RDPECLIP, MS-RDPEDISP, MS-RDPEMT); FreeRDP | Microsoft Open Specifications; FreeRDP Apache-2.0 | text quality via AVC444 (4:4:4 carried as two 4:2:0 H.264 streams) and progressive codecs; dynamic virtual channels; clipboard incl. files; drive/printer/audio redirection; resize | complex, decades of extensions; latency is not its goal | progressive "lossy first, lossless refinement" for static regions (C1 `desktop`); channel multiplexing with per-channel priorities (C1); display-control resize (C2); drive/printer redirection (C5) | implementing RDP itself as the main protocol (huge surface); we may later offer an RDP gateway for standard clients |
| VNC/RFB (RFC 6143); noVNC | RFC; noVNC MPL-2.0 | universal, simple, pre-OS | client-pull framebuffer updates round-trip per frame, so latency grows with RTT; no audio; pixel encodings not video codecs | keep as the `console` fallback (C1) | as the primary path |
| Amazon DCV (NICE DCV) | proprietary | GPU-encoded H.264/HEVC, QUIC (UDP) transport, lossless build-up for text, multi-monitor, USB, web client | closed | QUIC/UDP transport with TCP fallback idea; lossless build-up (C1/C3) | n/a |
| HP Anyware (Teradici PCoIP) | proprietary | build-to-lossless, pixel-perfect text for CAD/media pros | closed, needs its agents | build-to-lossless as an explicit `desktop` setting (C1) | n/a |
| Parsec | proprietary | low-latency UDP streaming, hardware encode on all vendors, 4:4:4 mode, Windows virtual display driver, virtual gamepad driver | closed, third-party service for NAT | virtual display driver for headless GPUs and a virtual gamepad in the guest tools (C2); 4:4:4 toggle in `gaming` (C2) | dependence on an external service |
| Moonlight + Sunshine (GameStream-compatible) | GPL-3.0 (both, and moonlight-common-c) | proven game streaming: RTP video with Reed-Solomon FEC, HEVC/AV1/HDR, PIN pairing with client certificates, controller support | its own protocol (no browser client), pairing per host | FEC on video frames (C3); pairing-free since Horcrux accounts authenticate (C1) | copying code (licence would be compatible, but it is a third-party viewer path the owner ruled out) |
| NVIDIA GameStream | proprietary, discontinued in 2023 | | | | |
| Steam Remote Play | proprietary | relay network for NAT, guest players | closed | shared input for a second player (later) | |
| GeForce NOW / Xbox Cloud Gaming | proprietary | WebRTC in the browser at high frame rates, low-latency pacing (Reflex) | needs data centres near the player | proof that browser WebRTC carries 1440p240-class streams; frame pacing to the client display (C3) | |
| Looking Glass (KVMFR) | GPL-2.0-or-later | same-host shared memory frames (ivshmem), zero encode, very low latency | same host only, its own client | ivshmem frame ring for the same-host path (C2), designed from scratch | its client/host apps in the user path (owner rule); no code copied even though the licence would allow it |
| Chrome Remote Desktop | proprietary service; libwebrtc BSD-3-Clause | WebRTC end to end, works through NAT, browser client | Google relay service | WebRTC transport, ICE/TURN (C1/C3) with a self-hosted relay | external relay/identity |
| RustDesk | AGPL-3.0 | self-hosted rendezvous + relay servers, NAT hole punching, cross-platform clients | its own protocol, AGPL | self-hosted relay integrated in Horcrux (C3) | AGPL code |
| Apple Screen Sharing | proprietary | HEVC high-performance mode (macOS 14+, Apple silicon), virtual displays, audio | Apple only | per-session virtual display sized to the client (C2) | |
| PipeWire ScreenCast + xdg-desktop-portal RemoteDesktop | PipeWire MIT; portal LGPL-2.1 | dmabuf zero-copy capture on Wayland, standard input injection with user consent | needs a desktop session | Linux guest tools capture and host-desktop remote access (C2, C6) | |
| gnome-remote-desktop | GPL-2.0-or-later | RDP server on Wayland with the graphics pipeline and GPU encode, headless remote login | RDP only | headless login sessions for the host desktop (C6) | |
| xrdp | Apache-2.0 | RDP for X11 sessions | X11 | | |
| Apache Guacamole | Apache-2.0 | clientless HTML5 gateway for VNC/RDP/SSH, session recording | adds a translation hop, latency | session recording for audit (C5, opt-in) | translation gateway in the hot path |

## 3. Architecture

```
 guest (VM)                      Horcrux node                       client
 ----------                      ------------                       ------
 [Horcrux guest tools]           [console broker]                   [web UI (browser WebRTC)]
   capture + encode  --vsock--->   session/auth/RBAC/audit  --WebRTC--> [Horcrux desktop app]
   input/clipboard   <-vserial-    frame source selection      ^        [native viewer, same host]
   virtual display                 relay (TURN) on the node    |
 (no tools: QEMU display) ------> [host capture + encode] -----+
 (same host, gaming) --ivshmem--> [native viewer, no encode]
```

- **Frame sources**, picked per session by the broker:
  1. QEMU's D-Bus display (`-display dbus`, QEMU 7.0+): scanouts (incl. dmabuf
     for virtio-gpu/virgl), cursor, keyboard/mouse, audio and clipboard
     interfaces. Needs no guest tools, works pre-OS, encoded on the host. This is
     the C1 path and the fallback for every VM.
  2. Guest tools capture and encode inside the guest (the guest owns the GPU in
     the gaming case): Windows DXGI Desktop Duplication or the Indirect Display
     Driver swap chain, NVENC/AMF/QSV via Media Foundation or vendor SDKs;
     Linux PipeWire ScreenCast or KMS, VA-API/NVENC. Encoded packets cross to
     the host over virtio-vsock; the host forwards them without transcoding.
  3. Same-host raw frames over a Horcrux-managed ivshmem device into the native
     viewer (no encode at all).
  4. Fallbacks: VNC (noVNC client code stays for the console page), SPICE relay,
     serial, LXD console.
- **Transport**: WebRTC everywhere (browser constraint; the native app uses the
  same stack so there is one protocol). Video/audio on SRTP; input, clipboard,
  cursor, control and file channels on SCTP data channels with their own
  reliability (input unordered/unreliable with sequence numbers for mouse
  motion, reliable for keys and clipboard). ICE with host candidates for LAN,
  STUN-style reflexive candidates and a Horcrux-hosted TURN relay for NAT. No
  third-party service.
- **Signalling**: the existing Horcrux API WebSocket (authenticated). The broker
  runs in horcrux-api on the node that hosts the guest; in a cluster the client
  talks to the cluster name/VIP and is redirected to the owning node; after a
  failover the client re-signals and reconnects (C4).
- **Input**: QMP `input-send-event` / virtio-input for the no-tools path; guest
  tools inject via SendInput and a virtual HID gamepad (Windows) or uinput
  (Linux). Pointer lock + raw relative motion in the client for `gaming`.
- **Audio**: QEMU audiodev (dbus/PipeWire) without tools; WASAPI loopback or
  PipeWire capture with tools; Opus in both directions (microphone opt-in).
- **Profiles** set rate control (CBR/low-delay, no B-frames, intra refresh for
  `gaming`; quality-based with lossless refinement for `desktop`), chroma
  (4:2:0 or 4:4:4), frame-rate policy (fixed vs damage-driven) and which
  channels are on.

Containers: system containers with a desktop use the Linux guest tools inside
the container (PipeWire capture); shells keep the text console channel.

Host desktop (C6): the same agent runs on a Horcrux desktop and captures through
the xdg-desktop-portal ScreenCast/RemoteDesktop portals (consent on first use)
or a headless login session.

Patronus can reuse the protocol later for its web console (text + framebuffer
profile); noted here so the wire format stays product-neutral.

## 4. Wire format (C1)

One WebRTC peer connection per session. Data channels:

| Channel | Reliability | Payload |
|---|---|---|
| `ctl` | reliable, ordered | JSON: hello (versions, codecs, profile), layout (monitors, sizes, refresh), resize requests, bitrate/keyframe requests, stats, bye |
| `input` | unordered, maxRetransmits=0 for motion; separate reliable channel `keys` | compact binary events (seq, timestamp, kind, data) |
| `cursor` | reliable | cursor shape (ARGB, hotspot) + position for client-side rendering |
| `clip` | reliable | clipboard offers/requests (text, images; files in C5), size-limited |
| `console` | reliable | serial/text bytes for the `console` profile |

Video: H.264, HEVC and AV1 (negotiated by client support and encoder
capability), 8- or 10-bit, 4:2:0 or 4:4:4 where both ends support it.
Every message carries a version; unknown fields are ignored.

## 4.1 Guest tools port (C2)

The node and the Horcrux guest tools talk over one byte stream per VM: a
virtio-serial port named by `[console] guest_tools_port` (default
`org.horcrux.console.0`; QEMU `virtserialport` on its own
`virtio-serial-pci`, host end `<disk>.hxgt.sock`). The crate
`crates/horcrux-guest-proto` is the whole wire format: `len u32 LE`,
`kind u8`, payload; JSON for control (`Hello` with probed capabilities,
`HostHello`, `StartVideo`, `VideoStatus`, `Clipboard`, `SetLayout`/`Layout`,
`StartAudio`), binary for media and input (`Video` = one encoded access unit
with keyframe flag, capture time and capture-to-port time; `Audio` = one
Opus packet; `Input` = key (QEMU qnum), button, raw relative motion,
absolute position, wheel, full gamepad state). Unknown kinds and fields are
skipped, so either side can be newer. The broker connects when a viewer
opens, sends `HostHello` and waits `guest_tools_wait_ms` (default 1500 ms)
for `Hello`; without an answer the session uses the node's capture, so
nothing needs the tools. Only one session holds the port at a time; other
viewers of the same VM use the node's capture.

What flows where in a session with tools:

| Data | Path |
|---|---|
| video | guest capture + encode (H.264 Annex B) -> port -> str0m RTP, no transcoding; the node's encoder rests while the guest stream runs and takes over if the tools stop or report an error; keyframe requests (PLI, new viewer) go to the guest |
| keyboard, buttons, absolute pointer | QEMU D-Bus input (works with and without tools, and pre-OS) |
| raw relative mouse (pointer lock, `gaming`) | client `input` channel kind 2 -> tools (uinput / SendInput) |
| gamepads | client `pad` channel (W3C standard mapping) -> tools (uinput gamepad on Linux; Windows needs the virtual HID gamepad driver, see 11) |
| clipboard | client `clip` channel <-> tools, only with VM.Console.Clipboard, size-limited (`clipboard_max_bytes`) |
| resize / layout | client `ctl` `resize` -> tools (`xrandr` with CVT-RB modes on Linux, ChangeDisplaySettingsEx on Windows) -> `guest_layout` back |
| audio | tools capture (PulseAudio/PipeWire via FFmpeg on Linux) -> Opus -> str0m audio track |
| serial text | client `console` channel <-> the VM's serial socket (input needs VM.Console.Input) |

Delivery: every console VM run gets the guest tools CD (`guest_tools_iso`,
label `HXTOOLS`, on its own virtio-scsi controller) when the node has the
image. When a viewer opens, no tools answer and the QEMU guest agent runs,
the node installs them (`guest_tools_auto_install`): Windows guests get the
MSI from the CD through `guest-exec` (Windows Installer, removable like any
package). Linux guests install the `app-emulation/horcrux-guest-tools`
package (OpenRC and systemd service files included); the CD carries the
static agent and service files for distributions without a package yet.

## 5. Security model

- Only authenticated Horcrux users (password + 2FA/SSO per account policy)
  obtain a **console ticket**: single guest, single session, short lifetime
  (default 60 s to redeem), bound to the user and the client's signalling
  connection.
- RBAC privileges: `VM.Console` (view), `VM.Console.Input`,
  `VM.Console.Clipboard`, `VM.Console.Files`, `VM.Console.USB`,
  `VM.Console.Audio.Mic`; container and host-desktop equivalents. Clipboard,
  file and USB redirection are per-guest settings, default off for shared
  guests and on for a user's own desktop/gaming VM (configurable).
- Media and data are encrypted with DTLS-SRTP/DTLS; the certificate
  fingerprint travels over the authenticated signalling channel, so a relay
  sees ciphertext only.
- Audit events: session start/stop (user, source address, path direct/relay),
  clipboard/file transfers (sizes, not content), USB attach, privilege denials.
- Guest tools talk to the host only over virtio-vsock/virtio-serial with a
  per-guest key provisioned by Horcrux; they never receive host or user
  credentials. A compromised guest can only affect its own session.
- The relay (TURN) authenticates with short-lived credentials minted per
  session; it is off by default on a standalone machine until WAN access is
  enabled, and it never decrypts.
- Optional session recording (C5) is opt-in per guest and audited.

## 6. Latency budget and targets

| Path | Target | Budget (gaming profile, 240 Hz) |
|---|---|---|
| Same host, raw frames (ivshmem) | < 8 ms frame-to-viewer | capture 1-2 ms, copy 1 ms, present up to 1 frame |
| LAN, encoded | < 20 ms click-to-photon | input 1 ms, game frame 4 ms, capture 1 ms, encode 2-4 ms, network 1-2 ms, decode 2-4 ms, present 4 ms |
| WAN | LAN + RTT + jitter buffer | adaptive bitrate fits the uplink; FEC instead of retransmission when RTT is high |
| Desktop profile, LAN | < 50 ms, text lossless within 500 ms of last change | |

Measured numbers per path go into this table and into the GeForce NOW
comparison in docs/design/gaming-vm.md section 14 as phases land. Tests measure
with a colour-flip probe in the guest and the client's frame timestamps.

### Measured in C1 (no guest tools, software x264, headless Chromium client)

Source: `horcrux-api/tests/hx_console_e2e.sh` (CI workflow `hx-console-e2e.yml`)
and `crates/horcrux-vm/tests/hxc_dbus_e2e.rs`, single standalone node, QEMU 10.2
with KVM, client and node on the same host (loopback ICE). Input-to-picture is
the time from the client sending an input event until a decoded frame whose
pixels changed (more than 3 pixels moved by more than 64 levels, so lossy
re-encoding does not count) reaches the page; pointer and click samples watch
the whole picture at full resolution.

| Measurement | Result | Conditions |
|---|---|---|
| Data-channel round trip | 2-7 ms | every run |
| Server encode, 1024x768 / 1280x800 software x264 (`ultrafast`, `zerolatency`, no B-frames) | median 14-31 ms on a lightly loaded host; 108 ms when the 8-core host ran at load 22-30 | `encode_ms_avg`, last 60 frames |
| Standalone FFmpeg probe, same settings, 1024x768 | median 15.3 ms | same host, idle-ish |
| Keystroke to picture, boot loader (pre-OS, 640x480) | median 104-168 ms (min 75 ms) | shared CI host, load 12-30 on 8 cores |
| Keystroke to picture, Linux console (1280x800) | median 73-147 ms (min 66 ms) | same |
| Pointer move / click to picture, Linux | median 85-375 ms (min 51 ms); seconds when the host was saturated | same; includes the guest kernel and `od` printing the event |
| Keystroke / absolute pointer to picture, Windows Setup (1024x768, no tools) | median 571 / 259 ms | same host at load 17-20 while Windows Setup itself was busy |
| Keyframe/noise false positives | 0 (view-only user's input: 0 picture changes, server counters `keys_in = pointer_in = 0`) | after the pixel-delta detector |

Reading: the transport (2-7 ms) is not the problem; the measured totals on the
shared CI host are dominated by host CPU contention (software encode and
headless software decode compete with other CI jobs). The desktop-profile LAN
target (< 50 ms) is NOT yet shown: it needs a run on an unloaded node and
hardware encode (VA-API/NVENC, code paths present, untested without a GPU
runner). These rows are rewritten when the C2 latency run on dedicated
hardware lands.

### Measured in C2 part 1 (guest tools, in-guest encode, port level)

Source: `crates/horcrux-guest-tools/tests/linux_guest_e2e.py` (CI workflow
`guest-tools.yml`), Alpine 3.24 guest with 2 vCPUs under KVM (QEMU 8.2), the
static agent from the tools CD, the test acting as the node on the
`<disk>.hxgt.sock` socket. No WebRTC and no client decode in this row: it
measures the guest half of the path (input injection, guest reaction,
capture, encode, virtio-serial) that the guest-tools session forwards
without transcoding.

| Measurement | Result | Conditions |
|---|---|---|
| Keystroke (tools' uinput keyboard) to an encoded frame carrying the guest's reaction (whole-screen reverse video), at the node | median 15.6 ms, p95 29.4 ms, 40/40 keystrokes | 1280x800, 60 fps fixed rate, software x264 in the guest (`ultrafast`, `zerolatency`, CBR 2 Mbit/s), 0 detector hits in 2 s without input |
| In-guest encode time per frame (agent-reported) | median 2.1 ms, p95 2.3 ms | same; mostly-static console content |
| Keyframe on request | 430-490 ms | the FFmpeg CLI encoder restarts for an IDR (hardware MF/NVENC encoders force one in-stream) |

Reading: at 60 fps half a frame period (8.3 ms) is the expected average wait
for the next capture, so the guest half costs about one frame period.
240 Hz gaming capture, hardware encode in the guest and the browser half
(decode, present) of this path are measured in C2b with a Windows guest and
on GPU hardware.

## 7. Standalone machine

One box runs everything: the guest, the broker, the encoder and, if WAN access
is turned on, the TURN relay on a configurable port (UPnP/NAT-PMP mapping is
opt-in; manual port forwarding documented). One GPU: the gaming VM uses
single-GPU hand-over (gaming-vm.md section 2) and the player connects from
another device; two GPUs: the host desktop keeps one and the native viewer shows
the VM locally. No shared storage or second node is assumed anywhere.

## 8. Fallbacks and compatibility

- Pre-OS screens and guests without tools: QEMU D-Bus display source (C1) or
  VNC; nothing requires guest tools to see a guest.
- Browsers without WebRTC H.264/HEVC/AV1 decode get the codec they support or
  VNC.
- Existing console URLs keep working until the new viewer covers them; the UI
  offers "classic console (VNC/SPICE/serial)" as a switch.

## 9. Settings (all with defaults; `[console]` in /etc/horcrux/config.toml, API, UI)

Default profile per guest type; per-profile codec preference order, chroma,
max resolution/refresh, bitrate floor/ceiling, keyframe interval, lossless
refinement delay; ticket lifetime; relay enabled/port/realm/credential
lifetime; UPnP mapping on/off; clipboard/file/USB/mic defaults for own vs
shared guests; audit of clipboard sizes on/off; recording on/off and retention;
native viewer default (window vs fullscreen, vsync). No value is baked into
code (owner rule: everything definable).

## 10. Libraries (licence-checked before use)

Candidates, to be confirmed with `cargo deny`/the licence inventory job when
added: webrtc-rs (MIT/Apache-2.0) or libwebrtc (BSD-3-Clause) for transport;
FFmpeg libraries (LGPL-2.1+ build, no non-free) or vendor SDK headers for
encode/decode; Opus (BSD-3-Clause); QEMU D-Bus display interfaces (consumed
over D-Bus, no linking); PipeWire (MIT). Windows guest tools: Windows Driver
Kit APIs (IddCx, HID) are used from Microsoft's documentation; the
Windows-driver-samples repository is MS-PL, which the FSF lists as
GPL-incompatible, so its sample code is not copied into GPL-3.0 guest tools.

## 11. Phases

| Phase | Content | Proof |
|---|---|---|
| C1 | Console broker in horcrux-api (tickets, RBAC, audit), QEMU D-Bus display source with host encode (VA-API/NVENC/software), WebRTC transport, web client in the Horcrux UI, native viewer window on a Horcrux desktop, input + audio + cursor, `desktop` and `console` profiles, VNC/SPICE/serial fallback switch | e2e in CI on a nested KVM runner: a Linux desktop VM and a Windows VM without tools viewed through a headless browser client; latency probe numbers published; standalone single node |
| C2 | Guest tools (Windows MSI + Linux package): clipboard, resize, multi-monitor, IDD virtual display for headless GPUs, in-guest capture/encode, virtual gamepad, raw input; `gaming` profile; same-host ivshmem raw-frame path | Windows guest in CI with tools installed automatically; same-host latency measured; real-GPU encode listed until a GPU runner exists |
| C3 | WAN: Horcrux TURN relay (standalone node or cluster), adaptive bitrate/resolution fitted to the link, FEC/NACK, Horcrux desktop app (Linux/Windows/macOS) | netem tests at fixed RTT/loss; numbers per path in section 6 |
| C4 | Cluster: endpoint on the cluster name/VIP, redirect to the owning node, reconnect after failover | vmtest HA scenario with a forced failover |
| C5 | File/drive/printer redirection, USB redirection, opt-in session recording | e2e per channel |
| C6 | System containers with desktops; the Horcrux host's own desktop (portals, headless login) | e2e on a desktop edition image |
| C7 | Android/Android TV, iOS/Apple TV clients, touch overlay | device lab |

Mapping to the gaming plan: gaming phase 3 = C1 + C2, gaming phase 4 = C3,
gaming phase 5 includes C4.

### C1 status

Done (on main with this phase): console broker in horcrux-api (60 s tickets
bound to the user and the signalling WebSocket connection, single use;
privileges VM.Console, VM.Console.Input, VM.Console.Clipboard,
VM.Console.Audio.Mic; audit events for session start/end and refused tickets);
signalling on the API WebSocket, which any authenticated user may open while
its broadcast event topics need Sys.Audit on `/`; QEMU D-Bus display source
(`-display dbus,p2p=yes` on every VM, VNC kept alongside); FFmpeg child-process
encoder (VA-API, NVENC, software; H.264 and VP8); str0m WebRTC transport with
the `ctl`, `keys`, `input` and `cursor` channels; web client page and the
"Horcrux Console" choice on the VM console page (`?classic=1` opens the classic
console); `[console]` settings with defaults in config.toml, the API, the
declarative config and System > Horcrux Console in the UI; `desktop` and
`console` profiles; ffmpeg in the ebuilds and package sets (OpenRC and systemd
images alike).

Proven in CI (`hx-console-e2e.yml`, kvm-x86_64 runner, standalone node): first
frame before any OS runs, keyboard at the boot loader, keyboard, pointer moves
and all three mouse buttons reaching a Linux guest kernel without guest tools,
Windows Setup rendering and answering keyboard and absolute pointer input
without tools, ticket binding (wrong connection refused, single use),
view-only user (picture yes, input refused server-side and nothing changes in
the guest), user without VM.Console (403), audit entries, classic noVNC page
next to a live session, settings API and UI. `hxc_dbus_e2e.rs` proves the D-Bus
source alone (frames, keyboard, absolute pointer through vmmouse or USB
tablet, buttons).

Moved to C2: native viewer window on a Horcrux desktop (Wayland), audio
forwarding to the client (QEMU audio is received but not sent), the `clip` and
`console` data channels, and a latency run on an unloaded node. Needs real GPU
hardware: VA-API and NVENC encode (code paths present, only software x264 ran).

### C2 status (part 1, 2026-11-19)

Done:
- Guest tools wire protocol (`crates/horcrux-guest-proto`, section 4.1) and
  the guest-tools port on every console VM (`[console] guest_tools`,
  `guest_tools_port`; virtio-serial, host end `<disk>.hxgt.sock`).
- Broker: connects to the tools when a viewer opens (`guest_tools_wait_ms`),
  asks for in-guest capture + encode with the profile's settings
  (`StartVideo`: fps, CBR low delay, no B-frames, GOP, fixed rate, chroma,
  HDR, intra refresh) and forwards the guest's H.264 access units into the
  WebRTC track without transcoding; its own encoder rests meanwhile and takes
  over again if the tools stop or fail. Keyframe requests go to the guest.
  Re-sends the stream requests when the tools reopen their end.
- Data channels `clip` (VM.Console.Clipboard, `clipboard_max_bytes`), `pad`
  (W3C standard gamepad -> tools), `console` (the VM's serial console; input
  needs VM.Console.Input), `ctl` `resize` (-> tools layout), raw relative
  mouse through the tools under pointer lock, guest Opus audio on a WebRTC
  audio track. The web client has pointer lock for `gaming`, gamepad polling,
  paste -> guest clipboard, guest clipboard -> browser, window-size resize,
  audio playback, the serial text channel.
- `gaming` profile: 120 fps fixed rate by default (up to 360), CBR, no
  B-frames, 2 s GOP; per-profile `fixed_rate`, `chroma` (420/444), `hdr`,
  `intra_refresh`, `guest_capture` settings (config.toml, API, declarative
  config, System > Horcrux Console).
- Linux guest tools (`horcrux-guest-agent`, static musl build; package
  `app-emulation/horcrux-guest-tools` with OpenRC and systemd services):
  FFmpeg kmsgrab (console or any KMS desktop) or x11grab capture, NVENC /
  VA-API / libx264 encode, uinput keyboard + relative mouse + absolute
  pointer + up to 4 gamepads, clipboard (wl-clipboard/xclip) and layout
  (xrandr with CVT-RB modes for any size and refresh) through the active
  graphical session, PulseAudio/PipeWire -> Opus.
- Windows guest tools (same binary for x86_64-pc-windows-gnu, MSI built with
  wixl, service `HorcruxGuestTools` that starts the agent in the active
  console session): DXGI Desktop Duplication (GDI fallback) -> Media
  Foundation H.264 (hardware MFTs = NVENC/AMF/QSV first, Microsoft software
  MFT fallback; low-latency CBR, no B-frames, forced keyframes), SendInput
  keyboard/mouse (scancodes, extended keys, absolute over the virtual
  desktop, raw relative), CF_UNICODETEXT clipboard with a change listener,
  display modes through ChangeDisplaySettingsEx, overlapped I/O on the
  virtio-serial port.
- Delivery: the guest tools CD (`guest_tools_iso`, label HXTOOLS, virtio-scsi)
  is attached to console VMs; with `guest_tools_auto_install` the node installs
  the MSI through the QEMU guest agent when a viewer opens and no tools answer
  (Windows). CI builds the MSI, the static Linux agent and the CD image
  (`guest-tools.yml`, ci/build-guest-tools-{msi,iso}.sh).

Proven:
- Linux, real VM through the virtio-serial guest-tools port
  (`crates/horcrux-guest-tools/tests/linux_guest_e2e.py`, CI `guest-tools.yml`,
  Alpine 3.24 guest with the tools CD, KVM): probed capabilities (kmsgrab,
  software encoder, gamepad, relative mouse; no clipboard/resize claimed
  without a graphical session), in-guest kmsgrab + x264 stream that ffprobe
  decodes at the guest's mode (1280x800, every packet a frame, first packet
  an IDR), keyframe on request, a shell command typed through the tools'
  uinput keyboard, a uinput gamepad and raw relative motion reaching
  /dev/input/mice, keystroke-to-encoded-frame latency (section 6).
- Windows Server 2022 (Windows eval VM, real desktop session, harness
  `crates/horcrux-guest-tools/tests/win_harness.py` speaking the protocol over
  TCP): Hello/caps, 1280x720 30 fps Media Foundation software H.264 that
  ffprobe decodes (181/181 frames, Main profile, no B-frames), keyframe 29 ms
  after the request, damage-only mode (0 packets idle), native size, SendInput
  keyboard incl. extended keys, absolute pointer to the exact pixel, raw
  relative motion (moves the cursor; Windows pointer acceleration scales it,
  which games reading raw input do not see), Unicode
  clipboard both ways without echo, display mode change and restore, MSI
  install -> service in session 0 + agent in session 1, clean uninstall.

Still open in C2 (queued as C2b-C2d):
- C2b: a Windows guest in Horcrux CI with the tools installed automatically
  (CD + guest agent) and viewed through the browser client with in-guest
  encode, including the virtio-serial transport on Windows (only TCP ran);
  WASAPI loopback audio; Linux guest through the full broker + browser path
  (proven here at the port level); deb/rpm/apk packages for non-Gentoo
  Linux guests; the API action "install guest tools".
- C2c: done, see "C2c status" below. Signing without a paid certificate: see
  "Windows driver signing without a paid certificate" (XInput via the optional
  ViGEmBus component and the Windows on Arm package are in; an Arm64 guest run
  is open).
- C2d: the same-host ivshmem raw-frame path and the native viewer window on a
  Horcrux desktop (Wayland), QEMU (no-tools) audio to Opus, a latency run on
  an unloaded node for the host-encode path.
- Needs real GPU hardware: NVENC/AMF/QSV inside a Windows guest, VA-API/NVENC
  inside a Linux guest, HDR, 4:4:4 in hardware encoders, 240 Hz capture.

### Windows driver signing without a paid certificate (decision 2026-11-20)

Microsoft's attestation signing (Partner Center, Hardware Dev Center) needs
an EV code-signing certificate, which costs money every year. Horcrux does not
enrol in it. The rule instead: use the least privileged mechanism that does the
job, in this order, and say plainly what each step costs the guest's owner.

| # | Mechanism | Used for | Needs a Microsoft signature? | Status |
|---|---|---|---|---|
| 1 | Plain user-mode APIs (no driver) | clipboard, resize, capture (Desktop Duplication / swap-chain), H.264 encode (Media Foundation), keyboard/mouse (`SendInput`), audio (WASAPI) | no | shipped |
| 2 | Inbox Microsoft drivers | the HID pass-through (`mshidumdf`) and the UMDF reflector under our pad; IddCx under our display | already signed by Microsoft | shipped |
| 3 | Our own UMDF 2 (user-mode) drivers, signed per guest | `hxidd` virtual display, `hxpad` virtual HID gamepad | no: a user-mode driver package only needs a catalog signature the guest trusts | x64 proven on a real guest; Arm64 builds in CI, not yet run on an Arm64 guest |
| 4 | Drivers somebody else already had signed, redistributable under their licence | virtio (Red Hat's virtio-win), optional XInput bus (ViGEmBus) | yes, and they have it | see below |
| 5 | A kernel driver of our own | nothing needs one today | yes | not built; see "What is left" |

Step 3 in detail. At install the MSI runs `--drivers install` as SYSTEM: a
code-signing certificate is generated for that one guest, its public part goes
into LocalMachine Root and TrustedPublisher, both driver catalogs are signed,
and the private key is deleted at once (non-exportable, then removed), so
nothing else can ever be signed with it. Uninstall removes the certificate.
Honest costs: the guest trusts one extra self-made publisher for code signing
(anyone with admin rights in the guest can see it, and the key no longer
exists); a guest whose Code Integrity policy (a WDAC/HVCI policy that pins
publishers) refuses unknown publishers refuses it; nothing about Secure Boot,
test mode, the hypervisor signature or the CPUID bits is touched. The status
output (`--drivers status`, field `signing`) reports `secure_boot` and
`test_signing` as read from the guest, so the UI can show the guest's state
without changing it.

Step 4a: virtio-win (Red Hat/Fedora). The sources are BSD-3-Clause; the
Windows 10+ binaries are signed with Microsoft's attestation signature by Red
Hat, and the project ships ARM64 builds (`win10_arm64` and `win11_arm64` in
kvm-guest-drivers-windows). Horcrux already fetches the virtio-win ISO into
the node's ISO library for installs and attaches it to the guest. It does not
re-host the image: the packaging README states origin and signing but no
explicit licence for the binary image, so a pinned download by the node is
the conservative form. Storage, network, balloon, serial, RNG, input and vsock
therefore need no signature of ours. (virtio-win's own `viogpu` display driver
is not used: it is a basic display driver, not the IddCx virtual display the
streaming path needs.)

Step 4b: XInput. XInput lists only Xbox-class (XUSB) devices, and an
Xbox-class device exists only through a kernel driver that Microsoft signed.
Horcrux will not present Microsoft's vendor ids through a driver of its own.
So the Horcrux HID pad (DirectInput, SDL, Windows.Gaming.Input, Steam Input)
is the default, and XInput-only games are served by an opt-in third-party
component:

- ViGEmBus 1.22.0 by Nefarius Software Solutions, BSD-3-Clause, x64, x86 and
  Arm64 in one setup, a production-signed release. Upstream retired the
  project in 2023 (repository archived), so no further updates or
  compatibility fixes are to be expected; the UI says so.
- Packaged as an optional MSI feature (`ADDLOCAL=Main,XInput`, off by
  default), unmodified, with its licence and a README next to it; never run
  automatically. `horcrux-guest-agent --drivers install-xinput` runs it after
  checking the setup's SHA-256 (setting `vigem_sha256`) and the publisher of
  its Authenticode signature (`vigem_signer`); a different or tampered file is
  refused. It then sets `gamepad_backend = xinput` and removes the Horcrux HID
  pads (two pads would show up as two players). `--drivers remove-xinput`
  reverses it; the bus is uninstalled only if Horcrux installed it (marker
  file), a bus the owner installed is left alone.
- The agent talks to the bus through its public IOCTL interface (ViGEmClient
  `BusShared.h`, MIT; the layouts are unit-tested against the header) and the
  bus drops the controllers when the agent exits.
- Costs, stated once: a third-party kernel driver with an Xbox 360 identity
  in the guest; some anti-cheat systems object to virtual-controller drivers
  (check the game's rules before enabling it; it stays off by default); no
  rumble feedback to the viewer yet; ViGEmBus 1.22.0's own setup refuses
  Windows Server (a launch condition in the setup), so it is for Windows 10/11
  client guests, which is what gaming guests are.
- Proof so far (Windows Server 2022 eval guest, 2026-11-20): the MSI feature
  path runs `--drivers install-xinput` as SYSTEM; the setup's SHA-256 and
  Authenticode checks pass for the real file (publisher "Nefarius Software
  Solutions e.U.", status Valid) and refuse a wrong hash, a wrong publisher
  and a file with one byte appended; the setup's own refusal on Server is
  reported with its reason (`ViGEm Bus Driver can not be installed on the
  following Windows versions ... Windows Server x64`), leaves no marker and
  keeps the Horcrux HID pad; MSI uninstall removes everything (devices,
  packages, certificate, files). NOT yet proven: the bus install and the
  XInput feed (`XInputGetState` through `--gamepad-test` with
  `gamepad_backend = xinput`) on a Windows 10/11 client guest; the IOCTL
  layouts are unit-tested against the bus header only.

Windows on Arm. The drivers and the agent are cross-built for Arm64 in CI
(drivers with clang-cl against the Arm64 WDK libraries, the agent with
llvm-mingw as linker) and packed into an `-arm64.msi` (wixl has no Arm64 mode,
so the template is patched to `Arm64;1033`; CI checks that). The INFs carry
`NTarm64` models. The signing path is the same per-guest UMDF path as on x64,
and Windows applies the same package signature policy to user-mode drivers on
Arm64, but no Arm64 Windows guest exists in CI, so a run there is still to be
done. Steps for whoever has an Arm64 host and guest: install the `-arm64.msi`,
then `horcrux-guest-agent --drivers status` must list both packages, the
`signing` block and `gamepads: 1`.

What is left, and the self-signed path for it. If a kernel driver of our own
is ever needed (nothing is today), the options without a Microsoft signature
are test-signing (`bcdedit /set testsigning on`: refused while Secure Boot is
on, shows a watermark, and anti-cheat systems refuse to run in it) or a guest
with Secure Boot off. Horcrux never switches either on by itself and never
hides the hypervisor or spoofs hardware to get around a game's checks: a title
that will not run in a VM is listed per game, not evaded. A kernel driver
would follow the same rule as ours: no Microsoft vendor ids.

### C2c status (Windows guest drivers, 2026-11-20)

Two user-mode drivers in the Windows guest tools, written from Microsoft's
IddCx, UMDF 2 and HID documentation and the WDK header declarations only
(GPL-3.0-or-later; no Windows-driver-samples code):
`crates/horcrux-guest-tools/drivers/windows/{hxidd,hxpad,include,common}`.

- **hxidd, Horcrux virtual display** (IddCx 1.4, UMDF 2.25, Windows 10 1903+ /
  Server 2022 / Windows 11). One software display adapter (`SWD\HorcruxEnum\HxIdd0`,
  Display class) with 1-16 monitors that exist only while the guest tools ask
  for them. A private control interface (`include/hxdrv.h`, mirrored in
  `src/hxdrv.rs`, size-checked on both sides) sets the monitors and their mode
  lists (any WxH, refresh in millihertz, 240+ Hz) and reports the committed
  mode, swap-chain frames and errors. Monitors can persist across reboots
  (driver setting); the agent puts them back in their requested mode at start
  because Windows picks its own mode for a returning monitor without EDID.
  With `capture = idd` every swap-chain frame is copied on the GPU into a
  named keyed-mutex texture (`Global\HorcruxIdd{id}Frame{generation}`, readable
  by SYSTEM, administrators, interactive and local-service users) that the
  agent opens directly: an alternative to Desktop Duplication.
- **hxpad, Horcrux virtual gamepad** (UMDF 2 HID minidriver under the inbox
  MsHidUmdf.sys). Report 1 is a standard Game Pad collection (16 buttons, 11 used in
  Xbox-style order; hat, 16-bit sticks, 8-bit triggers); report 2 is a vendor
  output collection the agent writes `Msg::Input Gamepad` states to. Vendor/
  product id default to 0 (Horcrux owns no USB vendor id; no vendor's ids are
  borrowed) and, like the strings, are settings in the device's hardware key.
- **Agent**: `virtual_display = auto|on|off` (auto = when the node's
  `HostHello.virtual_display` asks), the mode list, default rate, monitor cap,
  persistence, primary-display choice and timeouts are guest-tools settings;
  `SetLayout` sizes the virtual monitors and the `Layout` reply marks them
  (`Monitor.virtual_display`). `Caps.virtual_display`/`Caps.gamepad` are probed
  (driver present), not assumed. `--drivers install|remove|status`,
  `--layout WxH@HZ[,...]` and `--gamepad-test` are one-shot commands.
- **Node**: `[console] virtual_display = auto|always|never` (API, declarative
  config, UI). `auto` asks for virtual monitors when the GPU hand-off record
  (`crates/horcrux-vm/src/vm/handoff.rs`, new `gpu_outputs`: DRM connectors and
  attached monitors per handed-off GPU, read from sysfs before the hand-off)
  shows a GPU without outputs or without a monitor.
- **Build** (`ci/build-guest-drivers.sh`): clang-cl + lld-link on Linux against
  Microsoft's WDK/SDK nuget packages (pinned versions and sha256, downloaded at
  build time, never shipped); no Windows build host and no use of the
  Rustinion Windows runner. The MSI (`ci/build-guest-tools-msi.sh`) carries
  both packages; CI (`guest-tools.yml`) checks the MSI tables and that both
  DLLs only import system DLLs and export `FxDriverEntryUm`.
- **Signing v1**: at install the MSI runs the agent's `--drivers install` as
  SYSTEM: a code-signing certificate is generated for that guest, its public
  part goes to LocalMachine Root + TrustedPublisher, both catalogs are signed,
  and the private key is deleted at once (nothing else can be signed with it).
  Works on x64 with Secure Boot on and without test mode. Uninstall removes
  the devices, packages and certificate. `HXDRIVERS=0` skips the drivers.

Proof on the Windows Server 2022 eval VM (`tests/win_harness.py`, new tests
`virtual-display`, `video-virtual`, `gamepad`; harness flags
`--virtual-display --expect-gamepad [--expect-capture idd]`):
- MSI install from the customer path: both devices OK, catalogs `Valid`,
  certificate in Root/TrustedPublisher without a private key, service running.
- A virtual monitor at 2560x1440@240 becomes the primary display (EnumDisplaySettings
  and Win32_VideoController report 2560x1440@240; mode list incl. 3840x2160@120
  and 1920x1080@240) and comes back at 2560x1440@240 after a guest restart.
- Capture + encode of it at 2560x1440, 60 fps, software H.264 (no GPU in the
  VM): Desktop Duplication 294 frames in 6 s (encode median 18.6 ms), IDD
  swap-chain capture 350 of 360 frames (encode median 12.6 ms, p95 15.6 ms);
  both streams decode cleanly with ffprobe (294/294 and 350/350 frames,
  2560x1440, no B-frames), IDR 31-37 ms after a keyframe request.
- Gamepad states sent as `Msg::Input` reach the game-controller API joy.cpl and
  DirectInput games use (winmm `joyGetPosEx`): buttons, hat (45 degree
  up-right = 4500), both sticks and both analog triggers, for every state.
- Bugs found and fixed by these runs: UMDF queues refuse an internal-IOCTL
  callback (hxpad did not start, code 10); a monitor re-plug with the swap
  thread still running hit an IddCx verifier stop (the thread is now stopped
  before the departure); software devices lost after a hard power-off are
  re-created by the service at start.

Open:
- **XInput**: solved without a driver of ours (optional third-party ViGEmBus
  component, see "Windows driver signing without a paid certificate"). Open:
  the bus install + `XInputGetState` feed proof on a Windows 10/11 client
  guest (the eval Server guest is refused by the bus setup), rumble feedback
  to the viewer, a UI toggle for the XInput option.
- **Windows on Arm**: the package builds in CI; a run on an Arm64 Windows guest
  is open (no Arm64 Windows runner or Arm64 Horcrux node in CI).
- **C2b CI Windows guest**: add the three harness tests to that job once it is
  on main.
- **Real GPU hardware**: 240 Hz with a hardware encoder (NVENC/AMF/QSV) on a
  passed-through GPU without outputs, the zero-copy encode from the shared
  texture (today the agent maps it to system memory for the software encoder),
  and the `auto` decision on a real headless GPU (`gpu_outputs` is unit-tested
  against sysfs fixtures only).
