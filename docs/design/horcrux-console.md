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
     In p2p mode QEMU serves these objects on only its newest client
     connection (each `add_client` replaces and closes the previous one), so
     all viewer sessions of a VM share one control connection for keyboard
     and mouse, and one audio listener fanned out to every session (QEMU
     takes a single audio-out listener per VM in p2p mode); each session
     registers its own display listener. A connection lost to another D-Bus
     client is re-opened on the next input event.
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
| audio | tools capture (PulseAudio/PipeWire via FFmpeg on Linux; WASAPI shared-mode loopback of the default render endpoint -> libopus in the agent on Windows) -> Opus -> str0m audio track |
| serial text | client `console` channel <-> the VM's serial socket (input needs VM.Console.Input) |

Delivery: every console VM run gets the guest tools CD (`guest_tools_iso`,
label `HXTOOLS`, on its own virtio-scsi controller) when the node has the
image. When a viewer opens, no tools answer and the QEMU guest agent runs,
the node installs them (`guest_tools_auto_install`): Windows guests get the
MSI from the CD through `guest-exec` (Windows Installer, removable like any
package). Linux guests install the package for their distribution from the CD:
`linux/x86_64/horcrux-guest-tools.{deb,rpm,apk}` (built with nfpm by
`ci/build-guest-tools-pkgs.sh`; each holds the static agent, the config file
kept on upgrade, the systemd unit and the OpenRC script; depends on `ffmpeg`;
not enabled by the package, the node does that; removal stops and disables
it under either init). Gentoo uses the `app-emulation/horcrux-guest-tools`
ebuild. The CD also carries the loose static agent and service files
(`linux/`) for other distributions.

## 4.2 Same-host frame ring (C2d)

When the viewer runs on the node that runs the VM, frames skip encode and
decode: the guest tools write raw pixels into shared memory and the native
viewer copies them out. Designed from first principles for Horcrux; no code
or layout is taken from any other shared-memory frame transport
(`crates/horcrux-guest-proto/src/ring.rs`).

- Device: every console VM gets `-object memory-backend-file,id=hxfbmem,
  size=<shm_frames_size_mib>M,mem-path=<shm_frames_dir>/horcrux-frames-<vm>,
  share=on` and `-device ivshmem-plain,memdev=hxfbmem,id=hxframes` when
  `[console] shm_frames` is on (default on; size a power of two, default
  128 MiB = three 3840x2160 frames; `shm_frames_slots` 2-8, default 3;
  directory default `/dev/shm`, a hugetlbfs mount works too). The node
  creates the file root-only (0600) and writes the header before QEMU
  starts, removes it when the VM stops, and never touches it otherwise.
- Layout: 4 KiB header (magic `HXFRAME1`, version, region size, slot count,
  wanted fps, slot offset/stride, cursor offset, `latest = seq << 8 | slot`,
  reader clock, reader slot, reader fps, writer heartbeat/state/info), a
  cursor slot (256-byte header + up to 256x256 BGRA) and the frame slots
  (256-byte header: `seq_begin`, `seq_end`, size, stride, DRM fourcc, flags,
  capture/publish times and the reader clock copied at publish, then the
  pixels). Little-endian, every shared field a 32/64-bit atomic.
- Consistency: the writer sets `seq_begin = n`, writes pixels and fields,
  `seq_end = n`, then `latest`; the reader accepts a copy only when both
  sequence numbers equal `n` before and after it. The writer never reuses
  the slot named by `latest` or `reader_slot`, so with three slots a reader
  that keeps up never retries.
- Demand-driven: the viewer stamps its CLOCK_MONOTONIC into the header on
  every poll; the guest tools capture only while that stamp moves
  (`shm_reader_timeout_ms`, default 2 s) and stop otherwise, so an idle
  ring costs nothing. A stamp left behind by a viewer that went away does
  not count.
- Guest side: Linux maps BAR 2 through sysfs (`resource2_wc`,
  write-combined, `shm_write_combine`: through the uncached mapping a
  1280x800 frame took 65 ms to write on an AMD host, write-combined 2.7 ms) and captures with FFmpeg kmsgrab
  or x11grab as raw BGRX; Windows uses the virtio-win IVSHMEM driver
  (`IOCTL_IVSHMEM_REQUEST_MMAP`, write-combined) and DXGI Desktop
  Duplication. Capabilities report `shm_frames`.
- Trust: each side checks every size the other wrote against the slot
  before copying, so a hostile guest can only produce wrong pixels.

## 4.3 Local viewer socket (C2d)

The native viewer (`horcrux-viewer`, `crates/horcrux-viewer`, USE flag
`viewer` of app-emulation/horcrux) signs in with a Horcrux account and asks
for a local ticket: `POST /api/console/<vm>/hxc/local-ticket {uid, profile?}`
(same RBAC as any console ticket: VM.Console, input only with
VM.Console.Input). The ticket is single use, short lived and bound to the
viewer's Unix uid. The viewer connects to `[console] local_socket` (default
`/run/horcrux/console.sock`); the node reads the peer's uid (SO_PEERCRED),
redeems the ticket and answers `WELCOME` with the frame ring's file
descriptor attached (SCM_RIGHTS), so the ring file itself stays root-only.
Framing is the guest-tools framing (`len u32 LE`, `kind u8`, payload;
`crates/horcrux-guest-proto/src/local.rs`); input uses the WebRTC data
channel encodings (`keys` JSON, `input` binary motion, `pad` 16-byte
gamepad, `ctl` ping), cursor and layout come back as JSON. Keyboard and
absolute pointer go through QEMU's D-Bus input like the WebRTC path; raw
relative motion and gamepads go to the guest tools. Sessions appear in the
console session list with `video_source = shm` and are audited (start, end,
refused tickets).

The viewer window (winit + softbuffer, Wayland or X11, no GPU needed) shows
the newest ring frame scaled to fit, sends keyboard and mouse, grabs the
pointer for raw relative motion (Ctrl+Alt+`grab_key`, on by default for
the `gaming` profile), toggles fullscreen (Ctrl+Alt+`fullscreen_key`) and
forwards up to four evdev gamepads in the W3C standard layout. Every value
is a setting (`~/.config/horcrux/viewer.conf`, flags override).
`horcrux-viewer measure` is the same reader without a window (tests and
latency runs).

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

| Path | Target | Budget (gaming profile, 240 Hz) | Measured (2026-11-20, unloaded node, details below) |
|---|---|---|---|
| Same host, raw frames (ivshmem) | < 8 ms frame-to-viewer | capture 1-2 ms, copy 1 ms, present up to 1 frame | **met**: median 2.0 ms, p95 4.0 ms at 120 fps; median 2.7-2.8 ms, p95 3.7 ms at 60 fps (1280x800, without window present) |
| LAN, encoded | < 20 ms click-to-photon | input 1 ms, game frame 4 ms, capture 1 ms, encode 2-4 ms, network 1-2 ms, decode 2-4 ms, present 4 ms | guest half only: keystroke to encoded frame at the node median 16.5 ms (60 fps, software x264 in a 2-vCPU guest); full path needs GPU hardware |
| WAN | LAN + RTT + jitter buffer | adaptive bitrate fits the uplink; FEC instead of retransmission when RTT is high | C3 |
| Desktop profile, LAN | < 50 ms, text lossless within 500 ms of last change | | **met**: keystroke to picture in the browser median 41.6-45.7 ms in five runs, p90 51-61 ms, none of 150 keystrokes over 500 ms (was 64.6 / 66.0 ms before the QEMU display poll fix, and 7-20 % of keystrokes at ~1 s before the settle frame), QEMU display poll 120 Hz, frame bursts of 2, settle frame, host software encode, no tools, shared build host at load 3-7 (details: "Measured: QEMU display poll rate" and "Measured: settle frame") |

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

### Measured in C2b (guest tools, full path: guest encode -> node -> WebRTC -> browser)

