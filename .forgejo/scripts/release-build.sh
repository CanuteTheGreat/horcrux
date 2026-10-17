#!/usr/bin/env bash
# Release build of the `horcrux` CLI and the `horcrux-api` server for ONE
# Rust target, on the x86_64 Ubuntu CI job container
# (catthehacker/ubuntu:act-latest). Used by release.yml and build-iso.yaml.
#
#   .forgejo/scripts/release-build.sh <rust-target>
#
# horcrux-api links two C libraries through its default features: libvirt
# (feature `qemu` -> virt crate, found with pkg-config) and libpam (feature
# `pam`). Building without them fails at link time, so:
#
#   * *-linux-gnu: install libvirt-dev + libpam0g-dev for the TARGET
#     architecture (Debian multiarch; arm64/riscv64 packages come from
#     ports.ubuntu.com) plus the matching cross gcc, point cargo, cc-rs and
#     pkg-config at them, then build the CLI and the server.
#   * *-linux-musl: glibc's libvirt/libpam can't be linked into a static
#     musl binary, and a server without them would have no QEMU VMs and no
#     PAM realm, so musl artifacts are the static CLI only.
#
# Gentoo is the primary install path (gentoo/app-emulation/horcrux, USE flags
# select the backends); these tarballs are the "also runs on any modern
# distro" binaries.
#
# Env:
#   HORCRUX_API_FEATURES  cargo feature args for horcrux-api
#                         (unset: --all-features; empty: default features)
#   CARGO_PROFILE         cargo profile (default: release)
set -euo pipefail

target="${1:?usage: release-build.sh <rust-target>}"
api_features="${HORCRUX_API_FEATURES---all-features}"
profile="${CARGO_PROFILE:-release}"
export DEBIAN_FRONTEND=noninteractive

as_root() { if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi; }

# Enable a foreign Debian architecture. The stock Ubuntu sources only serve
# amd64 (other architectures live on ports.ubuntu.com), so pin every
# existing source to the native architecture first, or `apt-get update`
# fails fetching e.g. binary-arm64 indexes from archive.ubuntu.com.
enable_foreign_arch() {
    local arch="$1" native f
    native="$(dpkg --print-architecture)"
    as_root dpkg --add-architecture "$arch"
    for f in /etc/apt/sources.list.d/*.sources; do
        [ -e "$f" ] || continue
        case "$f" in */horcrux-ports.sources) continue ;; esac
        grep -q '^Architectures:' "$f" ||
            as_root sed -i "/^Types:/a Architectures: $native" "$f"
    done
    for f in /etc/apt/sources.list /etc/apt/sources.list.d/*.list; do
        [ -e "$f" ] || continue
        as_root sed -i -E "s/^deb (http|https|file)/deb [arch=$native] \1/" "$f"
    done
    # shellcheck disable=SC1091
    . /etc/os-release
    as_root tee /etc/apt/sources.list.d/horcrux-ports.sources >/dev/null <<EOF
Types: deb
URIs: http://ports.ubuntu.com/ubuntu-ports/
Suites: ${VERSION_CODENAME} ${VERSION_CODENAME}-updates ${VERSION_CODENAME}-security
Components: main universe
Architectures: ${arch}
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
EOF
}

# Point cargo (linker), cc-rs (CC/CXX/AR) and pkg-config at a cross gcc.
use_cross_gcc() {
    local gnu="$1" upper lower
    upper="$(echo "$target" | tr 'a-z-' 'A-Z_')"
    lower="${target//-/_}"
    export "CARGO_TARGET_${upper}_LINKER=${gnu}-gcc"
    export "CC_${lower}=${gnu}-gcc" "CXX_${lower}=${gnu}-g++" "AR_${lower}=${gnu}-ar"
}

build_api=true
case "$target" in
    x86_64-unknown-linux-gnu)
        as_root apt-get update -qq
        as_root apt-get install -y --no-install-recommends \
            build-essential pkg-config libvirt-dev libpam0g-dev
        ;;
    aarch64-unknown-linux-gnu | riscv64gc-unknown-linux-gnu)
        case "$target" in
            aarch64-*) arch=arm64 gnu=aarch64-linux-gnu ;;
            riscv64gc-*) arch=riscv64 gnu=riscv64-linux-gnu ;;
        esac
        enable_foreign_arch "$arch"
        as_root apt-get update -qq
        as_root apt-get install -y --no-install-recommends \
            build-essential pkg-config "gcc-${gnu}" "g++-${gnu}" "libc6-dev-${arch}-cross" \
            "libvirt-dev:${arch}" "libpam0g-dev:${arch}"
        use_cross_gcc "$gnu"
        # Only the target's .pc files (no host libvirt leaking in).
        export PKG_CONFIG_ALLOW_CROSS=1
        export PKG_CONFIG_LIBDIR="/usr/lib/${gnu}/pkgconfig:/usr/share/pkgconfig"
        unset PKG_CONFIG_PATH
        ;;
    x86_64-unknown-linux-musl)
        as_root apt-get update -qq
        as_root apt-get install -y --no-install-recommends build-essential musl-tools
        build_api=false
        ;;
    aarch64-unknown-linux-musl)
        as_root apt-get update -qq
        as_root apt-get install -y --no-install-recommends build-essential \
            gcc-aarch64-linux-gnu libc6-dev-arm64-cross
        # Rust's musl targets link their bundled (self-contained) musl crt
        # and libc; the gnu cross gcc only drives the link and compiles
        # ring's C (which needs just the cross <stdint.h> family).
        use_cross_gcc aarch64-linux-gnu
        build_api=false
        ;;
    *)
        echo "release-build.sh: unsupported target $target" >&2
        exit 2
        ;;
esac

if command -v rustup >/dev/null; then
    rustup target add "$target"
fi

set -x
cargo build --profile "$profile" --target "$target" -p horcrux-cli
if $build_api; then
    # shellcheck disable=SC2086 # feature args are a word list
    cargo build --profile "$profile" --target "$target" -p horcrux-api $api_features
fi
set +x

dir="${CARGO_TARGET_DIR:-target}/${target}/$([ "$profile" = dev ] && echo debug || echo "$profile")"
for bin in horcrux horcrux-api; do
    [ -f "$dir/$bin" ] || continue
    file "$dir/$bin" 2>/dev/null || ls -l "$dir/$bin"
    readelf -d "$dir/$bin" 2>/dev/null | grep NEEDED || echo "$bin: no dynamic libraries"
done
