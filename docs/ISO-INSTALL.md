# Installing Horcrux from the live ISO

Horcrux ships a Gentoo-based **live + installer ISO** (x86_64 UEFI **and** legacy BIOS, aarch64 UEFI; hybrid:
the same file boots from a CD/VM *and* from a USB stick; see "Architectures"). It contains a **guided installer** (a text-mode wizard that works on a VGA/laptop screen and on a serial
console), an **unattended installer** driven by an answer file, and a live system you can look around in.
There is no default password anywhere: the installer makes you set one, and SSH on the installed system is
key-only unless you opt in.

## Download

| | URL |
|---|---|
| Latest **daily** test build (rolling pre-release, replaced after the QEMU test passed) | `https://horcrux.errant.solutions/download/horcrux-daily-x86_64.iso` |
| SHA256 of the daily | `https://horcrux.errant.solutions/download/horcrux-daily-x86_64.iso.sha256` |
| **Stable** build (only for `v*` releases that met the criteria below) | `https://horcrux.errant.solutions/download/horcrux-<tag>-x86_64.iso` (tag like `v0.2.0`) |

Always verify: `sha256sum -c horcrux-daily-x86_64.iso.sha256` (run it next to the downloaded files).

## Architectures

| arch | boot | serial console | built on | tested with | assets |
|---|---|---|---|---|---|
| **x86_64** | UEFI **and** legacy BIOS (hybrid ISO) | `ttyS0` | rw-man-01 (`kvm-x86_64` runner) | QEMU/KVM, OVMF + SeaBIOS | `horcrux-daily-x86_64.iso` |
| **aarch64** | UEFI only (GRUB `arm64-efi`, `EFI/BOOT/BOOTAA64.EFI`); there is no BIOS on arm64 | `ttyAMA0` (PL011) | rw-man-04/05 (`arm64` runner, native Graviton build, no emulation) | QEMU `virt` + AAVMF; **TCG** (the Graviton hosts have no `/dev/kvm`), timeouts x3 | `horcrux-daily-aarch64.iso` |
| riscv64 | UEFI only (GRUB `riscv64-efi`) | `ttyS0` | not built in CI yet | not tested | - |

Per-architecture facts (stage3 tag, GRUB target, EFI file name, serial device, squashfs filter, the **per-arch
binhost**) live in `build/live-iso/common/arch/<arch>.env|.binrepos.conf|.make.conf`. The rootfs build checks that
the binhost serves the rootfs's own `CHOST` (so an aarch64 build can never pull x86 packages or silently fall back
to compiling everything), and the BuildKit cache mounts are keyed by arch. Download URLs:

| | URL |
|---|---|
| daily aarch64 | `https://horcrux.errant.solutions/download/horcrux-daily-aarch64.iso` (+ `.sha256`) |
| stable aarch64 | `https://horcrux.errant.solutions/download/horcrux-<tag>-aarch64.iso` |

Each architecture has its own job, its own assets and its own status marker (`daily-status` for x86_64,
`daily-status-aarch64`), so a failing arch never blocks the other. Test evidence is attached per arch
(`horcrux-daily-<arch>-test-evidence.tgz`). Build one yourself on a native host: `ARCH=aarch64 build/live-iso/build.sh -o ./iso`
(`ALLOW_EMULATED=1` is needed to build a foreign arch under qemu-user; not supported for the app stage).
On arm64 machines the ISO is written to the stick exactly as on x86 (`dd`); firmware must be UEFI (Secure Boot off).
Test it in QEMU: `qemu-system-aarch64 -machine virt -cpu max -m 4G -drive if=pflash,format=raw,readonly=on,file=AAVMF_CODE.fd -drive if=pflash,format=raw,file=VARS.fd -device qemu-xhci -device usb-storage,drive=iso -drive if=none,id=iso,file=horcrux-daily-aarch64.iso,format=raw -device virtio-gpu-pci -device usb-kbd -serial stdio`.

