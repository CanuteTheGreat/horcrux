#!/usr/bin/env bash
# Install-test the guest tools packages in their real distributions. Uses
# docker create + cp + start -a (a job container's filesystem is not visible to
# `docker run -v`). Env: PKG_DIR (default dist/linux/x86_64).
set -uo pipefail
cd "$(dirname "$0")/.."
PKG_DIR="${PKG_DIR:-dist/linux/x86_64}"
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p}' Cargo.toml | head -1)"
FILES="/usr/bin/horcrux-guest-agent /etc/horcrux/guest-tools.conf /usr/lib/systemd/system/horcrux-guest-agent.service /etc/init.d/horcrux-guest-agent"
fail=0
run() { # name image ext inner-script
  local name=$1 image=$2 ext=$3 script=$4 id rc
  id=$(docker create "$image" sh -ec "$script") || { echo "FAIL - $name: docker create"; fail=1; return; }
  docker cp "$PKG_DIR/horcrux-guest-tools.$ext" "$id:/horcrux-guest-tools.$ext" || { fail=1; docker rm -f "$id" >/dev/null; return; }
  docker start -a "$id"; rc=$?
  docker rm -f "$id" >/dev/null
  if [ $rc -eq 0 ]; then echo "ok - $name"; else echo "FAIL - $name (exit $rc)"; fail=1; fi
}
CHECK="
for f in $FILES; do test -e \$f || { echo \"missing \$f\"; exit 1; }; done
test -x /usr/bin/horcrux-guest-agent; test -x /etc/init.d/horcrux-guest-agent
/usr/bin/horcrux-guest-agent --help 2>&1 | grep -qi port || { echo 'agent --help gave no usage'; exit 1; }
"
GONE="
for f in $FILES; do test ! -e \$f || { echo \"left behind \$f\"; exit 1; }; done
"
run deb-debian-trixie debian:trixie deb "
export DEBIAN_FRONTEND=noninteractive
for i in 1 2 3; do apt-get update -qq && break; sleep 10; done
apt-get install -y -qq ./horcrux-guest-tools.deb >/dev/null
dpkg-query -W -f='\${Package} \${Version} \${Depends}\n' horcrux-guest-tools | tee /dev/stderr | grep -q ' $VERSION.*ffmpeg'
dpkg-query -W -f='\${Conffiles}\n' horcrux-guest-tools | grep -q /etc/horcrux/guest-tools.conf
command -v ffmpeg
$CHECK
test ! -e /etc/systemd/system/multi-user.target.wants/horcrux-guest-agent.service
apt-get remove -y -qq horcrux-guest-tools >/dev/null
test -e /etc/horcrux/guest-tools.conf
apt-get purge -y -qq horcrux-guest-tools >/dev/null
$GONE"
run rpm-fedora fedora:latest rpm "
dnf -y -q install ./horcrux-guest-tools.rpm >/dev/null
rpm -q --requires horcrux-guest-tools | grep -qx /usr/bin/ffmpeg
rpm -qc horcrux-guest-tools | grep -qx /etc/horcrux/guest-tools.conf
rpm -q --qf '%{VERSION} %{LICENSE}\n' horcrux-guest-tools | tee /dev/stderr | grep -q '^$VERSION '
command -v ffmpeg
$CHECK
dnf -y -q remove horcrux-guest-tools >/dev/null
$GONE"
run apk-alpine alpine:latest apk "
apk add -q --allow-untrusted ./horcrux-guest-tools.apk
apk info -R horcrux-guest-tools | grep -qx ffmpeg
apk info horcrux-guest-tools | grep -q '$VERSION'
apk info -L horcrux-guest-tools | grep -q etc/horcrux/guest-tools.conf
command -v ffmpeg
$CHECK
apk del -q horcrux-guest-tools
$GONE"
exit $fail
