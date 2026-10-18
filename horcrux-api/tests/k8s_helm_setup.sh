#!/bin/bash
# Real Kubernetes + Helm for the UI contract test's Helm section
# (horcrux-ui/tests/api_contract.rs: helm_pages_drive_a_real_cluster).
#
#  * installs the helm CLI (pinned release from get.helm.sh) if missing;
#  * packages the fixture chart (tests/fixtures/helm/horcrux-hello, one
#    ConfigMap, no images, so nothing is pulled) as versions 0.1.0 and 0.2.0
#    into a local chart repository served on 127.0.0.1:8879;
#  * starts a throwaway k3s server as a sibling container on this job
#    container's Docker network and writes a kubeconfig pointing at it;
#  * exports HORCRUX_CONTRACT_KUBECONFIG, HORCRUX_CONTRACT_HELM_REPO and
#    HORCRUX_CI_K3S (container name, for cleanup) to $GITHUB_ENV.
#
# Cleanup: horcrux-api/tests/k8s_helm_setup.sh --cleanup
set -euo pipefail
# A command that fails under `set -e` inside a $(...) exits the script with
# no output at all; name the line so a failed CI step says what broke.
trap 'rc=$?; echo "k8s_helm_setup.sh: line $LINENO exited $rc: $BASH_COMMAND" >&2' ERR

HERE=$(cd "$(dirname "$0")" && pwd)
HELM_VERSION=${HELM_VERSION:-v4.3.1}
K3S_IMAGE=${K3S_IMAGE:-rancher/k3s:v1.37.1-k3s1}
WORK=${WORK:-/tmp/horcrux-helm-ci}
REPO_PORT=${REPO_PORT:-8879}
HELM_INSTALL_DIR=${HELM_INSTALL_DIR:-/usr/local/bin}

if [ "${1:-}" = "--cleanup" ]; then
    [ -n "${HORCRUX_CI_K3S:-}" ] && docker rm -f "$HORCRUX_CI_K3S" >/dev/null 2>&1 || true
    [ -f "$WORK/http.pid" ] && kill "$(cat "$WORK/http.pid")" 2>/dev/null || true
    exit 0
fi

mkdir -p "$WORK"

# ---- helm -------------------------------------------------------------------
if ! command -v helm >/dev/null 2>&1 || ! helm version --short 2>/dev/null | grep -q "^${HELM_VERSION}"; then
    case "$(uname -m)" in
        x86_64) arch=amd64 ;;
        aarch64) arch=arm64 ;;
        *) echo "unsupported arch $(uname -m)" >&2; exit 1 ;;
    esac
    curl -fsSL "https://get.helm.sh/helm-${HELM_VERSION}-linux-${arch}.tar.gz" | tar xz -C "$WORK"
    if [ -w "$HELM_INSTALL_DIR" ]; then
        install -m 0755 "$WORK/linux-${arch}/helm" "$HELM_INSTALL_DIR/helm"
    else
        sudo install -m 0755 "$WORK/linux-${arch}/helm" "$HELM_INSTALL_DIR/helm"
    fi
fi
helm version --short

# ---- local chart repository ---------------------------------------------------
rm -rf "$WORK/repo" "$WORK/chart"
mkdir -p "$WORK/repo"
for v in 0.1.0 0.2.0; do
    rm -rf "$WORK/chart"
    cp -r "$HERE/fixtures/helm/horcrux-hello" "$WORK/chart"
    sed -i "s/^version: .*/version: $v/" "$WORK/chart/Chart.yaml"
    helm package "$WORK/chart" -d "$WORK/repo" >/dev/null
done
helm repo index "$WORK/repo" --url "http://127.0.0.1:${REPO_PORT}"
[ -f "$WORK/http.pid" ] && kill "$(cat "$WORK/http.pid")" 2>/dev/null || true
nohup python3 -m http.server "$REPO_PORT" --bind 127.0.0.1 --directory "$WORK/repo" \
    >"$WORK/http.log" 2>&1 &
echo $! >"$WORK/http.pid"
for i in $(seq 1 30); do
    curl -sf "http://127.0.0.1:${REPO_PORT}/index.yaml" >/dev/null && break
    sleep 1
