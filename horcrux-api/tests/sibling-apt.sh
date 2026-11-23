# Sourced by the e2e scripts that start sibling test containers.
#
# hx_sibling_apt_prep CONTAINER...
#   Leaves only the distribution's own archive in the container's APT sources
#   and makes every later `apt-get update` / `apt-get install` retry transient
#   mirror errors.
#
# Why: the catthehacker runner image ships third-party repositories (a
# Launchpad git PPA, Microsoft, git-lfs) that the tests never install from.
# When one of them answers 404/503 (the git PPA does both for new Ubuntu
# releases), `apt-get update` exits 100 and the whole e2e step fails before it
# tests anything, although the distro archive was fine.
hx_sibling_apt_prep() {
    local c helper=""
    # The build-cache hub serves one apt helper for every repo (Ubuntu sources -> its failover proxy, else a mirror
    # that answers). Fetched once here (bare ubuntu images have no curl) and run inside each container; fail-open.
    if [ -z "${RW_APT_OFF:-}" ]; then
        helper=$(mktemp)
        curl -fsS -m 5 "http://${RW_CACHE_HUB:-10.1.10.103:8088}/ci/apt-mirror.sh" -o "$helper" 2>/dev/null \
            || { rm -f "$helper"; helper=""; }
    fi
    for c in "$@"; do
        docker exec -i "$c" sh -c '
            set -e
            if [ -d /etc/apt/sources.list.d ]; then
                find /etc/apt/sources.list.d -type f \
                    ! -name ubuntu.sources ! -name debian.sources -delete
            fi
            if [ -d /etc/apt/apt.conf.d ]; then
                printf "Acquire::Retries \"5\";\nAcquire::http::Timeout \"60\";\n" \
                    > /etc/apt/apt.conf.d/80hx-ci-retries
            fi
        ' || { echo "hx_sibling_apt_prep: could not prepare $c" >&2; return 1; }
        if [ -n "$helper" ] && docker cp "$helper" "$c:/tmp/rw-apt-mirror.sh" 2>/dev/null; then
            docker exec -e RW_CACHE_HUB="${RW_CACHE_HUB:-10.1.10.103:8088}" "$c" sh /tmp/rw-apt-mirror.sh || true
        fi
    done
    [ -z "$helper" ] || rm -f "$helper"
}
