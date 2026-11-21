#!/bin/bash
# The installed Windows 11 test image, cached per runner in the Docker volume
# hxc-win-img (ISOs in hxc-win-iso), keyed by both ISOs and the answer file
# (ci/windows-test-image.py key): only the first run on a runner, or a changed
# ISO/answer file, installs Windows (40-60 min). Shared by the workflows that
# need a Windows guest (hx-console-guest-e2e.yml, console-desktop-win-e2e.yml).
#
# Usage: ci/windows-test-image-cached.sh OUT.qcow2
# Env:   WIN_EVAL_URL, VIRTIO_WIN_URL, VIRTIO_WIN_SHA256 (set by the workflow)
set -euo pipefail
OUT=${1:?usage: $0 OUT.qcow2}
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
: "${WIN_EVAL_URL:?}" "${VIRTIO_WIN_URL:?}" "${VIRTIO_WIN_SHA256:?}"
TMP=$(dirname "$OUT")
mkdir -p "$TMP"
docker volume create hxc-win-iso >/dev/null
docker volume create hxc-win-img >/dev/null
docker run --rm -e URL="$WIN_EVAL_URL" -e VURL="$VIRTIO_WIN_URL" -e VSUM="$VIRTIO_WIN_SHA256" \
  -v hxc-win-iso:/c debian:bookworm bash -c '
  set -e
  need=0; [ -s /c/win11-eval.iso ] || need=1
  echo "$VSUM  /c/virtio-win.iso" | sha256sum -c --quiet 2>/dev/null || need=1
  [ $need = 0 ] && exit 0
  apt-get update -qq >/dev/null && apt-get install -y -qq curl ca-certificates >/dev/null
  if [ ! -s /c/win11-eval.iso ]; then
    curl -fsSL --retry 3 -o /c/win11-eval.iso.part.$$ "$URL" && mv /c/win11-eval.iso.part.$$ /c/win11-eval.iso
  fi
  if ! echo "$VSUM  /c/virtio-win.iso" | sha256sum -c --quiet 2>/dev/null; then
    curl -fsSL --retry 3 -o /c/virtio-win.iso.part.$$ "$VURL"
    echo "$VSUM  /c/virtio-win.iso.part.$$" | sha256sum -c --quiet
    mv /c/virtio-win.iso.part.$$ /c/virtio-win.iso
  fi'
docker run --rm -v hxc-win-iso:/c:ro debian:bookworm cat /c/win11-eval.iso > "$TMP/win11-eval.iso"
docker run --rm -v hxc-win-iso:/c:ro debian:bookworm cat /c/virtio-win.iso > "$TMP/virtio-win.iso"
KEY=$(python3 "$HERE/windows-test-image.py" key "$TMP/win11-eval.iso" "$TMP/virtio-win.iso")
echo "image key $KEY"
if docker run --rm -v hxc-win-img:/c:ro debian:bookworm test -s "/c/win-$KEY.qcow2"; then
  docker run --rm -v hxc-win-img:/c:ro debian:bookworm cat "/c/win-$KEY.qcow2" > "$OUT"
  echo "cached image win-$KEY.qcow2"
else
  python3 -u "$HERE/windows-test-image.py" build "$TMP/win11-eval.iso" "$TMP/virtio-win.iso" "$OUT"
  # a unique part name: another workflow on this runner may be storing the
  # same key at the same time; older keys and stale parts (a day) are removed
  part="win-$KEY.qcow2.part.$(date +%s).$$"
  docker run -i --rm -v hxc-win-img:/c debian:bookworm sh -c \
    "cat > /c/$part && mv /c/$part /c/win-$KEY.qcow2 && find /c -name 'win-*.qcow2' ! -name win-$KEY.qcow2 -delete && find /c -name 'win-*.qcow2.part*' -mmin +1440 -delete" \
    < "$OUT"
fi
rm -f "$TMP/win11-eval.iso" "$TMP/virtio-win.iso"
ls -l "$OUT"
