#!/bin/sh
# build-kernel.sh: runs INSIDE the build container. Needs /usr/src/<KERNEL_SRC_DIR> (gentoo-sources), the patches in
# $PATCHES (default /t2patches), the Gentoo dist config in /dist.config and the kernel/ dir of gentoo/t2 in /t2/kernel.
set -eu
: "${KERNEL_SRC_DIR:?}" "${LOCALVERSION:?}"
PATCHES=${PATCHES:-/t2patches}; K=/usr/src/$KERNEL_SRC_DIR
cd "$K"
for p in "$PATCHES"/*.patch; do
  patch -p1 -N --no-backup-if-mismatch -s < "$p" || { echo "PATCH FAILED: $p" >&2; exit 1; }
done
[ -z "$(find . -name '*.rej' | head -1)" ] || { echo "reject files left" >&2; exit 1; }
echo "applied $(ls "$PATCHES"/*.patch | wc -l) t2linux patches"
cp /dist.config .config
# base = Gentoo dist config (olddefconfig'd up to this kernel), then our fragment, then t2linux's extra_config
make olddefconfig >/dev/null
scripts/kconfig/merge_config.sh -m .config /t2/kernel/t2.config "$PATCHES/extra_config" >/dev/null
make olddefconfig >/dev/null
grep -v '^#' /t2/kernel/required-config.txt | grep . | while read -r l; do
  grep -qx "$l" .config || { echo "MISSING in .config: $l" >&2; echo bad >/tmp/badcfg; }
done
[ ! -e /tmp/badcfg ] || exit 1
# Gentoo's dist config points the module-signing key/trusted keys at files that only exist in its own build dir:
# we do not sign modules (Secure Boot is off on the laptop), so clear everything key related
for o in MODULE_SIG MODULE_SIG_ALL MODULE_SIG_FORCE MODULE_SIG_SHA512 MODULE_SIG_SHA256 SYSTEM_TRUSTED_KEYRING SYSTEM_REVOCATION_LIST SECONDARY_TRUSTED_KEYRING; do scripts/config --disable $o; done
scripts/config --set-str MODULE_SIG_KEY "certs/signing_key.pem" --set-str SYSTEM_TRUSTED_KEYS "" --set-str SYSTEM_REVOCATION_KEYS ""
make olddefconfig >/dev/null
grep -E 'MODULE_SIG|TRUSTED_KEY|REVOCATION|CONFIG_LOCALVERSION' .config | sed 's/^/cfg: /'
# fail FAST (minutes, not hours) if the build prerequisites are broken
make -j"$(nproc)" prepare > /tmp/prepare.log 2>&1 || { tail -30 /tmp/prepare.log >&2; exit 1; }
make -j"$(nproc)" certs/ > /tmp/certs.log 2>&1 || { tail -30 /tmp/certs.log >&2; exit 1; }
if ! make -k -j"$(nproc)" bzImage modules > /tmp/make.log 2>&1; then
  echo "=== kernel build FAILED; first errors:" >&2
  grep -n -i -B2 -A6 'error\|\*\*\*' /tmp/make.log | head -80 >&2
  exit 1
fi
tail -3 /tmp/make.log
make modules_install INSTALL_MOD_STRIP=1 >/dev/null
kr=$(make -s kernelrelease)
install -D -m 0644 arch/x86/boot/bzImage "/boot/vmlinuz-$kr"
install -m 0644 System.map "/boot/System.map-$kr"
install -m 0644 .config "/boot/config-$kr"
# keep the build tree small but usable for out-of-tree modules (ZFS)
echo "built $kr"
echo "$kr" > /boot/.kernelrelease
