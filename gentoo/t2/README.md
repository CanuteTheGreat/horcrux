# gentoo/t2 - Apple T2 Mac kernel pieces (for sa-0-9e8eefd9 and the installer agents)

Owner: the "hogshead ISO" work. Everything here is additive; nothing under `build/live-iso/common` was touched.
Hardware target: MacBookPro16,2 (Ice Lake i5-1038NG7, Iris Plus G7, T2, BCM4364 Wi-Fi/BT, Apple ANS2 NVMe, Touch Bar).

## Kernel decision (read this first)
| item | value |
|---|---|
| kernel | `sys-kernel/gentoo-sources-7.2.13` (Gentoo tree, `~amd64`), built from source, `LOCALVERSION=-t2-horcrux` |
| patch set | `t2linux/linux-t2-patches` commit **`e167207`** (`PATCHSET`; its `version` file says KVER=7.2.10), 23 patches, **APFS (8001/8002) skipped** |
| why not 7.3 | OpenZFS 2.4.4 / 2.3.9 declare `Linux-Maximum: 7.2`; only unreleased master (2.4.99) lists 7.3. Root-on-ZFS needs <= 7.2 |
| ZFS | `sys-fs/zfs` + `zfs-kmod` **2.3.9** (Linux-Maximum 7.2) built out of tree against `/usr/src/linux` (`USE=-dist-kernel rootfs`). The Gentoo tree has `zfs-2.4.4` but NO `zfs-kmod-2.4.x` yet, so userland and module are both 2.3.9 (same as the existing Horcrux `>=2.4` mask) |
| base config | Gentoo's own dist-kernel config (taken from `gentoo-kernel-bin`), `olddefconfig`, then `kernel/t2.config`, then the patch set's `extra_config` |
| checked | `kernel/required-config.txt` is asserted against the final `.config` (build fails if an option is missing) |
| verified | all 23 patches apply to gentoo-sources-7.2.13 with no fuzz and no rejects (same commit applies to vanilla 7.2.13 per docs/T2-MACS.md) |

This matches the pin in `docs/T2-MACS.md` (7.2.x + t2linux 7.2.10 set); the commit is just the last one before the 7.3 rebase (newer APFS/t2bce fixes).

## What is in here
- `PATCHSET` - pin (repo, commit, kernel package, localversion, skipped patches). Source it from shell.
- `kernel/fetch-patches.sh OUTDIR` - clone the pinned set, drop skipped patches.
- `kernel/t2.config`, `kernel/required-config.txt`, `kernel/build-kernel.sh` - config fragment, assertions, the in-container build (apply patches, merge config, build, install to /boot + /lib/modules).
- `dracut/90-t2.conf` - **early** modules: `t2bce_core t2bce_dma t2bce_vhci hid_apple hid_appletb_kbd hid_appletb_bl usbhid hid_generic hid_multitouch` are `force_drivers` (loaded before udev), plus `i915 nvme thunderbolt xhci_pci usb_storage appletbdrm t2bce_audio` via `add_drivers`. Without them the internal keyboard/trackpad (USB devices behind the T2) is dead at LUKS / ZFS passphrase prompts. Copy this file into the installed system's `/etc/dracut.conf.d/` (and into the ZFSBootMenu initramfs config).
- `dracut/10-live.conf` - generic live-medium dracut config (only the live ISO needs it).
- `live/` - the standalone live ISO builder (`build.sh`, `Dockerfile`, `setup-rootfs.sh`, `assemble-hogshead-iso.sh`, `qemu-test.py`, `guest-checks.sh`, overlay with `hogshead-remote|unlock|backup|motd`).
  `build.sh -p PRIVDIR` bakes private inputs (firmware, authorized_keys, tailscale key) from outside the repo; without `-p` you get a shareable, secret-free T2 ISO.

## Boot cmdline for the laptop
`pm_async=off intel_iommu=on iommu=pt` (what the Ubuntu install runs today).

## Firmware policy
Apple Wi-Fi/BT firmware (`brcmfmac4364b3-pcie.apple,*`, `brcmbt*-apple,*`) is extracted from macOS and is NOT redistributable. It never goes in the repo or any public artifact; only the PRIVATE hogshead ISO carries it (`/lib/firmware/brcm`). Installers should import it from the old install or from the user's `firmware.tar`.

## Building the T2 kernel for the installed system
`live/Dockerfile` stages `kernel` and `rootfs` already produce `/boot/vmlinuz-<kr>`, `/lib/modules/<kr>` and the matching `zfs.ko`. To reuse them in the installer image: `COPY --from=kernel /boot /lib/modules /usr/src` as in `rootfs` there, then `emerge =sys-fs/zfs-kmod-2.3.9 =sys-fs/zfs-2.3.9` and run dracut with `dracut/*.conf` (add `zfs` module for root-on-ZFS).
