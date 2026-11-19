# Horcrux vGPU: a host-owned, vendor-agnostic paravirtual GPU

Status: research + spike (2026-11-19). Nothing in this document is built into
Horcrux yet; section 2 lists what was measured on real hardware, section 10 is
the phased plan with go/no-go numbers.

Goal (owner, 2026-11-19): "we could make our own gpu slicer that handles this
for the host-os side and guest drivers/agent that handles the guest-os side",
and "it could be graphics card agnostic so doesn't matter which gpu is under
the hood or how many gpus there are". Concretely:

- many VMs share the GPUs of a host through one paravirtual GPU;
- the HOST owns all GPU state, so a VM with a running 3D workload can be
  saved/restored, live-migrated, and moved between GPUs (even of different
  vendors), and can run against a GPU on another node;
- the guest always sees the same Horcrux virtual GPU, whatever GPU (NVIDIA,
  AMD, Intel, integrated or discrete, one or many) is underneath;
- no vendor vGPU feature is required; vendor hardware slicing (SR-IOV, vGPU)
  is used where it exists and the user is entitled to it, never by bypassing
  licensing.

Related designs: `gaming-vm.md` (full GPU passthrough for Windows gaming,
Horcrux Console viewer/streaming, "VM on one machine, GPU on another") and the
VM save-on-host-shutdown work (which method saves a GPU VM).

## 1. Short answer

| Question | Answer today (measured or sourced) |
|---|---|
| Can a Linux guest share the host GPU through a paravirtual Vulkan GPU? | **Yes.** virtio-gpu + Venus works with stock Mesa 26.3 / virglrenderer 1.3 / QEMU 11.1: a real game (vkQuake timedemo, 1080p) runs at **83 %** of the host frame rate (section 2). |
| Can such a VM be saved / live-migrated today? | **No.** QEMU refuses: `virgl is not yet migratable`. Guest hibernation is refused too (`S4 not allowed when VIRGL is enabled`). Nobody upstream migrates Vulkan/GL guest state. |
| Can the host capture a running guest's GPU state and rebuild it elsewhere? | **Yes, in principle, proven in the spike:** the live Vulkan state of a guest game was captured from the host side (inside the renderer process, without guest cooperation) and rebuilt + replayed on the same GPU **and on a different Vulkan implementation** (CPU rasteriser) after re-mapping memory types. That is the mechanism Horcrux vGPU needs; it has to be engineered properly (section 5). |
| Windows guests? | A Windows driver stack (WDDM kernel driver + Vulkan ICD + D3D9-12 via Vulkan) is a **34-67 person-month** project; community prototypes exist but are test-signed only. Production needs Microsoft signing (Partner Center, which requires an EV code-signing certificate). Long-term project with go/no-go gates (section 7). |
| Hardware slicing (SR-IOV/vGPU)? | Real but vendor-limited: Intel Arc Pro B-series / Flex / supported Core iGPUs (xe SR-IOV, migration via xe-vfio-pci in Linux 6.19), NVIDIA vGPU with the user's licence, AMD MxGPU on Instinct/Radeon PRO V710. None on GeForce or consumer Radeon. Integrate now (phase A). |

## 2. Spike results (real hardware, 2026-11-19)

Host: a lab laptop with an Intel Iris Xe (Raptor Lake-U, 96 EU) iGPU, 12
threads, 16 GB; QEMU 11.1.3, virglrenderer 1.3.0, Mesa 26.3.1 (ANV and
lavapipe), all in a container on the Horcrux host. Guest: Arch Linux cloud
image, kernel 7.3.5, Mesa 26.3.1 Venus, 4 vCPUs, 4 GiB; device
`virtio-vga-gl,blob=true,venus=true,hostmem=2G` with a memfd memory backend and
`-display egl-headless`. The guest reports the GPU as
`Virtio-GPU Venus (Intel(R) Iris(R) Xe Graphics (RPL-U))`, Vulkan 1.4.334.
Scripts: `hx-vgpu-spike/` next to this document.

### 2.1 Frame rate, guest vs host (same GPU, same 1920x1080)

| Workload | Host (ANV) | Guest (Venus) | Guest / host |
|---|---|---|---|
| vkQuake 1.37 `timedemo demo1`, 1080p, headless Wayland compositor, 5 runs | 306-309 fps (median 308) | 254-260 fps (median 255) | **83 %** |
| vkmark 2025.01 score, 1080p headless | 8510 | 3238 | 38 % |
| vkmark `effect2d blur` (GPU-heavy scene) | 1391 fps | 1212 fps | 87 % |
| vkmark `clear` (empty frame) | 14727 fps | 4288 fps | 29 % |

