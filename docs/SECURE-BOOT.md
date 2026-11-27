# UEFI Secure Boot

Secure Boot works with the x86_64 ISO and installed x86_64 systems, at no cost: the boot chain is signed with our
own key, and you enroll our public certificate **once**, on the first boot, through MokManager (the standard Linux "Machine
Owner Key" screen, the same one Ubuntu, Fedora and openSUSE use for extra keys). After that, Secure Boot stays ON.

## How the chain works

```
firmware (Microsoft UEFI CA in db - shipped on almost every PC)
  -> shim         Fedora's shim, signed by Microsoft (Gentoo package sys-boot/shim); includes MokManager
    -> GRUB       our build, signed with OUR key, with an SBAT section (shim refuses images without one)
      -> kernel   signed with OUR key (GRUB asks shim to verify it)
        -> modules  in-tree modules carry the kernel build key; out-of-tree modules (zfs, spl, ...) are signed with OUR key
```

With Secure Boot on, the kernel enforces module signatures (IMA architecture policy), and GRUB runs in lockdown mode (no
module loading from disk: every module the menus need is built into the signed GRUB image). GRUB also refuses unsigned font
files in lockdown, so with Secure Boot on the boot menu is the plain text menu (same entries).

The project certificate: `O=Errant Solutions, CN=Errant Solutions Secure Boot Signing 2026`,
SHA-256 fingerprint `3A:01:89:A4:34:1D:94:13:63:03:E2:51:26:EB:6A:64:4D:C0:E6:84:9A:CE:8C:EC:E7:E8:AC:17:D2:5E:E0:CE`.
It ships on every medium as `ENROLL_THIS_KEY_IN_MOKMANAGER.cer` and in the source tree as
`build/live-iso/common/boot/secureboot/signing.crt`. Horcrux and Patronus use the same certificate, so enrolling it once
covers both.

## First boot of the ISO with Secure Boot on (one time)

1. Boot the stick. shim cannot verify our GRUB yet and shows **"Verification failed: (0x1A) Security Violation"**. Press
   **Enter** (OK).
2. MokManager starts: **"Press any key to perform MOK management"** - press a key within 10 seconds.
3. Choose **Enroll key from disk**, pick the USB stick (shown as `EFIBOOT` or a device path), then
   **ENROLL_THIS_KEY_IN_MOKMANAGER.cer**.
4. **Continue** (or *View key 0* to compare the fingerprint above), then **Yes**, then **Reboot**.

The machine boots the installer; from now on the firmware accepts everything signed with our key. Nothing else in the
firmware needs to change. Checking from a running system: `mokutil --sb-state` says `SecureBoot enabled`, and
`mokutil --list-enrolled` lists the certificate.

Some PCs (often "Secured-core" laptops) ship with the **Microsoft 3rd-party UEFI CA** switched off; it signs shim (and most
Linux distributions and GPU option ROMs). If shim itself is refused ("Secure Boot violation" before any blue screen), enable
*Allow Microsoft 3rd Party UEFI CA* in the firmware setup, then start again.

## Installed systems

The installer (`liveinst/secureboot.py`) puts the same chain on the ESP (`EFI/BOOT/BOOTX64.EFI` = shim, `grubx64.efi`,
`mmx64.efi`, `grub.cfg` that finds `/boot` by UUID) whenever the image is signed and the machine boots UEFI. The kernel and
modules it copies are already signed, so the installed system boots with Secure Boot on right away.

For **updates** it creates a **machine key** in `/etc/secureboot/` (`machine.key` is readable by root and the `portage`
group only, and kept out of product configuration backups), and points Portage at it:

* `/etc/portage/package.use/zz-secureboot`: `secureboot` on the kernel packages (secureboot.eclass signs each new kernel
  image), `modules-sign` on every out-of-tree module package (linux-mod-r1 signs zfs etc. when they are rebuilt);
* `/etc/portage/make.conf`: `SECUREBOOT_SIGN_KEY/CERT`, `MODULES_SIGN_KEY/CERT`.

The machine certificate is queued with `mokutil --import`. **At the first reboot after the install MokManager appears once**
(blue screen, "Perform MOK management"): choose **Enroll MOK**, **Continue**, **Yes**, type the **MOK password** shown on
the installer's last page, then **Reboot**. Skipping it is safe: the system still boots the project-signed kernel, a kernel
install prints a warning while the key is not enrolled, and `mokutil --import /etc/secureboot/machine.der` queues
it again. Boot environments keep older kernels as automatic fallbacks.

Multi-disk ZFS installs copy the signed ESP files to every disk (`horcrux-boot-sync`), each pointing at its own `/boot` copy.

### Configuration

Answer file section `[secure_boot]` (all optional):

| key | values | default |
|---|---|---|
| `mode` | `auto` (shim chain whenever the image is signed and the machine boots UEFI), `on` (refuse to install without it), `off` (plain GRUB, as before) | `auto` |
| `key`, `cert` | bring your own machine key: PEM files (paths relative to the answer file) | generated |
| `mok_password` | one-time MokManager password for the machine key | generated, shown at the end |
| `machine_key` | `false` = no machine key (kernels installed later must be signed by hand) | `true` |

Building images: `SECUREBOOT=auto|on|off`, `SECUREBOOT_KEY_FILE` (or the PEM text in `SECUREBOOT_SIGNING_KEY`) and
`SECUREBOOT_CERT_FILE` (or `SECUREBOOT_CERT`) select the key `build.sh` signs with; your own key gives you an image that
trusts only your certificate. Without a key the image is built unsigned (and then needs Secure Boot off).

## Proof (CI)

Every live-ISO run with the signing secret builds a signed ISO and runs, on the Secure Boot firmware builds (OVMF/AAVMF with
Microsoft keys enrolled, Secure Boot enforcing):

* `secureboot-test.py`: **no key** - shim refuses our GRUB and opens MokManager, GRUB never runs; **enroll** - the exact
  steps above, typed over the serial console, then GRUB, a kernel reporting `Secure boot enabled` and the installer, and the
  key persisting to the next boot; **off** - the same chain boots with Secure Boot disabled.
* `qemu-test.py --secure-boot mok`: unattended install (ZFS), then the installed system boots with Secure Boot on and the
  test asserts SB is enabled inside the guest, the ESP holds the shim chain, the signed ZFS modules loaded, and the machine
  key is configured.

An x86_64 publish run fails if the ISO carries no signed chain. arm64 uses the same chain (shimaa64, grubaa64); its images are
signed when the kernel image is an EFI binary and the gates run in the aarch64 CI, but arm64 Secure Boot is not proven yet.
riscv64 has no Microsoft-signed shim and stays unsigned.

## Key custody

The private key is never in a repository or an image. It exists as the Forgejo Actions secret `SECUREBOOT_SIGNING_KEY` of the
two product repositories, plus an offline escrow copy held by the maintainer. Rotation: generate a new key + certificate,
ship the new certificate on the media and as a MokManager enrollment, sign with both during an overlap, then retire the old
one (`mokutil --delete`). A leaked key is revoked through the MOK deny list (`mokutil --mokx`) or, once we are in shim's
vendor certificate, through SBAT generation bumps.

## Turnkey Secure Boot (no enrollment) - planned

The enrollment step disappears once our own shim, with our certificate embedded, is signed by Microsoft. That goes through
the community shim-review process (free). Preparation is documented in [secure-boot/shim-review.md](secure-boot/shim-review.md);
nothing has been submitted yet.
