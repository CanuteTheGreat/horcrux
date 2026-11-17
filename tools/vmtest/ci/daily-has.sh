#!/bin/bash
# daily-has.sh STRING: exit 0 when the published daily manager tarball's contents (binary or UI) contain STRING.
# Lets a scenario for a brand-new feature say "the daily predates it" (and skip loudly) instead of failing on an old ISO.
set -o pipefail
url=https://github.com/CanuteTheGreat/horcrux/releases/download/daily/horcrux-manager-daily-x86_64.tar.zst
curl -fsSL --retry 3 "$url" | zstd -dc | grep -a -q -F -- "$1"