Reading: Venus adds a roughly **fixed 0.1-0.2 ms per frame** (guest/host
synchronisation and command serialisation: clear 0.068 -> 0.233 ms, blur 0.719
-> 0.825 ms), plus ~0.65 ms per frame in the vkQuake case (WSI + compositor
path). At game frame rates (4-16 ms per frame) that is a few percent to ~20 %;
it only dominates at thousands of fps. CPU-bound titles that do many
synchronous queries will lose more (section 7.3).

### 2.2 Save / restore

| Test | Result |
|---|---|
| QMP `migrate file:` with Venus enabled | `GenericError: virgl is not yet migratable` (QEMU blocker in `hw/display/virtio-gpu-base.c`) |
| Guest `systemctl hibernate` with Venus enabled | refused by the guest kernel: `virtio_gpu_pm_notifier: S4 not allowed when VIRGL is enabled` |
| Same VM with a 2D `virtio-vga` (control) | save 3.9 s / 116 MB, restore 0.65 s; a counter loop in the guest continued from where it stopped |

So a VM with today's 3D virtio-gpu can be neither host-saved nor
guest-hibernated: on host shutdown it can only be shut down. This is the
concrete gap Horcrux vGPU closes.

### 2.3 Host-side state capture and cross-GPU rebuild

The GFXReconstruct capture layer (LunarG, MIT; used here only as an experiment
tool, nothing shipped) was loaded into the host's Venus render server process,
i.e. on the host side, invisible to the guest. Its "trimmed capture" writes a
snapshot of every live Vulkan object and the contents of its memory at a chosen
queue submission, then records what follows.

| Guest workload | Snapshot taken | Snapshot size (LZ4) | Rebuild + replay on host ANV | Rebuild + replay on lavapipe (different driver/"vendor") |
|---|---|---|---|---|
| vkmark (phong scene), submit 300 | 31 ms | 1.6 MB | rc 0, 0.05 s | fails as-is (memory type indices and two extensions differ); **rc 0** with memory re-binding and unsupported extensions removed |
| vkQuake mid-demo, submit 600 (+500 submits) | 2.34 s | ~51 MB | rc 0, 0.72 s | rc 0, 10.8 s (with re-binding) |

What this proves: from the host, without guest help, the complete Vulkan
object graph of a running guest application plus its memory contents can be
captured in seconds and recreated on another Vulkan device, including one with
different memory types and extensions. What it does not prove yet: pixel
equality of the rebuilt state (the replays completed without errors; an
image-level comparison was not achieved in the time box), resuming the
*guest* against the rebuilt state (needs the guest-handle mapping of section
5.3), and coverage of every Vulkan feature.

### 2.4 Remote GPU volume (VM on node A, GPU on node B)

- vkQuake 1080p, 969 frames: the full host-side API stream, including a copy
  of every mapped allocation that may have changed at each submit (an upper
  bound), was ~14.5 KB per queue submission compressed, i.e. roughly **3.5
  MB/s (~28 Mbit/s) at 240 fps**, plus a one-off ~50-110 MB of assets.
- Requirements derived: bandwidth is not the problem for a game of this class
  on a LAN; **round-trip latency is** (every synchronous call: fence waits,
  query results, mapped-memory reads). On 10-100 GbE in one rack (RTT
  20-100 µs) this is workable; over WAN it is not, and a remote user is
  served by Horcrux Console streaming from the GPU node instead (section 6).

### 2.5 Vendor independence of today's Venus

Venus does *not* run on every Vulkan driver: with the host's lavapipe as the
backend the guest enumerates the device but its first blob allocation fails
(`RESOURCE_CREATE_BLOB` error 0x1200) because the host driver cannot export
device memory as a dma-buf. Venus needs host drivers with
`VK_EXT_external_memory_dma_buf` + `VK_EXT_image_drm_format_modifier`
(pre-GFX9 AMD is excluded for the same reason; Collabora documents NVIDIA
proprietary issues). Horcrux vGPU must fall back to copy-based memory when a
host driver can't export, so any conformant Vulkan driver works (section 5.4).

## 3. Prior art (ideas only, clean-room; licences checked 2026-11-19)

Full notes with sources: kept with the research for this design. Horcrux is
GPL-3.0; nothing below is copied.

