#!/bin/sh
# Vendor tools/vmtest into another repo (patronus): sync-to.sh /path/to/patronus   (copies code, never scenarios of the other product)
set -eu
dst=${1:?usage: sync-to.sh /path/to/other/repo}
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$dst/tools/vmtest"
rsync -a --delete --exclude scenarios --exclude __pycache__ "$here/" "$dst/tools/vmtest/"
mkdir -p "$dst/tools/vmtest/scenarios"
cp "$here/scenarios/selftest_alpine.py" "$dst/tools/vmtest/scenarios/"
echo "vendored vmtest $(grep -h __version__ "$here/vmtest/__init__.py") into $dst/tools/vmtest (commit it; scenarios/ is yours)"