Source: `horcrux-api/tests/hx_console_guest_e2e.{sh,py}` (CI workflow
`hx-console-guest-e2e.yml`), VMs created through the API on a single node,
QEMU 10.2 with KVM, headless Chromium on the same host, software decode.
Keystroke to picture = from the client sending the key until a decoded frame
whose pixels changed. No GPU anywhere: the guests encode in software.

| Measurement | Result | Conditions |
|---|---|---|
| Keystroke to picture, Debian 13 console, guest tools (FFmpeg kmsgrab + x264 in the guest) | median 76-101 ms, p90 151-178 ms (16/16 per run, 2 runs) | 2 vCPUs, 1280x800, shared rw-man host with other CI jobs |
| Keystroke to picture, Windows 11 text field (Notepad / Run box), guest tools (DXGI duplication + Media Foundation software H.264) | median 189-195 ms, p90 231-614 ms, fastest 3.5-29 ms (every keystroke seen, 3 runs) | 4 vCPUs, 1280x800, a minute after the tools were installed (Windows still busy: search indexing, first-logon tasks), same shared host |
| In-guest encode time per frame, Windows (agent-reported) | 24-64 ms |
| Absolute pointer + button to picture (QEMU tablet, desktop selection rectangle) | 497-617 ms | same Windows guest |
| Guest audio | ~100 Opus packets/s received by the browser (getStats), non-zero audio energy | HDA sound card, WASAPI loopback | Media Foundation's software MFT on 4 vCPUs |
| Data-channel round trip | 2 ms | |

Reading: the fastest samples (a few ms to 29 ms) show the path itself
adds little; the
medians are dominated by software encode inside a 4-vCPU guest and by the
shared CI host, not by the transport. The < 20 ms LAN gaming target needs a
GPU encoder in the guest (NVENC/AMF/QSV, code present, untested without GPU
hardware) and an unloaded node.

### Measured in C3 (WAN paths under netem, software x264, headless Chromium)

Source: `horcrux-api/tests/hx_console_wan_e2e.{sh,py}` (CI job
`hx-console-wan-e2e` in `hx-console-e2e.yml`), Alpine guest under KVM
(QEMU 10.2), the browser in its own network namespace behind a veth link
shaped with netem on both ends (half the RTT each way, loss each way, rate on
the video direction with a ~100 ms queue). Keystroke to picture as in C1
(Linux console, 1280x800, 14 keystrokes per path). Run on a shared 8-core
build host (load 4-6) on 2026-11-20; 85/85 checks passed.

| Path (netem) | Keystroke to picture median / min / p90 | ICE RTT | Lost / NACKed packets | Jitter buffer | Link estimate -> target |
|---|---|---|---|---|---|
| LAN direct, link adaptation off | 66 / 46 / 85 ms | 1 ms | 0 / 0 | 4 ms | fixed 4 Mbit/s |
| LAN direct, adaptation on | 86 / 71 / 477 ms | 1 ms | 0 / 0 | 24 ms | 4.4 Mbit/s -> 3.8 Mbit/s 1280x800@30 |
| Relay (TURN/UDP), 20 ms RTT | 110 / 89 / 120 ms | 21 ms | 0 / 0 | 33 ms | 4.3 -> 3.6 Mbit/s |
| Relay UDP, 50 ms RTT, 1% loss | 160 / 108 / 297 ms | 51 ms | 7 / 8 | 37 ms | 4.1 -> 3.9 Mbit/s |
| Relay UDP, 100 ms RTT, 2% loss | 273 / 176 / 862 ms | 101 ms | 9 / 10 | 52 ms | 4.6 -> 3.7 Mbit/s |
| Direct UDP blocked -> automatic relay fallback, 40 ms | 170 / 106 / 727 ms | 41 ms | 0 / 0 | 24 ms | 4.5 -> 4.2 Mbit/s |
| All UDP blocked -> relay over TCP, 40 ms | 130 / 98 / 680 ms | 41 ms | 0 / 0 | 20 ms | 4.2 -> 4.0 Mbit/s |
| Relay, 3 Mbit/s link, 40 ms, screen flooded | 168 / 89 / 872 ms | 42 ms | 1 / 1 | 69 ms | 0.4-2.6 Mbit/s -> 2.3 Mbit/s, 6 fps decoded |
| Relay, uplink setting 1 Mbit/s, 20 ms | 311 / 96 / 895 ms | 22 ms | 0 / 0 | 14 ms | capped at 1.0 Mbit/s 1280x800@19 (limit `uplink`) |

Reading:
- Every path works end to end, including UDP-blocked networks (TURN over TCP)
  and automatic fallback; the relay forwards ciphertext only and its
  per-session allocations are gone 0.5 s after the page closes (client `bye`).
- Total = LAN total + the added RTT + a larger jitter buffer; loss is
  repaired by NACK/RTX (`nack_history_packets`) at the cost of one RTT.
- Link adaptation costs latency today: the host encoder is the FFmpeg CLI,
  which cannot change bitrate, size or frame rate in-stream, so a new target
  restarts it (with hysteresis: new size/fps, or bitrate -30% / +50%) and the
  restart costs a keyframe and a few hundred ms; the p90 spikes in the table
  are mostly those restarts plus software encode under host load. In-stream
  rate control (encoder library API instead of the CLI; guest tools already
  restart their own encoder in place) is the fix, listed under C3 open items.
- The TWCC estimate is conservative when software encode is CPU-bound and the
  screen is flooded (estimate swings 0.15-2.6 Mbit/s): decoded frame rate
  drops to about 6 fps rather than the picture freezing. Not a GPU-encoder
  number; GPU encode and a real WAN (not netem) remain to be measured.
- FEC is not in place yet: str0m 0.25 has no ULPFEC/FlexFEC sender, so loss
  costs a retransmission RTT (see C3 open items).
- Superseded by the C3b rows below (in-stream encoder rate, FlexFEC,
  playout delay, pacer timing).

### Measured in C3b (in-process encoder, FlexFEC, desktop client)

Same harness and host as C3 (`hx_console_wan_e2e.{sh,py}`, KVM, Chromium in a
netns behind netem), run on 2026-11-20; every check passed.

| Path (netem) | Keystroke to picture median / min / p90 | Notes |
|---|---|---|
| LAN direct, link adaptation off (fixed 4 Mbit/s) | 64.8 / 42.3 / 83.1 ms | libx264 in process |
| LAN direct, adaptation on | 66.8 / 37.8 / 74.8 ms | 1 encoder for the session, rate changed in-stream (C3: 86 / 71 / 477 ms) |
| LAN direct, adaptation on, FFmpeg CLI encoder (`x264_in_process = false`) | 92.0 / 57.7 / 113.1 ms | 4 encoder restarts in the same run |
| LAN direct, browser jitter buffer (`low_latency_playout = false`) | 111.9 / 76.8 / 778.5 ms | 4 of 14 keystrokes wait 560-780 ms in the browser |
| Relay UDP, 100 ms RTT, 2% loss, no FEC | 171.6 / 136.2 / 294.6 ms | 16 lost, 18 NACKs, jitter buffer 57 ms |
| Relay UDP, 100 ms RTT, 2% loss, FlexFEC | 162.5 / 133.6 / 185.1 ms | 121 repair packets (~4%), jitter buffer 9 ms |
| Relay UDP, 100 ms RTT, 5% loss, no FEC | 240.4 / 132.1 / 469.1 ms | 34 lost, 42 NACKs, jitter buffer 107 ms |
| Relay UDP, 100 ms RTT, 5% loss, FlexFEC | 157.4 / 139.3 / 577.8 ms | 142 repair packets, 28 packets rebuilt by the browser over both FEC runs, jitter buffer 9 ms |
| Desktop client, LAN direct | 42.4 / 30.7 / 53.5 ms | headless `horcrux-console`, libavcodec decode, 1280x800 (before the pacer fix: 58.6 / 42.5 / 68 ms) |
| Desktop client, relay, 50 ms RTT | 96.5 / 70.1 / 104.5 ms | (before the pacer fix: 111 / 102 / 123 ms) |

