#!/usr/bin/env bash
# Fetch the unmodified ViGEmBus setup (Nefarius Software Solutions, BSD-3-Clause,
# final upstream release 1.22.0, Authenticode/Microsoft production-signed
# x64+x86+arm64 bundle) and its licence text for the guest tools' OPTIONAL
# XInput component. Pinned by sha256; redistributed unchanged with its licence
# as BSD-3-Clause requires. Prints the directory holding the files.
# Env: VIGEM_DIR (default ~/.cache/horcrux-vigem)
set -euo pipefail
DIR="${VIGEM_DIR:-$HOME/.cache/horcrux-vigem}"
EXE=ViGEmBus_1.22.0_x64_x86_arm64.exe
EXE_SUM=89220a7865076b342892f98865f3499fb7c4cfd673159e89d352c360fd014c6a
LIC_SUM=3959f6ca50466675190590dfd024585744b6456ae7d39a3222e6550d62882586
get() { # file url sum
  if ! { [ -f "$DIR/$1" ] && echo "$3  $DIR/$1" | sha256sum -c --status; }; then
    mkdir -p "$DIR"
    curl -fsSL --retry 3 -o "$DIR/$1" "$2"
    echo "$3  $DIR/$1" | sha256sum -c --quiet
  fi
}
get "$EXE" "https://github.com/nefarius/ViGEmBus/releases/download/v1.22.0/$EXE" "$EXE_SUM"
get ViGEmBus-LICENSE.txt "https://raw.githubusercontent.com/nefarius/ViGEmBus/v1.22.0/LICENSE" "$LIC_SUM"
cat > "$DIR/README-ViGEmBus.txt" <<'TXT'
ViGEmBus 1.22.0 - third-party component, NOT part of Horcrux and NOT installed
by default.

What it is: a Windows kernel driver (BSD-3-Clause, see ViGEmBus-LICENSE.txt)
by Nefarius Software Solutions that presents a virtual Xbox 360 controller.
It is distributed as a production-signed release by its author; check the
setup's digital signature (file properties) if you want to see who vouches
for it. The upstream project is retired (archived 2023); no further updates
are to be expected.

Why it is here: XInput-only games accept only Xbox-class controllers, and
Horcrux has no Microsoft driver signature to ship such a driver itself. The
Horcrux virtual gamepad (HID) works for DirectInput, SDL and
Windows.Gaming.Input games without this.

Install it only if you need XInput:  horcrux-guest-agent --drivers install-xinput
Remove it again:                     horcrux-guest-agent --drivers remove-xinput
The agent checks this file's SHA-256 and its Authenticode signature before it
runs it. Some anti-cheat systems object to virtual-controller drivers; check
the rules of the games you play before enabling it.
TXT
echo "$DIR"
