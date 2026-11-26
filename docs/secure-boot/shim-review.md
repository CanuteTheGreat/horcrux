# Turnkey Secure Boot: shim-review preparation (staged, not submitted)

Today Secure Boot works through a one-time MokManager enrollment of our certificate ([../SECURE-BOOT.md](../SECURE-BOOT.md)),
using Fedora's Microsoft-signed shim. Turnkey Secure Boot (no enrollment at all) needs **our own shim**, with our certificate
built in, signed by Microsoft. This page records what that takes, what is already in place, and what is missing. Nothing has
been sent to any third party: a submission (shim-review, Microsoft) is a maintainer decision.

## The process

1. **shim-review** (https://github.com/rhboot/shim-review, free): open an issue with a filled-in review template; community
   reviewers check the build, the key handling and the whole boot chain. Acceptance usually takes weeks to months and needs
   a track record (reviewers look for an existing, maintained distribution).
2. **Microsoft UEFI CA signing**: the accepted shim binary is submitted through the Microsoft Partner Center hardware
   dashboard, which requires a Partner Center account registered with an **EV code-signing certificate**.

## Requirements and our status

| requirement (from the shim-review template) | status |
|---|---|
| shim built from an unmodified upstream release tag (16.x) with a reproducible build (Dockerfile) and our vendor certificate embedded | not started: draft build recipe below |
| SBAT: shim, GRUB and our vendor entries with correct generations | done for GRUB (`grub,<upstream gen>` + `grub.horcrux`/`grub.patronus` lines, `boot/secureboot-sign.sh`) |
| GRUB with all upstream CVE fixes, NX-compatible, SBAT section, built-in modules only (lockdown) | GRUB 2.16 from Gentoo (upstream SBAT generation 6), standalone image with embedded modules |
| kernel enforces lockdown when booted with Secure Boot (no unsigned code: modules, kexec, hibernation image, /dev/mem) | **gap**: Gentoo's distribution kernel enforces module signatures under Secure Boot (IMA arch policy) but its lockdown default is `none`; it needs `lockdown=integrity` forced in Secure Boot mode (kernel config `LOCK_DOWN_IN_EFI_SECURE_BOOT`-style patch, or our own kernel build) |
| kernel module signing with an ephemeral or protected key | in-tree: Gentoo's ephemeral build key; out-of-tree: our key / the machine key |
| signing private key in a hardware security module (FIPS 140-2 class token) | **gap**: today a software key (CI secret + offline escrow); needs an HSM/token and a signing step that uses it |
| two security contacts with published PGP keys, a vulnerability disclosure process, and a commitment to ship fixes and SBAT bumps | partial: SECURITY.md exists; contacts/keys to be named |
| a revocation plan (SBAT generations, MOK deny list) | documented in SECURE-BOOT.md |
| proof the chain works | done: `secureboot-test.py` + `qemu-test.py --secure-boot mok` in every live-ISO CI run |

## Blockers that cost money (not pursued)

* Microsoft Partner Center registration needs an EV code-signing certificate (yearly fee).
* The HSM/token requirement for the signing key (one-off hardware cost).

Under the current "free only" constraint these are not purchased; the MokManager enrollment stays the supported path.

## Draft build recipe (for when it is pursued)

```
FROM gentoo/stage3:amd64-openrc
# shim upstream release, unmodified
RUN emerge-webrsync && emerge dev-vcs/git sys-devel/gcc app-crypt/sbsigntools
RUN git clone --branch 16.1 --recursive https://github.com/rhboot/shim /shim
COPY signing.der /shim/vendor.der          # our CA/vendor certificate (DER)
COPY sbat.errant.csv /shim/data/sbat.errant.csv
RUN cd /shim && make VENDOR_CERT_FILE=vendor.der EFI_PATH=/usr/lib && sha256sum shimx64.efi mmx64.efi fbx64.efi
```

`sbat.errant.csv`: `shim.errant,1,Errant Solutions,shim,16.1,https://horcrux.errant.solutions/` (one line per product
vendor entry as reviewers ask). The review template answers that are already known: product names Horcrux and Patronus
(Gentoo-based, public source at https://github.com/CanuteTheGreat/horcrux and https://github.com/CanuteTheGreat/patronus),
bootloader GRUB 2.16 + Gentoo patches, kernel Gentoo distribution kernel (signed per release), CI proof as above.
