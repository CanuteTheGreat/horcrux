#!/bin/bash
# Container + stack tests against real Podman (the preferred OCI runtime).
#
# The Test job's own container tests drive the runner host's Docker daemon.
# This runs the same container/stack suites against Podman instead: the built
# horcrux-api and the `it` test binary go into a throwaway privileged sibling
# container (its own cgroup/net/pid namespaces; nothing on the runner host is
# touched) that has podman installed and NO docker, so it also proves the
# runtime preference: requests that name no runtime must land on Podman even
# though the server is built with both USE=docker and USE=podman.
#
# Usage: horcrux-api/tests/podman_e2e.sh <horcrux-api binary> <it test binary> [image]
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/sibling-apt.sh"
BIN=${1:?usage: $0 <horcrux-api binary> <it test binary> [image]}
TESTBIN=${2:?usage: $0 <horcrux-api binary> <it test binary> [image]}
IMAGE=${3:-catthehacker/ubuntu:act-26.04}
NAME="horcrux-podman-e2e-${HX_RUN_TAG:-$$}"

cleanup() {
  rc=$?
  if [ $rc -ne 0 ]; then
    echo "::group::horcrux-api log"; docker exec "$NAME" tail -120 /tmp/pm/api.log 2>/dev/null || true; echo "::endgroup::"
    echo "::group::podman ps -a"; docker exec "$NAME" podman ps -a 2>&1 || true; echo "::endgroup::"
  fi
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  exit $rc
}
trap cleanup EXIT

docker run -d --name "$NAME" --privileged --cgroupns=private --memory 3g --memory-swap 3g "$IMAGE" sleep 3000 >/dev/null
hx_sibling_apt_prep "$NAME"
# Never let this container change host-global sysctls (see ci/sysctl-guard.sh).
bash "$(dirname "$0")/../../ci/sysctl-guard.sh" harden "$NAME"
# cgroup v2 nesting (what docker:dind's entrypoint does): the sibling's cgroup namespace root
# holds its processes, and the no-internal-processes rule then forbids enabling domain
# controllers (memory, io) for child cgroups, so crun fails every limited container with
# "opening file memory.max for writing: No such file or directory". Move every process into
# the leaf /init and delegate the controllers BEFORE podman first runs: podman's own probe
# otherwise turns the root "domain threaded", after which memory/io can never be enabled.
docker exec "$NAME" sh -c '
set -e
cg=/sys/fs/cgroup
[ -f $cg/cgroup.controllers ] || { echo "cgroup v1 host: nothing to delegate"; exit 0; }
echo "sibling cgroup: $(cat /proc/self/cgroup) type=$(cat $cg/cgroup.type)"
echo "available controllers: $(cat $cg/cgroup.controllers)"
mkdir -p $cg/init
for i in 1 2 3 4 5; do
  for p in $(cat $cg/cgroup.procs); do echo "$p" > $cg/init/cgroup.procs 2>/dev/null || :; done
  [ -z "$(cat $cg/cgroup.procs)" ] && break
done
for c in memory pids cpu io; do
  grep -qw "$c" $cg/cgroup.controllers || { echo "controller $c not available to the sibling"; exit 1; }
  echo "+$c" > $cg/cgroup.subtree_control
done
echo "delegated (cgroup.subtree_control): $(cat $cg/cgroup.subtree_control)"
for c in memory pids cpu io; do
  grep -qw "$c" $cg/cgroup.subtree_control || { echo "controller $c NOT delegated"; exit 1; }
done
'
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$TESTBIN" "$NAME":/usr/local/bin/it
docker exec "$NAME" bash -c '
set -e
for i in 1 2 3; do apt-get update -qq && break; sleep 5; done
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq podman netavark aardvark-dns crun iproute2 iputils-ping \
  curl libvirt0 libpam0g >/dev/null
# the CI image ships a docker CLI; drop it so nothing can silently fall back to Docker
for d in $(type -ap docker 2>/dev/null); do rm -f "$d"; done
hash -r
command -v docker >/dev/null 2>&1 && { echo "docker CLI still present in the podman sibling"; exit 1; }
mkdir -p /etc/containers/registries.conf.d
echo "unqualified-search-registries = [\"docker.io\"]
short-name-mode = \"permissive\"" > /etc/containers/registries.conf.d/00-hx.conf
# a nested rootful podman: vfs graph driver (no overlayfs-on-overlayfs), no systemd
cat > /etc/containers/storage.conf <<EOS
[storage]
driver = "vfs"
runroot = "/run/containers/storage"
graphroot = "/var/lib/containers/storage"
EOS
cat > /etc/containers/containers.conf <<EOS
[engine]
cgroup_manager = "cgroupfs"
events_logger = "file"
[containers]
cgroupns = "host"
EOS
podman --version
podman pull -q docker.io/library/busybox:latest
'

docker exec "$NAME" bash -c '
set -euo pipefail
mkdir -p /tmp/pm /var/lib/horcrux/backups /var/lib/horcrux/storage/snapshot-test && cd /tmp/pm
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/tmp/pm \
HORCRUX_DATABASE_URL=sqlite:///tmp/pm/horcrux.db HORCRUX_SSL_DIR=/tmp/pm/ssl \
ADMIN_PASSWORD=admin RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000 RUST_LOG=info \
  nohup horcrux-api > /tmp/pm/api.log 2>&1 &
for i in $(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:8006/api/health >/dev/null || { echo "API did not start"; exit 1; }
TOKEN=$(curl -skf -H "Content-Type: application/json" -d "{\"username\":\"admin\",\"password\":\"admin\"}" \
  https://127.0.0.1:8006/api/auth/login | sed -E "s/.*\"ticket\":\"([^\"]+)\".*/\1/")
# no runtime named: the preferred one (podman) is used. With docker absent a docker default would be 424.
code=$(curl -sk -o /tmp/pm/images.out -w "%{http_code}" -H "Authorization: Bearer $TOKEN" https://127.0.0.1:8006/api/images)
[ "$code" = 200 ] || { echo "GET /api/images (default runtime) -> $code: $(cat /tmp/pm/images.out)"; exit 1; }
grep -q busybox /tmp/pm/images.out || { echo "default runtime did not list podman images:"; cat /tmp/pm/images.out; exit 1; }
echo "ok - default OCI runtime is podman"
cd /tmp
export HORCRUX_TEST_OCI=podman HORCRUX_TEST_REQUIRE_DOCKER=1
it --test-threads=1 --exact \
  integration_tests::test_container_lifecycle \
  integration_tests::test_stack_networking_and_options \
  integration_tests::test_stack_stop_is_concurrent \
  integration_tests::test_failed_stack_deploy_rolls_back \
  integration_tests::test_stack_update_rolling_keeps_volumes
'