| Project | Licence | Status | GPU state saved/migrated? | Idea to borrow | Avoid |
|---|---|---|---|---|---|
| virtio-gpu + virglrenderer (virgl GL, **Venus** Vulkan, DRM native context) | MIT (virglrenderer, Mesa); QEMU GPL-2.0 | active (virglrenderer 1.3.0, Mesa 26.3.1, QEMU 11.1) | **no** (QEMU blocker) | virtio-gpu spec, capsets/contexts, Venus async ring + blob memory, separate render-server process | native context for migratable VMs (ties guest to the host's kernel driver UAPI) |
| QEMU vhost-user-gpu / rust-vmm vhost-device-gpu | GPL-2.0 / Apache-2.0 OR BSD-3-Clause | vhost-device-gpu 0.2.0, Venus not yet enabled | no | GPU work in a separate per-VM process selected by render node | same-host fd passing as the only transport |
| crosvm rutabaga_gfx + gfxstream | BSD-3-Clause / Apache-2.0 | very active | **partial**: gfxstream snapshot/restore "WiP", virgl "not supported", cross-host "not explicitly supported" | `snapshot(dir)`/`restore(dir)`; "renderer recreates objects, VMM re-attaches backing and remaps memory"; restore deferred to resume | assuming same host GPU |
| Android Emulator Quickboot (gfxstream) | Apache-2.0 | active | yes, same host: Vulkan calls recorded and replayed in dependency order; unsupported API => snapshot marked invalid, cold boot | record/replay ledger, trip wires, versioned metadata | replay that silently misses features |
| VMware SVGA3D / vSGA | proprietary host, vmwgfx in Linux | shipping | **yes**: suspend and vMotion with 3D on, to another GPU or the software renderer; capability check at restore | portable idealised protocol, feature-level check, software fallback, guest-memory-backed objects | D3D9-era scope |
| Hyper-V GPU-PV (WSLg dxgkrnl) / GPU-P | proprietary; Linux dxgkrnl out of tree | active | GPU-PV no; GPU-P yes on Server 2025 with NVIDIA vGPU 18+ / AMD V710 (vendor hardware state) | thin guest kernel driver + user-mode driver split | needs a Windows host and vendor UMDs in the guest |
| Parallels / Apple ParavirtualizedGraphics | proprietary | active | Parallels suspends 3D VMs (DX11 -> Metal); Apple's save API does not document GPU state | host API translation with suspend; VM video-memory size validated at resume | closed |
| NVIDIA vGPU (mdev / SR-IOV) | proprietary, licensed | 21.x; open nova-core vGPU manager under review | yes (same manager version, ECC, topology) | migration compatibility checks | lock-in; never unlock/bypass |
| Intel GVT-g | GPL-2.0 | **dead** (archived 2024-10-03) | - | - | - |
| Intel xe SR-IOV + xe-vfio-pci | GPL-2.0 | xe SR-IOV PF in 6.17+, xe-vfio-pci in 6.19 | yes, stop-and-copy (no pre-copy) | packetised, chunked, versioned state stream with a compatibility descriptor | only Arc Pro B / Flex / listed Core iGPUs |
| AMD MxGPU (GIM) | open repo (licence of new repo not verified) | 9.4.0.K 2026-11-18 | not on KVM (Hyper-V GPU-P only) | per-VF world-switch scheduler | card whitelist |
| Looking Glass | GPL-2.0 | active | no (display transport for passthrough) | shared-memory frame handoff | not a vGPU |
| rCUDA / Juice / Bitfusion / VAR thesis / Venus-over-TCP hobby repo | mixed | Juice stale (2023), Bitfusion EoL 2025 | no | async command streaming; latency budget < 25 ms | CUDA-only, synchronous round trips |
| Windows: viogpudo, viogpu3d PR #943, utmapp/viogpu-wddm, fastchain/vgpu3d | BSD-3/BSD-2; vgpu3d mixed (incl. LGPL vkd3d-proton) | viogpudo signed display-only; 3D drivers test-signed; PR #943 open since 2023 | no | WDDM 2 miniport over virtio-gpu, Venus ICD on Windows, DXVK/vkd3d-proton inside a real UMD, Zink for GL | test signing; unproven provenance |
| DXVK / vkd3d-proton / Zink | zlib / LGPL-2.1 / MIT | active | n/a | D3D -> Vulkan building blocks for the Windows guest | static linking of LGPL code |

Conclusions: (1) the protocol and guest drivers exist (Venus, Mesa); (2) the
missing layer everywhere is **host-side state ownership** (VMware has it,
gfxstream is building it, QEMU/virglrenderer have nothing); (3) hardware
slicing migrates only on specific vendor cards.

## 4. Architecture

```
 guest (Linux: Mesa Venus | Windows: Horcrux WDDM + Venus ICD + D3D UMD)
        | virtio-gpu (capsets: venus, virgl; later horcrux-vk)
 QEMU   | vhost-user-gpu front end  (+ device-state transfer for migration)
--------+-------------------------------------------------------------------
 node   | horcrux-gpud   one process per VM, sandboxed (seccomp, own uid)
        |   decoder ........ Venus wire protocol -> Vulkan calls
        |   handle table ... guest object ids -> host objects (indirection)
        |   state ledger ... creation records + dependency graph + content refs
        |   memory manager . virtual memory types, VRAM budget, dirty tracking
        |   scheduler hook . per-VM submission queue, weights, priority
        |   transport ...... local (shared memory) | remote (QUIC/TLS, RDMA)
        +--> any GPU via its standard Vulkan driver
 horcrux-api: GPU pool, placement, quotas, feature levels, save/migrate/move
```

### 4.1 One stable virtual GPU in the guest

- The guest device is always virtio-gpu (stable PCI IDs, same driver
  everywhere). Today Venus forwards the host's Vulkan identity (the spike guest
  saw "Intel Iris Xe"). Horcrux vGPU exposes a **Horcrux device profile**
  instead: a fixed name/vendor id, and properties, limits, features, formats
  and memory types taken from the VM's *feature level* (4.2), not from the
  physical GPU. An opt-in "pinned" mode exposes the host identity for
  applications that need vendor-specific paths, at the price of only moving
  between identical GPUs.
