# Gaming VM: Windows with a real GPU on Horcrux

Status: phase 1 (research, design, host tuning + managed GPU hand-off) implemented
and proven on a real kernel (section 8); phases 2-7 (section 15) are the build plan. Owner idea, 2026-11-18: install Windows for
gaming, with a virtualized/passed-through graphics card, able to play current
FPS games.

This document is honest about limits: a VM can give Windows a whole discrete GPU
at near-native speed, but several popular competitive games refuse to run in a
VM by design (section 4). Horcrux will **not** hide the hypervisor from
anti-cheat software, patch anti-cheat or fake hardware IDs: doing so breaks the
games' terms of service and puts the player's accounts at risk. Instead Horcrux
makes the VM an honest, secure, modern PC (TPM 2.0, Secure Boot, VBS-capable,
stable identity), tells the user per game what works before they buy or launch,
and builds the technical basis (measured boot, attestation, protected memory) for
publishers to allow Horcrux VMs (sections 4 and 12). Windows runs only as a VM:
Horcrux does not install Windows next to itself.

The feature is generic: any Horcrux install (desktop, server, cluster node), any
GPU vendor (NVIDIA, AMD, Intel), laptops and desktops, one GPU or several, GPUs
with or without monitors or display outputs. Hardware detection decides what is
possible on each machine and the UI says why when something is not. The picture
reaches the player through Horcrux's own viewer and streaming (section 10), on
the same machine, on the LAN, or over the internet, with no third-party viewer.

## 1. GPU options, with evidence

| Option | Gives DirectX 12 games a real GPU? | Host keeps a display? | Status (Nov 2026) |
|---|---|---|---|
| **Whole discrete GPU via VFIO** (dGPU passthrough, host keeps the iGPU) | Yes, native driver, ~bare-metal frame rates | Yes, on the iGPU | **Supported (phase 1 code).** The path for gaming. |
| **Single-GPU passthrough** (the only GPU goes to the VM) | Yes | No: the local screen is the VM's while it runs | Phase 3 (needs the display manager stopped and the console unbound; `allow_display_loss` already gates it) |
| **Intel iGPU SR-IOV** (iGPU split into virtual functions) | In principle (native Intel driver on a VF), small iGPU, no display output on a VF | Yes | Mainline xe enables PF SR-IOV for Tiger Lake/Alder Lake, Panther Lake and Battlemage, **not Meteor Lake/Arrow Lake**; out-of-tree strongtz/i915-sriov-dkms is "highly experimental" with mixed Arrow Lake reports. Not a gaming path; tracked as an info quirk. |
| **NVIDIA vGPU** | Only on datacenter/RTX professional cards with an NVIDIA vGPU licence | Yes | **Not available on GeForce.** NVIDIA's own FAQ: GeForce supports passthrough to one VM (the Code 43 VM check was removed in driver 465+), no vGPU. Community "unlock" work on Blackwell is compute-only/broken. |
| **AMD SR-IOV / MxGPU** | Instinct/some Radeon Pro only | Yes | Not on consumer Radeon. |
| **virtio-gpu / virgl / Venus** | **No.** Windows only has the 2D `viogpudo` driver; experimental community WDDM 3D drivers exist, none usable for games | Yes | Not a gaming path. Venus gives Vulkan to Linux guests only. |

Sources: docs.kernel.org xe SR-IOV docs and dgpu-docs.intel.com virtualization
guide; github.com/strongtz/i915-sriov-dkms (README, issues 359/397/429);
NVIDIA "GeForce GPU passthrough for Windows VM" FAQ; github.com/virtio-win/kvm-guest-drivers-windows
(viogpu); full research notes kept with the design work.

### Laptops: does the internal screen stay with the host?

Depends on wiring. On laptops with a MUX (Advanced Optimus) running in hybrid
mode, the typical layout is:

- the internal panel connector (e.g. `card0-eDP-1`) belongs to the **iGPU**; the
  dGPU has its own `eDP` connector (used only when the firmware MUX switches the
  panel to it) plus **the external ports** (DP/HDMI/USB-C). Vendors document
  this, e.g. "The dGPU handles the USB Type-C and HDMI connections in Hybrid
  Graphics Mode" (Dell KB 000303738, Alienware Area-51 16/18).
- the dGPU (VGA function + HDMI audio function) usually sits alone in its IOMMU
  group behind its own root port, so it can be handed over cleanly. The
  readiness check shows the actual group and connectors of each machine.

