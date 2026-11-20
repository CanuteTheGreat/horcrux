#!/usr/bin/env bash
# Build the Horcrux Windows guest drivers on Linux:
#   hxidd.dll  Horcrux virtual display (IddCx indirect display driver, UMDF 2)
#   hxpad.dll  Horcrux virtual gamepad (UMDF 2 HID minidriver)
# with clang-cl + lld-link against Microsoft's Windows Driver Kit and Windows
# SDK as published on nuget.org (pinned versions and sha256 below). The kits
# are downloaded at build time, used under their licence terms and never
# shipped; only the drivers built from our own sources are packaged.
# The catalogs are created and signed inside the guest at install time with
# a per-install Horcrux certificate (see src/windows/drivers.rs), so the
# packages here are unsigned.
#
# ARCH=x64 (default) or arm64 (Windows on Arm guests): the same sources and
# INFs, cross-compiled for the other architecture. The headers come from the
# x64 WDK package (they are architecture independent); the libraries from the
# architecture's own packages.
#
# Needs: clang (clang-cl driver mode), lld (lld-link), python3, curl, unzip.
# Env: ARCH (x64|arm64), OUT_DIR (default dist/drivers, dist/drivers-arm64),
#      KIT_DIR (default ~/.cache/horcrux-wdk),
#      CLANG (default clang), LLD_LINK (default lld-link), DRIVER_VERSION
#      (default <workspace version>.0).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
SRC="$ROOT/crates/horcrux-guest-tools/drivers/windows"
ARCH="${ARCH:-x64}"
case "$ARCH" in
  x64)   OUT_DIR="${OUT_DIR:-dist/drivers}"
         CL_TARGET=x86_64-pc-windows-msvc; MACHINE=x64; ARCH_DEFS=(/D_AMD64_ /DAMD64)
         WDK_LIBARCH=x64; SDK_LIBARCH=x64 ;;
  arm64) OUT_DIR="${OUT_DIR:-dist/drivers-arm64}"
         CL_TARGET=aarch64-pc-windows-msvc; MACHINE=arm64; ARCH_DEFS=(/D_ARM64_ /DARM64)
         WDK_LIBARCH=ARM64; SDK_LIBARCH=arm64 ;;
  *) echo "ARCH must be x64 or arm64, not $ARCH" >&2; exit 2 ;;
esac
KIT_DIR="${KIT_DIR:-$HOME/.cache/horcrux-wdk}"
CLANG="${CLANG:-clang}"
LLD_LINK="${LLD_LINK:-lld-link}"
VERSION="$(awk '/^\[workspace.package\]/{f=1;next} /^\[/{f=0} f&&/^version *=/{gsub(/[" ]/,"",$3);print $3;exit}' Cargo.toml)"
DRIVER_VERSION="${DRIVER_VERSION:-$VERSION.0}"

# name version sha256 (nuget.org flat container)
KITS="
microsoft.windows.wdk.x64 10.0.26100.6584 c393d03dfb640b5c92f546b32f6770ef68cd3aaf691956e7d66d8e2c28a1b55e
microsoft.windows.sdk.cpp 10.0.26100.7705 ff455660bee071a744715a9665666fa29c07681579d3d815959a9eec5a556424
microsoft.windows.sdk.cpp.x64 10.0.26100.7705 ad6ce90ff94010699d29548b3c2b1e52c89eb81143e2199155d9a5c38001b52b
"
if [ "$ARCH" = arm64 ]; then
  KITS="$KITS