- Memory types are virtual: the guest sees a small fixed set (device-local,
  host-visible coherent, host-cached); the manager maps them to whatever the
  current GPU offers. The spike showed this is required: replay on another
  driver failed until allocations were re-bound to that driver's types.

### 4.2 Feature levels (capability negotiation)

- A feature level is a named Vulkan profile (in the spirit of the Khronos
  Vulkan Profiles / roadmap profiles): API version, extension list, feature
  bits, limits, formats. Horcrux ships levels (e.g. `hx-vk13-baseline`,
  `hx-vk14-desktop`, `hx-vk14-raytracing`) and computes per pool which GPUs
  satisfy which level.
- A VM is created at a level (default: the highest level all GPUs of its
  placement pool support). It can be placed on, moved to, restored on or
  migrated to any GPU that satisfies that level, on any node, any vendor.
- Raising a VM's level is a cold change (guest restart). Lowering the pool
  (a GPU removed) never silently changes running VMs: the UI shows which VMs
  can no longer move where, with the reason.
- The spike's cross-driver replay dropped two extensions the destination did
  not have (`VK_EXT_image_drm_format_modifier`,
  `VK_EXT_external_memory_acquire_unmodified`): such host-only extensions
  stay internal to the transport and are never part of a guest feature level.

### 4.3 GPU pool, placement and fairness

- Every node advertises its GPUs (vendor, model, VRAM, driver, supported
  feature levels, encode engines, outputs) to the cluster; standalone single
  machines are the same with a pool of one node.
- Placement: each VM's GPU context goes to the GPU with the best fit (free
  VRAM budget, load, same NUMA node as the VM's vCPUs, encoder availability for
  its Console stream). Policy keys: `pack` / `spread` / pinned GPU.
  A VM can span GPUs only as separate devices (multi-GPU in the guest);
  splitting one context across GPUs is out of scope.
- Fairness: per-VM submission queues in `horcrux-gpud` with weights and a
  per-VM time budget per scheduling period, enforced by throttling
  submissions (a VM over budget waits for its fences before the next batch);
  host queue priority (`VK_EXT_global_priority` / `VK_KHR_global_priority`)
  for interactive vs batch VMs where the driver supports it. GPUs have no
  general preemption API in Vulkan, so a single very long shader can still
  hold the GPU until the host driver's own timeout resets that context; the
  reset is confined to that VM's process and reported.
- VRAM: per-VM budget (`vram_limit_mib`) checked at every allocation;
  `VK_EXT_memory_budget` feeds the pool view. Overcommit is opt-in: over-budget
  device-local allocations are placed in host memory (slower, never failing
  silently), shown per VM. Default is no overcommit, and allocations over the
  budget return out-of-device-memory to the guest, as on real hardware.
- Hot-add: a new GPU joins the pool and becomes a placement target. Hot-remove
  (or driver reset/GPU loss): the node moves the GPU's contexts to another GPU
  through the same snapshot path (5.2); if no target satisfies a VM's level,
  that VM is paused with the reason shown, never left with a lost device.