done
curl -sf "http://127.0.0.1:${REPO_PORT}/index.yaml" >/dev/null

# ---- k3s ------------------------------------------------------------------
# The job container talks to the host's Docker daemon; put k3s on the same
# Docker network as this container so its API server is reachable directly.
net=${K3S_NETWORK:-}
if [ -z "$net" ]; then
    self_ips=$(hostname -I 2>/dev/null || true)
    for n in $(docker network ls --format '{{.Name}}'); do
        for ip in $(docker network inspect "$n" -f '{{range .Containers}}{{.IPv4Address}} {{end}}' 2>/dev/null); do
            for mine in $self_ips; do
                [ "${ip%/*}" = "$mine" ] && net=$n
            done
        done
    done
fi
[ -n "$net" ] || { echo "could not find this container's Docker network (set K3S_NETWORK)" >&2; exit 1; }

# A job that was killed (runner restart, host reboot, OOM) never reaches
# its `--cleanup` step and leaves its k3s running with 2 GB reserved.
# Such a leftover is the last container on its job's network (the job
# container is gone): remove those. k3s of live jobs share their network
# with that job's container and are left alone.
for c in $(docker ps -aq --filter name=horcrux-ci-k3s-); do
    cname=$(docker inspect -f '{{.Name}}' "$c" 2>/dev/null | tr -d /) || continue
    [ -n "$cname" ] || continue
    orphan=1
    for n in $(docker inspect -f '{{range $k, $v := .NetworkSettings.Networks}}{{$k}} {{end}}' "$c" 2>/dev/null || true); do
        # Another job's cleanup can remove this k3s and its network between
        # `docker ps` and here. Under pipefail a failed inspect inside the
        # pipeline used to end this script silently with exit 1 (Test job
        # 22567, "Start k3s" step). Can't inspect it: leave it alone; a real
        # leftover gets removed by a later job.
        if ! members=$(docker network inspect "$n" -f '{{range .Containers}}{{.Name}} {{end}}' 2>/dev/null); then
            orphan=0
            continue
        fi
        others=$(printf '%s\n' $members | { grep -v -x -e "$cname" -e '' || true; } | wc -l)
        [ "$others" -gt 0 ] && orphan=0
    done
    if [ "$orphan" = 1 ]; then
        echo "removing k3s left behind by a killed job: $cname"
        docker rm -f "$c" >/dev/null || true
    fi
done

name="horcrux-ci-k3s-$$"
docker run -d --name "$name" --privileged --network "$net" --memory 2g \
    --tmpfs /run --tmpfs /var/run \
    "$K3S_IMAGE" server \
    --disable=traefik,metrics-server,servicelb,local-storage \
    --write-kubeconfig-mode 644 >/dev/null
[ -n "${GITHUB_ENV:-}" ] && echo "HORCRUX_CI_K3S=$name" >>"$GITHUB_ENV"

ready=
for i in $(seq 1 90); do
    if docker exec "$name" kubectl get --raw /readyz >/dev/null 2>&1 &&
        docker exec "$name" kubectl -n default get serviceaccount default >/dev/null 2>&1; then
        ready=1
        break
    fi
    sleep 2
done
if [ -z "$ready" ]; then
    echo "k3s did not become ready" >&2
    docker logs --tail 80 "$name" >&2 || true
    exit 1
fi
ip=$(docker inspect "$name" -f "{{(index .NetworkSettings.Networks \"$net\").IPAddress}}")
docker exec "$name" cat /etc/rancher/k3s/k3s.yaml |
    sed "s#https://127.0.0.1:6443#https://${ip}:6443#" >"$WORK/kubeconfig"
chmod 600 "$WORK/kubeconfig"
KUBECONFIG="$WORK/kubeconfig" helm list -A >/dev/null
echo "k3s ($K3S_IMAGE) ready at https://${ip}:6443 on network $net; chart repo on :${REPO_PORT}"

if [ -n "${GITHUB_ENV:-}" ]; then
    {
        echo "HORCRUX_CONTRACT_KUBECONFIG=$WORK/kubeconfig"
        echo "HORCRUX_CONTRACT_HELM_REPO=http://127.0.0.1:${REPO_PORT}"
    } >>"$GITHUB_ENV"
fi
