#!/usr/bin/env bash
# Install the pinned llvm-mingw toolchain (clang + lld + mingw-w64 runtime for
# aarch64/x86_64 Windows, Apache-2.0 WITH LLVM-exception; used only as the
# cross linker for the Windows on Arm guest agent, never shipped).
# Prints the toolchain's bin directory on stdout.
# Env: LLVM_MINGW_DIR (default ~/.cache/horcrux-llvm-mingw)
set -euo pipefail
VER=20261117
SUM=e7a15a49a2ea95e9f3d8a9d0cfabe5052a4b10fb4777cc54de5a8c27b64bb701
NAME="llvm-mingw-$VER-ucrt-ubuntu-22.04-x86_64"
DIR="${LLVM_MINGW_DIR:-$HOME/.cache/horcrux-llvm-mingw}"
if [ ! -x "$DIR/$NAME/bin/aarch64-w64-mingw32-clang" ]; then
  mkdir -p "$DIR"
  tgz="$DIR/$NAME.tar.xz"
  curl -fsSL --retry 3 -o "$tgz" "https://github.com/mstorsjo/llvm-mingw/releases/download/$VER/$NAME.tar.xz"
  echo "$SUM  $tgz" | sha256sum -c --quiet
  tar -C "$DIR" -xf "$tgz"
  rm -f "$tgz"
fi
echo "$DIR/$NAME/bin"
