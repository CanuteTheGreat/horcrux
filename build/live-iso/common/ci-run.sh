#!/usr/bin/env bash
# ci-run.sh daily|stable   -- run by .forgejo/workflows/live-iso.yml on the kvm-x86_64 runner.
# build ISO -> QEMU boot/install/reboot/web tests (UEFI+USB, BIOS+CD) -> publish only if ALL passed.
# ALWAYS exits 0 (a red job would e-mail the owner): the outcome goes to the `daily-status`
# marker release and the job summary; ~/.hermes iso-status-watch reads the marker.
set -uo pipefail
mode=${1:?daily|stable}
HERE=$(cd "$(dirname "$0")" && pwd); cd "$HERE"
. "$HERE/../project/project.env"
OUT=${OUT:-$PWD/../../../iso-out}; mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
EVI=$OUT/evidence; rm -rf "$EVI"; mkdir -p "$EVI"
T0=$(date +%s); sha=${GITHUB_SHA:-$(git rev-parse HEAD)}
result=pass; failed_step=""; summary=""
fail() { result=fail; failed_step=$1; summary="$1: $2"; echo "::error::$summary"; }

if [ "$mode" = stable ]; then tag=${GITHUB_REF_NAME:?}; suffix=$tag; else suffix=daily; fi

echo "::group::host deps"
export DEBIAN_FRONTEND=noninteractive
(apt-get update -qq && apt-get install -y -qq --no-install-recommends qemu-system-x86 qemu-utils ovmf python3 curl ca-certificates openssh-client sshpass e2fsprogs openssl >/dev/null) || fail deps "apt install failed"
echo "::endgroup::"

# stable gate: the repo's own CI must not be red on the tagged commit
if [ "$result" = pass ] && [ "$mode" = stable ]; then
  st=$(curl -fsS -H "Authorization: token ${FORGEJO_TOKEN:-}" "$GITHUB_SERVER_URL/api/v1/repos/$GITHUB_REPOSITORY/commits/$sha/status" 2>/dev/null | python3 -c 'import json,sys;print(json.load(sys.stdin).get("state",""))' 2>/dev/null)
  echo "repo CI state on $sha: ${st:-unknown}"
  case "$st" in failure|error) fail stable-gate "repo CI is $st on the tagged commit; no stable ISO" ;; esac
fi

if [ "$result" = pass ]; then
  echo "::group::installer unit tests"
  bash "$HERE/../sync-common.sh" "${SYNC_MODE:---check}" || fail common-sync "shared live-installer copy is out of sync (see sync-common.sh)"
  python3 -m unittest discover -s "$HERE/tests" 2>&1 | tail -15; [ "${PIPESTATUS[0]}" = 0 ] || fail unit-tests "guided-installer unit tests failed"
  echo "::endgroup::"
fi

