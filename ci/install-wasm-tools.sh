#!/usr/bin/env bash
# Install the web UI build tools (trunk, wasm-bindgen, wasm-opt) before `trunk build`, so trunk
# finds matching binaries on PATH and never downloads them itself in the middle of a build.
# trunk's own download has no retry: one reset connection to GitHub mid-archive failed a whole
# Build Web UI job ("running wasm-opt: failed downloading release archive ... Connection reset by
# peer"). Here every download is retried, checked against the release's published sha256 and
# unpacked before anything is installed. Run `trunk build --offline` afterwards to prove it.
#
# Versions are not pinned in this script:
#   wasm-bindgen = the version in Cargo.lock (trunk requires CLI == library version),
#   wasm-opt     = [tools] wasm_opt in horcrux-api/horcrux-ui/Trunk.toml,
#   trunk        = $TRUNK_VERSION (default: latest release; skipped when trunk is installed).
# Every value can be overridden through the environment (WASM_BINDGEN_VERSION, WASM_OPT_VERSION,
# *_BASE_URL, DEST, CARGO_LOCK, TRUNK_TOML).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
: "${CARGO_LOCK:=$root/Cargo.lock}"
: "${TRUNK_TOML:=$root/horcrux-api/horcrux-ui/Trunk.toml}"
: "${DEST:=/usr/local/bin}"
: "${TOOLS_DIR:=/opt/horcrux-wasm-tools}"
: "${TRUNK_VERSION:=latest}"
: "${TRUNK_BASE_URL:=https://github.com/trunk-rs/trunk/releases}"
: "${WASM_BINDGEN_BASE_URL:=https://github.com/wasm-bindgen/wasm-bindgen/releases/download}"
: "${WASM_OPT_BASE_URL:=https://github.com/WebAssembly/binaryen/releases/download}"
: "${DOWNLOAD_ATTEMPTS:=5}"

if [ -z "${WASM_BINDGEN_VERSION:-}" ]; then
    WASM_BINDGEN_VERSION="$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/version = |"/, ""); print; exit }' "$CARGO_LOCK")"
fi
if [ -z "${WASM_OPT_VERSION:-}" ]; then
    WASM_OPT_VERSION="$(awk -F'"' '/^\[tools\]/ { t = 1; next } /^\[/ { t = 0 } t && $1 ~ /^wasm_opt *= *$/ { print $2; exit }' "$TRUNK_TOML")"
fi
[ -n "$WASM_BINDGEN_VERSION" ] || { echo "no wasm-bindgen version in $CARGO_LOCK" >&2; exit 2; }
[ -n "$WASM_OPT_VERSION" ] || { echo "no [tools] wasm_opt in $TRUNK_TOML" >&2; exit 2; }

case "$(uname -m)" in
    x86_64) trunk_triple=x86_64-unknown-linux-gnu; wb_triple=x86_64-unknown-linux-musl; bin_arch=x86_64 ;;
    aarch64 | arm64) trunk_triple=aarch64-unknown-linux-gnu; wb_triple=aarch64-unknown-linux-gnu; bin_arch=aarch64 ;;
    *) echo "no prebuilt wasm tools for $(uname -m)" >&2; exit 2 ;;
esac

as_root() { if [ -w "$1" ] || { [ ! -e "$1" ] && [ -w "$(dirname "$1")" ]; }; then shift; "$@"; else shift; sudo "$@"; fi; }
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# fetch URL FILE: download with retries; a partial or reset transfer is retried, never kept
fetch() {
    local attempt
    for attempt in $(seq 1 "$DOWNLOAD_ATTEMPTS"); do
        rm -f "$2"
        curl -fsSL --connect-timeout 20 --max-time 300 -o "$2" "$1" && return 0
        echo "download attempt $attempt of $1 failed" >&2
        sleep $((attempt * 5))
    done
    echo "download of $1 failed after $DOWNLOAD_ATTEMPTS attempts" >&2
    return 1
}

# fetch_checked URL: download URL and URL.SUMEXT, compare sha256; retried as one unit
fetch_checked() {
    local url=$1 sumext=$2 out=$3 attempt want got
    for attempt in $(seq 1 "$DOWNLOAD_ATTEMPTS"); do
        fetch "$url" "$out" && fetch "$url$sumext" "$out.sum" || return 1
        want="$(cut -d' ' -f1 "$out.sum")"
        got="$(sha256sum "$out" | cut -d' ' -f1)"
        [ "$got" = "$want" ] && tar -tzf "$out" >/dev/null && return 0
        echo "checksum/archive check failed for $url (got $got, expected $want), retrying" >&2
    done
    return 1
}

if ! command -v trunk >/dev/null 2>&1; then
    if [ "$TRUNK_VERSION" = latest ]; then turl="$TRUNK_BASE_URL/latest/download/trunk-$trunk_triple.tar.gz"
    else turl="$TRUNK_BASE_URL/download/$TRUNK_VERSION/trunk-$trunk_triple.tar.gz"; fi
    for attempt in $(seq 1 "$DOWNLOAD_ATTEMPTS"); do
        fetch "$turl" "$tmp/trunk.tgz" && tar -xzf "$tmp/trunk.tgz" -C "$tmp" trunk && break
        [ "$attempt" = "$DOWNLOAD_ATTEMPTS" ] && { echo "trunk install failed" >&2; exit 1; }
    done
    as_root "$DEST" install -m 0755 "$tmp/trunk" "$DEST/trunk"
fi
trunk --version

if ! wasm-bindgen --version 2>/dev/null | grep -qx "wasm-bindgen $WASM_BINDGEN_VERSION"; then
    name="wasm-bindgen-$WASM_BINDGEN_VERSION-$wb_triple"
    fetch_checked "$WASM_BINDGEN_BASE_URL/$WASM_BINDGEN_VERSION/$name.tar.gz" .sha256sum "$tmp/wb.tgz"
    tar -xzf "$tmp/wb.tgz" -C "$tmp"
    as_root "$DEST" install -m 0755 "$tmp/$name/wasm-bindgen" "$DEST/wasm-bindgen"
fi
wasm-bindgen --version

opt_num="${WASM_OPT_VERSION#version_}"
if ! wasm-opt --version 2>/dev/null | grep -q "^wasm-opt version $opt_num\b"; then
    name="binaryen-$WASM_OPT_VERSION"
    fetch_checked "$WASM_OPT_BASE_URL/$WASM_OPT_VERSION/$name-$bin_arch-linux.tar.gz" .sha256 "$tmp/bn.tgz"
    tar -xzf "$tmp/bn.tgz" -C "$tmp"
    as_root "$(dirname "$TOOLS_DIR")" mkdir -p "$TOOLS_DIR"
    as_root "$TOOLS_DIR" rm -rf "$TOOLS_DIR/$name"
    as_root "$TOOLS_DIR" cp -a "$tmp/$name" "$TOOLS_DIR/$name"
    as_root "$DEST" ln -sf "$TOOLS_DIR/$name/bin/wasm-opt" "$DEST/wasm-opt"
fi
wasm-opt --version