Reading:
- Link adaptation no longer costs latency: the median with adaptation is
  within 2 ms of the fixed rate (target was 5 ms) and the p90 spikes are gone.
  Three causes were found and fixed: the FFmpeg CLI restart per bitrate change
  (now libx264 through its library API, `x264_encoder_reconfig`, rate and IDR
  in-stream; only a new picture size restarts it), the browser's jitter
  buffer (the RTP playout-delay extension 0/0 makes it render each frame on
  decode, setting `low_latency_playout` per profile) and str0m's pacer
  needing one timeout per released packet, which cost a ~1 ms tokio timer
  tick each (a 15-packet frame left ~15 ms late); due timeouts are now
  handled inline.
- FlexFEC turns most single losses into no-wait repairs: the browser's
  jitter buffer stays at ~9 ms instead of 57-107 ms and the 2% p90 drops from
  295 to 185 ms. A second run of the FEC paths gave 161.9 / 192.8 ms (2%, 11
  packets rebuilt) and 162.2 / 191.4 ms (5%, 22 rebuilt). When two losses hit
  one repair set a retransmission is still needed (the 578 ms p90 of the
  first 5% run); a higher `fec_overhead_percent` (default 25) trades
  bandwidth for that tail. The wire format is
  `flexfec-03` (draft-ietf-payload-flexible-fec-scheme-03), which is what
  browsers parse under that name; the e2e reads the browser's own WebRTC log
  and fails if a repair packet is discarded or nothing is rebuilt.
- Sample sizes are small (14-30 keystrokes per path on a shared host): read
  differences under ~10 ms as noise. A full run of every path the same day
  gave LAN 58.6 (adaptive) / 54.3 ms (fixed), 2% loss 180.4 -> 165.0 ms with
  FEC (p90 289 -> 239), 5% loss 168.4 -> 156.1 ms (p90 461 -> 276).
- The desktop client's numbers drop when the host is loaded right after the
  screen-flood paths (one run: 159 / 287 ms medians at a reduced 850x532
  target); the rows above are from a run on a quieter host.

### Measured in C2d (same host and host encode, unloaded node)

Conditions: one x86 laptop-class node (12 threads, 15 GiB), idle apart from
the test (load average 0.5-1.6 during every run); tests in an Ubuntu 24.04
container with KVM, QEMU 8.2.2; Alpine
3.24 guest with 2 vCPUs, 1280x800 console. Sources:
`crates/horcrux-viewer/tests/shm_guest_e2e.py` (ring path, guest tools
writer, `horcrux-viewer measure` reader), `crates/horcrux-guest-tools/tests/
linux_guest_e2e.py` (guest encode) and `horcrux-api/tests/hx_console_e2e.sh`
(host encode, real horcrux-api, headless Chromium client). All three also
run in CI (`guest-tools.yml`, `hx-console-e2e.yml`) on the shared runners,
whose numbers are higher under load.

Frame-to-viewer = guest capture-to-publish (guest clock) + publish-to-seen
(host clock: the guest copies the reader's clock stamp into each slot) +
the viewer's copy out of the ring. Keystroke-to-frame = QMP key event to
the first frame in the viewer with more than 3 pixels changed by more than
64 levels (the C1 detector); the stimulus switches the whole console to
reverse video, and 3 s without input before each series produced 0
changed frames.

| Measurement | Result | Conditions |
|---|---|---|
| Ring frame-to-viewer, 120 fps | median 1.99 ms, p95 3.97 ms (capture->publish 1.26, publish->seen 0.25, copy 0.42) | 930 frames in 8 s (119.8 fps), virtio-vga, write-combined BAR |
| Ring frame-to-viewer, 60 fps | median 2.84 / 2.73 ms, p95 3.65 / 3.72 ms | std VGA / virtio-vga, 59.2 fps |
| Keystroke to raw frame in the viewer (QEMU keyboard, no encode) | median 24.4 ms (min 15.6, p95 28.7) at 120 fps; 32.6-34.2 ms (min 23.8) at 60 fps | includes the guest's own reaction (shell + console redraw) and on average half a capture period |
| Keystroke to encoded frame at the node (guest tools: uinput keyboard, in-guest kmsgrab + x264) | median 16.5 ms, p95 19.8 ms, 40/40 | 60 fps, CBR 2 Mbit/s, encode median 2.1 ms |
| Keystroke to picture in the browser, host encode (no tools) | median 95.3 ms (min 64.5, max 197.7), 10/10 | QEMU D-Bus display, software x264 encode 13.3 ms average, data-channel RTT 2.8 ms, headless Chromium software decode |
| Keystroke to picture in the browser, host encode, per stage (C2d follow-up probe, 30-40 keystrokes per run, 4 runs) | median 54-73 ms total: node input -> QEMU 1.4-2.7 ms, QEMU injected -> first display update 26-36 ms (spread 3-50 ms), update -> encoder 0.9-1.9 ms, encode 5.4-10.8 ms, encoded -> sent 0.1-0.2 ms, sent -> last packet in the browser 4-5 ms typical (half RTT 0.5-1.6 ms), decode 0.8-1.8 ms, compositor hand-off 1-4 ms, frame callback 5-11 ms after presentation | shared 16-core runner host (load 5-11, test capped at 4-6 CPUs), QEMU 10.2.1, std VGA, Alpine console; pacing off vs on made no difference (median 60.4 vs 63.4 ms); the 95 ms row above was a 10-keystroke run without the probe |
| Keystroke to raw frame in the viewer, QEMU keyboard vs guest tools' uinput keyboard (same run) | median 36.3 vs 35.8 ms (min 25.1 vs 24.8, p95 45.8 vs 89.1), 30/30 each | 120 fps ring capture (111 fps delivered), release viewer, same shared host (load 6-8, 6 CPUs) |
| Guest sound to the browser (QEMU D-Bus audio, no tools) | 778-779 Opus packets in 8 s while the guest played a 440 Hz tone, browser `totalAudioEnergy` +0.54-0.59 (0 before), audio level 0.27-0.30 | Alpine + alsa-utils `speaker-test`, intel-hda, 10 ms Opus frames |
| Pre-OS keystroke / pointer move / click to picture, host encode | median 127 / 446 / 370 ms | boot loader 640x480; pointer and click include the guest kernel and `od` printing the event |
| Windows ring writer, ring frame-to-viewer (Server 2022 eval guest, virtio-win IVSHMEM driver 100.76.104.16100, DXGI duplication, `horcrux-viewer measure`) | median 6.5 ms, p95 11.9 ms (capture->publish 5.5, publish->seen 0.32, copy 0.67); 592 frames in 14 s (42 fps, desktop redrawn by a ~56 fps GDI animation; idle desktop ~2 fps) | 1024x768 raw BGRX, 2 vCPUs, std VGA, guest in a container with KVM; writer 0.5-2.7 ms per frame to the BAR (write-combined and uncached mappings showed no difference here) |

Reading:
- The ring path meets its target with room to spare: at 120 fps a frame
  reaches the viewer about 2 ms after the guest captured it. Window
  presentation adds up to one display refresh on top (not in these
  numbers: the measuring reader has no window).
- Keystroke-to-picture on the ring path is dominated by the guest, not the
  transport: after half a capture period (4-8 ms) the remaining 16-26 ms
  are the input path, the guest shell and the console redraw. Keys now go
  through the guest tools' uinput keyboard while the tools run (QEMU input
  before they start, and always for the Windows secure desktop); measured
  side by side on the same guest the two input paths are equal (36.3 vs
  35.8 ms median). The earlier 16.5 ms tools figure counts to an encoded
  frame at a different detector and is not comparable.