**riscv64** is not in CI: there is no RISC-V runner, the official binhost has only ~600 riscv64 packages (about 40 of the
ones the image needs would compile from source under emulation: QEMU, libvirt, docker, ZFS, GRUB, dracut, ...), no
`rust:bookworm` image and no `trunk` binary exist for it, and the registered binfmt handler needed for emulated builds is not present on the
runners. The code paths are ready (`arch/riscv64.env`, harness `--arch riscv64`); what is missing is a native builder or a
cross-compile stage for the app plus a long-running weekly emulated job.

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

1. **Install Horcrux (guided installer)** - the default entry; starts the wizard on the screen (tty1).
2. *Install Horcrux - serial console (ttyS0 115200)* - the same wizard on the serial port (headless boxes, QEMU).
3. *Automated install from an answer file (ERASES the target disk)* - see "Unattended install"; if no
   answer file is found it falls back to the guided installer.
4. *Horcrux - live system (no install)*, *live system, serial console* - a root shell on the console
   (no password: physical access only; **sshd is off** and root's password is locked).
5. *Install Horcrux - safe graphics (nomodeset)* - the wizard with `nomodeset` for odd GPUs.

Pick the entry with the arrow keys within 5 s, otherwise the guided installer starts.

## The guided installer

Keys: **Tab / Shift-Tab / Up / Down** move between fields, **Enter** goes to the next field (and presses the
highlighted button), **Left / Right / Space** change a choice or tick a box, in a list **type to filter**,
**Ctrl-U** clears a field, **Ctrl-L** repaints, **Esc** asks whether to abort. Nothing is written to a disk
before you type `ERASE` on the summary screen.

1. **Language, keyboard and time zone** - installed-system locale, console keymap, time zone (type to filter the list).

   ![language, keyboard, time zone](images/installer/horcrux/01-language-keyboard-timezone.png)

2. **File system** - ext4, XFS, or ZFS (single disk or a two-disk **mirror**, pool `rpool`, lz4 compression).

   ![file system](images/installer/horcrux/02b-filesystem-zfs-mirror.png)

3. **Target disk(s)** - every internal disk with model, size, bus type and its existing partitions; the USB stick you booted from is hidden. A ZFS mirror needs exactly two disks.

   ![target disks](images/installer/horcrux/03b-target-disk-selected.png)

4. **Management network** - pick the NIC (link state, MAC, driver shown; Wi-Fi adapters are listed too and lead to a Wi-Fi screen with SSID scan and passphrase), the host name (FQDN), DHCP or a static address / gateway / DNS.

   ![management network](images/installer/horcrux/04b-management-network-filled.png)

5. **Administrator account** - REQUIRED password (min. 12 characters, mixed, no guessable words; confirm; strength meter), an optional SSH public key, the notification e-mail, and the opt-in for SSH password login (off by default, with a warning when ticked). The one password protects console login (`root`) and the web UI (`admin`).

   ![administrator account](images/installer/horcrux/06b-administrator-account-filled.png)

6. **Summary** - everything you chose; you must type `ERASE` to confirm the wipe.

   ![summary](images/installer/horcrux/07b-summary-confirmed.png)

7. **Installing** - progress bar and live log (full log: `/var/log/live-installer.log`).

   ![installing](images/installer/horcrux/08-installing.png)

8. **Finished** - the web UI URL, how to log in, and a *Reboot* button (remove the stick first).

   ![finished](images/installer/horcrux/09-finished.png)

The screenshots are real QEMU screen captures of the VGA console (`qemu-test.py --flow tui-vga`), taken by
the nightly test.

After the install: remove the stick and boot. The web UI is at `https://<ip>:8006` (self-signed
certificate; user `admin`, the password you set).

## Unattended install (answer file)

Boot the *Automated install* entry with an **answer file** available in one of two ways:

* a volume (USB stick, disk, virtual disk) whose file-system label is **`HORCRUX-ANSWERS`** (ext4, FAT or ISO9660) holding `answer.toml`; or
* the kernel argument `horcrux.answers=https://server/answer.toml` (press `e` on the GRUB entry).

The file **must** carry a root password **hash** (never a plain password) and/or an SSH public key - without
either the installer refuses, because the system would be unreachable. Make a hash with
`openssl passwd -6` or `mkpasswd -m sha-512`.

```toml
[global]
keyboard = "us"
timezone = "Europe/Berlin"
hostname = "horcrux-1.example.com"
email = "ops@example.com"
root_password_hash = "$6$....."          # console login; omit if you only want key login
ssh_keys = ["ssh-ed25519 AAAA... admin@laptop"]
ssh_password_auth = false                 # default false: key-only root SSH

[disk]
filesystem = "ext4"                       # ext4 | xfs | zfs
disks = ["first"]                         # or ["/dev/nvme0n1"]; "first" = first internal non-USB disk >= 8 GB
zfs_raid = "mirror"                       # with filesystem = "zfs": single | mirror (needs two disks)

[network.management]
mode = "static"                           # or "dhcp"
iface = "auto"                            # NIC name, or auto = first wired NIC with a link
address = "192.168.1.50/24"
gateway = "192.168.1.1"
dns = ["192.168.1.1"]
```

The installed system's web-UI `admin` password is generated on first start into
`/var/lib/horcrux/initial-admin-password` (root-readable) - read it over SSH or the console, and change it.
The installer powers the machine off when it is done (`AUTOINSTALL-OK` on the console and in
`/var/log/autoinstall.log`). Invalid answer files stop with `ANSWER-FILE-ERROR ...` lines and start the guided installer instead.

## Security defaults

* **No default password.** The live image has root locked (the consoles auto-login for physical access; **sshd is off**). The guided installer requires a password (>= 12 characters); the answer file requires a hash and/or an SSH key.
* **Installed system:** `PermitRootLogin prohibit-password` and `PasswordAuthentication no` - root can log in over SSH only with the key you gave. If you tick *Allow SSH password login* (or set `ssh_password_auth = true`) the installer sets `PermitRootLogin yes` + `PasswordAuthentication yes` and warns you.
* The web-UI admin password you type is stored only as a first-boot file that is deleted once the service is up.

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
the AX211 Wi-Fi needs `iwlwifi` firmware, included; use `iwctl` (or the installer's Wi-Fi screen)). NVMe installs as `/dev/nvme0n1`.
Hybrid graphics/touchpad/suspend were not exercised; the installer image is console-only (no desktop).

**Surface Pro (gen 1, Ivy Bridge i5-3317U, 4 GB, UEFI + Secure Boot, no Ethernet)** - **not tested on the real device**:
power off, hold *Volume Down* while pressing power to boot from USB (use a USB hub/adapter for the stick);
to disable Secure Boot hold *Volume Up* while powering on -> Security -> Secure Boot -> Disabled.
The ISO is built for generic x86-64 (no AVX2 requirement), so it runs on Ivy Bridge. 4 GB RAM is enough for the live
system and installer (the live root is a RAM overlay; the 1.5 GB image is read from the stick). There is **no wired NIC**: the
installer works offline (installs from the image), but the web UI needs the network afterwards - plug a USB Ethernet
adapter or join Wi-Fi with `iwctl` (or the installer's Wi-Fi screen) (the Marvell 88W8797 `mwifiex_pcie` firmware ships in `linux-firmware`; unverified on this unit).
The Type Cover keyboard works if it presents as USB-HID (use a USB keyboard if the console does not react).

Horcrux specifics: ext4 / XFS / ZFS (single or mirror) from the wizard; the root partition fills the disk after ESP/BIOS-boot (512 MB / 1 MB), `/boot` (1 GB) and swap (up to 4 GB). VM guests need VT-x (present on both laptops; on the Surface Pro 1 nested guests will be slow). ZFS wants >= 4 GB RAM (the Surface Pro has exactly 4: prefer ext4 there).

## What is tested automatically

`.forgejo/workflows/live-iso.yml` (daily, and on every `v*` tag) builds the ISO on a rw-man host and runs
`build/live-iso/common/qemu-test.py` under QEMU/KVM. Five QEMU runs per build (four full installs plus the no-answer-file check):

| run | firmware / media | installer | file system | network | SSH |
|---|---|---|---|---|---|
| 1 | BIOS + CD | answer file (unattended) | ext4 | DHCP | key-only |
| 2 | BIOS + CD | guided TUI over the **serial** console (scripted keystrokes) | XFS | static `10.0.2.77` | key-only |
| 3 | UEFI + USB stick | guided TUI on the **VGA** console (QMP keystrokes + screenshots) | ZFS mirror, 2 disks | static `10.0.2.88` | key-only |
| 4 | UEFI + USB stick | answer file (unattended) | ZFS mirror, 2 disks | static `10.0.2.99` | **password opt-in** |
| 5 | BIOS + CD | *Automated install* entry **without** an answer file: must refuse and start the guided installer on the VGA screen; the disk stays untouched | - | - | - |

Every run then reboots from the installed disk alone (no ISO) and asserts:

* the live system's sshd is **off** (nothing answers on port 22 while installing);
* the old default root password (`horcrux`) is **rejected** on the console, the chosen one works;
* SSH **refuses password root login** (offered auth methods are `publickey` only; a real `sshpass` attempt is refused), **accepts the configured key**, and `sshd -T` shows `permitrootlogin without-password` / `passwordauthentication no` (runs with the opt-in: password login works and the config says `yes`);
* the web UI/API answers over HTTPS **on the chosen IP** (QEMU forwards to that exact guest address; static addresses and DHCP are both covered), the admin login works (with the password typed in the wizard, or the generated first-boot password for answer-file runs);
* host name, time zone, root file system type and, for ZFS, `zpool status` ONLINE with a two-disk mirror.

Publishing happens only when all of that passed. The run's serial logs, per-screen text captures and PNG
screenshots are attached to the release as `horcrux-daily-test-evidence.tgz`. The workflow never turns the job
red on a failure (that would e-mail the owner); the result is recorded in the `daily-status` release marker instead.
Unit tests of the installer's validation rules (`common/tests`) run first.

Build it yourself on any x86_64 Docker host: `build/live-iso/build.sh -o ./iso` (BuildKit required; a
first build compiles QEMU/libvirt/ZFS and takes ~1 hour, cached rebuilds take minutes).

## Stable ISO criteria

A `v*` release gets a stable ISO attached (non-pre-release asset
`horcrux-<tag>-x86_64.iso` + `.sha256`) only when, on the **tagged commit**:

1. the repository's own CI is not red (`failure`/`error`) for that commit;
2. the ISO builds from the tag from scratch-equivalent inputs (Gentoo stage3 + binhost, the tag's source);
3. ISO size < 2 GiB (release asset limit) and its SHA256 verifies;
4. **all five** QEMU runs above pass (both firmware types, both media types, unattended + serial wizard + VGA wizard), including the SSH/web assertions;
5. no step reported `FAIL` in the evidence bundle (attached to the release).

If any criterion fails, nothing is attached and the failure is recorded in the `daily-status` marker
(mode `stable`). The previous release stays as is. Tags must be cut by the maintainers; the workflow
never creates tags or bumps versions.

## Console menu (installed system)

After the install, the system console (VGA tty1, and the serial line `ttyS0` when the install was done from a
serial/automated boot entry) shows an OPNsense-style menu instead of a bare shell. By default the console asks for
the root password first (log in, the menu starts); tick **Open console menu** in the installer (or
`console_menu_open = true` in `answer.toml` `[global]`) for the OPNsense behaviour where anyone at the console gets
the menu without a login. tty2-6 and SSH always give a normal login shell. `touch /etc/live-installer/no-console-menu`
turns the menu off.

| Option | What it does |
|---|---|
| 1 Assign interfaces | Pick the management NIC, then set its address (same dialog as option 2). Link state is shown per NIC; `d` = identify a port by unplug/replug. |
| 2 Set interface IP address | DHCP or static (address/prefix, gateway, DNS) for one NIC; Wi-Fi NICs can (re)connect through iwd. Written to `/etc/dhcpcd.conf` (managed block) and applied at once. |
| 3 Reset the admin password | Sets a new web `admin` password directly in the product's database (argon2id), drops web sessions, optionally also sets the root console password. |
| 4 Reset to factory defaults | Type `RESET` to confirm; first writes a backup, then deletes the configuration database, re-applies the product's boot units and starts it with a new password. Interface roles and addresses are kept. |
| 5 / 6 | Power off / restart the machine (asks first). |
| 7 Ping host | Validated host name or address only (no shell metacharacters). |
| 8 Shell | Root shell; `exit` returns to the menu. |
| 9 Web UI URL and status | URL, uptime, service states, root fs, effective SSH settings, latest backups. |
| 10 Restore a configuration backup | Lists `/var/backups/horcrux/*.tar.gz` and `*.tar.gz` on any attached USB stick. Refuses archives of another product, with paths outside the product's config paths, or with links; takes a `pre-restore` backup first. |
| 11 Create a configuration backup | `/var/backups/horcrux/horcrux-config-manual-<time>.tar.gz` (consistent SQLite copy while the service runs), optionally copied to a USB stick. |

Screens (real QEMU captures from the CI run, UEFI, VGA console): the login on a protected console, then the menu.

![VGA login](images/console-menu/vga-login.png)
![VGA console menu](images/console-menu/vga-menu.png)

Tests (QEMU, UEFI + BIOS, serial and VGA): the serial runs drive options 9, 7 (including a rejected `x;reboot`), 2,
3, 11, 4, 10 and 5, and log in to the web UI with the new password after each reset (the old one must fail); the VGA run
logs in on tty1 by keyboard and checks that `console-menu` is the process on tty1. Both products.

## Shared installer source

`build/live-iso/common` in Horcrux is the master; Patronus `gentoo/live-iso/common` is a vendored copy.
`sync-common.sh --write-hash` (Horcrux, after editing) records `common/TREE.sha256`; `sync-common.sh --from <horcrux checkout>`
(Patronus) copies it. CI runs `--check` in both repos (and `--upstream` in Patronus) before the unit tests, so hand edits or a
stale copy are caught.

## Known gaps

* **Secure Boot**: not supported (see above). A shim + MOK-enrolment chain (own signing key for GRUB and the kernel) is the next step.
* **Real hardware**: only QEMU/KVM (OVMF + SeaBIOS, USB-storage and CD boot) is tested automatically. The ThinkPad and Surface Pro notes above are expectations, not results. In particular untested: Wi-Fi scan/connect (needs a real adapter; the code path is exercised only by unit logic), the *Auto-detect (replug the cable)* link detection, NVMe/eMMC device naming (`part_path` is unit-tested), the large console font on high-DPI panels (`setfont ter-v32n` when the framebuffer is >= 1600 px wide), touch/Type Cover input.
* The installer is text mode (curses). A graphical installer is not provided.
* Installer language: only the *installed system's* locale/keymap are selectable; the wizard text is English.
* ZFS mirror: /boot (ext4) and the swap partition live on the first disk only, so the box boots only while disk 1 is healthy (the ESP/MBR is installed on both). No RAIDZ, no encryption, no separate data pool yet. `zpool status` shows a harmless "pool hostid mismatch" notice (the generic initramfs imports the pool with `zfsforce=1`).
* The notification e-mail is recorded in `/etc/horcrux/notify-email`; the product does not read it yet.
* The live image has no desktop; the Portage tree is stripped from the image, so run `emerge-webrsync` before using `emerge` on a live/installed system.
