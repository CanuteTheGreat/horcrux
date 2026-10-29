#!/usr/bin/env bash
# ci-run.sh daily|stable   -- run by .forgejo/workflows/live-iso.yml, once per architecture (ARCH env; default = the host's):
#   x86_64 on the kvm-x86_64 runner (KVM), aarch64 on the arm64 runner (native build, QEMU TCG: Graviton has no /dev/kvm).
# build ISO -> QEMU boot/install/reboot/web tests (x86: UEFI+USB, BIOS+CD; arm64/riscv64: UEFI) -> publish only if ALL passed.
# ALWAYS exits 0 (a red job would e-mail the owner): the outcome goes to the `daily-status`
# marker release and the job summary; ~/.hermes iso-status-watch reads the marker.
set -uo pipefail
mode=${1:?daily|stable}
HERE=$(cd "$(dirname "$0")" && pwd); cd "$HERE"
. "$HERE/../project/project.env"
case $(uname -m) in x86_64|amd64) HOST_ARCH=x86_64;; aarch64|arm64) HOST_ARCH=aarch64;; riscv64) HOST_ARCH=riscv64;; *) HOST_ARCH=$(uname -m);; esac
ARCH=${ARCH:-$HOST_ARCH}; export ARCH
# Optional image variant (VARIANT=desktop): same pipeline, its own ISO/asset names (<p>-desktop-daily-<arch>.iso), status marker and a
# smaller QEMU matrix + the encrypted-ZFS desktop test (GDM greeter on VGA, boot-environment rollback, hibernate/resume).
VARIANT=${VARIANT:-}; export VARIANT; VPFX=${VARIANT:+$VARIANT-}
. "$HERE/arch/$ARCH.env" || { echo "unsupported ARCH=$ARCH"; exit 0; }
OUT=${OUT:-$PWD/../../../iso-out}; mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
EVI=$OUT/evidence; rm -rf "$EVI"; mkdir -p "$EVI"
T0=$(date +%s); sha=${GITHUB_SHA:-$(git rev-parse HEAD)}
result=pass; failed_step=""; summary=""
fail() { result=fail; failed_step=$1; summary="$1: $2"; echo "::error::$summary"; }
# Always-run cleanup (normal end, `fail`, a crash, SIGTERM): the 20-40 GB QEMU evidence disks, OVMF NVRAM and answer
# images go first (the evidence tgz below only keeps logs/screenshots/result.json), then, in CI, the whole output
# dir (ISO ~1.5 GB, build logs: everything needed was already published or tarred). A local run that sets OUT keeps
# its ISO + evidence tgz unless KEEP_OUT is unset and it runs under Actions.
cleanup() {
  find "$EVI" \( -name 'target*.qcow2' -o -name 'answers.img' -o -name 'OVMF_VARS.fd' -o -name '*.qmp' -o -name '*.raw' -o -name 'base.qcow2' -o -name '*-disk.qcow2' -o -name 'persist-*.raw' \) -delete 2>/dev/null
  if [ -n "${GITHUB_ACTIONS:-}" ] && [ -z "${KEEP_OUT:-}" ]; then rm -rf "$OUT"; fi
}
trap cleanup EXIT
trap 'exit 143' TERM INT

if [ "$mode" = stable ]; then tag=${GITHUB_REF_NAME:?}; suffix=$tag; else suffix=daily; fi

