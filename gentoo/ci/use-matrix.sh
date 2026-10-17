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
# exist (a USE-disabled feature must answer 404, an enabled one must not),
# that GET /api/features (what the web UI hides pages by) reports exactly
# the USE set, and that requests naming a backend that was not compiled in
# (a docker container, a qemu VM) are refused instead of reaching a CLI.
set -euo pipefail

EBUILD=/var/db/repos/horcrux-overlay/app-emulation/horcrux/horcrux-9999.ebuild
IMAGE=/var/tmp/portage/app-emulation/horcrux-9999/image
FAILS=0

fail() { echo "FAIL [$SET]: $*"; FAILS=$((FAILS + 1)); }
has() { [ -e "$IMAGE/$1" ] || fail "missing $1"; }
hasnt() { [ ! -e "$IMAGE/$1" ] || fail "should not install $1"; }

# Start the installed binary and send each request spec (authenticated as
# the bootstrap admin). A spec is "/path" (GET) or "METHOD /path [json]".
# Prints one "METHOD /path STATUS" line per spec, with " NOT_COMPILED"
# appended when the error says the backend is not compiled in, and one
# "FEATURES a b c" line with what GET /api/features (public) reports.
route_status() {
    local data
    data=$(mktemp -d)
    HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8099 HORCRUX_DATA_DIR="$data" \
    HORCRUX_DATABASE_URL="sqlite://$data/horcrux.db" ADMIN_PASSWORD=admin HORCRUX_SSL_DIR="$data/ssl" \
    HORCRUX_UI_DIR="$IMAGE/opt/horcrux/horcrux-ui" RUST_LOG=warn \
        "$IMAGE/usr/bin/horcrux-api" > "$data/api.log" 2>&1 &
    local pid=$!
    HX_SSL_DIR="$data/ssl" python3 - "$@" <<'PY' || { cat "$data/api.log"; kill $pid; return 1; }
import json, os, ssl, sys, time, urllib.request, urllib.error
# The API serves HTTPS from its node-local CA; plain HTTP is redirected.
base = "https://127.0.0.1:8099"
for _ in range(90):
    try:
        urllib.request.urlopen(base + "/api/health", timeout=2, context=ssl._create_unverified_context())
        break
    except Exception:
        time.sleep(1)
else:
    sys.exit("installed horcrux-api never became healthy")
# From here on, validate the certificate against the CA the node created.
ctx = ssl.create_default_context(cafile=os.path.join(os.environ["HX_SSL_DIR"], "ca.pem"))
https = urllib.request.build_opener(urllib.request.HTTPSHandler(context=ctx))
urllib.request.install_opener(https)
feats = json.load(urllib.request.urlopen(base + "/api/features"))
print("FEATURES " + " ".join(sorted(feats["features"])))
req = urllib.request.Request(base + "/api/auth/login",
    data=json.dumps({"username": "admin", "password": "admin"}).encode(),
    headers={"Content-Type": "application/json"})
token = json.load(urllib.request.urlopen(req))["ticket"]
for spec in sys.argv[1:]:
    parts = spec.split(" ", 2)
    if len(parts) == 1:
        method, path, body = "GET", parts[0], None
    else:
        method, path = parts[0], parts[1]
        body = parts[2].encode() if len(parts) > 2 else None
    r = urllib.request.Request(base + path, data=body, method=method,
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
    text = ""
    try:
        resp = urllib.request.urlopen(r, timeout=60)
        code = resp.status
    except urllib.error.HTTPError as e:
        code = e.code
        text = e.read().decode(errors="replace")
    print(method, path, code, "NOT_COMPILED" if "not compiled into this Horcrux build" in text else "")
PY
    kill $pid
    wait $pid 2>/dev/null || true
    rm -rf "$data"
}

# expect "<spec output line regex>" "<failure message>"
expect() { grep -qE "$1" <<<"$out" || fail "$2"; }

# features_are "<space-separated sorted list>": /api/features must report
# exactly the USE set's compiled features.
features_are() {
    local got
    got=$(sed -n 's/^FEATURES //p' <<<"$out")
    [ "$got" = "$1" ] || fail "/api/features reports [$got], expected [$1]"
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

# Requests every set sends; the expectations differ per USE set.
PROBES=(
    /api/nas/shares /api/k8s/clusters /api/containers /api/templates
    /api/backups /api/backup-jobs /api/cluster/status /api/cluster/nodes /api/ha/status
    /api/ha/groups "/api/images?runtime=podman" "/api/images?runtime=docker"
    /api/volumes /api/container-networks /api/stacks /api/lxc/host
    'POST /api/containers {"id":"um-ct","name":"um-ct","runtime":"docker","rootfs":"busybox:latest","memory":64,"cpus":1,"status":"stopped"}'
    'POST /api/vms {"id":"um-vm","name":"um-vm","hypervisor":"qemu","memory":64,"cpus":1,"disk_size":1,"status":"stopped","architecture":"x86_64"}'
    'POST /api/vms {"id":"um-vm2","name":"um-vm2","hypervisor":"incus","memory":64,"cpus":1,"disk_size":1,"status":"stopped","architecture":"x86_64"}'
)

# ---- 1. everything optional on, OpenRC ------------------------------------
run_set full "cli webui monitoring -systemd docker lxc podman lxd incus -qemu \
    kubernetes backup cluster nas smb nfs-server afp webdav ftp ldap ldap-server \
    kerberos ad timemachine s3-gateway iscsi-target rsync-server \
    nas-zfs nas-btrfs nas-mdraid nas-lvm pam"
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
out=$(route_status "${PROBES[@]}" /)
echo "$out"
features_are "ad afp backup cluster docker ftp incus iscsi-target kerberos kubernetes ldap ldap-server lxc lxd nas nas-auth nas-btrfs nas-lvm nas-mdraid nas-zfs nfs-server pam podman rsync-server s3-gateway smb timemachine webdav"
expect '^GET /api/nas/shares 200' "USE=nas: /api/nas/shares not served"
expect '^GET /api/k8s/clusters 200' "USE=kubernetes: /api/k8s/clusters not served"
expect '^GET /api/containers 200' "/api/containers not served"
expect '^GET /api/templates 200' "/api/templates not served"
expect '^GET /api/lxc/host 200' "USE=lxc: LXC routes (/api/lxc/host) not served"
expect '^GET /api/backups 200' "USE=backup: /api/backups not served"
expect '^GET /api/backup-jobs 200' "USE=backup: /api/backup-jobs not served"
expect '^GET /api/cluster/nodes 200' "USE=cluster: /api/cluster/nodes not served"
expect '^GET /api/ha/status 200' "USE=cluster: /api/ha/status not served"
expect '^GET /api/ha/groups 200' "USE=cluster: /api/ha/groups not served"
expect '^GET /api/stacks 200' "USE=docker: /api/stacks not served"
expect '^GET /api/images\?runtime=docker [^4]' "USE=docker: docker images refused"
expect '^GET /api/images\?runtime=podman [^4]' "USE=podman: podman images refused"
# -qemu: a QEMU VM is refused as not compiled in; incus is compiled in, so
# the refusal (no incus on the builder) must be the host check instead.
expect '^POST /api/vms 422 NOT_COMPILED' "USE=-qemu: QEMU VM create not refused as not compiled in"
grep -E '^POST /api/vms ' <<<"$out" | grep -vq NOT_COMPILED \
    || fail "USE=incus: incus VM create refused as not compiled in"
expect '^GET / 200' "USE=webui: UI index not served"

# ---- 2. minimal, systemd ---------------------------------------------------
run_set minimal "lxc systemd -docker -podman -qemu -webui -cli -monitoring \
    -kubernetes -nas -backup -cluster -pam"
has usr/bin/horcrux-api
hasnt usr/bin/horcrux
hasnt opt/horcrux/horcrux-ui
has usr/lib/systemd/system/horcrux.service
hasnt usr/lib/systemd/system/horcrux-monitoring.service
hasnt etc/init.d/horcrux
hasnt var/lib/horcrux/nas
out=$(route_status "${PROBES[@]}")
echo "$out"
features_are "lxc"
expect '^GET /api/lxc/host 200' "USE=lxc: LXC routes (/api/lxc/host) not served"
expect '^GET /api/nas/shares 404' "USE=-nas: NAS routes still compiled in"
expect '^GET /api/k8s/clusters 404' "USE=-kubernetes: k8s routes still compiled in"
expect '^GET /api/containers 200' "/api/containers not served"
expect '^GET /api/templates 200' "templates are not a USE flag but went missing"
for p in /api/backups /api/backup-jobs; do
    expect "^GET $p 404" "USE=-backup: $p still compiled in"
done
for p in /api/cluster/status /api/cluster/nodes /api/ha/status /api/ha/groups; do
    expect "^GET $p 404" "USE=-cluster: $p still compiled in"
done
for p in '/api/images\?runtime=podman' '/api/images\?runtime=docker' /api/volumes \
         /api/container-networks /api/stacks; do
    expect "^GET $p 404" "USE=-docker -podman: $p still compiled in"
done
expect '^POST /api/containers 422 NOT_COMPILED' "USE=-docker: docker container create not refused"
[ "$(grep -c '^POST /api/vms 422 NOT_COMPILED' <<<"$out")" = 2 ] \
    || fail "USE=-qemu -incus: VM creates not refused as not compiled in"

# ---- 3. podman + cluster, no docker/backup, OpenRC -------------------------
run_set podman-cluster "podman cluster -docker -lxc -lxd -incus -qemu -systemd \
    -webui -cli -monitoring -kubernetes -nas -backup -pam"
out=$(route_status "${PROBES[@]}")
echo "$out"
features_are "cluster podman"
expect '^GET /api/lxc/host 404' "USE=-lxc: LXC routes still compiled in"
expect '^GET /api/cluster/nodes 200' "USE=cluster: /api/cluster/nodes not served"
expect '^GET /api/ha/status 200' "USE=cluster: /api/ha/status not served"
expect '^GET /api/backups 404' "USE=-backup: /api/backups still compiled in"
expect '^GET /api/stacks 200' "USE=podman: /api/stacks not served"
expect '^GET /api/images\?runtime=docker 422 NOT_COMPILED' "USE=-docker: docker images not refused"
expect '^GET /api/images\?runtime=podman [^4]' "USE=podman: podman images refused"
expect '^POST /api/containers 422 NOT_COMPILED' "USE=-docker: docker container create not refused"

ebuild "$EBUILD" clean >/dev/null
if [ "$FAILS" -ne 0 ]; then
    echo "USE-flag matrix: $FAILS failure(s)"
    exit 1
fi
echo "USE-flag matrix: all checks passed"
