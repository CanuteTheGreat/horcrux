# Microsoft Surface on Horcrux (focus: Surface Pro 1 / 2013)

Status: **built and emulated, not yet run on the real device.** Everything below marked *verified* was checked in QEMU
(`-cpu IvyBridge -m 4096`, UEFI, DMI faked as the real SP1) or against kernel/linux-surface sources; everything marked
*device-only* needs the tablet (see the last section).

## Decision: stock Gentoo kernel + three small quirks, NOT the linux-surface patch set

| | |
|---|---|
| SP1 / SP2 (Ivy Bridge, 2013) | **mainline kernel** (`gentoo-kernel-bin`) + the quirks listed here |
| SP3 and newer, Surface Laptop/Book/Go | linux-surface patched kernel recommended (`gentoo/surface/kernel/`: pinned series for **7.2.x**, patches verified to apply; not shipped on the ISO) |

### Evidence

1. **The maintainers say so.** linux-surface issue #381 ("Surface Pro 1 and 2 do not correctly report lid open/closed state"):
   maintainer *archseer*: "there's not a lot of SP1/2 users anymore, and they largely **work out of the box without the
   need for our patches**." The wiki's feature matrix lists SP1 with every feature working and *none* of the footnotes
   "requires linux-surface kernel" (those are attached to touchscreen/pen on SP4 and newer).