echo "::group::host deps"
export DEBIAN_FRONTEND=noninteractive
(apt-get update -qq && apt-get install -y -qq --no-install-recommends $QEMU_PKGS qemu-utils python3 curl ca-certificates openssh-client sshpass e2fsprogs openssl >/dev/null) || fail deps "apt install failed"
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
  ARCH=$ARCH VARIANT=$VARIANT JOBS=${JOBS:-4} bash ../build.sh -o "$OUT" -n "$suffix" 2>&1 | tee "$EVI/build.log" | tail -60
  BUILD_S=$(( $(date +%s)-t ))
  ISO=$(ls "$OUT"/*-"$suffix"-$ARCH.iso 2>/dev/null | head -1)
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
  # Every run boots the guest with a conservative x86-64-v2 CPU model (qemu-test.py --cpu, default
  # qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt,+cx16), NEVER -cpu host: a binary built with target-cpu=native passed
  # under -cpu host and then crashed with "invalid opcode" on a different real CPU. --restarts 2 = install, then
  # restart the installed system twice from inside the guest and assert services started, ip_forward=1 (Patronus)
  # and no database errors in the logs after the first boot and after each restart.
  # The matrix covers both firmware types, both media types, the unattended answer-file install, the
  # guided installer driven over the serial line AND on the VGA console (screenshots), the SSH
  # opt-in branch, and (Horcrux) xfs and a 2-disk ZFS mirror.
  if [ "$ARCH" = x86_64 ]; then
  if [ "$PROJECT" = horcrux ]; then
    MATRIX=(
      "bios-cdrom-auto-ext4-dhcp-openconsole|--mode bios --media cdrom --flow auto --fs ext4 --console-open --restarts 2"
      "bios-cdrom-tui-serial-xfs-static|--mode bios --media cdrom --flow tui --fs xfs --static-ip 10.0.2.77"
      "uefi-usb-tui-vga-zfs-mirror-static|--mode uefi --media usb --flow tui-vga --fs zfs --zfs-raid mirror --static-ip 10.0.2.88"
      "bios-cdrom-fallback-no-answer-file|--mode bios --media cdrom --flow fallback"
      "uefi-usb-auto-zfs-mirror-sshpw|--mode uefi --media usb --flow auto --fs zfs --zfs-raid mirror --static-ip 10.0.2.99 --ssh-password-opt-in"
    )
  else
    MATRIX=(
      "bios-cdrom-auto-sshpw|--mode bios --media cdrom --flow auto --ssh-password-opt-in --restarts 2"
      "bios-cdrom-tui-serial|--mode bios --media cdrom --flow tui"
      "uefi-usb-tui-vga|--mode uefi --media usb --flow tui-vga"
      "uefi-usb-auto-openconsole|--mode uefi --media usb --flow auto --console-open --restarts 2"
      "bios-cdrom-fallback-no-answer-file|--mode bios --media cdrom --flow fallback"
    )
  fi
  else
  # arm64 / riscv64: UEFI only, QEMU TCG (a run takes ~15 min on a Graviton core). USB stick + optical drive,
  # answer file / serial TUI / VGA TUI / no-answer-file fallback, and (Horcrux) the ZFS mirror.
  if [ "$PROJECT" = horcrux ]; then
    MATRIX=(
      "uefi-usb-auto-ext4-dhcp-openconsole|--mode uefi --media usb --flow auto --fs ext4 --console-open"
      "uefi-cdrom-tui-serial-xfs-static|--mode uefi --media cdrom --flow tui --fs xfs --static-ip 10.0.2.77"
      "uefi-usb-tui-vga-ext4|--mode uefi --media usb --flow tui-vga --fs ext4"
      "uefi-cdrom-fallback-no-answer-file|--mode uefi --media cdrom --flow fallback"
      "uefi-usb-auto-zfs-mirror-sshpw|--mode uefi --media usb --flow auto --fs zfs --zfs-raid mirror --static-ip 10.0.2.99 --ssh-password-opt-in"
    )
  else
    MATRIX=(
      "uefi-usb-auto-sshpw|--mode uefi --media usb --flow auto --ssh-password-opt-in"
      "uefi-cdrom-tui-serial|--mode uefi --media cdrom --flow tui"
      "uefi-usb-tui-vga|--mode uefi --media usb --flow tui-vga"
      "uefi-cdrom-auto-openconsole|--mode uefi --media cdrom --flow auto --console-open"
      "uefi-cdrom-fallback-no-answer-file|--mode uefi --media cdrom --flow fallback"
    )
  fi
  fi
  if [ -n "$VARIANT" ]; then
    # the variant adds a desktop on top of the same installer: the server matrix is run in its short form (BIOS+CD ext4, UEFI+USB ZFS mirror)
    # and the variant's own test below does the rest
    MATRIX=(
      "bios-cdrom-auto-ext4-dhcp-openconsole|--mode bios --media cdrom --flow auto --fs ext4 --console-open --restarts 1"
      "uefi-usb-auto-zfs-mirror-sshpw|--mode uefi --media usb --flow auto --fs zfs --zfs-raid mirror --static-ip 10.0.2.99 --ssh-password-opt-in"
    )
  fi
  failed_list=""
  for entry in "${MATRIX[@]}"; do
    name=${entry%%|*}; args=${entry#*|}
    echo "::group::QEMU test $name"
    python3 -u "$HERE/qemu-test.py" --iso "$ISO" --project "$PROJECT" --arch "$ARCH" $args --out "$EVI/$name" 2>&1 | tee "$EVI/test-$name.log" | tail -40
    rc=${PIPESTATUS[0]}
    echo "::endgroup::"
    rm -f "$EVI/$name"/target*.qcow2 "$EVI/$name"/answers.img
    if [ "$rc" != 0 ]; then failed_list="$failed_list $name"; [ -z "$failed_step" ] && fail "qemu-$name" "$(grep -E '^\[FAIL\]' "$EVI/test-$name.log" | head -1)"; fi
  done
  if [ "$VARIANT" = desktop ] && [ "$ARCH" = x86_64 ]; then
    # encrypted ZFS desktop: passphrase on a USB keyboard at the VGA prompt, GDM greeter ASSERTED on the VGA console, boot-environment
    # rollback, then the portable hibernate flow (one passphrase, hibernate, power off, boot, resume: marker + process survive, scrub clean)
    name=zfs-enc-desktop-hibernate
    echo "::group::QEMU test $name"
    python3 -u "$HERE/zfs-enc-test.py" --iso "$ISO" --out "$EVI/$name" --desktop --hibernate 2>&1 | tee "$EVI/test-$name.log" | tail -60
    rc=${PIPESTATUS[0]}
    echo "::endgroup::"
    rm -f "$EVI/$name"/target*.qcow2 "$EVI/$name"/answers.img
    if [ "$rc" != 0 ]; then failed_list="$failed_list $name"; [ -z "$failed_step" ] && fail "qemu-$name" "$(grep -E '^\[FAIL\]' "$EVI/test-$name.log" | head -1)"; fi
  fi
  # boot menu + rescue / forensic / live / persistence modes (UEFI + BIOS): see boot-modes-test.py
  if [ "${BOOT_MODES_TESTS:-1}" = 1 ] && [ -z "$VARIANT" ]; then
    for fw in uefi bios; do
      [ $fw = bios ] && [ "${HAS_BIOS:-1}" = 0 ] && continue
      name=boot-modes-$fw; media=usb; [ $fw = bios ] && media=cdrom
      echo "::group::QEMU test $name"
      python3 -u "$HERE/boot-modes-test.py" --iso "$ISO" --project "$PROJECT" --arch "$ARCH" --mode $fw --media $media ${NO_MEMTEST:+--no-memtest} --out "$EVI/$name" 2>&1 | tee "$EVI/test-$name.log" | tail -40
      rc=${PIPESTATUS[0]}
      echo "::endgroup::"
      rm -f "$EVI/$name"/*.raw "$EVI/$name"/*.qcow2 "$EVI/$name"/install/target*.qcow2 "$EVI/$name"/install/answers.img
      if [ "$rc" != 0 ]; then failed_list="$failed_list $name"; [ -z "$failed_step" ] && fail "qemu-$name" "$(grep -E '^\[FAIL\]' "$EVI/test-$name.log" | head -1)"; fi
    done
  fi
  TEST_S=$(( $(date +%s)-t ))
fi
ACCEL=KVM; [ -e /dev/kvm ] && [ "$ARCH" = "$HOST_ARCH" ] || ACCEL="TCG emulation, no KVM"
[ "$result" = pass ] && summary="PASS ($ARCH, QEMU $ACCEL): ${#MATRIX[@]} QEMU runs (boot, install, reboot; $([ "$ARCH" = x86_64 ] && echo 'UEFI+USB, BIOS+CD; answer-file, serial TUI, VGA TUI, no-answer-file fallback' || echo 'UEFI, USB + optical; answer-file, serial TUI, VGA TUI, no-answer-file fallback')) - installed system refuses password root SSH, accepts the key, serves the web UI on the chosen IP (build ${BUILD_S}s, test ${TEST_S}s)"

# evidence bundle (serial logs, screenshots, result.json - small)
( cd "$EVI" && tar czf "$OUT/$PROJECT-$VPFX$suffix-$ARCH-test-evidence.tgz" --exclude='target*.qcow2' --exclude='*.fd' --exclude='*.qmp' --exclude=test_key . )
cat > "$OUT/status.json" <<JSON
{"project":"$PROJECT","arch":"$ARCH","mode":"$mode","variant":"$VARIANT","result":"$result","failed_step":"$failed_step","summary":$(python3 -c 'import json,sys;print(json.dumps(sys.argv[1]))' "$summary"),"sha":"$sha","run_id":"${GITHUB_RUN_ID:-manual}","run_url":"${GITHUB_SERVER_URL:-}/${GITHUB_REPOSITORY:-}/actions/runs/${GITHUB_RUN_ID:-0}","time":"$(date -u +%FT%TZ)","build_seconds":$BUILD_S,"test_seconds":$TEST_S}
JSON
cat "$OUT/status.json"

if [ -n "${FORGEJO_TOKEN:-}" ]; then
  if [ "$result" = pass ]; then
    bash "$HERE/publish.sh" "$mode" "$PROJECT" "$ISO" "$OUT/$PROJECT-$VPFX$suffix-$ARCH-test-evidence.tgz" "$OUT/status.json" || { echo "::error::publish failed"; result=fail; sed -i 's/"result":"pass"/"result":"fail"/; s/"failed_step":""/"failed_step":"publish"/' "$OUT/status.json"; }
  fi
  [ "$mode" = daily ] && bash "$HERE/publish.sh" status "$PROJECT" "" "" "$OUT/status.json" || true
fi
# Second publish step: the public GitHub mirror (the product site's /download/ URLs redirect there). Only when the
# Forgejo publish above ran and everything passed; a failure here is a warning in the summary, not a red job.
gh_note=""
if [ -n "${FORGEJO_TOKEN:-}" ] && [ -n "${GH_RELEASE_TOKEN:-}" ] && [ "$result" = pass ]; then
  bash "$HERE/publish-github.sh" "$mode" "$PROJECT" "$ISO" "$OUT/$PROJECT-$VPFX$suffix-$ARCH-test-evidence.tgz" "$OUT/status.json" \
    && gh_note="GitHub release updated." || { gh_note="GitHub publish FAILED (Forgejo publish is unaffected)."; echo "::warning::$gh_note"; }
fi
{ echo "### $PROJECT $mode ISO: $result"; echo "$summary"; [ -z "$gh_note" ] || echo "$gh_note"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
# docker hygiene on the shared host (build.sh already removed this build's containers/images; keep the BuildKit cache)
docker image prune -f >/dev/null 2>&1 || true
exit 0
