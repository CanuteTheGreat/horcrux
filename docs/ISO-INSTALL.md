# Installing Horcrux from the live ISO

Horcrux ships a Gentoo-based **live + installer ISO** (x86_64, UEFI **and** legacy BIOS, hybrid:
the same file boots from a CD/VM *and* from a USB stick). It contains Horcrux's own installer
(`horcrux-installer`), a live system you can try, and an unattended "automatic install" boot entry.

## Download

| | URL |
|---|---|
| Latest **daily** test build (rolling pre-release, replaced after the QEMU test passed) | `https://git.canutethegreat.com/CanuteTheGreat/horcrux/releases/download/daily/horcrux-daily-x86_64.iso` |
| SHA256 of the daily | `.../releases/download/daily/horcrux-daily-x86_64.iso.sha256` |
| **Stable** build (only for `v*` releases that met the criteria below) | `.../releases/download/<tag>/horcrux-<tag>-x86_64.iso` |

Always verify: `sha256sum -c horcrux-daily-x86_64.iso.sha256` (run it next to the downloaded files).

## Write it to a USB stick (this ERASES the stick)

Linux (replace `/dev/sdX` - check with `lsblk`; **not** a partition like `sdX1`):

    sudo dd if=horcrux-daily-x86_64.iso of=/dev/sdX bs=4M conv=fsync status=progress
    sync

macOS: `diskutil list`, `diskutil unmountDisk /dev/diskN`, then
`sudo dd if=horcrux-daily-x86_64.iso of=/dev/rdiskN bs=4m`.

Windows: [Rufus](https://rufus.ie) -> select the ISO -> when asked choose **"Write in DD Image mode"**
(not "ISO Image mode"), or use balenaEtcher / `dd` for Windows. Ventoy also works.

The stick is a GPT disk with an EFI System Partition plus the ISO9660 data, so it boots on UEFI
machines (via `EFI/BOOT/BOOTX64.EFI`) and on BIOS machines (GRUB in the MBR).

## Boot menu

1. *live system* - boots a RAM overlay; log in on the console as `root` / `horcrux` and run
   `horcrux-installer` for the interactive installer.
2. *live system (serial console ttyS0)* and *(safe graphics: nomodeset)*.
3. *AUTOMATIC INSTALL to first internal disk (ERASES IT)* - installs unattended to the first
   internal, non-removable disk >= 8 GiB that is not the USB stick, then powers off. To pick the
   disk yourself, press `e` on a boot entry and add `horcrux.disk=/dev/nvme0n1` (and
   `horcrux.autoinstall=1`).

After the install: remove the stick and boot. The web UI/API is at `https://<ip>:8006` (self-signed certificate; user `admin`, the generated password is in `/var/lib/horcrux/initial-admin-password`, or set `ADMIN_PASSWORD` in `/etc/conf.d/horcrux-api` before the first start). libvirt, docker and sshd start at boot.

**Change the default root password (`root` / `horcrux`) right away**; sshd allows root login with a
password on the live system and on a fresh install.

## Secure Boot

**Not supported yet - turn Secure Boot OFF in the firmware setup before booting this ISO.**
The ISO uses an unsigned GRUB and an unsigned Gentoo distribution kernel; no shim/MOK chain is
shipped. With Secure Boot on, firmware will refuse `BOOTX64.EFI` ("Secure Boot violation" /
"Access Denied"). Nothing else needs to change in the firmware. The installed system is also
unsigned, so keep Secure Boot off after installing.

## Target-hardware notes

**ThinkPad 21KH (i5-1335U, 16 GB, NVMe, Intel I219 wired, UEFI)** - expected to work, **not yet tested on the real laptop**:
firmware setup (F1) -> Security -> Secure Boot -> *Disabled*; F12 for the one-time boot menu, pick the USB stick (UEFI entry).
The kernel is Gentoo's generic distribution kernel with the full `linux-firmware` set (I219 uses the in-kernel `e1000e`;
the AX211 Wi-Fi needs `iwlwifi` firmware, included; use `iwctl`). NVMe installs as `/dev/nvme0n1`.
Hybrid graphics/touchpad/suspend were not exercised; the installer image is console-only (no desktop).