So on such a laptop: **yes**, the dGPU can go to Windows while the host keeps the
internal panel on the iGPU; Windows' picture is shown either on an external
monitor plugged into the laptop's HDMI/USB-C ports (driven natively by the
passed-through GPU) or in a window on the laptop panel through the Horcrux
viewer (section 10). The firmware must stay in hybrid mode (dGPU-only/MUX-to-dGPU mode
moves the panel to the dGPU and the host would lose it).

Muxless laptops (dGPU shows as "3D controller" with no outputs) can still pass the
dGPU through, but only the Horcrux viewer (with the guest tools' virtual display)
can show the picture; some need the vBIOS supplied from ACPI `_ROM` (romfile) and a fake
battery for the NVIDIA driver.

## 2. What runs on the host while the VM runs

### Managed hand-off (implemented, `crates/horcrux-vm/src/vm/handoff.rs`)

`hardware.hostpci[].managed: true` makes Horcrux own the device's driver:

1. Start: collect the device + the other non-bridge functions of its IOMMU
   group; refuse when a connected monitor on it is the host's only display
   (unless `allow_display_loss`), stop `passthrough.handoff_stop_services`
   (default `nvidia-persistenced`) through the OpenRC/systemd abstraction, then
   refuse with the program names when anything still has the GPU's device files
   open (`/dev/dri/cardN`, `renderD*`, `/dev/nvidiaN`, and the NVIDIA control
   nodes when it is the last NVIDIA GPU). Then each function moves to
   `vfio-pci` through `driver_override` with a timeout
   (`passthrough.handoff_timeout_secs`), and `<disk>.handoff.json` records the
   original drivers.
2. Stop/shutdown/guest power-off/crash: an exit watcher (re-attached after an
   API restart) waits until no process holds `/dev/vfio/<group>`, then clears the
   override, re-probes the original driver and restarts the stopped services.
3. A failing start undoes everything it did.

Programs are never killed. On a GNOME desktop, mutter opens every GPU; the
supported setup is a udev rule tagging the passthrough GPU `mutter-device-ignore`
(phase 2 ships it as a per-GPU setting), so the compositor leaves it alone and the
hand-off needs no logout. Unbinding the NVIDIA driver while a program uses it
blocks inside the kernel, which is why the hand-off refuses instead of trying.

### NVIDIA RTX 50 (Blackwell) reset problem

Reported since 2025 for the RTX 5090 and RTX PRO 6000: after a VM shuts down the
function-level reset times out (`not ready 65535ms after FLR; giving up`) and the
card is dead until a cold power cycle; a host soft lockup can follow. NVIDIA
attributed it to a device firmware defect; a kernel quirk ("PCI: Avoid FLR for
NVIDIA 5090 GPU", linux-pci, 2026-04-16) is not merged. Whether the laptop GB203M
parts are affected is not documented either way. Consequences built in:

- the shipped quirks table (`/usr/share/horcrux/gpu-quirks.toml`, data, extendable
  in `/etc/horcrux/gpu-quirks.toml`) warns for Blackwell device ids;
- the wizard's power button for a GPU VM is **Shut down** (ACPI), never Stop;
- a stuck unbind is bounded by the hand-off timeout and reported, the device is
  left on vfio-pci rather than retried in a loop.

## 3. Performance (implemented in phase 1 unless noted)

| Setting | VM config | Default | Notes |
|---|---|---|---|
| CPU pinning | `adv.cpu_pinning {vcpus, emulator}` | off; wizard sets `auto` | `auto` = performance cores of a hybrid Intel CPU (`/sys/devices/cpu_core/cpus`), CPU 0 last; never mixes P- and E-cores in one guest. QEMU's other threads go to the rest (E-cores). Applied after start from QMP `query-cpus-fast` + `sched_setaffinity`. |
| Huge pages | `adv.hugepages {size_kb: 2048 or 1048576}` | off; wizard sets 2 MiB | The pool grows by the shortfall at start (`memory-backend-memfd,hugetlb=on,prealloc=on`) and shrinks back at stop (`passthrough.hugepages_release_on_stop`). 1 GiB pages usually only succeed when reserved at boot (`hugepages=` kernel option); the error says so. |
| Hyper-V enlightenments | `adv.hyperv: true` | off; wizard sets it | `-cpu <model>,hv-relaxed,hv-vapic,hv-spinlocks=0x1fff,hv-vpindex,hv-runtime,hv-synic,hv-stimer,hv-stimer-direct,hv-reset,hv-frequencies,hv-reenlightenment,hv-tlbflush,hv-ipi,hv-time` (QEMU's documented set; the list and the model are node settings `passthrough.hyperv_enlightenments` / `hyperv_cpu_model`) plus `kvm-pit.lost_tick_policy=discard`. No `kvm=off`/vendor-id spoofing. |
| Input | `adv.evdev [{path, grab_all}]` | none | QEMU `input-linux`: keyboard/mouse go straight to the guest; both Ctrl keys toggle between host and guest. USB passthrough of a whole controller works through `hostpci`. |
| Storage | existing VM disks | - | Phase 2 wizard: virtio-blk with iothread on a ZFS zvol (`volblocksize=16k`, `compression=lz4`), or a whole NVMe through `hostpci`. |
| Audio | - | - | Phase 2: HDMI/DP audio of the passed GPU works today (it moves with the group); PipeWire backend for a virtual sound card. |
| Latency checks | - | - | Phase 2 doc + guest script: MSI mode check for the GPU/HDA, LatencyMon guidance. |

## 4. Anti-cheat reality (checked 2026-11-18)

"VM" = Windows in KVM/QEMU with a passed-through GPU, no hiding. "Host" = Steam
on Horcrux (Proton or native).

| Game | Anti-cheat | In a Horcrux VM | On the host (Proton/native) | Horcrux shows |
|---|---|---|---|---|
| Valorant | Riot Vanguard | **Blocked** (Riot: VMs not supported, error VAN 138) | Denied | Blocked by publisher: warn before buy/launch |
| League of Legends | Riot Vanguard | **Blocked** | Broken | Blocked by publisher: warn before buy/launch |
| Call of Duty BO6 / BO7 / Warzone | RICOCHET + Secure Boot/TPM remote attestation | Unsupported (attestation-based; only "works" with hiding) | Broken | Blocked by publisher: warn before buy/launch |
| Battlefield 6 / REDSEC, BF2042 | EA Javelin, Secure Boot required (BF6) | Unsupported (EA names VMs as a threat) | Broken | Blocked by publisher: warn before buy/launch |
| EA Sports FC | EA Javelin | Unsupported (same family) | Broken | Blocked by publisher: warn before buy/launch |
| ARC Raiders | Denuvo Anti-Cheat | **Blocked, ban risk** (Embark: "VMs are disallowed") | **Works** | Host |
| The Finals | EAC + Denuvo AC | Unknown, same studio as ARC Raiders | **Works** | Host |
| Fortnite | Easy Anti-Cheat | Kicks reported | Denied (Epic working on "EAC Linux v2") | Blocked by publisher: warn before buy/launch |
| Apex Legends | EAC | Unknown | Broken (Linux blocked Oct 2024) | Blocked by publisher: warn before buy/launch |
| Rainbow Six Siege X | BattlEye | Kicks reported | Denied | Blocked by publisher: warn before buy/launch |
| PUBG | BattlEye | Unsupported (support reply, reported) | Broken | Blocked by publisher: warn before buy/launch |
| Escape from Tarkov | BattlEye | Blocks launch | Broken | Blocked by publisher: warn before buy/launch |
| Destiny 2 | BattlEye | Reported to work | Broken (Bungie blocks Linux) | VM (reported), Secure Boot + TPM on |
| GTA Online | BattlEye | Unknown | Single-player only | Blocked by publisher: warn before buy/launch |
| Counter-Strike 2 | VAC | Works (reported, no policy) | **Works, native** | Host |
| Deadlock | VAC | Works (reported) | **Works** | Host |
| Marvel Rivals | NetEase NEAC | Unknown | **Works** | Host |
| Overwatch 2 | Blizzard | Works (reported) | Works (community) | Host or VM |
| Halo Infinite | EAC | Unknown | **Supported** | Host |
| Splitgate 2 | RedKard | Unknown | **Works** | Host |
| Delta Force | Tencent ACE | Unknown | SteamOS only | Blocked by publisher: warn before buy/launch |

Sources: Riot support (error VAN 138), Embark help centre (ARC Raiders anti-cheat
FAQ), EA Battlefield 6 Secure Boot page, Activision RICOCHET Season 05 posts,
GamingOnLinux anti-cheat database (updates through Sep 2026),
areweanticheatyet.com, Valve Steamworks Proton anti-cheat page, Rockstar
BattlEye FAQ, linux-pci/forum reports marked "reported". Statuses change often:
the wizard reads them from a data file (`gaming-compat.toml`, phase 2) with a
`checked_on` date per row, and unknown titles show "not tested: may kick or ban".

Summary for "latest FPS games": in a VM, expect CS2, Deadlock, Overwatch 2 and
most single-player/DRM-only games to work; the big competitive shooters with
kernel anti-cheat (Valorant, CoD, Battlefield, R6, Tarkov, PUBG, Fortnite, Apex,
ARC Raiders) do not, until their publishers allow VMs. Several of them (ARC
Raiders, The Finals, Marvel Rivals, CS2, Deadlock, Halo Infinite, Splitgate 2)
run on Horcrux itself through Steam; the UI says so.

## 5. Honest anti-cheat support (no evasion)

What Horcrux does so that games which allow VMs work, and nobody is surprised
by the ones that don't:

1. **A normal, secure modern PC, honestly.** Per-VM persistent TPM 2.0
   (swtpm), UEFI Secure Boot with Microsoft keys (OVMF `secboot` + MS vars),
   Windows 11 requirements met without bypasses, VBS/HVCI (Memory Integrity) and
   Hyper-V inside the guest (nested virtualisation, `vmx`/`svm` exposed), IOMMU
   DMA protection on the host, and a **stable** hardware identity per VM
   (SMBIOS UUID/serial generated once and kept, never randomised, never copied
   from a real vendor model). Hyper-V enlightenments stay on: the guest knows it
   is a VM.
2. **Per-game compatibility data** (`gaming-compat.toml`, shipped as data,
   refreshed on a schedule from public sources and our own test results, each row
   with source and `checked_on`): title, anti-cheat, VM status (`works`,
   `works-if` + the setting, `blocked-by-publisher`, `unknown`). Shown in the UI
   and by the guest tools, which detect installed launchers/games and warn
   **before** a purchase/install/launch of a blocked title.
3. **One-click legitimate settings** per game ("this game requires Secure Boot +
   TPM: turned on"; "requires VBS: turned on").
4. **Publisher allow-listing (a project decision; nothing is sent yet).** A
   proposal to EAC/Epic, BattlEye, Riot and Activision to allow Horcrux VMs on
   verifiable evidence: measured boot of host and VM, a TPM-signed attestation
   report, and protected guest memory (section 12). The technical basis is built;
   the outreach itself is decided separately.
5. **Bans/false positives:** guidance to contact the publisher; Horcrux never
   ships unban tooling.

## 6. Gaming VM wizard (phase 2)

UI page "New gaming VM" + `POST /api/vms/gaming` composing an ordinary VM config:

1. Host check (`GET /api/gpu/pci/gaming-check`, implemented): KVM, IOMMU, P/E
   cores and suggested vCPUs/pinning, huge-page pools, and per GPU: functions
   that move with it, monitors on it, whether the host keeps a screen, programs
   holding it, reset methods, known issues, verdict (`ready`,
   `ready-with-warnings`, `single-gpu`, `blocked`).
2. Choices with defaults from the check: GPU, vCPUs (all P-cores), memory
   (half of RAM, max 32 GB), disk (zvol), display (monitor on the GPU, or the
   Horcrux viewer/stream with a virtual display when no monitor or output exists),
   keyboard+mouse (evdev, USB controller, or the viewer's raw input).
3. Media: Windows 11 ISO (user-provided or Microsoft's download page; the
   90-day Enterprise evaluation ISO for testing), virtio-win ISO from
   fedorapeople stable-virtio; both go to the ISO library.
4. Unattended install: generated `autounattend.xml` on a small ISO (locale,
   user, virtio storage/net drivers from the virtio-win ISO, Horcrux guest
   tools, Fast Startup off); TPM 2.0 + Secure Boot OVMF so no requirement bypass
   is needed. The licence key is the user's. Proven: section 8.
5. Viewer: Horcrux guest tools (virtual display, capture, encode, input) and the
   Horcrux viewer (section 10); no third-party viewer.
6. One-click Start/Shut down; Start does the hand-off, Shut down gives the GPU
   back; the VM page shows who holds the GPU when the start is refused.

## 7. Compatibility matrix (platforms Horcrux supports)

| Platform | dGPU VFIO | Hand-off | Pinning `auto` | Huge pages | Horcrux viewer | Notes |
|---|---|---|---|---|---|---|
| amd64, Intel hybrid (Alder Lake+), iGPU + dGPU desktop | Yes | Yes | P-cores | 2M/1G | phase 3 | Best case |
| amd64 laptop with MUX/Advanced Optimus (hybrid mode) | Yes | Yes | P-cores | 2M | phase 3 | Panel stays on iGPU; external ports follow the dGPU |
| amd64 muxless laptop | Yes (romfile/ACPI quirks) | Yes | P-cores | 2M | phase 3 (only way to see the guest) | Not tested on hardware yet |
| amd64 single-GPU desktop | Yes | `allow_display_loss` (phase 2 finishes display-manager stop/console) | all but CPU 0 | 2M/1G | remote only | Screen goes to the VM and back |
| Headless server / GPU without outputs (NVIDIA L4/A-series, AMD Instinct/Radeon Pro, Intel Flex/Arc Pro) | Yes | Yes | as above | 2M/1G | stream (phase 4) | Virtual display in the guest; datacenter NVIDIA graphics needs the vendor's licensed driver (phase 2 research) |
| HA cluster nodes | Yes | Yes | as above | as above | follows the VM | Cold restart on another GPU node; no live migration with a passed-through GPU |
| amd64 AMD Ryzen/EPYC | Yes | Yes | all but CPU 0 (no P/E) | 2M/1G | phase 3 | CCD-aware pinning: phase 2 |
| arm64 hosts | No Windows-on-ARM gaming target | Code path neutral | Yes | x86 guests only | No | Out of scope for gaming |
| OpenRC / systemd | Same code; services stopped through the init abstraction | | | | | Both |
| T2 Macs (Intel) | Single-GPU or iGPU-only machines; no supported dGPU passthrough | - | - | - | - | Not a gaming target |

## 8. Proof so far and what needs real hardware

Proven in CI-style tests (fixtures of the real sysfs layout, real syscalls where
possible): group collection, display/holder refusal, record + release ordering
(waits for `/dev/vfio/N` to close), Hyper-V/evdev/huge-page arguments, CPU
layout + `auto` pinning, real `sched_setaffinity` on test threads, huge-page
pool accounting, quirks table.

Real-kernel spike (`ci/gaming-handoff-e2e.sh`, workflow
`gaming-handoff-e2e.yml`, passed 2026-11-18 on rw-man-01): a Debian 13 VM with an
emulated IOMMU (`intel-iommu`) and a spare PCI display on bochs-drm runs the
`handoff_e2e` test as root. Results: verdict `ready`; hand-off refused while a
program held `/dev/dri/card1` (the error names it); device moved
bochs-drm -> vfio-pci with the record written; 128 x 2 MiB huge pages reserved
and used by a nested QEMU started with the device, `-cpu host` + all Hyper-V
flags; vCPU threads pinned to their CPUs and QEMU's own threads to the rest;
release waited while QEMU held the VFIO group; after QEMU exited the device was
back on bochs-drm with its device nodes, and the huge pages were returned.

Windows spike (2026-11-18, rw-man-01, no GPU): Windows 11 Enterprise evaluation
installed unattended with the gaming configuration (q35 + SMM, Secure Boot OVMF
with Microsoft keys, swtpm TPM 2.0, `-cpu host` + Hyper-V flags, virtio-blk/net
drivers from virtio-win in windowsPE, guest agent). Result: installed and at
the desktop in 41 minutes; from inside the guest: Windows 11 Enterprise
Evaluation build 26100, `Confirm-SecureBootUEFI` = True, TPM present and ready,
hypervisor reported (no hiding), VirtIO disk/network/serial drivers bound,
network up, evaluation licence active; vCPU threads pinned from QMP; clean ACPI
shutdown. One answer-file fix found: the oobeSystem pass needs the
International-Core component or OOBE stops at the region page (the wizard's
generator includes it).

Only provable on a host with a real discrete GPU (planned on a
hybrid laptop with a discrete NVIDIA GPU): NVIDIA driver unbind/rebind under a live
GNOME session (mutter ignore tag), the Blackwell reset behaviour after guest
shutdown, Windows frame rates/latency with pinning, the viewer on the panel,
external monitor output from the guest, AMD reset behaviour, Intel Arc.

## 9. Settings added (all with defaults, `[passthrough]` in /etc/horcrux/config.toml)

`hyperv_enlightenments`, `hyperv_cpu_model`, `handoff_stop_services`,
`handoff_refuse_when_held`, `handoff_timeout_secs`, `hugepages_release_on_stop`,
`quirks_files`. Per VM: `hostpci[].managed`, `hostpci[].allow_display_loss`,
`adv.cpu_pinning`, `adv.hugepages`, `adv.hyperv`, `adv.evdev`.

Protected/confidential tier (`[protected]`, section 12): `protected_tier`,
`confidential_tier`, `required_lockdown`, `lockdown_path`, `cmdline_path`,
`ptrace_scope_path`, `sev_status_path`, `cpuinfo_path`, `tpm_pcr_dir`,
`attested_pcrs`, `attestation_key_path`, `vmm_binary_path` — every detection
source and the attestation key path is a config key with a default, so a
different TPM/measured-boot layout needs no rebuild.

## 10. Native viewer and streaming (no third-party viewers)

No Looking Glass, Sunshine/Moonlight, Parsec or similar in the user's path;
Horcrux ships its own. Standard building blocks underneath are fine
(ivshmem, VFIO, codec SDKs via FFmpeg, WebRTC libraries) after a licence check
against Horcrux's licence; Looking Glass is GPL-2.0 and its code is not copied
(clean-room design).

- **Horcrux guest tools** (signed MSI, delivered into the VM by the wizard):
  an Indirect Display Driver (virtual monitor, configurable resolution/refresh up
  to 4K/5K and 240-360 Hz, used automatically for GPUs without a monitor or
  without outputs; a dummy plug is only an optional fallback), capture (IDD
  swap-chain / DXGI Desktop Duplication), hardware encode (NVENC, AMF, QSV;
  software fallback), audio capture (WASAPI loopback), input injection (raw
  mouse, keyboard, gamepads via a virtual HID), clipboard, cursor channel,
  health. Built with the Windows Driver Kit in CI, signed.
- **Same host:** a Horcrux-managed ivshmem shared-memory device carries raw
  frames into the host; the Horcrux viewer window on the host desktop (and the
  console in the Horcrux UI) shows them with no encode step.
- **LAN/WAN:** Horcrux's own low-latency streaming: hardware-encoded video (AV1,
  HEVC, H.264, picked by client), Opus audio, input back; WebRTC transport
  (ICE with a Horcrux-hosted relay through the cluster for NAT, no third-party
  service), adaptive bitrate/resolution, FEC/NACK for loss, encrypted (DTLS-SRTP),
  authenticated with Horcrux accounts/RBAC/2FA and audited. Clients: the Horcrux
  web UI in a browser, the Horcrux desktop app (Linux/Windows/macOS), later
  Android/Android TV and iOS/Apple TV.
- **Latency budget (target, to be measured per path in tests):** same host <
  8 ms frame-to-viewer; LAN < 20 ms click-to-photon at 120+ fps; WAN = LAN + RTT
  + jitter buffer. Tests publish measured numbers per path.

## 11. HA clusters and remote players

- GPU nodes advertise GPUs (vendor, model, VRAM, outputs, free/used) to the
  cluster. Placement and HA restarts target only nodes with a compatible free
  GPU. Node failure = cold restart elsewhere (a passed-through GPU cannot be
  live-migrated; the UI says so; migratable vGPU/SR-IOV is used where a vendor
  offers it). Disks on replicated/shared storage, so a restart loses nothing.
  Same GPU model preferred; a different model shows a driver warning.
- Players connect to the cluster name/VIP, never a node; the stream endpoint
  follows the VM and clients reconnect after failover.
- Tests: vmtest multi-VM HA scenarios (no lab-laptop reboots); real-GPU failover
  only where two GPU nodes exist (listed as unproven until then).

## 12. Protected gaming VMs (attestation basis)

Two honest ways to keep game memory out of the host's reach, strongest first.
Neither hides or spoofs anything; both are an opt-in tier the UI offers only
where the hardware/kernel supports it, and explains why when it can't
(`GET /api/gpu/pci/protected-check`, `crates/horcrux-vm/src/vm/protected.rs`).

### 12.1 Threat model (what each layer actually stops)

| Adversary | Ordinary VM | Protected VM (consumer CPU) | Confidential VM (SEV-SNP / TDX) |
|---|---|---|---|
| **Host userspace** (the QEMU process, another local process reading the VMM via `ptrace`/`process_vm_readv`, QMP `dump-guest-memory`, the gdbstub) | reads guest RAM freely | **blocked**: guest RAM is in a `guest_memfd` never mapped into host userspace; VMM is `lockdown`-protected from ptrace, QMP memory dump + gdbstub disabled, seccomp-confined | **blocked**: guest memory is encrypted with a key the host never has |
| **Host kernel** (a compromised or malicious kernel, `/dev/mem`, kcore, BPF, kexec, hibernation image) | reads guest RAM | *partially* — `lockdown=confidentiality` closes the standard read paths and measured boot detects a tampered kernel, but a kernel that is both malicious **and** unmeasured could still reach `guest_memfd` pages; **not cryptographically excluded** | **blocked**: the CPU denies the host hypervisor plaintext access; integrity protection (SNP RMP / TDX) detects remapping/replay |
| **Physical / DMA** (cold-boot, a malicious PCIe device doing DMA, an interposer) | reads DRAM | *partially* — only if the platform has SME/TSME (AMD) or TME (Intel) enabled, which encrypt DRAM against a bus/cold-boot attacker (not against the host OS); IOMMU limits rogue-DMA | **blocked**: guest DRAM is always ciphertext; SEV-SNP/TDX add integrity |
| **Malicious admin** (root on the host deliberately dumping a tenant's VM) | reads guest RAM | *deterred, not prevented* — every read path is closed and the act would change the measured/audited host state, but root is inside the protected tier's TCB | **blocked**: root is outside the guest TCB; the attestation report proves the host could not read the VM |

The attestation report (12.3) states the tier honestly so a verifier (an
anti-cheat vendor) can see exactly which adversaries are excluded.

### 12.2 Hardware confidential computing (the confidential tier)

AMD SEV-SNP (EPYC 7003 "Milan" and newer) and Intel TDX (Xeon Scalable 5th
gen "Emerald Rapids" and newer; some 4th-gen "Sapphire Rapids" SKUs) encrypt
and integrity-protect guest memory and attest the launch state; even the host
hypervisor cannot read guest RAM. Arm CCA (Realms) is the equivalent, not yet
in shipping silicon. Consumer **Core Ultra / Ryzen** parts do **not** offer
these — AMD SME/TSME and Intel TME encrypt whole-DRAM against a *physical*
attacker only and never isolate a VM from the host OS, so Horcrux never counts
them as a confidential tier (detected separately as `memory_encryption`).

Windows confidential guests exist (Azure offers Windows 11 and Windows Server
as SEV-SNP and TDX confidential VMs), so the Windows gaming guest is not the
blocker. **Passing a gaming GPU into a confidential VM is** — it needs PCIe
TDISP/IDE (AMD SEV-TIO; Intel TDX Connect) or a vendor confidential-GPU mode
(NVIDIA H100/Blackwell CC mode, datacenter only). No consumer gaming GPU
supports it today, so `confidential_gpu_passthrough` is always reported
`false` and the UI says a confidential VM would have to run the GPU emulated
(not useful for gaming). The confidential tier is therefore real for the
*compute/headless* case and a documented future for gaming GPUs.

### 12.3 Protected gaming VM (consumer CPUs) — built and proven

Layers, each a config key with a default (nothing hard-coded, `[protected]`):

- **Private guest memory** — guest RAM in a KVM `guest_memfd` (`KVM_CAP_GUEST_MEMFD`,
  Linux 6.8) under the `KVM_X86_SW_PROTECTED_VM` machine type. These pages are
  never mapped into host userspace, so the VMM process and anything reading it
  cannot see guest RAM. **Proven on this project's kernel (6.8):** the ignored
  test `crates/horcrux-vm/tests/protected_memfd.rs` creates a sw-protected VM +
  `guest_memfd` and asserts that `mmap` of it from host userspace is **refused
  (`ENODEV`)** — see `ci/protected-vm-e2e.sh` and the `protected-vm-e2e`
  workflow on the `kvm-x86_64` runner. (SW_PROTECTED_VM runs the guest with
  emulated/paravirt memory today; wiring it under the running gaming guest is a
  follow-up — phase 6 proves the private-memory property and ships the
  detection, report, API, UI and attestation.)
- **Locked-down host** — `lockdown=confidentiality` (no `/dev/mem`, kcore,
  kprobes/BPF memory reads, kexec, hibernation), Secure Boot + signed kernel,
  no `ptrace`/`process_vm_readv` of the VMM (Yama ptrace scope), KVM QMP
  `dump-guest-memory` and the gdbstub disabled, seccomp-confined VMM, IMA/EVM
  for host binaries.
- **Platform memory encryption where present** (Intel TME / AMD SME/TSME) —
  reported, counted only against the physical/DMA adversary, never as isolation.
- **Measured boot** — firmware, bootloader, kernel, cmdline, lockdown state and
  the VMM binary hash measured into TPM PCRs.

### 12.4 Horcrux attestation report

`GET /api/gpu/pci/attestation` returns a signed report: the measured-boot PCR
values, kernel cmdline, lockdown level and VMM SHA-256, a SHA-256 digest over
the canonical measurement block, an ECDSA P-256 signature by a per-node
attestation key, and the report bound to the node's TLS fingerprint. It states
the tier and its caveats honestly ("host userspace cannot read guest RAM; a
malicious host *kernel* is not cryptographically excluded as it would be with
SEV-SNP/TDX"). On consumer hardware this report is the evidence behind the
anti-cheat allow-listing proposal (section 5, step 4); on SEV-SNP/TDX hosts a
hardware attestation quote is added (future phase). The TPM quote (PCRs signed
by the TPM's attestation key) is the cryptographic root and is wired next.

Sources (checked 2026-11-18): Linux KVM `Documentation/virt/kvm/api.rst`
(`KVM_CAP_GUEST_MEMFD`, `KVM_CREATE_GUEST_MEMFD`, `KVM_X86_SW_PROTECTED_VM`,
merged 6.8); kernel `admin-guide/hw-vuln` and `security/self-protection` +
`Documentation/admin-guide/kernel-parameters` (`lockdown=`); AMD SEV-SNP ABI
and SEV-TIO/TDISP spec; Intel TDX module/`tdx` kernel docs and TDX Connect;
QEMU `confidential-guest-support` / `sev-snp-guest` / `tdx-guest` docs; EDK2
OVMF `AmdSev`/`IntelTdx` packages; Microsoft Azure confidential-VM Windows
support; NVIDIA H100 confidential-computing documentation.


## 13. VM on one machine, GPU on another (later)

Practical form first: compute follows the GPU. The VM's disk lives on node A or
shared storage; the VM runs on GPU node B with its disk over NVMe-oF/TCP or
replicated ZFS, so any machine can lend its GPU (same VM, same viewer endpoint).
True remote GPU (CPU/RAM on A, GPU on B) would need API remoting (Vulkan
command streams over RDMA; DirectX through DXVK/vkd3d-proton in the guest);
PCIe over the network doesn't exist for commodity GPUs. Expected to serve
slower-paced games at best, not FPS; prototyped only if measurements justify it.

## 14. Compared with GeForce NOW (published specs, Nov 2026)

| | GeForce NOW Ultimate | Horcrux target |
|---|---|---|
| GPU | RTX 5080-class servers | User's own GPU (any vendor) |
| Max stream | 5K 120 fps; 1440p 240 fps; 1080p 360 fps | Same where GPU/encoder/link allow; 4K120, 1440p240 |
| Codecs | H.264, HEVC, AV1; 10-bit HDR; 4:4:4 (cinematic mode) | Same set, picked per client |
| Bandwidth | 55 Mbit/s for 240/360 fps, 65 Mbit/s for 5K120 | Fits the link; WAN capped by the home upload |
| Latency | < 80 ms to a data centre required, < 40 ms recommended; "under 30 ms" claimed for 1080p360 + Reflex | LAN < 20 ms click-to-photon target, measured |
| Anti-cheat | Publisher agreements | No agreements yet: per-game data, attestation proposal |
| Clients | Windows, macOS, browser, Chromebook, Steam Deck, TV, mobile | Browser, desktop app; TV/mobile later |

Sources: NVIDIA GeForce NOW system requirements and premium membership pages,
NVIDIA support answer 5223. Measured Horcrux numbers are added as phases land.

## 15. Phases

1. **Done:** research, design, managed GPU hand-off, pinning, huge pages,
   Hyper-V, evdev, readiness API/UI, real-kernel e2e, Windows unattended spike.
2. Gaming VM wizard + `POST /api/vms/gaming`, unattended Windows install with
   TPM/Secure Boot/VBS/stable SMBIOS, `gaming-compat.toml` + UI warnings,
   single-GPU display-manager hand-over, AMD CCD pinning, MSI/latency checks.
3. Horcrux guest tools (IDD, capture, encode, input) + same-host shared-memory
   viewer.
4. Remote streaming (WebRTC, relay, adaptive bitrate) + web/desktop clients,
   latency tests per path.
5. HA placement and failover for GPU VMs; GPU pool, sessions, quotas.
6. **Prototyped (this phase):** protected gaming VM + attestation. Host
   capability detection (SEV-SNP/TDX vs guest_memfd + lockdown), the
   `protected-check` / `attestation` APIs and UI tier, a signed attestation
   report, and a real-kernel test proving host userspace cannot read guest_memfd
   RAM (refused with `ENODEV` on 6.8). Still to wire: the gaming guest actually
   launched under `guest_memfd`, a TPM quote as the cryptographic root, and the
   confidential tier once a gaming GPU supports TDISP.
7. GPU lending across nodes (compute follows the GPU); remote-GPU research.
