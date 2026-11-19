# Default CPU model of new VMs

QEMU starts an x86 VM that is given no `-cpu` argument on its bare `qemu64`
model. `qemu64` has no SSE4.2/POPCNT: Windows 11 24H2 hangs forever at the boot
logo on it, and Linux distributions built for the x86-64-v2 baseline do not
start. So Horcrux never leaves the CPU type open on a new x86 VM.

## What happens

- A new x86 QEMU VM created without `hardware.cpu_type` (API, UI, declarative
  spec, libvirt import) gets the node's **default CPU model** written
  into its own configuration at create time. The default is `x86-64-v2-AES`
  (what Proxmox VE 8 uses): every KVM host of the last decade can run it and it
  live-migrates between different CPUs.
- The model is stored in the VM. Changing the default later never changes a VM
  that exists, running or not.
- An explicit `cpu_type` (`host`, `x86-64-v3`, `EPYC-v4`, `qemu64`, ...) is kept
  as given. Non-x86 VMs and LXD/Incus VMs are not touched. VMs with Hyper-V
  enlightenments (`hardware.adv.hyperv`) keep using
  `[passthrough] hyperv_cpu_model` (default `host`) when they name no model.
- The Create VM form preselects the default (`GET /api/vms/host-options` ->
  `default_cpu_model`).

## Existing VMs without a CPU type

VMs created by a release before this default existed keep `cpu_type = null` and
keep their exact old command line (no `-cpu`, so `qemu64`): a CPU model changes
what the guest sees, and silently changing it under a running Windows or
licensed guest is the wrong default. Their hardware page shows a warning and a
one-click **Switch to x86-64-v2-AES** button (the VM must be stopped; it applies
at the next start). The CPU type select there labels the empty choice "None
(QEMU's bare qemu64 ...)".

## Setting the default

Highest priority first:

1. Run-time setting: Settings > Configuration > "Defaults for new VMs",
   `PUT /api/vms/defaults {"cpu_model": "x86-64-v3"}`, or the `vm_defaults`
   section of a declarative spec (`plan` / `apply` / `export`). Stored in the
   node's database, survives restarts. An empty value clears it.
2. `[vm_defaults] cpu_model = "..."` in `/etc/horcrux/config.toml`, or the
   environment variable `HORCRUX_VM_DEFAULT_CPU_MODEL`.
3. Built-in `x86-64-v2-AES`.

The setting is per node (a cluster applies the same spec to each node). The API
refuses a model this node's `qemu-system-x86_64 -cpu help` does not list, since
a typo would stop every new VM from starting.