microsoft.windows.wdk.arm64 10.0.26100.6584 e705b2a63eab891def8f98087666f93e8f21da8e3b5def81a624b83fef5bdae9
microsoft.windows.sdk.cpp.arm64 10.0.26100.7705 03bc0c03d439cef85074bbcd425f9d5d3a6d3b467967acd21da3ba6dfeeafa67
"
fi
mkdir -p "$KIT_DIR"
while read -r name ver sum; do
  [ -n "$name" ] || continue
  dir="$KIT_DIR/$name-$ver"
  [ -f "$dir/.ok" ] && continue
  pkg="$KIT_DIR/$name.$ver.nupkg"
  if ! { [ -f "$pkg" ] && echo "$sum  $pkg" | sha256sum -c --status; }; then
    curl -fsSL --retry 3 -o "$pkg" "https://api.nuget.org/v3-flatcontainer/$name/$ver/$name.$ver.nupkg"
    echo "$sum  $pkg" | sha256sum -c --quiet
  fi
  rm -rf "$dir"; mkdir -p "$dir"
  unzip -q -o "$pkg" -d "$dir"
  touch "$dir/.ok"
done <<< "$KITS"

WDK="$KIT_DIR/microsoft.windows.wdk.x64-10.0.26100.6584/c"
SDK="$KIT_DIR/microsoft.windows.sdk.cpp-10.0.26100.7705/c"
if [ "$ARCH" = arm64 ]; then
  WDKLIB="$KIT_DIR/microsoft.windows.wdk.arm64-10.0.26100.6584/c"
  SDKLIB="$KIT_DIR/microsoft.windows.sdk.cpp.arm64-10.0.26100.7705/c"
else
  WDKLIB="$WDK"
  SDKLIB="$KIT_DIR/microsoft.windows.sdk.cpp.x64-10.0.26100.7705/c"
fi
KV=10.0.26100.0
UMDF_MINOR=25   # UMDF 2.25 = Windows 10 1803+; matches UmdfLibraryVersion in the INFs
IDDCX_MINOR=4   # IddCx 1.4 = Windows 10 1903+ / Server 2022 / Windows 11
INC_DIRS=(
  "$WDK/Include/$KV/km/crt"
  "$SDK/Include/$KV/shared"
  "$SDK/Include/$KV/um"
  "$WDK/Include/$KV/shared"
  "$WDK/Include/$KV/um"
  "$WDK/Include/wdf/umdf/2.$UMDF_MINOR"
  "$WDK/Include/$KV/um/iddcx/1.$IDDCX_MINOR"
  "$SRC/include"
)
LIB_DIRS=(
  "$SDKLIB/um/$SDK_LIBARCH"
  "$WDKLIB/Lib/wdf/umdf/$WDK_LIBARCH/2.$UMDF_MINOR"
  "$WDKLIB/Lib/$KV/um/$WDK_LIBARCH/iddcx/1.$IDDCX_MINOR"
  "$WDKLIB/Lib/$KV/um/$WDK_LIBARCH"
)

# The kits are written for a case-insensitive file system: give every
# #include / library name that differs only in case a symlink.
CASE="$KIT_DIR/case-$KV-$ARCH"
python3 - "$CASE" "${INC_DIRS[@]}" -- "${LIB_DIRS[@]}" -- "$SRC" <<'PY'
import os, re, sys
case = sys.argv[1]
args = sys.argv[2:]
i = args.index('--'); inc = args[:i]; rest = args[i+1:]
j = rest.index('--'); libs = rest[:j]; srcs = rest[j+1:]
os.makedirs(case + '/inc', exist_ok=True); os.makedirs(case + '/lib', exist_ok=True)
def index(dirs):
    m = {}
    for d in dirs:
        for root, _, files in os.walk(d):
            for f in files:
                rel = os.path.relpath(os.path.join(root, f), d)
                m.setdefault(rel.lower(), os.path.join(root, f))
    return m
hdr = index(inc)
pat = re.compile(r'^\s*#\s*include\s*[<"]([^>"]+)[>"]', re.M)
wanted = set()
for d in inc + srcs:
    for root, _, files in os.walk(d):
        for f in files:
            if f.lower().endswith(('.h', '.hpp', '.inl', '.c', '.cpp', '')):
                try: t = open(os.path.join(root, f), encoding='latin-1').read()
                except OSError: continue
                wanted.update(n.replace('\\', '/') for n in pat.findall(t))
