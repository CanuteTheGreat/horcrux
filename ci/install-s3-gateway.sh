#!/usr/bin/env bash
# Install the S3 gateway's server and client for CI: Silo (the community
# MinIO fork that still publishes binaries; MinIO itself stopped in 2025)
# and its MinIO client `mcli`. Pinned release, SHA-256 verified.
#
# usage: ci/install-s3-gateway.sh [prefix]   (default /usr/local/bin)
set -euo pipefail

PREFIX=${1:-/usr/local/bin}
RELEASE=RELEASE.2026-10-18T00-00-00Z
VER=20261018000000.0.0

case "$(uname -m)" in
    x86_64) ARCH=amd64
        SILO_SHA=e069b6425f0de944fea6c301773d5620fe8992bf0d16b6c5e42be57d666ee794
        MCLI_SHA=b5392a642dbc9685b517ee901f032d1d5e88f41704436f0f08dde6060b577db3 ;;
    aarch64) ARCH=arm64
        SILO_SHA=f711c4d46d57c3b3fa365afab01150aab59b77c617f3d7987d77c6f9b6ea38b6
        MCLI_SHA=17f3eb694ed34e155b331b1215832140749656d8d4b60e2e3a4d0573b8f320d7 ;;
    *) echo "unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

if command -v silo >/dev/null 2>&1 && command -v mcli >/dev/null 2>&1 \
    && silo --version 2>/dev/null | grep -q "$RELEASE"; then
    echo "silo + mcli $RELEASE already installed"
    exit 0
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fetch() { # url sha out
    for i in 1 2 3; do
        curl -fsSL --retry 3 -o "$3" "$1" && break
        sleep $((i * 5))
    done
    echo "$2  $3" | sha256sum -c -
}
fetch "https://github.com/pgsty/silo/releases/download/$RELEASE/silo_${VER}_linux_${ARCH}.tar.gz" "$SILO_SHA" "$tmp/silo.tgz"
fetch "https://github.com/pgsty/mc/releases/download/$RELEASE/mcli_${VER}_linux_${ARCH}.tar.gz" "$MCLI_SHA" "$tmp/mcli.tgz"
tar -xzf "$tmp/silo.tgz" -C "$tmp" silo
tar -xzf "$tmp/mcli.tgz" -C "$tmp" mcli
SUDO=""
[ -w "$PREFIX" ] || SUDO=sudo
$SUDO install -m 0755 "$tmp/silo" "$tmp/mcli" "$PREFIX/"
silo --version | head -1
mcli --version | head -1