## 5. State ownership: save, restore, move, migrate

### 5.1 What makes it possible

All guest GPU work passes through `horcrux-gpud` as Vulkan calls. The daemon
therefore knows every live object, how it was created, what memory backs it
and which submissions are in flight. Two kinds of state exist:

1. object state: instances, devices, queues, memory, buffers, images, views,
   samplers, descriptor layouts/pools/sets, pipelines (from SPIR-V; the driver
   cache is rebuilt), render passes, command buffers (recorded commands are
   kept as encoded streams so they can be re-recorded), sync objects;
2. contents: device-local buffer/image contents (read back at a quiesce
   point), host-visible memory (lives in a guest-visible region and travels
   with guest RAM, see 5.3).

The ledger (creation records + a dependency graph) is maintained continuously
and cheaply (record what the decoder already parses); contents are read only
when saving. This is the gfxstream/Android-Emulator method and what
GFXReconstruct's trimmed capture did in the spike (31 ms / 2.3 s).

### 5.2 Save and restore (host shutdown, suspend to disk)

1. pause vCPUs; stop accepting guest submissions;
2. drain: wait for every queue's fences (bounded by `gpu_quiesce_timeout_s`;
   on timeout the save falls back to the next method in the save order);
3. read back device-local contents (staging copies on the GPU's copy queue);
4. write the device state section: ledger, contents, handle table, feature
   level, versioned format with a compatibility descriptor (borrowed from the
   xe-vfio-pci stream design); it goes into the normal QEMU migration stream
   via the vhost-user device-state transfer, so a VM save stays one file;
5. restore: create a fresh `horcrux-gpud` on the chosen GPU, replay the
   ledger with remapping (memory types, allocation sizes and alignments,
   queue families), upload contents, rebind the guest handle table (guest ids
   are stable, host objects are new), then resume vCPUs.

Estimate for a game: downtime is dominated by VRAM contents. Readback and
upload run at several GB/s on PCIe, so a 4-8 GB working set costs a few
seconds each way; pipeline rebuild on a different GPU adds shader compile
time (seconds to tens of seconds, hidden behind a Horcrux-managed pipeline
cache per feature level and GPU model).

### 5.3 Guest-visible memory

Host-visible ("mapped") allocations are written directly by the guest without
any call the daemon can see. Horcrux vGPU backs them with memory that is part
of the VM's migratable RAM (a dedicated shared-memory region registered with
KVM), so they are saved and live-migrated with guest RAM and their dirty pages
are tracked by KVM's dirty log. Copy-based backing (the daemon copies on
flush/submit) is used when the host driver can't import that memory; this is
also what makes drivers without dma-buf export usable (2.5).

### 5.4 Live migration and moving between GPUs

- Same node, other GPU (rebalancing, hot-remove): save to memory, restore on
  the other GPU; the VM pauses for the drain + copy (seconds), no network.
- Other node: guest RAM (incl. mapped GPU memory) pre-copies as usual; at
  stop-and-copy the GPU section (ledger + device-local contents) is sent; the
  target restores it on its GPU. GPU contents have no dirty tracking, so they
  are copied once at the end (like xe-vfio-pci, no pre-copy); downtime ~
  VRAM working set / network bandwidth (8 GB at 25 GbE ~= 2.6 s).
- Across vendors: allowed when the destination satisfies the VM's feature
  level; memory types and alignments are remapped (spike: required and
  working); image contents are copied in a layout-independent format
  (linear staging), never as raw tiled bytes.
- Anything the ledger does not cover (an extension without capture support)
  sets a per-VM "not movable" flag with the reason, and the save order falls
  back (guest hibernate is impossible for 3D virtio-gpu guests, 2.2, so the
  fallback is clean shutdown); never a corrupt restore.

## 6. Remote GPU (VM on node A, GPU on node B)

