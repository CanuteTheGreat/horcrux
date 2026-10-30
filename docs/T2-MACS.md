# Apple T2 Macs on Horcrux

Horcrux supports Macs with the Apple T2 chip (MacBookPro15,x/16,x, MacBookAir8,x/9,1, Macmini8,1, iMac19,x/20,x, iMacPro1,1, MacPro7,1) as a desktop/laptop. Detection is by PCI id `106b:1801` (T2 Bridge Controller), not by model.

## Kernel version pin (finding, 2026-10-28)

* t2linux's patch set (`github.com/t2linux/linux-t2-patches`) tracks the newest mainline: `main` = **7.3**.
* OpenZFS 2.4.4 (newest release; 2.3.9 likewise) declares `Linux-Maximum: 7.2`. Only master (2.4.99) lists 7.3. Root on native-encrypted ZFS therefore needs a kernel **<= 7.2**.
* The Horcrux image kernel is **6.18 LTS** (`gentoo-kernel-bin` 6.18.57 on the 2026-10-28 ISO). Use the same series for the T2 kernel: vanilla 6.18.x + the t2linux set of the time (`linux-t2-patches` commit `4cd1efe`, "6.18.7"). Newer ZFS-capable alternative, verified 2026-10-28: vanilla **7.2.13 + commit `76d5932` ("7.2.10")**: all 23 patches apply with `patch -Np1`, no rejects. The `main`/7.3 set applies to 7.3 only, but 7.3 cannot carry a released ZFS yet; revisit when OpenZFS ships `Linux-Maximum: 7.3`.
* The kernel package must also build `zfs.ko` for the T2 kernel (the live installer creates the pool with the running kernel, and the installed initramfs imports it): build `sys-fs/zfs-kmod` (or OpenZFS 2.4.4 from source) against the T2 kernel tree in the same image stage.
* Pitfall: `patch --dry-run` per file reports false failures for multi-diff patches (4001/4003/4004 touch the same file twice). Apply for real, in order, with `-Np1`; skip the APFS patches (8001/8002) and `7003-amdgpu-add-support-for-5K` (does not apply to plain 7.3).

## Boot path and the keyboard (critical)

The internal keyboard/trackpad are USB devices behind the T2: they exist only after the **apple-bce** stack (`t2bce_core`, `t2bce_dma`, `t2bce_vhci`) is loaded, plus `hid_apple`/`hid_appletb_*`. Every initramfs that prompts for a passphrase must carry them and load them first (`force_drivers`/`rd.driver.pre`). Statically verified by `liveinst.t2.initramfs_check()` (lsinitrd) in the installer and in CI.

Required kernel cmdline: `pm_async=off` (suspend). Also used on the reference machine: `intel_iommu=on iommu=pt`.

## Apple firmware

Never in the ISO. The installer offers import from an existing install (`/lib/firmware/brcm/*apple*`) or from a t2linux `firmware.tar`.

## What ships (installer + images)

* `VARIANT=t2 build.sh` (or `VARIANT=gnome-light,t2`) adds the T2 kernel to the finished image (`project/variants/t2/Dockerfile`, reusing `gentoo/t2/kernel/*`): gentoo-sources 7.2.13 + pinned t2linux set, `zfs.ko` 2.3.9 built for it, the stock kernel removed so the live USB boots the T2 kernel, `pm_async=off intel_iommu=on iommu=pt` on the live kernel line (`/etc/live-iso-cmdline`), and an initramfs with apple-bce + HID loaded early. **The image build fails** if `lsinitrd` does not list `t2bce_core`, `t2bce_dma`, `t2bce_vhci` and `hid-apple` (`liveinst.t2.initramfs_check`); the listing is kept in the image as `/etc/live-iso-t2-initramfs.lst`.
* The installer detects PCI `106b:1801` (`liveinst/t2.py`), refuses before touching the disk when the image has no T2 kernel, rebuilds the target initramfs with `/etc/dracut.conf.d/90-horcrux-t2.conf`, re-checks it, adds `pm_async=off intel_iommu=on iommu=pt` to the boot-environment kernel line, enables `t2fanrd` and `tiny-dfr` (OpenRC), installs the t2linux audio profile sets and imports Wi-Fi/Bluetooth firmware from an existing install (`/lib/firmware/brcm/*apple*`) or a `firmware.tar` (path/size/symlink filtered). `horcrux-t2-firmware PATH` does the same on a running system.
* Apple firmware is never in the public image (`liveinst.t2.iso_ships_apple_firmware`, asserted by the QEMU test). The owner's private live ISO (`gentoo/t2/live`) is the only image that carries it.
* Not emulatable in QEMU: the T2 hardware itself. The QEMU tests prove the installer path (forced with answer-file `t2 = "yes"`), the initramfs gate, ZFS on the T2 kernel, and typing the passphrase on a USB-only keyboard.

## Hibernate (encrypted ZFS)

A T2 laptop install gets the LUKS2 hibernation swap (see docs/ISO-INSTALL.md "Hibernate on encrypted ZFS"): one passphrase prompt on the
internal keyboard (apple-bce loads first), resume before the pool import. Tested in QEMU only. Not tested on real T2 hardware:
whether the T2/NVMe/GPU come back after resume, `pm_async=off` interplay, Touch Bar after resume. Build variant `desktop,t2`
(`VARIANT=gnome-light,t2 build.sh`) carries the T2 kernel; the public daily GNOME (light) image has no T2 kernel and the installer refuses an
ENCRYPTED install on a T2 Mac with it (the keyboard would be dead at the prompt).
