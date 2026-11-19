# Gaming VM: Windows with a real GPU on Horcrux

Status: phase 1 (research, design, host tuning + managed GPU hand-off) implemented;
phases 2-4 below are the build plan. Owner idea, 2026-11-18: install Windows for
gaming, with a virtualized/passed-through graphics card, able to play current
FPS games.

This document is honest about limits: a VM can give Windows a whole discrete GPU
at near-native speed, but several popular competitive games refuse to run in a
VM by design (section 4). Horcrux will **not** hide the hypervisor from
anti-cheat software: doing so breaks the games' terms of service and puts the
player's accounts at risk. For those games the supported routes are Steam/Proton
on the Horcrux host (when the game allows Linux) or Windows on bare metal next
to Horcrux (dual boot, phase 4).

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
passed-through GPU) or in a window on the laptop panel through Looking Glass
(phase 2). The firmware must stay in hybrid mode (dGPU-only/MUX-to-dGPU mode
moves the panel to the dGPU and the host would lose it).

Muxless laptops (dGPU shows as "3D controller" with no outputs) can still pass the
dGPU through, but only Looking Glass (with its Indirect Display Driver) can show
the picture; some need the vBIOS supplied from ACPI `_ROM` (romfile) and a fake
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

| Game | Anti-cheat | In a Horcrux VM | On the host (Proton/native) | Route |
|---|---|---|---|---|
| Valorant | Riot Vanguard | **Blocked** (Riot: VMs not supported, error VAN 138) | Denied | Bare-metal Windows |
| League of Legends | Riot Vanguard | **Blocked** | Broken | Bare-metal Windows |
| Call of Duty BO6 / BO7 / Warzone | RICOCHET + Secure Boot/TPM remote attestation | Unsupported (attestation-based; only "works" with hiding) | Broken | Bare-metal Windows |
| Battlefield 6 / REDSEC, BF2042 | EA Javelin, Secure Boot required (BF6) | Unsupported (EA names VMs as a threat) | Broken | Bare-metal Windows |
| EA Sports FC | EA Javelin | Unsupported (same family) | Broken | Bare-metal Windows |
| ARC Raiders | Denuvo Anti-Cheat | **Blocked, ban risk** (Embark: "VMs are disallowed") | **Works** | Host |
| The Finals | EAC + Denuvo AC | Unknown, same studio as ARC Raiders | **Works** | Host |
| Fortnite | Easy Anti-Cheat | Kicks reported | Denied (Epic working on "EAC Linux v2") | Bare-metal Windows |
| Apex Legends | EAC | Unknown | Broken (Linux blocked Oct 2024) | Bare-metal Windows |
| Rainbow Six Siege X | BattlEye | Kicks reported | Denied | Bare-metal Windows |
| PUBG | BattlEye | Unsupported (support reply, reported) | Broken | Bare-metal Windows |
| Escape from Tarkov | BattlEye | Blocks launch | Broken | Bare-metal Windows |
| Destiny 2 | BattlEye | Reported to work | Broken (Bungie blocks Linux) | VM (reported) / bare metal |
| GTA Online | BattlEye | Unknown | Single-player only | Bare-metal Windows |
| Counter-Strike 2 | VAC | Works (reported, no policy) | **Works, native** | Host |
| Deadlock | VAC | Works (reported) | **Works** | Host |
| Marvel Rivals | NetEase NEAC | Unknown | **Works** | Host |
| Overwatch 2 | Blizzard | Works (reported) | Works (community) | Host or VM |
| Halo Infinite | EAC | Unknown | **Supported** | Host |
| Splitgate 2 | RedKard | Unknown | **Works** | Host |
| Delta Force | Tencent ACE | Unknown | SteamOS only | Bare-metal Windows |

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
ARC Raiders) do not. Several of them (ARC Raiders, The Finals, Marvel Rivals,
CS2, Deadlock, Halo Infinite, Splitgate 2) run on Horcrux itself through Steam.

## 5. Bare-metal Windows next to Horcrux (phase 4)

For the games above that need real Windows: Windows on its own disk (preferred)
or partition, with its own ESP entry, chosen from the Horcrux boot menu.

- Installer: "keep/add Windows" option that leaves a chosen disk untouched and
  adds a GRUB `chainloader /EFI/Microsoft/Boot/bootmgfw.efi` entry next to the
  boot-environment entries; `horcrux-be` keeps it when regenerating grub.cfg.
- Secure Boot: Windows needs it on for BF6/CoD/Valorant. GRUB must then be
  shim-signed or Horcrux must enroll its own keys; until that exists the
  firmware's boot menu (F12) is the documented way to start Windows with Secure
  Boot on. BitLocker binds to PCR7 (Secure Boot state): changing Secure Boot
  settings triggers BitLocker recovery, so the recovery key must be saved first.
- Windows Fast Startup hibernates the NTFS volumes: turned off in the guide;
  the Horcrux side never mounts Windows partitions read-write.
