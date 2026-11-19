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

- **Hardware confidential computing:** AMD SEV-SNP (EPYC 3rd gen+) and Intel
  TDX (Xeon 4th/5th gen+) encrypt and integrity-protect guest memory and attest
  the launch state. Consumer Core Ultra / Ryzen parts don't offer them (SME/TSME
  and TME only encrypt DRAM against physical attacks; they don't isolate a VM
  from the host). Windows confidential guests exist (Azure lists Windows 11 and
  Windows Server for SEV-SNP/TDX), but passing a gaming GPU into one needs PCIe
  TDISP/IDE (AMD SEV-TIO spec 0.92, Aug 2026) or vendor confidential-GPU modes
  (datacenter H100/Blackwell only): nothing works for gaming GPUs today. Opt-in
  tier where the hardware allows; the UI shows why it isn't available otherwise.
- **On consumer CPUs ("protected gaming VM"):** guest RAM in KVM guest_memfd
  private memory (not mapped in host userspace; `KVM_X86_SW_PROTECTED_VM` type,
  in-place conversion work in kvm-x86 2026), kernel lockdown=confidentiality,
  no ptrace/process_vm_readv of the VMM, QMP memory dumps and gdbstub disabled,
  seccomp-confined VMM, IMA/EVM, measured boot of firmware/bootloader/kernel/
  cmdline/lockdown/VMM into TPM PCRs, and a Horcrux attestation report
  ("unmodified, locked-down Horcrux host; nothing outside the VM can read its
  memory"). Weaker than SEV-SNP/TDX against a malicious host kernel or physical
  access; the threat-model table states it. A test proves that reads of guest
  RAM from host userspace fail.

## 13. VM on one machine, GPU on another (phase 7 research, 2026-11-19)

Two different things hide behind "VM here, GPU there". They get different answers.

1. **Compute follows the GPU (build this).** The VM's disk stays on node A or on
   shared storage; the VM itself runs on GPU node B. Any Horcrux machine can
   "lend" its GPU, and the viewer/stream endpoint follows the VM (section 11).
2. **True remote GPU (do not build now).** The VM's CPU and RAM stay on A and its
   GPU is on B, with the GPU API carried over the network.

PCIe cannot be extended over commodity Ethernet for a consumer GPU. Composable
PCIe fabrics (GigaIO FabreX, Liqid and similar) exist, but they need their own
PCIe switch hardware and cabling, not an Ethernet NIC (product knowledge, not
re-checked in this phase). So option 2 can only mean API remoting.

### 13.1 Measured: game-like disk load over NVMe/TCP vs a local disk

**What was measured.** The available build hosts cannot reach each other (network
rules isolate them), so there was no real 10/25 GbE wire between two machines and
the network is modelled on one host:

- Host: an AWS m8i.2xlarge (8 vCPU Xeon 6975P-C, kernel 7.0.0-1014-aws, ENA).
- Target: kernel `nvmet` + `nvmet-tcp`, 8 GiB RAM-backed block device (`brd`),
  so the media is not the limit and only protocol + link show. Initiator: kernel
  `nvme-tcp` over `127.0.0.1`. Local baseline: the same RAM device read directly.
- Link: `tc netem` (rate + delay, per direction) on the loopback for TCP port
  4420 only. Profiles are modelled values, not wires: 25 GbE/10 GbE RTT 0.1 ms,
  2.5 GbE RTT 0.15 ms, 1 GbE RTT 0.2 ms, plus 10 GbE with RTT 0.5 ms and 2 ms
  and 1 GbE with RTT 2 ms for "far away" cases.
- Workloads (fio, `direct=1`, libaio): **load** = 6 GiB of random reads, block
  sizes 4 KiB 20% / 64 KiB 30% / 256 KiB 30% / 1 MiB 20%, QD16 x 2 jobs (a level
  load; DirectStorage's own guidance is that NVMe reads top out at 64 KiB);
  **QD1** = 4 KiB random reads at queue depth 1 (a read the frame is waiting
  on); **stutter** = that QD1 job while a 1 MiB QD4 asset stream runs in the
  background; **small** = 4 KiB QD32 x 4 jobs.
- Harness and raw results: `tools/lending-bench/` (`lending-bench.sh`,
  `lending-bench-qos.sh`, `analyze.py`, `results/`). Both scripts are settings-
  driven (`PROFILES`, `OUT`, `IMG`, `RDKB`) and clean up the modules, qdiscs
  and configfs entries they create.

| link (modelled) | 6 GiB load | throughput | 4 KiB QD1 p50 / p99 / p99.9 | stutter fg p50 / p99 / p99.99 (bulk stream running) |
|---|---|---|---|---|
| local device | 1.5 s | 7.9 GiB/s | 1.9 / 2.7 / 17 us | 1.9 us / 2.8 us / 22 us |
| NVMe/TCP, loopback, no shaping | 2.8 s | 4.3 GiB/s | 12 / 24 / 39 us | 9 us / 23 us / 138 us |
| 25 GbE | 6.4 s | 1.9 GiB/s (*) | 138 / 226 / 1483 us | 0.5 ms / 4.1 ms / 8.7 ms |
| 10 GbE | 10.3 s | 1.16 GiB/s | 142 / 192 / 510 us | 2.5 ms / 4.1 ms / 7.5 ms |
| 2.5 GbE | 41 s | 0.29 GiB/s | 202 / 277 / 1614 us | 10 ms / 13 ms / 16 ms |
| 1 GbE | 103 s | 0.12 GiB/s | 272 / 391 / 1729 us | 25 ms / 34 ms / 92 ms |
| 10 GbE, RTT 0.5 ms | 10.5 s | 1.14 GiB/s | 545 us / 2.9 ms / 4.1 ms | 2.5 ms / 6.5 ms / 13 ms |
| 10 GbE, RTT 2 ms | 11.8 s | 1.01 GiB/s | 2.1 / 4.5 / 6.9 ms | 3.3 ms / 7.5 ms / 64 ms |
| 1 GbE, RTT 2 ms | 103 s | 0.12 GiB/s | 2.1 / 2.9 / 5.1 ms | 25 ms / 34 ms / 42 ms |

(*) 25 GbE is a floor. The netem shaper plus the target, initiator and fio all
sharing 8 vCPUs gave 1.9 GiB/s where the wire would give 2.9 GiB/s; the
unshaped loopback ceiling on this VM is 4.3 GiB/s. No 25/100 GbE hardware was
available, so those speeds are extrapolated from the 10 GbE rows and from
published numbers, not measured on a wire.

Repeat runs (a second run of the 10 GbE profile on the parameterised scripts)
agreed within about 5% on load time (10.8 s vs 10.3 s). Tail numbers (p99.9 and
up) move by a factor of 2 between runs; read them as orders of magnitude. Each
cell is a single 10-20 s run.

**What the numbers say.**

- The 4 KiB QD1 penalty is about 140 us at 10 GbE and above (the modelled 0.1 ms
  RTT plus about 12 us of NVMe/TCP stack, which matches published results: a
  thesis measured NVMe/TCP over 100 GbE adding 22.5 us (27%) to a local SSD's
  83.5 us). A local gen-4 NVMe answers a QD1 read in about 80 us, so a remote
  one at 10 GbE costs roughly three times as long per read. At 60 fps (16.7 ms
  frames) that is invisible for any read the game overlaps, and visible only for
  a game that blocks the frame on tens of serial small reads.
- Load time is link-bound, as expected: a 6 GiB level takes about 10 s at
  10 GbE (SATA-SSD class), 41 s at 2.5 GbE and 103 s at 1 GbE (slower than a
  hard disk). Intel's published DirectStorage BulkLoad demo (9.14 GB in 1.34 s
  on a local NVMe, uncompressed) is the case remote disks are worst at.
- **Stutter comes from queueing behind bulk reads, not from distance.** With a
  1 MiB asset stream running, small reads wait behind it: p50 25 ms at 1 GbE,
  10 ms at 2.5 GbE, even though the idle RTT is 0.2 ms. Distance adds only its
  RTT (10 GbE with RTT 2 ms: QD1 p50 2.1 ms).
- **Separating latency-class I/O fixes most of it.** Second experiment
  (`lending-bench-qos.sh`): a second NVMe/TCP connection (own port, own
  controller) for the game's small reads, with strict priority over the bulk
  connection on one shared link (an htb class pair; on real gear a DSCP/802.1p
  class or a per-queue egress priority):

| link | foreground reads share the bulk connection | foreground reads on a prioritised connection |
|---|---|---|
| 1 GbE | p50 17-26 ms, p99.99 58-60 ms | p50 0.34 ms, p99.99 4.8-5.1 ms |
| 2.5 GbE | p50 0.3 ms, p99 19 ms, p99.99 24 ms | p50 0.19 ms, p99 1.1 ms, p99.99 4.0 ms |
| 10 GbE | p50 0.16-0.31 ms, p99 5.5-6.1 ms, p99.99 9.6-20 ms | p50 0.16 ms, p99 2.0-3.6 ms, p99.99 4.7-13 ms |

  (Two runs each at 1 GbE and 10 GbE, one at 2.5 GbE. The 16.7 ms frame budget is
  met at p99.99 on every link only with the split.) The bulk stream keeps 83-100% of its
  throughput (1 GbE: 0.10 vs 0.12 GiB/s). The 10 GbE shaper model in this experiment (htb) queues less than
  the netem-rate model in the main table, so the p50 there is better; the 1 GbE
  and 2.5 GbE conclusion is the same in both models.
- Everything above is a model of the link, and the target is RAM. A real remote
  NVMe adds the device's own 80 us and its queueing. Real two-machine runs on
  10/25 GbE (and RDMA) remain to be done on lab hardware (section 13.5, step 7f).

**Policy that follows (all values are settings, see 13.4):**

1. **Default for lending a GPU: replicate ahead, run local.** The VM's game
   library disk is a zvol or qcow2 on ZFS; Horcrux's existing snapshot replication
   (`zfs send` incremental, `crates/horcrux-vm/src/vm/replication.rs`) keeps a
   copy on every node that may lend its GPU, and a lend session runs on that
   copy at local speed. Cost in time at full link rate, per 100 GB of changed
   data (arithmetic, not measured): 1 GbE about 15 min, 2.5 GbE about 6 min,
   10 GbE about 1.5 min. After the first full copy only changed blocks move.
2. **On-demand remote disk (NVMe/TCP) only when the link qualifies:** at least
   10 GbE and RTT at most 0.5 ms by default (measured by a link probe between
   the two nodes, not assumed), always with a separate latency-class connection
   for the guest's small reads and a read cache on the GPU node. Below that the
   UI says why ("link 1 GbE: a 6 GiB level would take about 100 s from the
   remote disk; replicate the disk first") and offers replication instead.
3. **iSCSI exists in Horcrux already** (`crates/horcrux-storage/src/storage/iscsi.rs`);
   NVMe/TCP is a new pool type beside it, not a replacement. NVMe/RDMA (RoCE) is
   an optional later transport where both NICs support it (published
   comparisons: RDMA gives clearly better 4 KiB IOPS and CPU use than TCP, while
   at 1 MiB both hit the link or media ceiling).
4. **No dirty-state surprises:** a lend session never runs on a replica that is
   behind unless the user accepts it; at session end the replica's changes are
   sent back (or the replica becomes primary), same as a planned cold move.

### 13.2 True remote GPU: evaluation

What it would be: the guest sees a Vulkan device, a guest-side driver serialises
Vulkan calls, a renderer on node B replays them on the real GPU. For Windows:
DirectX 9-12 goes through DXVK/vkd3d-proton to Vulkan inside the guest, so
Vulkan is the only API to carry.

**What exists (checked 2026-11-19):**

- **Venus (virtio-gpu Vulkan serialisation, Mesa).** It is designed for a guest
  and host on the same machine: commands travel over shared-memory rings, GPU
  memory is shared as blob resources via file descriptors, and the host side
  is virglrenderer. The vtest server variant uses a Unix socket and needs host
  file descriptors. As far as I found, nothing in Venus is built for a network between guest and
  renderer.
- **Windows guest drivers for Venus** now exist, all alpha: Yttrium's
  `viogpu3d` WDDM driver (BSD-3-Clause/MPL-2.0) with Mesa's Venus ICD (MIT),
  D3D12 through vkd3d-proton (LGPL-2.1+) and DXVK (zlib), packaged by third
  parties (vmlab-win3d, vgpu3d, Venera). They are test-signed and need Secure
  Boot off, which is the opposite of the anti-cheat posture in section 5, and
  NVIDIA hosts need a patched QEMU. Useful as same-host virtual GPU (vGPU-like
  sharing for light 3D), not as a replacement for passthrough.
- **A standalone Vulkan-over-TCP ICD** (`vulkan_remote`, 2026, GitHub) measured
  its own cost: 15.8 us round trip on loopback, about 1059 synchronous calls
  per 16.7 ms frame in the naive protocol, 7.2 ms RTT on a real link; with
  asynchronous command recording and shadow mapped memory it reaches 2
  synchronous calls per frame, and `vkcube` over a real link still runs under 7
  fps. Its author concludes that a real game's thousands of calls per frame
  make remote rendering a job for shipped frames (video), not API calls.
- **GPU API remoting research for AI** (Wang et al., arXiv 2401.13354): with
  asynchronous calls and shadow state, latency of 5-20 us is enough for under 5%
  overhead; 200 Gbit RDMA at 2-5 us RTT gives at most 6% overhead on AI
  workloads. The reason it works is that AI kernels run 10-100 ms on the GPU, and
  a frame of a game is 4-17 ms with thousands of small calls and readbacks.
- **CUDA remoting** (rCUDA, and others) is compute only; it does not serve a
  game and is NVIDIA-specific.

**Numbers for a game (derived):**

Blocking round trips a frame can afford if remoting may use at most 25% of the
frame (frame 16.7 / 8.3 / 4.2 ms for 60 / 120 / 240 fps):

| round-trip time | 60 fps | 120 fps | 240 fps |
|---|---|---|---|
| RDMA 3 us (paper: 2-5 us) | 1389 | 694 | 347 |
| local-socket class 16 us (`vulkan_remote` loopback) | 260 | 130 | 65 |
| LAN TCP 100 us (modelled above) | 41 | 20 | 10 |
| TCP stack only, no wire (our loopback QD1 p50, 12 us) | 347 | 173 | 87 |

A naive protocol needs about 1000 per frame (the `vulkan_remote` measurement),
so even RDMA misses 120 fps. Only an asynchronous protocol that hides almost all
round trips can work, and that conflicts with what games do (read back query
results, wait on fences, map host-coherent memory, update descriptors every
draw).

Bandwidth: textures and buffers the CPU side produces cross the link too. A
6 GiB level load is the same 10 s at 10 GbE as in 13.1, and modern games stream
at hundreds of MB/s to GB/s while playing (DirectStorage's stated target is
50,000 IOPS; previous-generation budgets were about 50 MB/s). If the finished
frame had to come back to A for display, raw frames are 3.98 GB/s at 4K120
(31.9 Gbit/s), 3.54 GB/s at 1440p240 (28.3 Gbit/s), 0.50 GB/s at 1080p60
(4.0 Gbit/s); with encode on B (the normal Horcrux path) frames never cross the
A-B link, and then the only thing true remote GPU buys over lending is that
CPU/RAM/disk stay on A.

**Game types it could serve:** turn-based, strategy, visual novels, 30 fps
cinematic games, on 25-100 GbE or RDMA with an asynchronous protocol. Not
competitive shooters, racing or anything at 120 fps+ on TCP.

### 13.3 Decision

- **Phase 7 builds compute-follows-GPU** (13.1 policy and the pool in 13.4).
- **No prototype of true remote GPU.** The numbers do not justify it: a game
  frame needs hundreds to thousands of call round trips against a 4-17 ms budget
  and a LAN RTT of 100 us at best (3 us with RDMA only), the Windows Venus
  drivers are alpha and test-signed, nothing carries Venus over a network, and
  Horcrux ends up streaming encoded frames from B anyway. Remote CUDA is out of
  scope for gaming and third-party remoting products are out by owner rule.
- **Revisit when all three hold:** a Windows Venus-class driver is signed for
  Secure Boot; an asynchronous remoting protocol shows under 10 blocking calls
  per frame on a DXVK or vkd3d-proton trace of a real game; and an RDMA link
  exists in lab hardware to measure on. Until then the question stays closed.

### 13.4 GPU pool: cluster scheduler design

Goal: every Horcrux node can contribute GPUs to a pool; sessions (gaming VMs,
later other GPU guests) are placed on a free GPU anywhere in the cluster, with
the disk handled by the policy in 13.1. A single node is a pool of one and
needs none of the cluster parts.

Phase 5 builds the pool for GPUs inside one cluster with shared or local disks;
the parts below that are specific to lending (link probe, disk modes, lender
priority, seeding) are phase 7 steps 7b-7d on top of it.

**Inventory (existing code to extend).** `NodeResources.available_passthrough_devices`
(`crates/horcrux-cluster/src/cluster/balancer.rs`) lists free passthrough
devices by vendor:device id, and `find_best_node` already filters on it, on
architecture, weight and labels. Add a richer, advertised record per physical
GPU, `GpuUnit`: node, PCI address, vendor/model/device id, VRAM, display outputs
present (headless flag), IOMMU group clean, reset method (flr, bus, none: from
the quirks table), driver class (for the "same model preferred" warning),
encoder capabilities (NVENC/AMF/QSV, AV1/HEVC/H.264), state (`free`, `held_by_host`,
`reserved`, `in_session`, `lent`, `disabled`), and the node's `lendable` policy.
Nodes publish it with the usage report they already send (`cluster::usage`).

**Request.** A session asks for a `GpuRequest`: vendor (any/specific), minimum
VRAM, encoder codecs needed, `class` (a named group of models the admin
declares compatible, for example "rtx50"), count (default 1), and a disk mode
(`local`, `replicate`, `remote_nvme`, `auto`). `required_passthrough_devices`
stays the low-level form; the pool translates a request into it.

**Placement, in order (hard filters, then score):**

1. Filters: architecture; GPU matches the request and is `free` (not
   `held_by_host`, which is a node whose display is on it and the host
   hand-off would refuse); node is `lendable` for this user and time window;
   node role/labels/limits as today; for disk mode `auto`/`remote_nvme`, the
   link from the disk's node passes the thresholds in 13.1 (otherwise only
   `replicate`/`local` nodes remain); for `replicate`, a replica of the VM's
   disks exists or can be seeded within `seed_budget_secs`.
2. Score: prefer the node that already holds the disk (no data movement), then a
   node with a fresh replica, then the same GPU model as last time (stable driver
   and hardware identity, which also keeps anti-cheat hardware identity stable),
   then the lowest link RTT to the user's endpoint, then lowest load; the
   existing node `priority`/placement weight applies as a tie-breaker.
3. Result carries a reason list per rejected node (shown in the UI; never a
   silent "no capacity").

**Sessions and queueing.** States: `queued` -> `placing` (seed/replicate/attach
disk) -> `starting` (hand-off, VM start) -> `running` -> `stopping` (hand-back,
sync-back) -> `ended`. A queue per class with FIFO within a user group and
weighted fair share between groups; `max_queue_wait_secs` then a clear refusal.
No preemption of a running session. A lender's own use has priority by
setting: `lender_priority` = `owner` (its local VM or desktop reclaims the GPU
after a notice of `reclaim_notice_secs`, session is saved/stopped cleanly) or
`pool` (lending wins). Idle timeout, maximum session length and per-user quota
(concurrent sessions, GPU hours per period) are settings with defaults; usage is
audited.

**HA.** A GPU session is a cold-restart guest (a passed-through GPU does not
live-migrate). On node loss the HA manager re-places it through the same
scheduler with `reserve_spare_gpus` per class kept free if the admin asks for
N+1; disk mode `replicate` makes the restart lose at most the replication
interval. The endpoint (section 11) follows the session.

**Link probe.** Each node pair measures RTT and throughput (a few seconds of
TCP, on a schedule and on demand, never during a session on that link) and
publishes a link class; admins can override with a declared speed. This is what
the disk-mode thresholds read. Without a measurement the pair counts as
"unknown", and `auto` falls back to `replicate`.

**Settings (`[gpu_pool]` in `/etc/horcrux/config.toml`, declarative, API and UI;
the values below are defaults, none is baked into code):**
`enabled`, `lendable` (per node, default false for the node's display GPU),
`lender_priority` (owner), `reclaim_notice_secs` (60), `min_link_mbps_remote_disk`
(10000), `max_rtt_us_remote_disk` (500), `disk_mode` (auto),
`latency_class_connection` (true), `read_cache_size_gib`, `seed_budget_secs`,
`replication_interval_secs`, `max_queue_wait_secs`, `idle_timeout_secs`,
`max_session_secs`, `quota_sessions_per_user`, `quota_gpu_hours_per_period`,
`reserve_spare_gpus`, `link_probe_interval_secs`, `link_probe_secs`,
`nvmeof_transport` (tcp), `nvmeof_port`, `nvmeof_nqn_prefix`,
`nvmeof_io_queues`, `compatible_classes` (admin list).

**Surfaces.** Pool management (units, links, queue, policy) lives under
`/api/gpu/pool/...`, which the existing `gpu` rule in `privileges.rs` already
maps to Sys.Audit (read) and Sys.Modify (write) on `/nodes`: administrators.
User-facing session routes (request, queue position, stop) are a NEW area and
need their own `privileges.rs` rule on the VM path (for example VM.PowerMgmt on
`/vms/<id>`) plus the route-scan tests, so that a non-admin user can run their
own sessions. UI: a pool
page (units with state and reason, link matrix with the measured class, queue,
active sessions, per-node lend switch), and the gaming VM wizard shows the
disk mode it will use and why. Audit entries for lend, start, reclaim, stop.
Metrics: GPU utilisation per unit, queue depth and wait, link RTT/throughput,
replica lag.

**Code placement.** `horcrux-cluster` gets `gpu_pool` (inventory, scheduler,
session state machine, link table); `horcrux-storage` gets `nvmeof.rs` (target
export via configfs with a declarative config generated at boot for both inits,
initiator connect/disconnect, latency-class second connection, read cache);
`replication.rs` learns seed/sync-back for lending; `horcrux-api` gets the
handlers; the wizard calls the pool.

### 13.5 Phase 7 split

| step | content | needs |
|---|---|---|
| 7a | `nvmeof.rs` pool type + target/initiator + OpenRC/systemd units, e2e on two vmtest guests with netem | vmtest (Debian 13 guests have nvme-tcp) |
| 7b | link probe + link matrix API/UI | none |
| 7c | `GpuUnit` advertisement, `gpu_pool` scheduler + unit tests (many nodes, mixed vendors, filters, scoring, reasons) | none |
| 7d | replicate-ahead seed/sync-back, `lend session` state machine, reclaim, quotas | ZFS in test guest |
| 7e | UI pool page + wizard disk-mode explanation | none |
| 7f | real 10/25 GbE and RDMA two-machine runs of `lending-bench`, real remote NVMe media, a real second GPU node | lab hardware |

### 13.6 Proven here and what is not

Proven (this phase): the NVMe/TCP path works on the real kernel (nvmet-tcp +
nvme-tcp, kernel 7.0) with the harness above; the load-time, QD1 and stutter
numbers in 13.1; the effect of a prioritised second connection; the
derivations in 13.2 from sourced numbers. Not proven: any real wire at 10 GbE or
faster (modelled), a real SSD behind the target, RDMA, game-level frame pacing
on a remote disk, and the scheduler itself (design only). Needs real GPU
hardware: everything in 7d that starts a VM on a lent GPU, and any test of
hand-back between two GPU nodes.

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
6. Protected gaming VM + attestation; confidential tier where hardware allows.
7. **Research and design done (2026-11-19):** GPU lending across nodes (compute follows
   the GPU, disk by replicate-ahead or NVMe/TCP) with measured numbers, true
   remote GPU evaluated and rejected for now, GPU pool scheduler design (section
   13). Build steps 7a-7f are in section 13.5; 7f needs lab hardware.
