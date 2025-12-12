# GPU Passthrough Guide

Horcrux provides comprehensive GPU passthrough support including PCI passthrough, NVIDIA vGPU, AMD MxGPU, and Intel GVT-g.

## Table of Contents

- [Overview](#overview)
- [Prerequisites](#prerequisites)
- [PCI Passthrough](#pci-passthrough)
- [NVIDIA vGPU](#nvidia-vgpu)
- [AMD MxGPU](#amd-mxgpu)
- [Intel GVT-g](#intel-gvt-g)
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
| NVIDIA vGPU | NVIDIA | Multiple | Yes |
| AMD MxGPU | AMD | Multiple | Limited |
| Intel GVT-g | Intel | Up to 7 | No |

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

### 4. Create VM with GPU Passthrough

```bash
# Via API
curl -X POST http://localhost:8006/api/vms \
  -H "Content-Type: application/json" \
  -d '{
    "id": "vm-100",
    "name": "gpu-workstation",
    "cpus": 8,
    "memory": 16384,
    "disk_size": 100,
    "gpu_passthrough": {
      "pci_address": "0000:01:00.0",
      "rom_file": "/usr/share/vgabios/nvidia.rom",
      "multifunction": true,
      "primary_gpu": true
    }
  }'
```

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

## NVIDIA vGPU

NVIDIA vGPU allows multiple VMs to share a single GPU with dedicated framebuffer.

### Prerequisites

- NVIDIA GRID/Tesla GPU (A100, A30, A40, T4, etc.)
- NVIDIA vGPU software license
- NVIDIA vGPU Manager installed on host

### Setup

1. **Install NVIDIA vGPU Manager**:

```bash
# Download from NVIDIA Licensing Portal
chmod +x NVIDIA-vGPU-Linux-*.run
./NVIDIA-vGPU-Linux-*.run --dkms
```

2. **List vGPU Profiles**:

```bash
# Via API
curl http://localhost:8006/api/vgpu/devices
```

Response:
```json
[
  {
    "pci_id": "0000:01:00.0",
    "vendor": "NVIDIA",
    "device_name": "Tesla A100",
    "vgpu_type": "nvidia",
    "available_profiles": [
      {
        "name": "nvidia-256",
        "vgpu_type": "GRID A100-4C",
        "framebuffer_mb": 4096,
        "max_instances": 16,
        "description": "4GB framebuffer, compute workloads"
      },
      {
        "name": "nvidia-512",
        "vgpu_type": "GRID A100-8C",
        "framebuffer_mb": 8192,
        "max_instances": 8,
        "description": "8GB framebuffer, graphics workloads"
      }
    ]
  }
]
```

3. **Create VM with vGPU**:

```bash
curl -X POST http://localhost:8006/api/vms \
  -H "Content-Type: application/json" \
  -d '{
    "id": "vm-101",
    "name": "vgpu-workstation",
    "cpus": 4,
    "memory": 8192,
    "disk_size": 50,
    "vgpu": {
      "enabled": true,
      "vgpu_type": "nvidia",
      "device_id": "0000:01:00.0",
      "profile": "nvidia-256",
      "migration_enabled": true
    }
  }'
```

### Live Migration with vGPU

NVIDIA vGPU supports live migration:

```bash
curl -X POST http://localhost:8006/api/vms/vm-101/migrate \
  -H "Content-Type: application/json" \
  -d '{
    "target_node": "node2",
    "live": true
  }'
```

## AMD MxGPU

AMD MxGPU uses SR-IOV to partition supported GPUs.

### Prerequisites

- AMD FirePro S7150 series or newer with MxGPU support
- SR-IOV enabled in BIOS

### Setup

1. **Enable SR-IOV**:

```bash
# Check SR-IOV support
lspci -vvv -s 0000:03:00.0 | grep -i sr-iov

# Enable virtual functions
echo 4 > /sys/bus/pci/devices/0000:03:00.0/sriov_numvfs
```

2. **Create VM with MxGPU**:

```bash
curl -X POST http://localhost:8006/api/vms \
  -H "Content-Type: application/json" \
  -d '{
    "id": "vm-102",
    "name": "mxgpu-workstation",
    "cpus": 4,
    "memory": 8192,
    "disk_size": 50,
    "vgpu": {
      "enabled": true,
      "vgpu_type": "amd",
      "device_id": "0000:03:00.1"
    }
  }'
```

## Intel GVT-g

Intel GVT-g allows sharing integrated Intel GPUs.

### Prerequisites

- Intel 5th generation (Broadwell) or newer CPU with integrated graphics
- Kernel 4.14 or newer with GVT-g support

### Setup

1. **Enable GVT-g**:

Add to kernel parameters:
```
i915.enable_gvt=1
```

2. **Create vGPU**:

```bash
# List available profiles
ls /sys/class/mdev_bus/0000:00:02.0/mdev_supported_types/

# Available profiles:
# - i915-GVTg_V5_4 (low memory)
# - i915-GVTg_V5_8 (high memory)
```

3. **Create VM with GVT-g**:

```bash
curl -X POST http://localhost:8006/api/vms \
  -H "Content-Type: application/json" \
  -d '{
    "id": "vm-103",
    "name": "gvtg-workstation",
    "cpus": 2,
    "memory": 4096,
    "disk_size": 30,
    "vgpu": {
      "enabled": true,
      "vgpu_type": "intel",
      "device_id": "0000:00:02.0",
      "profile": "i915-GVTg_V5_8"
    }
  }'
```

## API Reference

### GPU Device Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/gpu/devices` | GET | List all GPU devices |
| `/api/gpu/devices/scan` | POST | Rescan for GPU devices |
| `/api/gpu/devices/:pci` | GET | Get specific GPU device |
| `/api/gpu/devices/:pci/bind-vfio` | POST | Bind GPU to vfio-pci |
| `/api/gpu/devices/:pci/unbind-vfio` | POST | Unbind GPU from vfio-pci |
| `/api/gpu/devices/:pci/iommu-group` | GET | Get IOMMU group devices |
| `/api/gpu/iommu-status` | GET | Check IOMMU status |

### vGPU Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/vgpu/devices` | GET | List vGPU-capable devices |
| `/api/vgpu/profiles` | GET | List available vGPU profiles |
| `/api/vms/:id/vgpu` | POST | Attach vGPU to VM |
| `/api/vms/:id/vgpu` | DELETE | Detach vGPU from VM |

### GPU Passthrough Config Structure

```json
{
  "pci_address": "0000:01:00.0",
  "rom_file": "/path/to/vbios.rom",  // Optional: GPU VBIOS ROM
  "multifunction": true,              // Pass all functions in IOMMU group
  "primary_gpu": true                 // Make this the primary display
}
```

### vGPU Config Structure

```json
{
  "enabled": true,
  "vgpu_type": "nvidia",              // nvidia, amd, intel, passthrough
  "device_id": "0000:01:00.0",        // PCI device ID
  "profile": "nvidia-256",            // vGPU profile name
  "migration_enabled": true           // Enable live migration (NVIDIA only)
}
```

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