BUILD_S=0
if [ "$result" = pass ]; then
  echo "::group::build ISO"
  t=$(date +%s)
  JOBS=${JOBS:-4} bash ../build.sh -o "$OUT" -n "$suffix" 2>&1 | tee "$EVI/build.log" | tail -60
  BUILD_S=$(( $(date +%s)-t ))
  ISO=$(ls "$OUT"/*-"$suffix"-x86_64.iso 2>/dev/null | head -1)
  [ -n "$ISO" ] && [ -s "$ISO" ] || fail build "no ISO produced (see build.log)"
  echo "::endgroup::"
fi

TEST_S=0
if [ "$result" = pass ]; then
  size=$(stat -c %s "$ISO")
  [ "$size" -lt 2147483648 ] || fail size "ISO is $size bytes, over the 2 GiB Forgejo asset limit"
  ( cd "$OUT" && sha256sum -c "$(basename "$ISO").sha256" ) || fail checksum "sha256 mismatch"
fi
if [ "$result" = pass ]; then
  t=$(date +%s)
  # The matrix covers both firmware types, both media types, the unattended answer-file install, the
  # guided installer driven over the serial line AND on the VGA console (screenshots), the SSH
  # opt-in branch, and (Horcrux) xfs and a 2-disk ZFS mirror.
  if [ "$PROJECT" = horcrux ]; then
    MATRIX=(
      "bios-cdrom-auto-ext4-dhcp-openconsole|--mode bios --media cdrom --flow auto --fs ext4 --console-open"
      "bios-cdrom-tui-serial-xfs-static|--mode bios --media cdrom --flow tui --fs xfs --static-ip 10.0.2.77"
      "uefi-usb-tui-vga-zfs-mirror-static|--mode uefi --media usb --flow tui-vga --fs zfs --zfs-raid mirror --static-ip 10.0.2.88"
      "bios-cdrom-fallback-no-answer-file|--mode bios --media cdrom --flow fallback"
      "uefi-usb-auto-zfs-mirror-sshpw|--mode uefi --media usb --flow auto --fs zfs --zfs-raid mirror --static-ip 10.0.2.99 --ssh-password-opt-in"
    )
  else
    MATRIX=(
      "bios-cdrom-auto-sshpw|--mode bios --media cdrom --flow auto --ssh-password-opt-in"
      "bios-cdrom-tui-serial|--mode bios --media cdrom --flow tui"
      "uefi-usb-tui-vga|--mode uefi --media usb --flow tui-vga"
      "uefi-usb-auto-openconsole|--mode uefi --media usb --flow auto --console-open"
      "bios-cdrom-fallback-no-answer-file|--mode bios --media cdrom --flow fallback"
    )
  fi
  failed_list=""
  for entry in "${MATRIX[@]}"; do
    name=${entry%%|*}; args=${entry#*|}
    echo "::group::QEMU test $name"
    python3 -u "$HERE/qemu-test.py" --iso "$ISO" --project "$PROJECT" $args --out "$EVI/$name" 2>&1 | tee "$EVI/test-$name.log" | tail -40
    rc=${PIPESTATUS[0]}
    echo "::endgroup::"
    rm -f "$EVI/$name"/target*.qcow2 "$EVI/$name"/answers.img
    if [ "$rc" != 0 ]; then failed_list="$failed_list $name"; [ -z "$failed_step" ] && fail "qemu-$name" "$(grep -E '^\[FAIL\]' "$EVI/test-$name.log" | head -1)"; fi
  done
  TEST_S=$(( $(date +%s)-t ))
fi
[ "$result" = pass ] && summary="PASS: ${#MATRIX[@]} QEMU runs (4 installs + the no-answer-file fallback) (UEFI+USB, BIOS+CD; answer-file, serial TUI, VGA TUI) - installed system refuses password root SSH, accepts the key, serves the web UI on the chosen IP (build ${BUILD_S}s, test ${TEST_S}s)"

# evidence bundle (serial logs, screenshots, result.json - small)
( cd "$EVI" && tar czf "$OUT/$PROJECT-$suffix-test-evidence.tgz" --exclude='target*.qcow2' --exclude='*.fd' --exclude='*.qmp' --exclude=test_key . )
cat > "$OUT/status.json" <<JSON
{"project":"$PROJECT","mode":"$mode","result":"$result","failed_step":"$failed_step","summary":$(python3 -c 'import json,sys;print(json.dumps(sys.argv[1]))' "$summary"),"sha":"$sha","run_id":"${GITHUB_RUN_ID:-manual}","run_url":"${GITHUB_SERVER_URL:-}/${GITHUB_REPOSITORY:-}/actions/runs/${GITHUB_RUN_ID:-0}","time":"$(date -u +%FT%TZ)","build_seconds":$BUILD_S,"test_seconds":$TEST_S}
JSON
cat "$OUT/status.json"

if [ -n "${FORGEJO_TOKEN:-}" ]; then
  if [ "$result" = pass ]; then
    bash "$HERE/publish.sh" "$mode" "$PROJECT" "$ISO" "$OUT/$PROJECT-$suffix-test-evidence.tgz" "$OUT/status.json" || { echo "::error::publish failed"; result=fail; sed -i 's/"result":"pass"/"result":"fail"/; s/"failed_step":""/"failed_step":"publish"/' "$OUT/status.json"; }
  fi
  [ "$mode" = daily ] && bash "$HERE/publish.sh" status "$PROJECT" "" "" "$OUT/status.json" || true
fi
{ echo "### $PROJECT $mode ISO: $result"; echo "$summary"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
# docker hygiene on the shared host (keep BuildKit cache; drop dangling stuff)
docker image prune -f >/dev/null 2>&1 || true
exit 0