- Host encode, per stage (probe: the node stamps input, injection, the first
  display update, encode and send and sends them with the frame; the page
  adds receive, decode and presentation from requestVideoFrameCallback):
  the largest single stage is QEMU injected -> first display update,
  26-36 ms median with a 3-50 ms spread. That is the guest's own reaction
  (~15-20 ms, see the ring rows) plus 0-30 ms waiting for QEMU's display
  refresh timer: `ui/console.c` polls a std VGA/cirrus/qxl display every
  30 ms (GUI_REFRESH_INTERVAL_DEFAULT) unless a listener asks for another
  interval, and the D-Bus listener never does. Encode (5-11 ms software),
  network (4-5 ms on loopback), decode (1-2 ms) and the node's own steps
  (~3 ms) add ~15-20 ms; the browser's frame callback lands 5-11 ms after
  presentation (headless Chromium's frame clock, not seen by a user).
  Zero playout delay (`playout-delay` extension, `jitterBufferTarget = 0`)
  was already on; pacing off changed nothing on a LAN. On a busy shared host
  the medians were 54-73 ms; the QEMU display poll fix and the settle frame
  (next two subsections) brought them under the 50 ms target.
- Write-combining matters on AMD hosts (guest memory types honoured): an
  uncached BAR mapping took 65 ms per 1280x800 frame (14 fps), write-
  combined 2.7 ms. kmsgrab does not hold its frame rate once copy-out is
  faster than the frame period (376 frames/s asked for 120 starved the
  guest), so raw capture caps its output with `-fpsmax`.

### Measured: QEMU display poll rate (2026-11-21)

Where the host-encode time went (per-stage probe of `hx_console_e2e.sh`,
`HXC_LAT_SAMPLES=30`, std VGA, Alpine 1280x800 text console, desktop
profile at 30 fps): of 60-66 ms keystroke-to-picture, 31-35 ms was "QEMU
injected the key -> first D-Bus display update". QEMU polls display devices
that do not report their own changes (std VGA, cirrus, QXL in VGA mode,
virtio-vga before its driver loads) from its gui timer, every 30 ms for a
D-Bus listener (`GUI_REFRESH_INTERVAL_DEFAULT`; only the GTK, SDL, VNC and
SPICE displays set their own interval), so on top of the guest's own ~15 ms
reaction a change waited 0-30 ms. No QEMU release or master (checked
v11.1.3 and master 2026-11-20) lets a D-Bus client change it; the `UIInfo`
refresh rate on master is guest-facing (EDID) and refuses exactly these
devices.

Fix, two parts:
- QEMU: an optional `RefreshRate` property (millihertz) on the client's
  `org.qemu.Display1.Listener`, read when the listener registers and
  followed on PropertiesChanged, turned into that listener's update
  interval like the GTK display does. Carried as a patch in the Horcrux
  overlay (`gentoo/app-emulation/qemu`, ::gentoo 11.1.3 ebuild + patch,
  revision -r100; the binhost builds it). QEMU builds without it ignore the
  property, so nothing else changes on them.
- Node: the listener offers `display_poll_hz` of the session's profile
  (desktop 120, gaming 240, console 60; 0 = QEMU's default). The faster
  poll then exposed a second wait: when a poll catches the guest mid-draw,
  that partial frame used up the 30 fps slot and the completing frame
  waited a whole frame interval. The encoder's cap is now a token bucket
  (`frame_burst`, desktop/console 2, gaming 1): same average rate, but the
  completing frame leaves at once.

Same host (shared 16-thread build host, load 5-11 during the runs; Ubuntu
24.04 container with KVM, 4 CPUs; QEMU 11.1.3 built from the release
tarball, stock vs patched), interleaved, 30 keystrokes per run:

| Setup | Keystroke to picture, median (min-max) | QEMU injected -> display update, median | Node total (input -> frame sent) |
|---|---|---|---|
| Before: stock QEMU, `frame_burst` 1 (two runs) | 66.0 (40.8-107.5) / 64.6 (44.4-94.0) ms | 33.9 / 34.1 ms | 45.0 / 45.5 ms |
| Stock QEMU, `frame_burst` 2 (two runs) | 65.5 / 65.4 ms | 32.6 / 33.1 ms | 43.5 / 44.5 ms |
| Patched QEMU, `display_poll_hz` 0 (= QEMU default, two runs) | 58.5 / 59.9 ms | 31.2 / 35.0 ms | 41.2 / 44.0 ms |
| Patched QEMU, 120 Hz, `frame_burst` 1 (two runs) | 49.0 / 50.2 ms | 18.6 / 16.8 ms | 28.0 / 27.4 ms |
| **After (defaults): patched QEMU, 120 Hz, `frame_burst` 2 (two runs)** | **47.6 (31.7-1050.8) / 49.9 (31.5-1135.1) ms** | **17.1 / 16.4 ms** | **27.2 / 29.2 ms** |
| Patched QEMU, 120 Hz, `max_fps` 60 / 120 (strict gap) | 45.2 / 44.1 ms | 17.9 / 17.0 ms | 27.0 / 27.1 ms |

Reading:
- QEMU injected -> display update drops by 16-18 ms (median), and the
  keystroke-to-picture median by 15-17 ms, to just under the 50 ms desktop
  target. 240 Hz polls gave nothing over 120 Hz (14.1 vs 13.7 ms in the
  same series): the rest is the guest's own reaction.
- The bucket matters for the samples whose first update came within 12 ms
  of the key (the guest still drawing): with a strict 30 fps gap they took
  65-75 ms median, with `frame_burst` 2 44-55 ms. How often that happens
  varies by run (0-14 of 30 samples): in the interleaved series above the
  median gain was about 1 ms; an earlier series of strict-gap 120 Hz runs
  with many such samples had medians of 57-61 ms.
- The ~1.1 s maxima in some runs were a browser rendering effect, found and
  fixed afterwards: see "Measured: settle frame".
- Remaining budget at the median: guest reaction ~15 ms, encode 6 ms, node
  bookkeeping ~3 ms, network + decode ~7 ms, browser present and frame
  callback ~8 ms.

### Measured: settle frame (2026-11-22)

With the poll fix, 2-6 of every 30 keystrokes (7-20 %) still took ~1.05 s,
so p90 was up to 1049 ms while the median looked fine. A frame trace (trace
target `hxc_frames`: input, display update, submit, send) lined up with the
page's stages showed the same picture every time: the node sent the frame
that caught the guest mid-draw and the frame that completed it 6-18 ms
apart, 25-60 ms after the key, the browser received both at once
(`first_receive_ms` 16-48 ms), and the change appeared only with the next
frame the node sent, the idle refresh one second later (with
`idle_refresh_fps` 4 the outliers became ~300 ms). The FFmpeg program
encoder showed it as well, so it is not the in-process encoder. Chromium's
WebRTC renderer holds a frame that arrives sooner than the stream's recent
pace until a newer one comes; after a still screen two frames a few ms
apart are exactly that. Two frames back to back became common once QEMU
polled at 120 Hz and the frame cap allowed bursts, which is why the earlier
30 ms poll with a strict gap never showed it.

Fix (node, every browser): `settle_frame` (per profile, default on;
fixed-rate profiles send every frame anyway): one frame interval after the
last frame
of a change the picture goes out once more, so a held frame is always
pushed out at once. Costs one small frame (~1 KB on a text console) per
change. The page now reports `first_receive_ms`, `frames_after_input` and
`timing_age_ms` (how long the node's answering frame sat in the browser
before its picture showed), and `hx_console_e2e` fails when any keystroke
picture waited more than `HXC_HELD_MS` (400 ms) in the browser; with the
setting off that check failed on the 10-key series.

Same shared 16-thread build host (load 3-7 from other jobs during the
runs), Ubuntu 24.04 container with KVM and 4 CPUs, QEMU 11.1.3 built from
the release tarball, `HXC_LAT_SAMPLES=30`, node built from main 2026-11-22:

| Setup | Keystroke to picture, median | p90 | max | Samples > 500 ms |
|---|---|---|---|---|
| Stock QEMU, defaults before the fix (3 runs) | 59.1 / 59.8 / 57.3 ms | 68-71 ms | 75-236 ms | 0 / 90 |
| Patched QEMU, defaults before the fix (6 runs) | 43.2-49.5 ms | 53-1049 ms | 1053-1133 ms | 20 / 180 |
| Patched QEMU, `frame_burst` 1 (2 runs) | 50.5 / 45.2 ms | 70 / 77 ms | 76 / 96 ms | 0 / 60 |
| **Patched QEMU, settle frame (defaults, 5 runs)** | **41.6 / 42.9 / 44.3 / 45.7 / 45.2 ms** | **51-61 ms** | **91-129 ms** | **0 / 150** |
| Patched QEMU, settle frame, `max_fps` 60 strict gap (2 runs) | 40.0 / 48.7 ms | 58-60 ms | 75-79 ms | 0 / 60 |
| Stock QEMU, settle frame (2 runs) | 55.1 / 53.4 ms | 66-69 ms | 78-80 ms | 0 / 60 |

Reading: the desktop target (< 50 ms) is met at the median in every run
with the patched QEMU and the settle frame, and the second-long stalls are
gone (0 of 210 keystrokes over 500 ms with the fix, 20 of 180 without).
A stock QEMU stays around 55 ms (its 30 ms display poll). The host was
not idle: no lab machine was free, so these are shared-host numbers, which
only flatter the idle case. The first three settle runs repeated the
picture only after bursts (frames closer than one interval); the released
rule repeats it after every change, because a 60 fps strict-gap stream
showed the same hold once in 30 keys before the fix.

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
- A QEMU without the D-Bus listener `RefreshRate` patch (any distribution
  build) ignores the node's `display_poll_hz` and keeps its 30 ms poll:
  the console works the same, about 16 ms slower per change on polled
  display devices.

## 9. Settings (all with defaults; `[console]` in /etc/horcrux/config.toml, API, UI)

Default profile per guest type; per-profile codec preference order, chroma,
max resolution/refresh, bitrate floor/ceiling, keyframe interval, lossless
refinement delay, QEMU display poll rate (`display_poll_hz`), frame
burst (`frame_burst`) and the repeat after a change (`settle_frame`); ticket lifetime; relay enabled/port/realm/credential
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

Chosen for the Windows guest agent's audio (C2b, checked 2026-11-20 on
crates.io): `opus` 0.4 (MIT OR Apache-2.0, maintained, release 2026-08) over
`opusic-sys` 0.7 (BSD-3-Clause, maintained, release 2026-11), which builds
its bundled libopus 1.6.1 (BSD-3-Clause, Xiph.Org) with cmake and links it
statically, so the agent stays one self-contained exe. Windows-only
dependency: the Linux agent keeps encoding with FFmpeg and the static musl
build does not pull it in. libopus is built without `-fstack-protector` and
`_FORTIFY_SOURCE` (opusic-sys features `no-stack-protector`,
`no-fortify-source`) because their runtime lives in GCC's libssp on
mingw-w64 runtimes before v11; the encoder only reads PCM the agent produced
itself. Rejected: `audiopus_sys` (ISC, last release 2021), `magnum-opus`
(last release 2020). Licence texts: THIRD-PARTY-LICENSES.txt (opusic-sys's
LICENSE is libopus's).

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

### C2b status (2026-11-20)

Done and proven in CI (`hx-console-guest-e2e.yml`, KVM runner):
- Test images: `ci/windows-test-image.py` installs Windows 11 (evaluation
  ISO) unattended with Secure Boot, TPM 2.0, virtio-win drivers and the QEMU
  guest agent but WITHOUT the Horcrux tools (40-60 min once); the image is
  cached per runner in the Docker volume `hxc-win-img`, keyed by both ISOs,
  the answer file and its first-logon script (`IMAGE_VERSION`), so later runs
  only import it. `ci/linux-test-image.py` makes a Debian 13 image (official
  nocloud image + FFmpeg + qemu-guest-agent, no Horcrux tools) in about a
  minute.
- Windows end to end: VM created through the API (q35, OVMF Secure Boot,
  TPM, guest agent, HDA sound, virtio video), disk imported with
  `/vms/:id/import-disk`; the node attaches the tools CD; opening a viewer
  makes the node install the MSI through the guest agent (automatic install,
  recorded as `by: auto`) and the OPEN session links to the tools without a
  reconnect (`guest_tools_retry_secs`); the browser then gets the
  guest-encoded stream (`video_source: guest`, `guest_encoder:
  mediafoundation-software`) over the real virtio-serial port; keyboard into
  the Run box's text field, absolute pointer + button (desktop selection rectangle), raw
  relative motion through the tools, clipboard guest -> browser and browser
  -> guest, the guest's Opus audio track in the browser (getStats audio
  packetsReceived grows ~100/s), resize through the tools (Windows switches
  modes; the stream follows).
- Linux end to end: Debian VM created through the API; `POST
  /api/vms/:id/console/guest-tools/install` (VM.Config; a VM.Console-only user
  gets 403 but can read `GET .../guest-tools`) installs the .deb from the CD
  with apt and starts the systemd unit; audit events for request and
  result; the browser gets the guest-encoded stream (FFmpeg kmsgrab + x264)
  and keystrokes come back in it; the VM console page shows the guest tools
  bar with the Install button.
- deb/rpm/apk packages of the Linux tools (nfpm, `ci/build-guest-tools-pkgs.sh`,
  install-tested in Debian, Fedora and Alpine containers in `guest-tools.yml`)
  on the tools CD under `linux/<arch>/`, next to the Gentoo ebuild.
- New settings: `guest_tools_retry_secs` (in-session re-check, default 5 s),
  `guest_tools_install_timeout_secs` (default 600 s),
  `guest_tools_auto_install_retry_secs` (default 600 s); Windows agent:
  `port_write_max`, `port_busy_wait_ms`, `port_busy_tries`,
  `port_write_timeout_ms`, `port_handle` (set by the service).

Found and fixed by running it on real Windows (the C2 part 1 Windows tests
spoke TCP and ran as an administrator):
- The virtio-serial port admits only administrators and SYSTEM, so the agent
  the service starts as the logged-on user could never open it ("Access is
  denied"). The service now opens the port and hands the agent an inherited
  handle (PROC_THREAD_ATTRIBUTE_HANDLE_LIST, only that handle).
- While the node's end is not connected QEMU throttles the port: writes never
  complete and reads fail with ERROR_NO_SYSTEM_RESOURCES. The agent's writer
  blocked forever and the agent never noticed the node connecting. Writes
  now time out (`port_write_timeout_ms`) and a throttled read counts as "node
  not connected"; large writes are split (`port_write_max`).
- The node's port reader outlived a connection attempt to a silent guest and
  kept QEMU's only chardev connection, so later sessions could not reach the
  tools; the reader now ends with the link.
- msiexec through the guest agent: the quoted MSI path did not survive the
  agent's command line (1619); Debian: `systemctl enable` handed the OpenRC
  script to update-rc.d, which refused it without an LSB header.
- The guest agent pauses for seconds while the MSI installs; the install
  wait now retries until its deadline instead of failing on the first
  timeout.
- With std VGA under UEFI Windows' Basic Display Adapter is fixed to the
  firmware's mode, so resize cannot work; the test VM uses virtio video
  (virtio-win's display driver). Resize on std VGA needs the Horcrux virtual
  display (C2c).
- The browser client kept the old guest size after a guest-side mode change
  (pointer mapping); it now follows the decoded size.

### C2b: Windows guest audio (2026-11-20)

The Windows agent captures the default render endpoint (or the endpoint
named by `audio_source`: an endpoint ID or part of its name) with WASAPI
shared-mode loopback on its own COM thread, converts any mix format (float
or PCM, 8-32 bit, any channel count with speaker-mask downmix, any rate
through a windowed-sinc resampler) to the requested Opus rate and channels,
and encodes CBR Opus (`audio_opus_application`, default restricted low
delay; `audio_bitrate_kbps` when StartAudio leaves the bitrate at 0) in 5-60
ms frames. Loopback delivers nothing while nothing plays, so the stream is
clocked by the wall clock: after `audio_idle_ms` without samples the agent
encodes silence for the elapsed time (at most `audio_max_fill_ms` at once)
and wakes when the next idle frame is due, so the WebRTC track keeps its
cadence. A vanished endpoint or a new default device is reported with a
`Log` message and reopened (`audio_retry_ms`, `audio_device_check_ms`).
`caps.audio` is probed at Hello (an endpoint exists). The conversion logic
is platform-independent and unit-tested on Linux (`src/audio.rs`).

Proven on Windows Server 2022 (eval VM with a QEMU HDA sound card,
`win_harness.py --tests audio,audio-recover`): first packet 97 ms after
StartAudio; 300 packets in 3 s while silent with capture times exactly
10 ms apart (arrival median 10.0 ms, p95 14 ms); every packet 160 bytes
(128 kbit/s CBR); a 1 kHz tone played in the guest session decodes to
1000.0 Hz at -5.97 dBFS (played at -6.02 dBFS) with digital silence before
it; 20 ms frames; nothing after StopAudio; the Windows Audio service
stopped under a running stream -> Log + silence at full cadence, restarted
-> capture resumes.

Still open in C2 (queued as C2c-C2d):
- C2c: Horcrux virtual display (IddCx Indirect Display Driver, clean-room from
  Microsoft's documentation; configurable resolution/refresh incl. 240+ Hz,
  used automatically for GPUs without monitors/outputs) and the virtual HID
  gamepad for Windows; both are drivers and need signing (below).
- C2d: done, see "C2d status" below.
- Needs real GPU hardware: NVENC/AMF/QSV inside a Windows guest, VA-API/NVENC
  inside a Linux guest, HDR, 4:4:4 in hardware encoders, 240 Hz capture.

Driver signing (C2c): the IddCx virtual display and a virtual HID gamepad
are user-mode (UMDF 2) drivers. On x64 Windows a UMDF driver package installs
without test mode when its catalog is signed with a code-signing certificate
the guest trusts (Horcrux can add its own publisher certificate to the
guest's TrustedPublisher/Root stores during the MSI install, guest by guest).
Installing without that per-guest trust, on Windows on Arm, and with
stricter Code Integrity policies needs Microsoft's attestation signing
through Partner Center (Hardware Dev Center), which requires an EV
code-signing certificate. Test-signing mode (bcdedit testsigning) is not an
option for gaming VMs: it needs Secure Boot off, which anti-cheat refuses.
Sources: learn.microsoft.com driver signing policy and code-signing
attestation pages.

### C3 status (2026-11-20, C3b and C3c same day)

Done:
- `crates/horcrux-turn`: STUN (RFC 8489) / TURN (RFC 8656) relay and client,
  clean-room from the RFCs (RFC 5769 test vectors), long-term credentials with nonces, channels,
  permissions; UDP and TCP transport to the relay (RFC 8656 framing). It is not an open relay: peers are limited to the node's own
  console media sockets (`PeerRule`), and it never terminates DTLS/SRTP.
- Relay built into every node (`[console] wan`, off by default): starts when
  WAN access is enabled, also on a standalone machine; per-session
  TURN-REST-style credentials (`<expiry>:<session tag>`, HMAC-SHA1 of a
  0600 node secret `console-relay.key`) carried in the console ticket's ICE
  servers, revoked when the session ends (allocations dropped at once); a
  relay restart only when a listening setting changes.
- Optional router mapping (opt-in `upnp`): NAT-PMP (RFC 6886) first, then
  UPnP IGD (SSDP + SOAP AddPortMapping, UDP and TCP), renewed at half the
  lease, removed when WAN access is turned off.
- `force_relay` (clients only use the relay), automatic fallback when direct
  UDP is blocked, the session's path (direct/relay) in its stats.
- Link adaptation (`adaptive`, default on): str0m TWCC bandwidth estimate ->
  bitrate, resolution and frame rate (quality floor in bits per pixel,
  minimum bitrate and fps, fast step-down / slow step-up), capped by the
  profile, by the node uplink setting for relayed sessions and by the
  client's display refresh (frame pacing to the client display); the limit
  in force (`link`, `uplink`, `profile`, `floor`) and the numbers are sent to
  the client, which shows what the link allows. NACK/RTX history is a
  setting (`nack_history_packets`).
- `GET /api/console/relay` (Sys.Audit on /): relay listen/public address,
  allocations, auth failures, forbidden peers, late requests of ended
  sessions, bytes, router mapping. Every WAN value is a setting with a
  default in `[console] wan`, the console settings API, the declarative
  config (console kind) and System > Horcrux Console > WAN access.
- WAN e2e under netem (section 6) in CI on the KVM runner.

C3b (2026-11-20):
- In-process H.264 encoder (`crates/horcrux-media/src/x264.rs`): the system
  libx264 loaded at run time (no build-time link, GPL-2+ library used through
  its C API), baseline profile, zero-latency tuning, no reordering (POC type
  2), bitrate/VBV and frame-rate budget changed between two frames, IDR on
  request without a restart. Setting `x264_in_process` (default on; off =
  the FFmpeg CLI path).
- FlexFEC (`crates/horcrux-media/src/flexfec.rs`, `hxc/fec.rs`): clean-room
  from draft-ietf-payload-flexible-fec-scheme-03 (the `flexfec-03` format
  browsers parse); negotiated only when the client offers `flexfec-03`
  (Chromium: behind its FlexFEC field trial), repair stream announced as an
  `FEC-FR` SSRC group, per-frame interleaved repair sets, used while the RTT
  is at least `fec_min_rtt_ms`. Settings `fec` (default on),
  `fec_overhead_percent`, `fec_min_rtt_ms` in `[console] wan`. str0m matches
  RID `None` against every stream of a MID, so with a repair stream the media
  stream carries an internal RID.
- Low-latency playout (`low_latency_playout` per profile, default on),
  `pace_video` (frames paced by str0m's pacer, default on), due pacer
  timeouts handled inline.
- Horcrux desktop client for Linux (`horcrux-desktop`, binary
  `horcrux-console`): the web client's ticket + WebSocket signalling +
  WebRTC (str0m, direct or through the node's relay via `horcrux-turn`),
  H.264 decode through the system libavcodec loaded at run time, winit +
  softbuffer window, raw relative mouse while captured, gamepads via evdev,
  display refresh reported to the node, headless mode for CI (`--headless
  --trials N --exercise-input`, WAN e2e paths `desktop_direct` and
  `desktop_relay_rtt50`). Settings in `~/.config/horcrux/console.toml`
  (`horcrux-console --print-defaults`, packaged example
  `packaging/linux/horcrux-console.toml`), ebuild
  `gentoo/app-emulation/horcrux-console`.

C3c (2026-11-20): the desktop client on Windows and macOS, connection window.
- Per-OS H.264 decoder behind one interface (`crates/horcrux-media/src/decoder.rs`,
  setting `decoder` = auto | avcodec | mediafoundation | videotoolbox, and
  `hardware_decode`): Linux/Unix the system libavcodec (run-time loaded, as in
  C3b), Windows the Media Foundation H.264 decoder MFT (`mfdec.rs`; DXVA through
  a D3D11 device manager when `hardware_decode` is on and the GPU offers it,
  the software MFT otherwise), macOS VideoToolbox (`vtdec.rs`, AVCC samples from
  the Annex B stream, NV12 out). No codec library is shipped on any OS. A
  shared test stream (`tests/data/gray-128x96.h264`) is decoded by each
  backend; `HX_REQUIRE_DECODER=1` makes a missing decoder a test failure.
- Gamepads: evdev (Linux), XInput (Windows, `xinput1_4.dll` loaded at run
  time, Guide button where the system exposes it), GameController.framework
  (macOS, extended gamepad profile), all polled into the same 16-byte pad
  state. Raw relative mouse while captured comes from winit's device events on
  every OS (WM_INPUT on Windows), with the cursor confined/locked by winit.
- Connection window (all OSes, `horcrux-desktop/src/ui.rs`, drawn in the same
  software surface, no toolkit): node URL, user, password, certificate
  fingerprint check with "Trust certificate" (stored per node in the recent
  file next to the settings), VM list from `GET /api/vms` filtered to running
  QEMU VMs whose Horcrux Console is available, profile, Open. Starting the
  program without `--node`/`--vm` (Start menu, Applications, desktop menu)
  opens it.
- `horcrux-console://HOST[:PORT]/VM?profile=P&user=U` links: the web UI's
  console page offers "Open in the desktop app" when the Horcrux Console is
  available; the app fills its connection window from the link and asks for
  the password (nothing secret in the link). Handlers: the MSI's
  `HKCR\horcrux-console`, the Linux `.desktop` file
  (`x-scheme-handler/horcrux-console`), `CFBundleURLTypes` in the macOS bundle.
- Settings per OS: `$XDG_CONFIG_HOME/horcrux/console.toml` (Linux/Unix),
  `%APPDATA%\horcrux\console.toml` (Windows), `~/Library/Application
  Support/horcrux/console.toml` (macOS); the packaged example is still
  `horcrux-console --print-defaults`.
- Packages: Windows MSI (`ci/build-console-msi.sh`, wixl,
  `packaging/windows/horcrux-console.wxs`: program, Start menu entry, link
  handler, settings example), macOS `.pkg` with a universal
  `Horcrux Console.app` (`ci/build-console-pkg.sh`, pkgbuild/productbuild,
  runs on macOS only), Gentoo ebuild with the `.desktop` file.
- CI (`.forgejo/workflows/console-desktop.yml`): Linux tests; Windows and
  macOS (aarch64 + x86_64) clippy from Linux (mingw-w64; zig cc for the C
  parts of the macOS check); MSI built and its tables checked; on the Windows
  Server 2022 runner native MSVC unit tests with Media Foundation decoding the
  test stream, then MSI install / check / removal.
- Proven on a Windows Server 2022 VM (QEMU, software MFT) against a real node
  (debug `horcrux-api` with an Alpine VM): headless runs decoded 1280x800 at
  0 decode errors over the direct path (58-83 frames, keystroke-to-picture
  median 51-75 ms on a busy CI host) and through the node's relay with
  `force_relay` (66 frames, median 50 ms); typed text logged in on the guest's
  console (snapshot of the decoded picture shows the shell); the node counted
  the client's gamepad states (`gamepad_rx` = 2; `gamepad_in` stays 0 because
  that guest has no guest tools to hand them to). The MSI installed with
  msiexec, created the Start menu entry and the link handler; opening a
  `horcrux-console://` link showed the connection window with node and user
  filled in, the certificate fingerprint check, and after "Trust certificate"
  the VM console at ~34 fps.
- New session stat `gamepad_rx` (pad states received from the client), next to
  `gamepad_in` (handed to the guest tools); the WAN e2e checks it for the
  desktop client paths.

C3c follow-ups (2026-11-21):
- Windows build of the desktop client against a real node in CI
  (`.forgejo/workflows/console-desktop-win-e2e.yml`,
  `horcrux-api/tests/hx_console_desktop_win_e2e.{sh,py}`): a node with an
  Alpine console VM, and the cached Windows 11 test image of the guest-tools
  e2e (C2b; now fetched by the shared `ci/windows-test-image-cached.sh`) booted
  as the CLIENT machine with QEMU user-mode networking. The release
  `horcrux-console.exe` (x86_64-pc-windows-gnu) goes in through the QEMU guest
  agent (`guest-file-write`, SHA-256 checked in the guest) and runs there
  headless (`guest-exec`): `--exercise-input --type-text ... --snapshot`,
  direct and with WAN access + `force_relay` (the relay is pinned to the
  node address the guest uses while the node lacks
  `wan.relay_reply_from_request_address`; with it, the relay stays on every
  address). Checks, as for the WAN e2e desktop paths: at least 10
  frames decoded by Media Foundation with 0 decode errors, the path seen by
  the node and by the client, `keys_in` >= 24, `gamepad_rx` >= 2, and the
  decoded snapshot copied back (PNG artifact) and not blank. Run like CI
  (act-26.04, 6 GiB / 4 CPUs, KVM; Windows 11 24H2 client, build 26100,
  software Media Foundation MFT): direct 89 frames 1280x800, 0 decode errors,
  keystroke-to-picture median 62 ms (p90 87), 12/12 keystrokes answered;
  relay 96 frames, 0 errors, median 49 ms (p90 63); both `keys_in` 98,
  `gamepad_rx` 2; the snapshot shows the typed login and command output.
- macOS links: Launch Services hands a clicked `horcrux-console://` link to
  the app as an Apple Event (`kInternetEventClass`/`kAEGetURL`), never on the
  command line, and winit has no handler for it. `horcrux-desktop/src/macos_link.rs`
  registers one with `NSAppleEventManager` (objc2) before the event loop runs;
  the URL goes through the same strict `config::parse_link` and fills the
  connection window (password always asked; a running console keeps going).
  Compile-checked only (clippy for aarch64 and x86_64-apple-darwin in
  console-desktop.yml): there is no macOS runner for this repo, so link
  opening, the VideoToolbox decoder, GameController and the `.pkg` build have
  never been executed on a Mac.
- Connection window, found in Windows QA and fixed: a new window larger than
  the monitor is shrunk to 90 % and, when the system placed it partly off the
  monitor (Windows cascades new windows), centred on it; keyboard focus on a
  button is a ring clear of the button (the outline drawn on the button was
  nearly invisible against its fill); status lines wrap to five lines before
  an ellipsis (the certificate check's advice was cut at three); VM names in
  the list sit at the list's edge (they were indented by the whole list's
  padding). Re-checked on the Windows Server 2022 eval VM (1024x768): link ->
  window wholly on screen with node and user filled in, Tab -> focus ring on
  Connect, Enter -> certificate advice and the full fingerprint inside the
  window, Enter on "Trust certificate" -> the link's VM opened.
- Screenshot review: a unit test (`ui::connection_window_screenshots`) draws
  the window's states with the app's own renderer (empty, from a link with
  focus on Connect, untrusted certificate, VM list with focus on Open, small
  window) and checks focus ring, wrapping and the whole fingerprint;
  console-desktop.yml uploads them as PNGs (artifact
  `console-desktop-connection-window`) for the UX screenshot review.

Open:
- macOS: everything is compile-checked only (no macOS runner exists for this
  repo); first run on a Mac when one is available to CI.
- Relay bound to the unspecified address answers from the address of the
  outgoing route, not the one the request reached (seen with a client inside
  a container on the node itself: the reply came from the bridge address and
  was dropped by the client's NAT). Workaround: set `relay_listen_address`;
  fix: answer with IP_PKTINFO / per-address sockets.
- FlexFEC receive in the desktop client (str0m has no FEC receiver).
- Cluster: relay on the cluster name/VIP (C4), shared relay secret across
  nodes.
- Needs real GPU hardware / a real WAN: GPU-encode numbers per path,
  click-to-photon with a photodiode or high-speed camera, a real residential
  uplink instead of netem.

### C2d status (2026-11-20)

Done:
- Same-host frame ring (section 4.2): the node adds the ivshmem-plain device
  over a root-only file under `[console] shm_frames_dir` to every console VM
  (`shm_frames`, `shm_frames_size_mib`, `shm_frames_slots`; config.toml,
  API, declarative config, System > Horcrux Console), writes the ring header
  before QEMU starts and removes the file when the VM stops. Clean-room ring
  protocol in `crates/horcrux-guest-proto/src/ring.rs`.
- Guest tools write raw frames only while a viewer reads: Linux through the
  write-combined sysfs BAR mapping with FFmpeg kmsgrab/x11grab raw BGRX
  (`-fpsmax` holds the asked rate), Windows through the virtio-win IVSHMEM
  driver with DXGI Desktop Duplication (compiled and clippy-clean for
  x86_64-pc-windows-gnu; not yet run on a Windows guest with the device).
  New guest settings `shm_frames`, `shm_poll_ms`, `shm_reader_timeout_ms`,
  `shm_pci_dir`, `shm_device`, `shm_write_combine`.
- Local viewer socket and local tickets (section 4.3):
  `POST /api/console/<vm>/hxc/local-ticket`, `[console] local_socket`, ring
  descriptor over SCM_RIGHTS, tickets bound to the viewer's uid, single use,
  same RBAC and audit as the WebRTC console; sessions listed with
  `video_source = shm`.
- Native viewer `horcrux-viewer` (`crates/horcrux-viewer`, USE flag `viewer`
  in app-emulation/horcrux, on by default with `qemu`): Wayland/X11 window
  (winit + softbuffer, no GPU needed), keyboard, mouse, pointer lock with raw
  relative motion, fullscreen, up to four evdev gamepads, every value a
  setting; `horcrux-viewer measure` for tests and latency runs.
- QEMU D-Bus audio without guest tools: console VMs with a sound card get
  `-audiodev dbus`; the broker registers an AudioOutListener (bounded: a VM
  without D-Bus audio no longer stalls the console connect), encodes the PCM
  with an FFmpeg libopus child (low delay, one packet per Ogg page,
  `audio_frame_ms` 5/10/20/40/60) and sends it on the WebRTC audio track;
  guest-tools audio takes priority when it arrives.
- Latency run on an unloaded node (section 6, "Measured in C2d").
- Follow-ups (2026-11-21): per-stage latency probe (node stamps sent with
  the frame as a ctl `timing` message, client stages from
  requestVideoFrameCallback, `window.hxc.stages`, STAGES lines in
  `hx_console_e2e`); keys, buttons, wheel and absolute pointer through the
  guest tools' uinput devices while the tools run and offer them (caps
  `keyboard`/`absolute_pointer`, guest setting `offer_input`, node setting
  `input_via_guest_tools`, default on, API + UI; QEMU input pre-OS, after
  the tools stop and for the Windows secure desktop; a key is released
  through the path that pressed it), WebRTC and local socket sessions;
  `-display dbus` now names the audiodev (without it QEMU exports no Audio
  object and the guest's sound never reached the console).
- Follow-ups (2026-11-21/22): QEMU display poll rate per profile
  (`display_poll_hz`, Horcrux QEMU patch) and the frame-cap token bucket
  (`frame_burst`); the settle frame (`settle_frame`, default on) after a
  browser renderer was found holding the last frame of a change until the
  next idle refresh (~1 s for 7-20 % of keystrokes); `hx_console_e2e`
  checks that no keystroke picture waits in the browser (section 6).

Proven:
- Ring path in a real VM (`shm_guest_e2e.py`, CI `guest-tools.yml`, std VGA
  and virtio-vga): the tools find the ring on the ivshmem BAR, capture nothing
  without a reader, deliver 1280x800 raw frames at the asked 60/120 fps,
  stop when the reader leaves; frame-to-viewer and keystroke-to-frame times
  in section 6.
- Windows ring writer on a real Server 2022 guest (`tests/win_ring.py`,
  `tests/win_ring_anim.ps1`): virtio-win IVSHMEM driver installed with
  `pnputil`, the agent mapped the device with `IOCTL_IVSHMEM_REQUEST_MMAP`,
  captured with DXGI Desktop Duplication in the logged-in session and
  `horcrux-viewer measure --dump` read 1024x768 frames from the ring file
  (the dumped frame is the Windows desktop). Found and fixed: the frame
  time was taken after the capture wait returned (a frame then looked
  ~20 ms newer than it was; it now counts from `LastPresentTime`), the
  viewer's measure reported a bogus publish-to-seen time for a frame an
  earlier reader left in the ring, and the writer had no way to report its
  own rate (`shm_stats_secs`). `shm_write_combine=false` now also selects
  the driver's uncached mode on Windows.
- Node half end to end (`hx_local_viewer_e2e.sh`, CI `hx-console-e2e.yml`,
  real horcrux-api + QEMU + Alpine): ring device and D-Bus audio in the VM's
  argv, root-only ring file with the node's header, 403 without VM.Console,
  view-only tickets, a ticket bound to another uid refused, `horcrux-viewer`
  signing in and mapping the ring received over the socket (same inode as the
  root-only file), keys over the local socket running a command in the
  guest, a view-only session's keys refused and counted, single use, audit
  entries, ring file removed at VM stop. The host-encode console e2e still
  passes with the audio change.
- Viewer window on a real Wayland compositor (`viewer_window_e2e.sh` /
  `.py`, CI `guest-tools.yml` job `viewer-window`): headless sway (pixman,
  no GPU) with the real `horcrux-viewer --vm` signed in to a real
  `horcrux-api` over the local socket, VM booted with the tools CD and the
  ring. The window opens as an xdg_shell toplevel with keyboard focus; the
  guest paints red/green/blue/magenta bands and a `grim` screenshot of the
  compositor matches QMP `screendump` of the guest on the whole sampled grid
  (aspect-fit, per-channel tolerance) while the ring sequence advances; keys
  typed into the compositor through the virtual-keyboard protocol (real evdev
  codes, `wl_keys.py`) reach the window, go over the local socket to the
  guest, and run a command there (marker on the guest's serial port, console
  turns red in the next screenshot; the comparison is shown to discriminate).
  Screenshots are uploaded as an artifact (best effort).
- Guest sound to the browser (`hx_console_e2e`, CI `hx-console-e2e.yml`):
  an Alpine guest with a sound card and alsa-utils (packages from the
  mirror on a data CD, `alpine_apks.py`) plays a 440 Hz tone; the browser's
  inbound audio stream counts ~780 Opus packets in 8 s and audio energy
  (0 before the guest plays).
- Input through the guest tools (`shm_guest_e2e.py` series
  `latency_tools`): the tools report keyboard/absolute pointer in their
  Hello and 30/30 keystrokes sent as tool input changed the guest's
  screen, timed next to the QEMU keyboard (section 6).
- Unit tests: ring layout/consistency/hostile sizes/stale reader stamps,
  local protocol framing, uid-bound tickets, frame-ring/local-socket/audio
  settings validation, QEMU argv, FFmpeg PCM -> Opus packets with the real
  FFmpeg, evdev -> standard gamepad mapping, viewer settings.

Not proven yet:
- The viewer window on a real Horcrux desktop session with a GPU (the CI
  proof above uses a headless compositor with the pixman renderer).
- The Windows ring writer's write-combined mapping on AMD hardware, and
  its behaviour with a GPU-driven desktop (it ran on a QEMU std VGA guest;
  see the Windows row in section 6). The agent's virtio-serial path on
  Windows was not part of that run (no port was given to the VM).
- Host-encode desktop target (< 50 ms) on an idle node and with the patched
  QEMU from the Horcrux binhost on a real install: met at the median on a
  shared build host (41.6-45.7 ms, p90 51-61 ms, section 6 "Measured:
  settle frame"); no idle lab machine was free for the run, and the binhost
  package (gentoo-binhost lists app-emulation/qemu since 2026-11-22) has not
  yet run on an installed node.
- Needs real GPU hardware: 240 Hz capture of a GPU-rendered desktop into the
  ring, hardware encode in the guest and on the host.