2. **What the patch set contains** (`linux-surface/linux-surface` @ `8c62d84`, `patches/7.2`, 15 patches, BASE `v7.2.13`; the
   config fragment `configs/surface-7.2.config` enables exactly these): Surface Aggregator Module (`SURFACE_AGGREGATOR*`, `SURFACE_HID`,
   `SURFACE_KBD`, battery/charger/temp/fan over SAM: SP5+/Laptop/Book 2+), **IPTS** and **ITHC** touch (SP4+/SL: Intel's touch
   processor), cameras (IPU3, OV5693/OV8865: SP4+), `SURFACE_3_*`, `SURFACE_PRO3_BUTTON`, `SURFACE_GPE`, `INTEL_THC_HID`/`QUICKSPI`,
   the secure-boot/lockdown patches, `ath10k`, AMD GPIO. **None of these exist on the SP1** (no SAM, no IPTS, no cameras
   in the patched drivers' ID tables). A text search of the 15 patches finds no SP1/SP2 DMI string and no `88W8797`.
3. **Touch and pen are plain USB-HID** (owner's observation, consistent with the kernel tables): mainline `hid-ids.h` has
   `USB_DEVICE_ID_MS_SURFACE_PRO_2 0x0799`, `TOUCH_COVER_2 0x07a7`, `TYPE_COVER_2 0x07a9` and `hid-quirks.c` already carries
   `HID_QUIRK_NO_INIT_REPORTS` for them; the touchscreen runs on `hid-multitouch`, the pen on `wacom`/`hid-multitouch`.
   linux-surface's `0010-surface-typecover.patch` only touches the **2017** Type Cover USB id, Surface Go 3's USB delay and
   the backlight. No patch changes anything an SP1 uses.
4. **Wi-Fi: mwifiex_pcie with the Marvell 88W8797.** linux-surface's `0003-mwifiex.patch` adds a DMI table with a PCI-bridge
   reset/"no bridge D3" quirk for **Surface Pro 4, 5, 5 LTE, 6, Book 1/2, Laptop 1/2**. Mainline 7.2 already carries
   the older `QUIRK_FW_RST_D3COLD` table for the same models. **The SP1 is in neither table**; the wiki's "Marvell 88W8897
   quirks" page lists per-device workarounds (ASPM L1.x, D3cold reset) for SP3-SP6/Book/Laptop only. So the SP1 needs
   no driver patch. What the page does say for *all* Marvell-PCIe Surfaces is that card power saving misbehaves after
   suspend: we therefore ship `wifi.powersave = 2` (off) in NetworkManager. The firmware (`mrvl/pcie8897_uapsta.bin` /
   `mwifiex_pcie` family) is in `linux-firmware`, redistributable, already on the ISO.
5. **ACPI quirk the SP1 does need** (issue #381 itself): buggy `_LID` in the DSDT; after S3 the kernel reports the lid as closed and the
   desktop suspends again, forever. Mainline has no DMI quirk; the documented fix is `button.lid_init_state=open`, which the
   hardware profile adds to the installed kernel command line of `surface-pro-1`.
6. Related open item: #2049 "linux-surface kernel on Surface Pro 1: gnome-control-center display segfaults" (March 2026). We do not use GNOME.
7. **Secure Boot.** linux-surface's secureboot patch + MOK package is only relevant when Secure Boot stays on. Horcrux has no
   shim/MOK chain yet (docs/ISO-INSTALL.md): turn it **off** on the tablet.
8. **No iptsd**: it serves IPTS/ITHC touch processors (SP4+). The SP1's touch is HID; installing it would be pointless.

### What the SP1 gets instead of patches

| need | how |
|---|---|
| lid/suspend loop | kernel cmdline `button.lid_init_state=open` (hardware profile, only on SP1/SP2) |
| Wi-Fi stability | NetworkManager `wifi.powersave=2`; mwifiex firmware from linux-firmware; fall-backs if the real unit misbehaves: `pcie_aspm=off`, `iw dev wlan0 set power_save off`, USB Wi-Fi dongle |
| touch + pen | in-kernel `hid-multitouch` + `wacom`; sway maps touchscreen and pen to the internal panel (`surface-hidpi`) |
| Type Cover / keyboard | `usbhid`/`hid-microsoft`/`hid-multitouch`, **also inside the initramfs** so the LUKS passphrase can be typed |
| graphics | i915 (HD 4000, Mesa `crocus`/`swrast`); the initramfs includes `i915` for a native-resolution passphrase screen |
| 1920x1080 on 10.6" | `surface-hidpi` reads the EDID size (~208 ppi) and sets sway scale 1.5; foot/wvkbd/bar fonts are sized for touch |

## Install profile for a 4 GB tablet

* Detection: DMI `sys_vendor = Microsoft Corporation` and `product_name` in {`Surface with Windows 8 Pro`, `Surface Pro`, `Surface Pro 2`} →
  profile `surface`, model `surface-pro-1`. Any other `Surface*` product gets the generic Surface profile (no model quirks).
  Data: `gentoo/surface/hwprofile.toml`; code: `liveinst/hwprofile.py`.
* Disk: GPT with **ESP 1 GiB (FAT32, unencrypted)** + **/boot 1 GiB (ext4, unencrypted)** + **LUKS2 → XFS** root (no LVM), `xfs` mounted
  with default options (no `discard`). **zram is the only swap** (100 % of RAM, zstd; OpenRC `zram-swap`). Weekly `fstrim` via cron;
  the LUKS mapping is opened with `allow-discards` so TRIM reaches the SSD (trade-off: free-block pattern is visible to someone with the raw disk).
* LUKS2 keyslot: `argon2id`, memory cost = 3/16 of RAM, ≤ 1 GiB (4 GB → 768 MiB; 8 GB+ → 1 GiB), 2 lanes, ~2 s on the install machine.
* The installer offers **XFS + LUKS as the default file system for every machine with < 8 GB RAM** (ZFS stays the default above that;
  the interactive default only, answer files name their file system explicitly).
* Desktop: **sway** (Wayland; ~150-250 MB), foot terminal, wmenu launcher, **wvkbd** on-screen keyboard (toggle from the bar's *[Keyboard]* button
  or `Mod+k`), swaybar with tappable buttons, NetworkManager (`nmtui`), PipeWire. GNOME/KDE are deliberately not installed: too heavy for 4 GB.
  No browser in the default image (Firefox needs X11 libraries and a source build under a Wayland-only USE set; install one later from the binhost).
* Binpkg-first: the tablet never compiles anything. The installer copies the ISO's squashfs; the ISO build uses the official binhost and
  compiles only what its USE flags force (sway/wlroots/pipewire/wvkbd, on the build host).

## AVX2 / Ivy Bridge (verified)

The CPU is an i5-3317U: AVX yes, **AVX2/FMA/BMI1/BMI2 no**.

* Rust binaries: `RUSTFLAGS=-C target-cpu=x86-64-v2` in `Dockerfile.app` + `check-cpu-baseline.py` (objdump signature scan for BMI/LZCNT).
* Gentoo packages: the rootfs is built from the official generic **x86-64** binhost (CHOST guard in `Dockerfile.rootfs`), `CFLAGS=-O2 -pipe`, no `-march`.
* The default QEMU CPU of the test harness is `qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt,+cx16`; the Surface test runs **`-cpu IvyBridge`** and asserts
  `grep avx2 /proc/cpuinfo` = 0, no `invalid opcode` in `dmesg` after a full boot with horcrux-api, sway, NetworkManager, wvkbd running.

## Verified vs device-only

Verified in QEMU: hardware autodetect from DMI, XFS+LUKS install, argon2id parameters, LUKS prompt answered by a **USB keyboard with the PS/2
controller disabled** (so the initramfs HID stack is proven), zram, fstrim through dm-crypt, `button.lid_init_state=open`, sway/wvkbd/swaybar session,
IvyBridge CPU model. Screenshots: `docs/images/surface/`.

Device-only (cannot be emulated): that the **real** SP1's touchscreen and Wacom pen enumerate and are accurate; mwifiex_pcie firmware load and Wi-Fi stability
(and whether the SP1 needs `pcie_aspm=off`); Type Cover (SP1 generation) at the LUKS prompt over the real USB path; real suspend/resume with the lid quirk;
HD 4000 acceleration under Mesa; HiDPI scale feel; battery reporting; UEFI quirks (boot from USB via the hub, Secure Boot off).
`horcrux-surface-report` (in the image) collects exactly these facts for the remote hand-off.

## Sources

* linux-surface wiki: *Supported Devices and Features* (SP1 column), *Marvell 88W8897 quirks*, *Installation and Setup*, *Known Issues and FAQ*, *Disk Encryption*
* linux-surface/linux-surface `patches/7.2/*`, `configs/surface-7.2.config` (commit 8c62d84193a38bd097238b192a4f7262db8a9f7e)
* linux-surface issues #381 (SP1/SP2 `_LID`, `button.lid_init_state=open`), #2049 (SP1 + GNOME)
* kernel v7.2: `drivers/net/wireless/marvell/mwifiex/pcie_quirks.c`, `drivers/hid/hid-ids.h`, `hid-quirks.c`, `drivers/acpi/button.c`
* docs/T2-MACS.md (kernel 7.2 pin because of OpenZFS), Gentoo handbook (XFS)