**Surface Pro (gen 1, Ivy Bridge i5-3317U, 4 GB, UEFI + Secure Boot, no Ethernet)** - **not tested on the real device**:
power off, hold *Volume Down* while pressing power to boot from USB (use a USB hub/adapter for the stick);
to disable Secure Boot hold *Volume Up* while powering on -> Security -> Secure Boot -> Disabled.
The ISO is built for generic x86-64 (no AVX2 requirement), so it runs on Ivy Bridge. 4 GB RAM is enough for the live
system and installer (the live root is a RAM overlay; the 1.5 GB image is read from the stick). There is **no wired NIC**: the
installer works offline (installs from the image), but the web UI needs the network afterwards - plug a USB Ethernet
adapter or join Wi-Fi with `iwctl` (the Marvell 88W8797 `mwifiex_pcie` firmware ships in `linux-firmware`; unverified on this unit).
The Type Cover keyboard works if it presents as USB-HID (use a USB keyboard if the console does not react).

Horcrux specifics: the installer partitions GPT (ESP 512 MB, /boot 1 GB, swap 4 GB, root = rest, ext4 by default; ZFS/btrfs/xfs menu entries exist in the interactive installer, only ext4 is covered by the automated test). VM guests need VT-x (present on both laptops; on the Surface Pro 1 nested guests will be slow).

## What is tested automatically

`.forgejo/workflows/live-iso.yml` (daily, and on every `v*` tag) builds the ISO on a rw-man host and
runs `build/live-iso/common/qemu-test.py` under QEMU/KVM:

* UEFI (OVMF) booting the ISO as a **USB mass-storage stick**, and BIOS (SeaBIOS) booting it as a **CD**;
* GRUB menu -> automatic install to a blank virtio disk with Horcrux's own installer;
* reboot from the installed disk only (no ISO), serial login, web UI/API answers over HTTPS,
  admin login with the generated first-boot password works.

Publishing happens only when all of that passed. The run's serial logs and screenshots are attached to
the release as `horcrux-daily-test-evidence.tgz`. The workflow never turns the job red on a failure
(that would e-mail the owner); the result is recorded in the `daily-status` release marker instead.

Build it yourself on any x86_64 Docker host: `build/live-iso/build.sh -o ./iso` (BuildKit required; a
first build compiles QEMU/libvirt and takes ~1 hour, cached rebuilds take minutes).

## Stable ISO criteria

A `v*` release gets a stable ISO attached (non-pre-release asset
`horcrux-<tag>-x86_64.iso` + `.sha256`) only when, on the **tagged commit**:

1. the repository's own CI is not red (`failure`/`error`) for that commit;
2. the ISO builds from the tag from scratch-equivalent inputs (Gentoo stage3 + binhost, the tag's source);
3. ISO size < 2 GiB (Forgejo asset limit) and its SHA256 verifies;
4. **both** boot paths pass the full test above: UEFI+USB and BIOS+CD, including unattended install,
   reboot into the installed system, web UI/API reachable, admin login OK;
5. no step reported `FAIL` in the evidence bundle (attached to the release).

If any criterion fails, nothing is attached and the failure is recorded in the `daily-status` marker
(mode `stable`). The previous release stays as is. Tags must be cut by the maintainers; the workflow
never creates tags or bumps versions.

## Known gaps

* **Secure Boot**: not supported (see above). A shim + MOK-enrolment chain (own signing key for GRUB and the kernel) is the next step.
* **Real hardware**: only QEMU/KVM (OVMF + SeaBIOS, USB-storage and CD boot) is tested automatically. The ThinkPad and Surface Pro notes above are expectations, not results.
* The live image has no ZFS, no desktop and no Wi-Fi GUI (`iwctl` only); the Portage tree is stripped from the image, so run `emerge-webrsync` before using `emerge` on a live/installed system.
* Default console login is `root` / project name; sshd allows root password login. Change it first.
* Only the ext4 install path is covered by the automated test.
