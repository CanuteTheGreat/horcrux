# GPU Passthrough Guide

Horcrux provides comprehensive GPU passthrough support including PCI passthrough, NVIDIA vGPU, AMD MxGPU, and Intel GVT-g.

## Table of Contents

- [Overview](#overview)
- [Prerequisites](#prerequisites)
- [PCI Passthrough](#pci-passthrough)
- [vGPU / mediated devices](#vgpu--mediated-devices-nvidia-vgpu-intel-gvt-g-amd-mxgpu)
- [API Reference](#api-reference)
- [Troubleshooting](#troubleshooting)

## Overview

GPU passthrough allows VMs to have direct access to physical GPU hardware for:
- High-performance graphics workloads
- Machine learning and AI training
- Video encoding/transcoding
- Gaming and VDI
- CAD/CAM applications

Horcrux supports multiple GPU virtualization technologies:

| Technology | Vendor | VMs per GPU | Live Migration |
|------------|--------|-------------|----------------|
| PCI Passthrough | All | 1 | No |
| NVIDIA vGPU (mdev) | NVIDIA | Multiple | No |
| AMD MxGPU (mdev) | AMD | Multiple | No |
| Intel GVT-g (mdev) | Intel | Up to 7 | No |

## Prerequisites

### Hardware Requirements

1. **CPU with IOMMU support**:
   - Intel: VT-d (Virtualization Technology for Directed I/O)
   - AMD: AMD-Vi (IOMMU)

2. **GPU Requirements**:
   - PCI Passthrough: Any PCIe GPU
   - NVIDIA vGPU: NVIDIA GRID/Tesla GPU with vGPU license
   - AMD MxGPU: AMD SR-IOV capable GPU
   - Intel GVT-g: 5th generation (Broadwell) or newer Intel integrated GPU

### BIOS/UEFI Configuration

Enable the following in BIOS:
- Intel VT-d or AMD-Vi (IOMMU)
- Above 4G Decoding (for GPUs >4GB VRAM)
- SR-IOV (for MxGPU)

### Kernel Parameters

Add to `/etc/default/grub`:

```bash
# For Intel
GRUB_CMDLINE_LINUX_DEFAULT="intel_iommu=on iommu=pt"

# For AMD
GRUB_CMDLINE_LINUX_DEFAULT="amd_iommu=on iommu=pt"
```

Update GRUB and reboot:

```bash
sudo grub-mkconfig -o /boot/grub/grub.cfg
sudo reboot
```

### VFIO Modules

Ensure VFIO modules are loaded:

```bash
# Add to /etc/modules-load.d/vfio.conf
vfio
vfio_iommu_type1
vfio_pci
vfio_virqfd
```

## PCI Passthrough

### 1. Discover Available GPUs

```bash
# Via CLI
horcrux gpu list

# Via API
curl http://localhost:8006/api/gpu/devices
```

Response:
```json
[
  {
    "pci_address": "0000:01:00.0",
    "vendor_id": "10de",
    "device_id": "1b80",
    "vendor_name": "NVIDIA Corporation",
    "device_name": "GP104 [GeForce GTX 1080]",
    "driver": "nvidia",
    "iommu_group": "1",
    "in_use": false
  }
]
```

### 2. Check IOMMU Status

```bash
# Via API
curl http://localhost:8006/api/gpu/iommu-status
```

Response:
```json
{
  "enabled": true,
  "message": "IOMMU is enabled and ready for GPU passthrough"
}
```

### 3. Bind GPU to VFIO Driver

```bash
# Via API
curl -X POST http://localhost:8006/api/gpu/devices/0000:01:00.0/bind-vfio
```

### 4. Give the device to a VM

The device goes into the VM's hardware (`hostpci`), while the VM is stopped
or when it is created. A PCI address needs the device bound to `vfio-pci`
(step 3) and an IOMMU group; Horcrux checks both when you save and again when
the VM starts, and says which one is missing.

```bash
curl -X PUT http://localhost:8006/api/vms/vm-100/config \
  -H "Content-Type: application/json" \
  -d '{"hardware": {"hostpci": [
        {"device": "0000:01:00.0", "primary_gpu": true},
        {"device": "0000:01:00.1"}]}}'
```

QEMU gets `-device vfio-pci,host=0000:01:00.0,id=hostpci0,x-vga=on`. Pass the
GPU's audio function (and every other device of its IOMMU group, step 5) too.
At most 8 devices per VM, at most one `primary_gpu`; clones don't inherit
devices (a device belongs to one guest). A VM with passthrough devices can't
be live-migrated.

### 5. Check IOMMU Group Devices

Some GPUs have multiple functions (GPU, Audio, USB) in the same IOMMU group. All devices must be passed through together:

```bash
curl http://localhost:8006/api/gpu/devices/0000:01:00.0/iommu-group
```

Response:
```json
[
  {
    "pci_address": "0000:01:00.0",
    "device_name": "GP104 [GeForce GTX 1080]"
  },
  {
    "pci_address": "0000:01:00.1",
    "device_name": "GP104 High Definition Audio Controller"
  }
]
```

## vGPU / mediated devices (NVIDIA vGPU, Intel GVT-g, AMD MxGPU)

These all work through the kernel's mediated device (mdev) framework: the host
driver exposes **types** (profiles) per GPU, you create an mdev of a type, and
a VM uses it like a PCI device. Horcrux reads and writes
`/sys/class/mdev_bus` and `/sys/bus/mdev/devices` directly; the vendor driver
(NVIDIA vGPU manager with its licence, `i915` with `kvmgt`, AMD's GIM) must
already be installed and loaded - Horcrux does not install drivers.

```bash
# what the host can create (parent GPU, type id, name, free instances)
curl http://localhost:8006/api/gpu/mdev/types

# create one (uuid is generated unless you pass "uuid")
curl -X POST http://localhost:8006/api/gpu/mdev/devices \
  -H "Content-Type: application/json" \
  -d '{"parent": "0000:00:02.0", "type": "i915-GVTg_V5_4"}'
# -> {"uuid": "6b1f3a6e-...", "parent": "0000:00:02.0", "mdev_type": "i915-GVTg_V5_4"}

# give it to a VM (hostpci accepts the mdev UUID)
curl -X PUT http://localhost:8006/api/vms/vm-100/config \
  -H "Content-Type: application/json" \
  -d '{"hardware": {"hostpci": [{"device": "6b1f3a6e-5d0b-4b5f-9f61-0123456789ab"}]}}'

# list (shows the VM using each one) and remove
curl http://localhost:8006/api/gpu/mdev/devices
curl -X DELETE http://localhost:8006/api/gpu/mdev/devices/6b1f3a6e-5d0b-4b5f-9f61-0123456789ab
```

QEMU gets `-device vfio-pci,sysfsdev=/sys/bus/mdev/devices/<uuid>,id=hostpciN`.
Creation is refused with a reason when the type or parent doesn't exist (404),
no instance is left (409) or the UUID exists; removal is refused (409) while a
VM lists the device. mdev devices don't survive a host reboot: create them
again before starting the VM (a VM start fails with "mediated device ... does
not exist" otherwise).

Tested: the sysfs handling and the QEMU arguments against fixture trees in CI,
and the HTTP contract (validation, refusals) against a real server. Not tested
in CI: a real vGPU/GVT-g guest, which needs the hardware.

## API Reference

### GPU Device Endpoints
- `GET /api/gpu/devices`, `POST /api/gpu/devices/scan`, `GET /api/gpu/devices/:pci_address`
- `POST /api/gpu/devices/:pci_address/bind-vfio`, `.../unbind-vfio`, `GET .../iommu-group`
- `GET /api/gpu/iommu-status`

### Mediated device (vGPU) Endpoints
- `GET /api/gpu/mdev/types`
- `GET /api/gpu/mdev/devices`, `POST /api/gpu/mdev/devices`, `DELETE /api/gpu/mdev/devices/:uuid`

### VM hardware
`PUT /api/vms/:id/config` with `hardware.hostpci: [{"device": "<pci address or mdev uuid>", "primary_gpu": false}]`.

## Troubleshooting

### IOMMU Not Enabled

**Symptom**: `/api/gpu/iommu-status` returns `enabled: false`

**Solution**:
1. Enable VT-d/AMD-Vi in BIOS
2. Add `intel_iommu=on` or `amd_iommu=on` to kernel parameters
3. Reboot

### GPU Not in Separate IOMMU Group

**Symptom**: GPU shares IOMMU group with other devices

**Solution**:
1. Use ACS override patch (not recommended for production)
2. Use a different PCIe slot
3. Check motherboard documentation for isolated slots

### vfio-pci Binding Fails

**Symptom**: Error when binding to vfio-pci

**Solution**:
```bash
# Check if vfio modules are loaded
lsmod | grep vfio

# Load modules
modprobe vfio-pci

# Check for conflicting drivers
dmesg | grep vfio
```

### Guest Display Not Working

**Symptom**: VM boots but no display output

**Solution**:
1. Use GPU VBIOS ROM file (extract with GPU-Z on Windows)
2. Ensure `primary_gpu: true` is set
3. Add `vendor-reset` module for AMD GPUs:
   ```bash
   modprobe vendor-reset
   ```

### Code 43 Error (NVIDIA)

**Symptom**: NVIDIA driver shows error 43 in guest

**Solution**:
1. Add `hidden=on` to hypervisor (done automatically)
2. Use patched VBIOS
3. Disable hypervisor CPUID reporting:
   ```xml
   <hyperv>
     <vendor_id state='on' value='randomid'/>
   </hyperv>
   <kvm>
     <hidden state='on'/>
   </kvm>
   ```

### vGPU Instance Creation Fails

**Symptom**: Cannot create vGPU instance

**Solution**:
1. Check vGPU manager is installed: `nvidia-smi vgpu`
2. Verify license: `nvidia-smi -q | grep -i license`
3. Check max instances not exceeded
4. Ensure no conflicting vGPU types

## Performance Tips

1. **CPU Pinning**: Pin VM vCPUs to physical cores
2. **NUMA Awareness**: Place VM on same NUMA node as GPU
3. **Hugepages**: Enable 1GB hugepages for GPU memory
4. **PCIe ACS**: Ensure proper PCIe ACS for isolation
5. **IOThreads**: Use dedicated IOThreads for disk I/O

## Security Considerations

1. **IOMMU Isolation**: Ensure proper IOMMU grouping
2. **DMA Attacks**: GPU has DMA access - trust VM guests
3. **vGPU Security**: Each vGPU instance is isolated
4. **Driver Updates**: Keep GPU drivers updated
5. **Reset Issues**: Some GPUs require host reboot after VM shutdown