- Clock: Horcrux keeps the RTC in UTC; the guide sets Windows'
  `RealTimeIsUniversal=1`.
- Never boot the physical Windows install inside the VM (activation and
  BitLocker recovery trouble).

## 6. Gaming VM wizard (phase 2)

UI page "New gaming VM" + `POST /api/vms/gaming` composing an ordinary VM config:

1. Host check (`GET /api/gpu/pci/gaming-check`, implemented): KVM, IOMMU, P/E
   cores and suggested vCPUs/pinning, huge-page pools, and per GPU: functions
   that move with it, monitors on it, whether the host keeps a screen, programs
   holding it, reset methods, known issues, verdict (`ready`,
   `ready-with-warnings`, `single-gpu`, `blocked`).
2. Choices with defaults from the check: GPU, vCPUs (all P-cores), memory
   (half of RAM, max 32 GB), disk (zvol), display (external monitor / Looking
   Glass), keyboard+mouse (evdev or USB controller).
3. Media: Windows 11 ISO (user-provided or Microsoft's download page; the
   90-day Enterprise evaluation ISO for testing), virtio-win ISO from
   fedorapeople stable-virtio; both go to the ISO library.
4. Unattended install: generated `autounattend.xml` on a small ISO (locale,
   user, virtio storage/net drivers from the virtio-win ISO, guest tools,
   Looking Glass host, Fast Startup off); TPM 2.0 + Secure Boot OVMF so no
   requirement bypass is needed. The licence key is the user's.
5. Looking Glass: KVMFR module (`app-emulation/looking-glass` in GURU has the
   client; Horcrux packages the module + client) with the IDD driver (no dummy
   plug), shared memory sized from the resolution; the client runs in the user's
   desktop session (Wayland supported).
6. One-click Start/Shut down; Start does the hand-off, Shut down gives the GPU
   back; the VM page shows who holds the GPU when the start is refused.

## 7. Compatibility matrix (platforms Horcrux supports)

| Platform | dGPU VFIO | Hand-off | Pinning `auto` | Huge pages | Looking Glass | Notes |
|---|---|---|---|---|---|---|
| amd64, Intel hybrid (Alder Lake+), iGPU + dGPU desktop | Yes | Yes | P-cores | 2M/1G | phase 2 | Best case |
| amd64 laptop with MUX/Advanced Optimus (hybrid mode) | Yes | Yes | P-cores | 2M | phase 2 | Panel stays on iGPU; external ports follow the dGPU |
| amd64 muxless laptop | Yes (romfile/ACPI quirks) | Yes | P-cores | 2M | phase 2 (only way to see the guest) | Not tested on hardware yet |
| amd64 single-GPU desktop | Yes | `allow_display_loss` (phase 3 finishes DM stop/console) | all but CPU 0 | 2M/1G | n/a | Screen goes to the VM |
| amd64 AMD Ryzen/EPYC | Yes | Yes | all but CPU 0 (no P/E) | 2M/1G | phase 2 | CCD-aware pinning: phase 3 |
| arm64 hosts | No Windows-on-ARM gaming target | Code path neutral | Yes | x86 guests only | No | Out of scope for gaming |
| OpenRC / systemd | Same code; services stopped through the init abstraction | | | | | Both |
| T2 Macs (Intel) | Single-GPU or iGPU-only machines; no supported dGPU passthrough | - | - | - | - | Not a gaming target |

## 8. Proof so far and what needs real hardware

Proven in CI-style tests (fixtures of the real sysfs layout, real syscalls where
possible): group collection, display/holder refusal, record + release ordering
(waits for `/dev/vfio/N` to close), Hyper-V/evdev/huge-page arguments, CPU
layout + `auto` pinning, real `sched_setaffinity` on test threads, huge-page
pool accounting, quirks table.

Nested spike (vmtest, phase 1): a Linux guest with an emulated IOMMU
(`intel-iommu`) hands an emulated PCI display device from its host driver to
`vfio-pci`, runs a nested QEMU with it, and gets it back on stop.

Only provable on a host with a real discrete GPU (planned on a
hybrid laptop with a discrete NVIDIA GPU): NVIDIA driver unbind/rebind under a live
GNOME session (mutter ignore tag), the Blackwell reset behaviour after guest
shutdown, Windows frame rates/latency with pinning, Looking Glass on the
panel, external monitor output from the guest.

## 9. Settings added (all with defaults, `[passthrough]` in /etc/horcrux/config.toml)

`hyperv_enlightenments`, `hyperv_cpu_model`, `handoff_stop_services`,
`handoff_refuse_when_held`, `handoff_timeout_secs`, `hugepages_release_on_stop`,
`quirks_files`. Per VM: `hostpci[].managed`, `hostpci[].allow_display_loss`,
`adv.cpu_pinning`, `adv.hugepages`, `adv.hyperv`, `adv.evdev`.