for n in wanted:
    if any(os.path.exists(os.path.join(d, n)) for d in inc): continue
    src = hdr.get(n.lower())
    if not src: continue
    dst = os.path.join(case, 'inc', n)
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    if not os.path.lexists(dst): os.symlink(src, dst)
for d in libs:
    for f in os.listdir(d):
        if f.lower().endswith('.lib') and f != f.lower():
            dst = os.path.join(case, 'lib', f.lower())
            if not os.path.lexists(dst): os.symlink(os.path.join(d, f), dst)
PY

CFLAGS=(--driver-mode=cl --target=$CL_TARGET -fms-compatibility-version=19.40
  /nologo /c /O2 /GS- /Zl /GR- /EHs-c- /W3 /Gy /Zc:wchar_t /std:c++17 /utf-8
  -Wno-ignored-pragma-intrinsic -Wno-pragma-pack -Wno-unknown-pragmas -Wno-microsoft-anon-tag
  -Wno-nonportable-include-path -Wno-duplicate-decl-specifier -Wno-ignored-attributes -Wno-microsoft-enum-forward-reference
  "${ARCH_DEFS[@]}" /D_WIN64 /DUNICODE /D_UNICODE /DWINVER=0x0A00 /D_WIN32_WINNT=0x0A00
  /DNTDDI_VERSION=0x0A000007 /DUMDF_VERSION_MAJOR=2 /DUMDF_VERSION_MINOR=$UMDF_MINOR
  /DIDDCX_VERSION_MAJOR=1 /DIDDCX_VERSION_MINOR=$IDDCX_MINOR /DHX_DRIVER_VERSION_STR="\"$DRIVER_VERSION\"")
# DRIVER_CFLAGS: extra compiler flags, e.g. -DHX_TRACE for a debug trace build
# shellcheck disable=SC2206
CFLAGS+=(${DRIVER_CFLAGS:-})
for d in "${INC_DIRS[@]}" "$CASE/inc"; do CFLAGS+=("-imsvc$d"); done
LFLAGS=(/nologo /dll /machine:$MACHINE /subsystem:windows,10.0 /noentry /nodefaultlib /opt:ref /opt:icf
  /guard:no /stack:0x100000,0x100000 /dynamicbase /nxcompat /export:FxDriverEntryUm)
for d in "${LIB_DIRS[@]}" "$CASE/lib"; do LFLAGS+=("/libpath:$d"); done

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
mkdir -p "$OUT_DIR"
OUT="$(cd "$OUT_DIR" && pwd)"

# Source files go after "--": in cl driver mode an absolute Linux path is
# parsed as an option when it starts like one (a checkout under /workspace
# reads as "/wo<n>" and leaves clang with "no input files").
build() { # name libs...
  local name="$1"; shift
  "$CLANG" "${CFLAGS[@]}" "/Fo$WORK/$name.obj" -- "$SRC/$name/$name.cpp"
  "$CLANG" "${CFLAGS[@]}" "/Fo$WORK/${name}_rt.obj" -- "$SRC/common/hxrt.cpp"
  "$LLD_LINK" "${LFLAGS[@]}" "/out:$WORK/$name.dll" "$WORK/$name.obj" "$WORK/${name}_rt.obj" \
    WdfDriverStubUm.lib "$@" kernel32.lib advapi32.lib ntdll.lib
  mkdir -p "$OUT/$name"
  cp "$WORK/$name.dll" "$OUT/$name/"
  sed "s/@DRIVERVER@/$DRIVER_VERSION/" "$SRC/$name/$name.inf" > "$OUT/$name/$name.inf"
  echo "built $OUT/$name [$ARCH] ($(stat -c %s "$OUT/$name/$name.dll") bytes, version $DRIVER_VERSION)"
}
build hxidd iddcxstub.lib avrt.lib dxgi.lib d3d11.lib
if [ -f "$SRC/hxpad/hxpad.cpp" ]; then build hxpad; fi
