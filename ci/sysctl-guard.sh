#!/usr/bin/env bash
# Privileged sibling containers (incus/lxc/k3s e2e) must never change HOST-global
# sysctls. kernel.apparmor_restrict_unprivileged_{userns,unconfined} are not
# namespaced: a writable /proc/sys plus anything that applies Ubuntu's
# /usr/lib/sysctl.d/10-apparmor.conf (= 1) flips the whole runner host, and
# every other job there then fails (VM stop EACCES, container kill EPERM).
#
#   ci/sysctl-guard.sh harden <container>   run right after `docker run --privileged`
#       - policy-rc.d denies service (re)starts from package maintainer scripts
#       - dpkg path-exclude keeps 10-apparmor.conf from ever being installed
#         (removed if the image ships it)
#       (A read-only bind of the two /proc/sys files was tried and rejected:
#       it makes `incusd forkstart` fail, so Incus containers don't start.)
#   ci/sysctl-guard.sh snapshot             record the host values, start a sampler
#   ci/sysctl-guard.sh assert               fail if the host values changed
#
# Host values are read through an unprivileged container (`docker run busybox`),
# which shows the global value; the sampler reads the job's own /proc/sys.
set -uo pipefail
KEYS="apparmor_restrict_unprivileged_userns apparmor_restrict_unprivileged_unconfined"
BEFORE=${HX_SYSCTL_BEFORE:-/tmp/hx-host-sysctl.before}
FLIPS=/tmp/hx-host-sysctl.flips
PIDF=/tmp/hx-host-sysctl.pid

read_host() { # prints "key=value" lines as seen from an unprivileged container
  docker run --rm --pull=never busybox:latest sh -c \
    'for k in '"$KEYS"'; do f=/proc/sys/kernel/$k; [ -e $f ] && echo "$k=$(cat $f)" || echo "$k=absent"; done' 2>/dev/null ||
  docker run --rm busybox:latest sh -c \
    'for k in '"$KEYS"'; do f=/proc/sys/kernel/$k; [ -e $f ] && echo "$k=$(cat $f)" || echo "$k=absent"; done'
}

case "${1:-}" in
harden)
  c=${2:?container}
  docker exec -i "$c" sh -s <<'EOS'
set -u
[ -d /usr/sbin ] && printf '#!/bin/sh\nexit 101\n' > /usr/sbin/policy-rc.d && chmod +x /usr/sbin/policy-rc.d
if [ -d /etc/dpkg ]; then
  mkdir -p /etc/dpkg/dpkg.cfg.d
  echo 'path-exclude=/usr/lib/sysctl.d/10-apparmor.conf' > /etc/dpkg/dpkg.cfg.d/90-no-host-apparmor-sysctl
fi
rm -f /usr/lib/sysctl.d/10-apparmor.conf /etc/sysctl.d/10-apparmor.conf
EOS
  ;;
snapshot)
  read_host > "$BEFORE" || { echo "::error::cannot read host sysctls"; exit 1; }
  echo "host sysctls before: $(tr '\n' ' ' < "$BEFORE")"
  : > "$FLIPS"
  # Catch a flip that something else resets within seconds (rw-userns-guard).
  setsid -f bash "$0" sampler >/dev/null 2>&1 </dev/null
  sleep 1
  ;;
sampler)
  echo $$ > "$PIDF"
  while :; do
    for k in $KEYS; do
      v=$(cat /proc/sys/kernel/$k 2>/dev/null || echo absent)
      want=$(sed -n "s/^$k=//p" "$BEFORE")
      if [ "$v" != "$want" ]; then
        priv=$(docker ps -q 2>/dev/null | xargs -r docker inspect -f '{{if .HostConfig.Privileged}}{{.Name}} {{end}}' 2>/dev/null)
        foreign=$(printf '%s\n' $priv | grep -v '^/horcrux-' || true)
        tag=other-jobs; { [ -n "$priv" ] && [ -z "$foreign" ]; } && tag=ours-only
        echo "$(date +%T) $k=$v (was $want); $tag; privileged containers: ${priv:-none}" >> "$FLIPS"
        sleep 2
      fi
    done
    sleep 0.2
  done
  ;;
assert)
  [ -s "$BEFORE" ] || { echo "::error::sysctl-guard: no snapshot taken"; exit 1; }
  [ -s "$PIDF" ] && kill "$(cat "$PIDF")" 2>/dev/null
  after=$(read_host) || { echo "::error::cannot read host sysctls"; exit 1; }
  rc=0
  if [ "$after" != "$(cat "$BEFORE")" ]; then
    echo "::error::host sysctls changed during the e2e steps: before [$(tr '\n' ' ' < "$BEFORE")] after [$(echo $after)]"
    rc=1
  fi
  if [ -s "$FLIPS" ]; then
    # Our own siblings are named horcrux-*; a flip while another job's privileged
    # container (e.g. a live-ISO build) also ran is not ours to blame.
    if grep -q 'ours-only' "$FLIPS"; then
      echo "::error::host sysctls flipped while only our privileged containers ran:"; rc=1
    else
      echo "::warning::host sysctls flipped, but other jobs' privileged containers were running:"
    fi
    cat "$FLIPS"
  fi
  [ $rc = 0 ] && echo "host sysctls unchanged: $(echo $after)"
  exit $rc
  ;;
*) echo "usage: $0 harden <container>|snapshot|assert" >&2; exit 2 ;;
esac