- `horcrux-gpud` runs on node B; the VM's virtio-gpu back end on node A
  forwards the Venus stream over the cluster network (QUIC with TLS using the
  cluster's node certificates; RDMA when available). Mapped memory is synced as
  page diffs on flush/submit (the 2.4 volume), device-local memory never
  leaves node B.
- The display never travels back to node A: scanout images stay on node B and
  are encoded there by Horcrux Console with node B's hardware encoder, so the
  user's stream comes straight from the GPU node (`gaming-vm.md` section 10).
- Latency budget: every synchronous call costs one network RTT. Measured
  volume (~28 Mbit/s for vkQuake at 240 fps) fits 1 GbE; usable RTT is a
  same-rack/same-switch network (<= 100 µs). Target classes: desktop,
  creative/CAD, strategy/RPG and older titles. Competitive FPS keeps local
  GPU or passthrough. "Compute follows the GPU" (run the whole VM on node B
  with its disk over the network) remains the default for gaming
  (`gaming-vm.md` section 13); remote GPU is for spare GPU capacity.
- PCIe over the network does not exist for commodity GPUs; this is API
  remoting only.

## 7. Guest side

### 7.1 Linux guests

Mesa's Venus driver (Vulkan 1.4 in the spike), Zink on top for OpenGL, and
virgl for older GL-only stacks. Nothing to write; Horcrux ships the device
configuration and checks the guest's Mesa/kernel versions (Venus needs Linux
6.13+ for host blobs, Mesa 22.1+, newer is better). The guest kernel's
`S4 not allowed when VIRGL is enabled` rule stays: hibernation is replaced by
host-side save.

### 7.2 Windows guests (long project)

Required pieces: a WDDM 2.x kernel driver over virtio-gpu (GpuMmu or IoMmu
model, scheduling, TDR/reset, display), the Venus Vulkan ICD built for
Windows, D3D10/11 and D3D12 user-mode drivers translating to Vulkan
(DXVK/vkd3d-proton-class code inside a real UMD, not replaced system DLLs, so
DXGI, DWM, flip-model swap chains and shared surfaces work), D3D9 via the
system's D3D9On12 or a D3D9 front end, OpenGL via Zink. Community prototypes
(utmapp/viogpu-wddm v0.6, fastchain/vgpu3d) prove the shape works; none is
signed, none has a native-vs-guest benchmark.

| Component | Person-months (low / high) |
|---|---|
| WDDM kernel driver over virtio-gpu (incl. display, reset, HVCI-ready) | 8 / 14 |
| Venus ICD on Windows | 3 / 6 |
| D3D10/11 UMD on Vulkan | 6 / 10 |
| D3D12 UMD on Vulkan | 6 / 12 |
| D3D9 path | 0.5 / 3 |
| Display + DWM composition | 3 / 6 |
| Test/QA incl. Windows HLK, HVCI, compatibility matrix | 4 / 8 |
| Host-side integration for Windows-specific needs | 3 / 6 |
| Signing and release engineering | 1 / 2 |
| **Total** | **~34 / ~67** (9-18 calendar months with 3-5 engineers; ~40-60 % less if a permissively licensed base is adopted and hardened) |

### 7.3 Constraints (facts, no workarounds)

- **Signing.** 64-bit Windows 10/11 loads kernel drivers only with a Microsoft
  signature (attestation signing or WHQL/HLK through Partner Center).
  Partner Center registration requires an EV code-signing certificate, which
  is a recurring cost (roughly $280-700 per year at the CAs checked). Drivers
  must pass HVCI readiness. Test-signing mode works for development but needs
  Secure Boot off.
- **Anti-cheat.** BattlEye and EAC are reported to refuse to run with
  test-signing on, and Vanguard documents that it does not support VMs.
  Horcrux does not hide the VM or the virtual GPU and does not evade
  anti-cheat (`gaming-vm.md` section 5); kernel anti-cheat titles are out of
  scope for this path, as for every VM.
- **Unknown GPU.** Games see a "Horcrux vGPU" adapter; titles with vendor
  allow-lists or vendor-specific paths may refuse or take slow paths; the
  per-game compatibility database covers it.
- **Performance ceiling.** Guest D3D -> Vulkan translation (DXVK-class:
  typically 10-20 % below native D3D) + Venus serialisation (2.1) + host
  driver: estimated 80-95 % of passthrough for GPU-bound games and 50-80 % for
  CPU-bound/high-fps ones. Latest competitive FPS at high refresh: use
  passthrough.
