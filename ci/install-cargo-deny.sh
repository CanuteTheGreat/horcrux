#!/usr/bin/env bash
# Install the pinned cargo-deny release binary (checksum-verified) into $DEST (default /usr/local/bin).
# A prebuilt musl binary instead of `cargo install`: ~2 s instead of ~4 min of compiling per run.
#
# Bump deliberately: change CARGO_DENY_VERSION and both checksums together (sha256 of
# cargo-deny-<version>-<arch>-unknown-linux-musl.tar.gz from the GitHub release page).
set -euo pipefail

: "${CARGO_DENY_VERSION:=0.21.0}"
: "${CARGO_DENY_SHA256_X86_64:=a2170b854ff697e79e39c4c538981c9c1bdd803addbae6719a4cb66b4585a903}"
: "${CARGO_DENY_SHA256_AARCH64:=47cc5d88d43cd568028080fece02d729f56f0ff8e6c168563ce6589e3c0f4738}"
: "${CARGO_DENY_BASE_URL:=https://github.com/EmbarkStudios/cargo-deny/releases/download}"
: "${DEST:=/usr/local/bin}"

case "$(uname -m)" in
    x86_64) arch=x86_64; want="$CARGO_DENY_SHA256_X86_64" ;;
    aarch64 | arm64) arch=aarch64; want="$CARGO_DENY_SHA256_AARCH64" ;;
    *) echo "no prebuilt cargo-deny for $(uname -m)" >&2; exit 2 ;;
esac
name="cargo-deny-${CARGO_DENY_VERSION}-${arch}-unknown-linux-musl"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for attempt in 1 2 3; do
    curl -fsSL -o "$tmp/cd.tgz" "${CARGO_DENY_BASE_URL}/${CARGO_DENY_VERSION}/${name}.tar.gz" && break
    [ "$attempt" = 3 ] && { echo "download of $name failed" >&2; exit 1; }
    sleep $((attempt * 5))
done
got="$(sha256sum "$tmp/cd.tgz" | cut -d' ' -f1)"
[ "$got" = "$want" ] || { echo "checksum mismatch for $name: $got, expected $want" >&2; exit 1; }
tar -xzf "$tmp/cd.tgz" -C "$tmp"
if [ -w "$DEST" ]; then install -m 0755 "$tmp/$name/cargo-deny" "$DEST/cargo-deny"
else sudo install -m 0755 "$tmp/$name/cargo-deny" "$DEST/cargo-deny"; fi
cargo-deny --version
