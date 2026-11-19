#!/usr/bin/env bash
# Install the pinned cargo-audit release binary (checksum-verified) into $DEST (default /usr/local/bin).
# A prebuilt binary instead of `cargo install cargo-audit`: that compiled ~400 crates on every run and
# on a cold linux-any runner overran the job's 20 min limit (run 3955: 'context deadline exceeded'
# while still compiling).
#
# Bump deliberately: change CARGO_AUDIT_VERSION and both checksums together (sha256 of
# cargo-audit-<triple>-v<version>.tgz from the rustsec GitHub release page, tag cargo-audit/v<version>).
set -euo pipefail

: "${CARGO_AUDIT_VERSION:=0.23.0}"
: "${CARGO_AUDIT_SHA256_X86_64:=4f01c74ed84b2dffd0b1d37012a62cdf2a92c96e25bdf8eec7fcc0d4d3eb7811}"
: "${CARGO_AUDIT_SHA256_AARCH64:=00eff3158429b9629abe8748e971f29f99ee53d4cff54a25072cbb15167bdda8}"
: "${CARGO_AUDIT_BASE_URL:=https://github.com/rustsec/rustsec/releases/download}"
: "${DEST:=/usr/local/bin}"

case "$(uname -m)" in
    x86_64) triple=x86_64-unknown-linux-musl; want="$CARGO_AUDIT_SHA256_X86_64" ;;
    aarch64 | arm64) triple=aarch64-unknown-linux-gnu; want="$CARGO_AUDIT_SHA256_AARCH64" ;;
    *) echo "no prebuilt cargo-audit for $(uname -m)" >&2; exit 2 ;;
esac
name="cargo-audit-${triple}-v${CARGO_AUDIT_VERSION}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for attempt in 1 2 3 4; do
    curl -fsSL -o "$tmp/ca.tgz" "${CARGO_AUDIT_BASE_URL}/cargo-audit%2Fv${CARGO_AUDIT_VERSION}/${name}.tgz" && break
    [ "$attempt" = 4 ] && { echo "download of $name failed" >&2; exit 1; }
    sleep $((attempt * 5))
done
got="$(sha256sum "$tmp/ca.tgz" | cut -d' ' -f1)"
[ "$got" = "$want" ] || { echo "checksum mismatch for $name: $got, expected $want" >&2; exit 1; }
tar -xzf "$tmp/ca.tgz" -C "$tmp"
if [ -w "$DEST" ]; then install -m 0755 "$tmp/$name/cargo-audit" "$DEST/cargo-audit"
else sudo install -m 0755 "$tmp/$name/cargo-audit" "$DEST/cargo-audit"; fi
cargo-audit --version