- **Consumer GPUs have no SR-IOV** (GeForce: passthrough to one VM only;
  consumer Radeon: not in AMD's MxGPU list). That is why the paravirtual path
  is the only way to share them.

### 7.4 Windows go / no-go gates

Go to a full build only if all hold:
1. a 4-6 week performance spike on Intel, AMD and NVIDIA hosts reaches >= 70 %
   of native fps on a GPU-bound D3D11 and a D3D12 title and >= 50 % on a
   CPU-bound title at 1080p, p99 frame time <= 2x native, no tearing;
2. a prototype kernel driver passes HVCI readiness and Driver Verifier with
   Secure Boot on (pre-production signing);
3. the owner decides on the signing account/certificate (a cost);
4. licence plan done (vkd3d-proton LGPL as separate DLLs; no GPL-2.0-only code
   copied into Horcrux's GPL-3.0 tree);
5. >= 70 % of a ~50-title offline game/app matrix runs without systematic
   crash classes.

No-go if Microsoft signing or HVCI readiness can't be reached, the GPU-bound
result is < 50 %, or the target workloads need kernel anti-cheat titles.

## 8. Hardware slicing where it exists (phase A)

| Hardware | Mechanism | Save/migrate | Horcrux work |
|---|---|---|---|
| Intel Arc Pro B-series and Data Center GPU Flex; iGPUs on Intel's SR-IOV list (not consumer Arc B, not Lunar Lake). Mainline xe PF support is confirmed for Battlemage and Panther Lake (and Tiger/Alder Lake when driven by xe); Intel lists Meteor/Arrow Lake but mainline xe does not enable them yet, and the out-of-tree DKMS route is not used | xe SR-IOV PF (Linux 6.17+), VFs to VMs; native Intel guest drivers (Windows/Linux) | stop-and-copy via `xe-vfio-pci` (Linux 6.19+) | detect PF capability + `sriov_totalvfs`, provision VFs (count, VRAM per VF) declaratively, assign VFs, read the VFIO migration flags per device |
| NVIDIA vGPU-capable cards with the user's own vGPU licence | mdev (older) or per-VF SR-IOV (Ada/Blackwell) | `virsh`-style live migration with identical manager versions | mdev support exists in Horcrux (`horcrux-vm` mdev); add profile display, licence status shown, migration capability per device |
| AMD Instinct / Radeon PRO V710 | GIM SR-IOV | no KVM migration found | VF assignment; migration shown as unavailable |

The UI shows per GPU: which modes are available (passthrough, SR-IOV VFs,
vendor vGPU, Horcrux vGPU), why others aren't, and per VM which save method
applies. SR-IOV VFs and Horcrux vGPU combine: a VF is just another Vulkan
device for the pool.

## 9. Security

- One `horcrux-gpud` per VM, own uid, seccomp, no network unless remote mode,
  only its GPU's render node; a crash or GPU reset affects that VM only.
- Guest isolation relies on the host driver's per-context GPU address spaces
  (as for any multi-process desktop). A host GPU driver bug is shared kernel
  surface: weaker isolation than passthrough or SR-IOV; the UI says so.
  Robust buffer access (`robustness2`) is forced on every guest device.
- Remote mode: mutually authenticated TLS between nodes, per-VM stream keys,
  audited start/stop/move.
- Confidential VMs (SEV-SNP/TDX) can't use a host-owned GPU without
  TEE-IO; a protected gaming VM keeps passthrough.

## 10. Plan

| Phase | Scope | Exit criterion |
|---|---|---|
| **A (now)** | Hardware slicing: Intel xe SR-IOV VFs (provisioning, assignment, VFIO migration capability), NVIDIA vGPU with user licence, AMD VFs; per-GPU capability view; save method per VM shown | a VF assigned to a Linux and a Windows guest on supported Intel hardware; save/migrate of a VF VM where xe-vfio-pci reports migration support |
| **B1** (done on Intel; AMD/NVIDIA unproven) | Linux guests on 3D virtio-gpu (Venus + virgl) as a supported VM option: render node selection, `hostmem`, memfd backend, QEMU blockers surfaced; save order resolves to shutdown with the reason | Venus guest created from the UI/API on Intel, AMD and NVIDIA hosts; vkQuake/vkmark fps recorded per host; OpenRC and systemd. **Status 2026-11-19:** shipped as `video: virtio-3d` + `display.gpu3d` (docs/GPU_3D_VIRTIO.md). Proven on an Intel Iris Plus (Ice Lake) lab laptop through the API; AMD and NVIDIA hosts: no hardware, listed as unproven (see B1 result) |
| **B2** | `horcrux-gpud` as vhost-user-gpu back end (Venus decoder on virglrenderer as a library first), per-VM process, VRAM budget, submission weights, pool placement, feature levels | two VMs on one GPU with enforced VRAM budgets and a measured fairness split; same guest image on Intel, AMD, NVIDIA |
| **B3** | State ledger + save/restore through the migration stream; same-GPU first, then cross-GPU, then cross-node live migration | a running Vulkan game saved and resumed (same GPU), moved iGPU <-> dGPU, live-migrated between nodes; downtime and fps recorded |
| **B4** | Remote GPU over the cluster network + Console encode on the GPU node | a desktop/CAD workload and an RPG-class game playable from node A on node B's GPU at measured latency |
| **C** | Windows guest driver stack | gates in 7.4 |

### B1 result (2026-11-19)

Real-effect test, Intel Iris Plus Graphics (ICL GT2) lab laptop (Horcrux dev manager built from this branch,
run in an Arch container with QEMU 11.1.3, virglrenderer 1.3.0, Mesa 26.3.1; guest Arch, kernel 7.3.4,
Mesa 26.3.1). A VM was created through the API (`PUT /api/vms/:id/display` with `video: virtio-3d`),
started, and driven over its serial console:

| check | result |
|---|---|
| `GET /api/gpu/3d` | ready; QEMU 11.1.3 with `virtio-vga-gl`, `egl-headless`, `venus`; GPU `/dev/dri/renderD128` i915, Vulkan driver exports dma-bufs |
| Venus guest | `vulkaninfo`: "Virtio-GPU Venus (Intel(R) Iris(R) Plus Graphics (ICL GT2))", Vulkan 1.4.334; dmesg `+virgl +resource_blob +host_visible`, host memory window 2 GiB |
| vkmark 1920x1080 headless | guest score 2698-2702 (3431 fps vertex, 4190 fps clear) vs host 5811 on the same GPU = 46 % (vkmark is a per-frame-overhead micro-benchmark; vkQuake timedemo in the spike measured 83 % of host fps) |
| virgl-only guest (`venus: false`) | OpenGL 4.3 core, renderer "virgl (Mesa Intel(R) Iris(R) Plus Graphics (ICL GT2))"; no Vulkan device (correct) |
| VNC console with egl-headless | console endpoint answers (`POST /api/console/:id/vnc` 200) |
| save | `POST /api/vms/:id/save-state` 409 and the power overview `can_save=false` with "QEMU cannot save 3D virtio-gpu state; guest hibernation is refused by the guest driver; will shut down cleanly" |
| boot to login | 20 s |

QEMU 11 has no `virgl` on/off switch on `virtio-vga-gl`: virgl stays on (the host check records
`virgl_switch=false` and the VM's effective runtime shows virgl on). Image packages: QEMU
`opengl virgl`, virglrenderer `venus`, libglvnd `X`, Mesa `vulkan` are in the image, role and
`horcrux-bin` package sets (stable Gentoo resolves qemu 10.2.3 + virglrenderer 1.2.0, no keyword changes;
the headless core gate allow-lists the GL/Vulkan client libraries this adds).

Unproven: AMD (radeonsi/RADV) and NVIDIA (proprietary or NVK) hosts; multi-GPU hosts (render-node
selection is unit-tested only); OpenRC vs systemd on a real image (the change is package-set only, no
service involved); Windows guests (phase C); vkQuake fps on this laptop (vkmark only).

## 11. Settings (all configurable, declarative, with defaults)

`[vgpu]` in the declarative config / API: `enabled` (false), `default_feature_level`
(auto: highest common), `placement` (spread | pack | pinned), `vram_limit_mib`
per VM (none = share), `vram_overcommit` (false), `weight` per VM (100),
`priority` (normal | interactive), `gpu_quiesce_timeout_s` (10),
`hostmem_mib` (2048), `remote_transport` (quic | rdma), `remote_max_rtt_us`
(200, refuse remote placement above it), `expose_host_identity` (false),
`pipeline_cache_dir`, `render_nodes` (allow-list, empty = all). B1 implements `hostmem_mib` and `render_nodes` (plus `dri_dir`, `drm_sysfs_dir`, `ignore_drivers`, `vulkaninfo`, `probe_timeout_secs`, `probe_cache_secs`) in `[vgpu]`. No GPU, node
or path is hard-coded.

## 12. Reproducing the spike

`hx-vgpu-spike/` contains the throwaway scripts: an Arch container with QEMU,
virglrenderer and Mesa (`Dockerfile`), the guest launcher (`run-vm.sh`: venus,
venus-lvp, 2d), the benchmarks (`bench.sh` vkmark, `quake.sh` vkQuake timedemo
with the shareware data and a focus shim, needed because headless compositors
never give a window input focus and vkQuake then sleeps 16 ms per frame), the
save tests (`save-test.sh`, `s4-test.sh`), the host-side capture/replay
(`capture.sh`, `apistream.sh`; GFXReconstruct built from its
`vulkan-sdk-1.4.363.0` tag) and the stream-volume trace (`trace-bw.sh`). They
need a Linux host with KVM and a GPU render node; never run them on a machine
whose GPU is someone's desktop.
