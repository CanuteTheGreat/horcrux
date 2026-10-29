# Apple T2 Macs on Horcrux

Horcrux supports Macs with the Apple T2 chip (MacBookPro15,x/16,x, MacBookAir8,x/9,1, Macmini8,1, iMac19,x/20,x, iMacPro1,1, MacPro7,1) as a desktop/laptop. Detection is by PCI id `106b:1801` (T2 Bridge Controller), not by model.

## Kernel version pin (finding, 2026-10-28)

* t2linux's patch set (`github.com/t2linux/linux-t2-patches`) tracks the newest mainline: `main` = **7.3**.
* OpenZFS 2.4.4 (newest release; 2.3.9 likewise) declares `Linux-Maximum: 7.2`. Only master (2.4.99) lists 7.3.
* Root on native-encrypted ZFS therefore needs a kernel **<= 7.2**. Pin: **vanilla 7.2.x (7.2.13 at the time) + the t2linux patch set at commit `76d5932` ("7.2.10")**. Verified 2026-10-28: all 23 patches in that set apply with `patch -Np1` to linux-7.2.13 without rejects (skip `8001/8002` APFS, which are not in that set anyway).
* Applying the `main` (7.3) set to 7.2 or 7.3-patches to ZFS-supported kernels is NOT possible, and building ZFS master against 7.3 means an unreleased module; revisit when OpenZFS releases 2.4.5/2.5 with `Linux-Maximum: 7.3`.
* Pitfall: `patch --dry-run` per file reports false failures for multi-diff patches (4001/4003/4004 touch the same file twice). Apply for real, in order, with `-Np1`.

## Boot path and the keyboard (critical)

The internal keyboard/trackpad are USB devices behind the T2: they exist only after the **apple-bce** stack (`t2bce_core`, `t2bce_dma`, `t2bce_vhci`) is loaded, plus `hid_apple`/`hid_appletb_*`. Every initramfs that prompts for a passphrase must carry them and load them first (`force_drivers`/`rd.driver.pre`). Statically verified by `liveinst.t2.initramfs_check()` (lsinitrd) in the installer and in CI.

Required kernel cmdline: `pm_async=off` (suspend). Also used on the reference machine: `intel_iommu=on iommu=pt`.

## Apple firmware

Never in the ISO. The installer offers import from an existing install (`/lib/firmware/brcm/*apple*`) or from a t2linux `firmware.tar`.
