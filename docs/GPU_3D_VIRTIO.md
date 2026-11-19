# 3D virtio-gpu for Linux guests (Venus Vulkan + virgl OpenGL)

A VM whose video card is **virtio-gpu 3D** (`display.video = "virtio-3d"`) draws on the host's
GPU: the guest sees a virtio-gpu, its Vulkan applications run through Venus and its OpenGL
applications through virgl, and the host's own Mesa/vendor drivers execute the work. No
passthrough, no dedicated GPU: any number of such VMs share the host GPU, and the host keeps
its display. Design and phases: `docs/design/hx-vgpu.md` (phase B1).

## What the guest needs

A Linux guest with the in-kernel `virtio_gpu` driver and Mesa's Venus driver
(Arch/Fedora/Debian: `vulkan-virtio` / `mesa-vulkan-drivers`; OpenGL comes with Mesa). Shown
through the Horcrux console (VNC). x86_64 guests only; SPICE is not offered with it.

## What the host needs

`GET /api/gpu/3d` (and the Create VM form, when the card is picked) checks the host and says
what is missing and what to do about it:

| reason code | meaning | fix |
|---|---|---|
| `qemu-missing` | no `qemu-system-x86_64` | install QEMU |
| `qemu-no-3d-device` | QEMU has no `virtio-vga-gl` | QEMU with USE `opengl virgl` (Horcrux images and the `horcrux-bin` dependencies set it) |
| `qemu-no-egl-headless` | no `egl-headless` display | QEMU with USE `opengl` |
| `qemu-no-venus` | the device lacks `venus`/`blob`/`hostmem` (Vulkan unavailable; OpenGL-only guests still work) | QEMU >= 9.2 with virglrenderer USE `venus` |
| `no-render-node` | no `/dev/dri/renderD*` | a GPU with a kernel DRM driver |
| `no-suitable-gpu` | every GPU is software, excluded, or its Vulkan driver cannot export dma-bufs (`VK_EXT_external_memory_dma_buf`) | install the GPU's Mesa/vendor Vulkan driver; software Vulkan (lavapipe) cannot serve Venus |

Each GPU is listed with its Vulkan name, kernel driver, whether its driver exports dma-bufs
(read from `vulkaninfo`; "not verified" when `vulkaninfo` is missing) and, when unusable, why.

## VM settings (`display.gpu3d`)

| key | default | meaning |
|---|---|---|
| `venus` | true | Vulkan in the guest |
| `virgl` | true | OpenGL in the guest (a QEMU without the `virgl` switch always offers it) |
| `hostmem_mib` | node `[vgpu] hostmem_mib` (2048) | host-visible memory window Venus maps (64..65536) |
| `render_node` | first suitable GPU | `/dev/dri/renderD128` or the GPU's PCI address (`0000:00:02.0`) |

The VM's RAM becomes a shareable memfd (`-machine ...,memory-backend=hx3dmem`), the display
`-display egl-headless,rendernode=<node>`, the card `-device virtio-vga-gl,hostmem=..,blob=true,venus=true`.
`MESA_VK_DEVICE_SELECT=<vendor>:<device>` steers Venus to the chosen GPU. Not combinable with NUMA
nodes, huge pages or memory hot-plug (refused with the reason).

## Node settings (`[vgpu]` in `/etc/horcrux/config.toml`)

`hostmem_mib` (2048), `render_nodes` (allow-list of paths or PCI addresses, empty = all),
`dri_dir` (`/dev/dri`), `drm_sysfs_dir` (`/sys/class/drm`), `ignore_drivers`
(`vgem vkms virtio_gpu udl evdi`), `vulkaninfo` (program name), `probe_timeout_secs` (20),
`probe_cache_secs` (30).

## Save, hibernate, migrate

Not possible today, and shown that way: QEMU refuses to migrate or save a VM with a 3D
virtio-gpu ("virgl is not yet migratable") and the Linux guest driver refuses hibernation
("S4 not allowed when VIRGL is enabled"). The VM list and the Power page show
"QEMU cannot save 3D virtio-gpu state; guest hibernation is refused by the guest driver; will
shut down cleanly"; when the host stops, the guest is shut down cleanly (not saved), live
migration is refused with the same reason, and drain/rebalance report the VM as not movable.
Closing this gap is phase B3 of the design (host-side state capture).

## Measured

Intel Iris Plus (lab laptop): Venus guest sees "Virtio-GPU Venus (Intel ... Iris Plus ...)", Vulkan 1.4;
vkmark 1080p headless 46 % of the host score; virgl-only guest gets OpenGL 4.3. AMD and NVIDIA hosts are
unproven. Details: "B1 result" in `docs/design/hx-vgpu.md`.
