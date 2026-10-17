#!/bin/bash
# USE-flag matrix for the Horcrux ebuild, run inside the Gentoo builder
# image (Dockerfile target `use-matrix`, CI job "Gentoo USE-flag matrix").
#
# The main Gentoo Docker Build job emerges one fixed USE set. USE flags are
# Horcrux's selling point, so this runs the real ebuild phases
# (src_unpack .. src_install, via `ebuild ... install`) for contrasting USE
# sets and checks that each flag really changes what gets built and
# installed: binaries, web UI bundle, init system files, NAS directories,
# and -- by starting the installed horcrux-api -- which API feature routes
# exist (a USE-disabled feature must answer 404, an enabled one must not).
set -euo pipefail

EBUILD=/var/db/repos/horcrux-overlay/app-emulation/horcrux/horcrux-9999.ebuild
IMAGE=/var/tmp/portage/app-emulation/horcrux-9999/image
FAILS=0

fail() { echo "FAIL [$SET]: $*"; FAILS=$((FAILS + 1)); }
has() { [ -e "$IMAGE/$1" ] || fail "missing $1"; }
hasnt() { [ ! -e "$IMAGE/$1" ] || fail "should not install $1"; }

# Start the installed binary and report the HTTP status of each path
# (authenticated as the bootstrap admin), one "path status" per line.
route_status() {
    local data
    data=$(mktemp -d)
    HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8099 HORCRUX_DATA_DIR="$data" \
    HORCRUX_DATABASE_URL="sqlite://$data/horcrux.db" ADMIN_PASSWORD=admin \
    HORCRUX_UI_DIR="$IMAGE/opt/horcrux/horcrux-ui" RUST_LOG=warn \
        "$IMAGE/usr/bin/horcrux-api" > "$data/api.log" 2>&1 &
    local pid=$!
    python3 - "$@" <<'PY' || { cat "$data/api.log"; kill $pid; return 1; }
import json, sys, time, urllib.request, urllib.error
base = "http://127.0.0.1:8099"
for _ in range(90):
    try:
        urllib.request.urlopen(base + "/api/health", timeout=2)
        break
    except Exception:
        time.sleep(1)
else:
    sys.exit("installed horcrux-api never became healthy")
req = urllib.request.Request(base + "/api/auth/login",
    data=json.dumps({"username": "admin", "password": "admin"}).encode(),
    headers={"Content-Type": "application/json"})
token = json.load(urllib.request.urlopen(req))["ticket"]
for path in sys.argv[1:]:
    r = urllib.request.Request(base + path, headers={"Authorization": "Bearer " + token})
    try:
        code = urllib.request.urlopen(r).status
    except urllib.error.HTTPError as e:
        code = e.code
    print(path, code)
PY
    kill $pid
    wait $pid 2>/dev/null || true
    rm -rf "$data"
}

run_set() {
    SET=$1
    shift
    echo "=== USE set '$SET': $* ==="
    ebuild "$EBUILD" clean >/dev/null
    # USE=debug (cargo.eclass) = unoptimized cargo build: this job checks
    # what each flag compiles in and installs, not codegen, and the
    # release profile (LTO, codegen-units=1) would triple its runtime.
    USE="debug $*" ebuild "$EBUILD" install
}

# ---- 1. everything optional on, OpenRC ------------------------------------
run_set full "cli webui monitoring -systemd docker lxc podman lxd incus -qemu \
    kubernetes backup cluster nas smb nfs-server afp webdav ftp ldap ldap-server \
    kerberos ad timemachine s3-gateway iscsi-target rsync-server \
    nas-zfs nas-btrfs nas-mdraid nas-lvm"
has usr/bin/horcrux-api
has usr/bin/horcrux
has opt/horcrux/horcrux-ui/index.html
ls "$IMAGE"/opt/horcrux/horcrux-ui/*.wasm >/dev/null 2>&1 || fail "web UI bundle has no .wasm"
has opt/horcrux/horcrux-ui/favicon.ico
has etc/init.d/horcrux
has etc/init.d/horcrux-monitoring
hasnt usr/lib/systemd/system/horcrux.service
has etc/horcrux/config.toml
has var/lib/horcrux/nas/shares
has var/lib/horcrux/nas/s3
has var/lib/horcrux/nas/iscsi
"$IMAGE/usr/bin/horcrux" --help >/dev/null || fail "installed CLI does not run"
out=$(route_status /api/nas/shares /api/k8s/clusters /api/containers /)
echo "$out"
grep -q '^/api/nas/shares 200$' <<<"$out" || fail "USE=nas: /api/nas/shares not served"
grep -q '^/api/k8s/clusters 200$' <<<"$out" || fail "USE=kubernetes: /api/k8s/clusters not served"
grep -q '^/api/containers 200$' <<<"$out" || fail "/api/containers not served"
grep -q '^/ 200$' <<<"$out" || fail "USE=webui: UI index not served"

# ---- 2. minimal, systemd ---------------------------------------------------
run_set minimal "lxc systemd -docker -podman -qemu -webui -cli -monitoring \
    -kubernetes -nas -backup -cluster"
has usr/bin/horcrux-api
hasnt usr/bin/horcrux
hasnt opt/horcrux/horcrux-ui
has usr/lib/systemd/system/horcrux.service
hasnt usr/lib/systemd/system/horcrux-monitoring.service
hasnt etc/init.d/horcrux
hasnt var/lib/horcrux/nas
out=$(route_status /api/nas/shares /api/k8s/clusters /api/containers)
echo "$out"
grep -q '^/api/nas/shares 404$' <<<"$out" || fail "USE=-nas: NAS routes still compiled in"
grep -q '^/api/k8s/clusters 404$' <<<"$out" || fail "USE=-kubernetes: k8s routes still compiled in"
grep -q '^/api/containers 200$' <<<"$out" || fail "/api/containers not served"

ebuild "$EBUILD" clean >/dev/null
if [ "$FAILS" -ne 0 ]; then
    echo "USE-flag matrix: $FAILS failure(s)"
    exit 1
fi
echo "USE-flag matrix: all checks passed"
