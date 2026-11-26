#!/usr/bin/env bash
# ci-run.sh daily|stable   -- run by .forgejo/workflows/live-iso-<variant>.yml, once per architecture (ARCH env; default = the host's):
#   x86_64 on the kvm-x86_64 runner (KVM), aarch64 on the arm64 runner (native build, QEMU TCG: Graviton has no /dev/kvm).
# build ISO -> QEMU boot/install/reboot/web tests (x86: UEFI+USB, BIOS+CD; arm64/riscv64: UEFI) -> publish only if ALL passed.
# The outcome goes to the `daily-status-*` marker release and the job summary (~/.hermes iso-status-watch reads the
# marker), and a failed build/test/publish step FAILS THE RUN (exit 1, after the marker is written): a green run
# must mean an image was built, tested and published (run 8637 was green while `dracut: Failed to find module zfs`).
set -uo pipefail
mode=${1:?daily|stable}
HERE=$(cd "$(dirname "$0")" && pwd); cd "$HERE"
. "$HERE/../project/project.env"
case $(uname -m) in x86_64|amd64) HOST_ARCH=x86_64;; aarch64|arm64) HOST_ARCH=aarch64;; riscv64) HOST_ARCH=riscv64;; *) HOST_ARCH=$(uname -m);; esac
ARCH=${ARCH:-$HOST_ARCH}; export ARCH
# Optional image variant (VARIANT=gnome-light; `desktop` = old alias): same pipeline, its own ISO/asset names
# (<p>-<DE|server>-<init>-<channel>-<arch>.iso, common/image-name.sh), status marker (daily-status-$IMG-<arch>) and a
# smaller QEMU matrix + the encrypted-ZFS desktop test (GDM greeter on VGA, boot-environment rollback, hibernate/resume).
VARIANT=${VARIANT:-}; export VARIANT
. "$HERE/image-name.sh"; export INIT=$IMG_INIT; VPFX="$IMG-"
. "$HERE/arch/$ARCH.env" || { echo "::error::unsupported ARCH=$ARCH"; exit 1; }
# per-job output dir: the openrc/systemd aarch64 (and x86_64) live-iso jobs can share one runner workspace path; a shared iso-out made one job rm -rf the other's evidence and qemu fail "Failed to get write lock" on target.qcow2
OUT=${OUT:-$PWD/../../../iso-out-$IMG-$ARCH}; mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
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
# The runner image carries Launchpad PPAs (git-core) we never install from; a PPA that has not published the image's
# series/arch (404 on .../dists/<series>/main/binary-arm64/Packages) makes `apt-get update` exit non-zero (job 145970).
# Drop them before updating (APT_DROP_PPAS=0 keeps them) and retry the update+install once for transient mirror errors.
if [ "${APT_DROP_PPAS:-1}" = 1 ]; then
  rm -f /etc/apt/sources.list.d/*ppa* /etc/apt/sources.list.d/*launchpad* 2>/dev/null || true
  grep -lE 'ppa\.launchpad(content)?\.net' /etc/apt/sources.list /etc/apt/sources.list.d/* 2>/dev/null | while read -r f; do
    case $f in *.sources) rm -f "$f" ;; *) sed -i -E '/ppa\.launchpad(content)?\.net/d' "$f" ;; esac
  done
fi
host_deps() { apt-get update -qq && apt-get install -y -qq --no-install-recommends $QEMU_PKGS swtpm qemu-utils python3 curl ca-certificates openssh-client sshpass e2fsprogs openssl gnupg gpgv xorriso squashfs-tools gdisk dosfstools mtools binutils >/dev/null; }
host_deps || { echo "host deps failed, retrying in 20 s" >&2; sleep 20; host_deps; } || fail deps "apt install failed"
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
  python3 -m unittest discover -s "$HERE/tests" > "$EVI/unit-tests.log" 2>&1; ut_rc=$?
  tail -15 "$EVI/unit-tests.log"
  [ "$ut_rc" = 0 ] || { grep -E '^(FAIL|ERROR):|Error|assert' "$EVI/unit-tests.log" | head -12; fail unit-tests "guided-installer unit tests failed (rc=$ut_rc): $(grep -E '^(FAIL|ERROR):' "$EVI/unit-tests.log" | head -3 | tr '\n' ' ')"; }
  # /boot space handling on a REAL 512 MB ext4 loop mount (30 snapshots, <= 80% used); needs root + loop devices, skipped otherwise
  # `losetup -f` alone only names a free /dev/loopN and succeeds on hosts where the node is absent or the loop pool is
  # exhausted (run 15819: mount failed to set up a loop device and failed the whole publish): attach a real file first.
  loop_ok=0
  if [ "$(id -u)" = 0 ] && command -v mkfs.ext4 >/dev/null; then
    lp_img=$(mktemp); truncate -s 1M "$lp_img"
    if lp_dev=$(losetup -f --show "$lp_img" 2>/dev/null); then loop_ok=1; losetup -d "$lp_dev" 2>/dev/null; fi
    rm -f "$lp_img"
    [ "$loop_ok" = 1 ] || echo "::warning::be-boot-space skipped: this runner cannot attach a loop device"
  fi
  if [ "$loop_ok" = 1 ]; then
    python3 -W ignore "$HERE/be-boot-space-test.py" > "$EVI/be-boot-space.log" 2>&1; bs_rc=$?
    grep -E '^(RESULT|ok)' "$EVI/be-boot-space.log"
    if [ "$bs_rc" != 0 ]; then tail -n 30 "$EVI/be-boot-space.log"; fail be-boot-space "horcrux-be /boot space test failed (rc=$bs_rc): $(tail -n 1 "$EVI/be-boot-space.log" | cut -c1-200)"; fi
  fi
  echo "::endgroup::"
fi

BUILD_S=0
if [ "$result" = pass ]; then
  echo "::group::build ISO"
  t=$(date +%s)
  # Secure Boot: with the repo secret present the build MUST sign (SECUREBOOT=on fails the build otherwise); riscv64 has no signed shim
  if [ -n "${SECUREBOOT_SIGNING_KEY:-}" ] && [ "$ARCH" != riscv64 ]; then export SECUREBOOT=on; fi
  ARCH=$ARCH VARIANT=$VARIANT JOBS=${JOBS:-4} bash ../build.sh -o "$OUT" -n "$suffix" 2>&1 | tee "$EVI/build.log" | tail -60
  build_rc=${PIPESTATUS[0]}
  BUILD_S=$(( $(date +%s)-t ))
  ISO=$(ls "$OUT"/*-"$suffix"-$ARCH.iso 2>/dev/null | head -1)
  if [ "$build_rc" != 0 ]; then fail build "build.sh exited $build_rc: $(grep -E 'ERROR|Failed|failed' "$EVI/build.log" | tail -1 | cut -c1-200)"
  elif [ -z "$ISO" ] || [ ! -s "$ISO" ]; then fail build "no ISO produced (see build.log)"; fi
  echo "::endgroup::"
fi

TEST_S=0
if [ "$result" = pass ]; then
  size=$(stat -c %s "$ISO")
  [ "$size" -lt 2147483648 ] || fail size "ISO is $size bytes, over GitHub's 2 GiB per-release-asset limit (the ISO is delivered from the GitHub release)"
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
      "uefi-usb-gui-ext4|--mode uefi --media usb --flow gui --fs ext4"
      "bios-cdrom-gui-xfs-static|--mode bios --media cdrom --flow gui --fs xfs --static-ip 10.0.2.66"
    )
  else
    MATRIX=(
      "bios-cdrom-auto-sshpw|--mode bios --media cdrom --flow auto --ssh-password-opt-in --restarts 2"
      "bios-cdrom-tui-serial|--mode bios --media cdrom --flow tui"
      "uefi-usb-tui-vga|--mode uefi --media usb --flow tui-vga"
      "uefi-usb-auto-openconsole|--mode uefi --media usb --flow auto --console-open --restarts 2"
      "bios-cdrom-fallback-no-answer-file|--mode bios --media cdrom --flow fallback"
      "uefi-usb-gui|--mode uefi --media usb --flow gui"
      "bios-cdrom-gui|--mode bios --media cdrom --flow gui"
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
  # product-specific matrix (Patronus: ZFS default + TPM-unlock path, XFS fallback, no-TPM path): <project>/ci-matrix.sh may append to MATRIX
  # and may set GATES_ONLY_X86 entries; sourced after the shared defaults so both products keep one runner.
  [ -f "$HERE/../project/ci-matrix.sh" ] && . "$HERE/../project/ci-matrix.sh"
  if [ -n "$VARIANT" ]; then
    # the variant adds a desktop on top of the same installer: the server matrix is run in its short form (BIOS+CD ext4, UEFI+USB ZFS mirror)
    # and the variant's own test below does the rest
    MATRIX=(
      "bios-cdrom-auto-ext4-dhcp-openconsole|--mode bios --media cdrom --flow auto --fs ext4 --console-open --restarts 1"
      "uefi-usb-auto-zfs-mirror-sshpw|--mode uefi --media usb --flow auto --fs zfs --zfs-raid mirror --static-ip 10.0.2.99 --ssh-password-opt-in"
    )
  fi
  # UEFI SECURE BOOT (common/boot/secureboot-sign.sh): a signed ISO is installed and its installed system booted with Secure Boot ON
  # (Microsoft keys in the firmware, the ISO's own certificate in MokList), asserting SB is on inside the guest and the signed
  # out-of-tree modules load. secureboot-test.py (below) covers the no-key refusal, the MokManager enrollment walk and SB off.
  SB_SIGNED=0
  if xorriso -osirrox on -indev "$ISO" -ls / 2>/dev/null | grep -q ENROLL_THIS_KEY_IN_MOKMANAGER.cer; then SB_SIGNED=1; fi
  if [ "$SB_SIGNED" = 1 ]; then
    if [ "$PROJECT" = horcrux ]; then sbfs="--fs zfs --zfs-raid mirror"; else sbfs="--fs zfs --zfs-raid single"; fi
    MATRIX+=("uefi-usb-auto-secureboot-mok|--mode uefi --media usb --flow auto $sbfs --secure-boot mok")
  fi
  # Hard per-test deadlines (a stuck QEMU test must FAIL in minutes-to-an-hour, not hold the single KVM runner for hours): the harness arms a
  # watchdog (QEMU_TEST_DEADLINE: kills its QEMUs, prints [FAIL] hard-deadline, exits 124) and `timeout` is the backstop (SIGTERM = same cleanup).
  if [ "$ARCH" = "$HOST_ARCH" ] && [ -e /dev/kvm ]; then DL_QEMU=2700; DL_ZFS=3600; DL_MODES=3000; else DL_QEMU=5400; DL_ZFS=10800; DL_MODES=10800; fi   # DL_ZFS under TCG 7200 -> 10800: systemd aarch64 run 3281 passed every step of the headless gate and was cut by the 7200 s deadline in its last stage (install alone 1446 s)
  # DL_MODES under TCG: boot-modes = 8 menu-mode boots + a ~1900 s base install + forensic + rescue boots; it hit the old 6000 s deadline on a loaded
  # arm64 runner (patronus run 783: all asserts passed up to 'rescue', then hard-deadline). Only the deadline moved; the job cap is 420 min.
  with_deadline() { local s=$1; shift; QEMU_TEST_DEADLINE=$s timeout -k 30 $((s+180)) "$@"; }
  # zfs-enc-test.py with its hard deadline. A run that dies by SIGKILL (rc 137: exit status of a process killed from OUTSIDE the harness, which prints
  # [FAIL] hard-deadline and exits 124 on its own deadline/SIGTERM and prints a [FAIL]/FAILED STEPS line on every assertion) says nothing about the image:
  # gnome-light run 2726 (rw-man-06, elastic kvm lane) lost the whole 60-min test ~27 min in, during the boot2 passphrase wait, with no [FAIL] at all, while
  # the identical hibernate/resume flow had passed 5 h earlier on the same image code. Such a kill is infrastructure (a runner/host action on the job's
  # processes): print what the job container's cgroup saw, then run the test ONCE more on a fresh disk. Assertion failures and deadline hits are never retried.
  zfs_enc_run() {
    local n=$1; shift
    local attempt
    for attempt in 1 2; do
      with_deadline $DL_ZFS python3 -u "$HERE/zfs-enc-test.py" --iso "$ISO" --out "$EVI/$n" "$@" 2>&1 | tee "$EVI/test-$n.log" | tail -60
      rc=${PIPESTATUS[0]}
      # `| tail -60` above keeps only the end of the log, which on a hibernate/resume death is the post-mortem boot's kernel messages and
      # hides the [DIAG] lines saying HOW QEMU ended (exit status, stderr, `-d cpu_reset` dump, serial tail, guest clock after the resume;
      # runs 3963/3964 failed with those cut off). Print every [DIAG] block and [FAIL] line from the full log, uncut, in its own group.
      if [ "$rc" != 0 ] && grep -aqE '^\[(DIAG|FAIL)\]' "$EVI/test-$n.log" 2>/dev/null; then
        echo "::group::full [DIAG]/[FAIL] output of $n (rc $rc)"
        grep -a -A30 -E '^\[(DIAG|FAIL)\]' "$EVI/test-$n.log" | grep -av '^--$' | cut -c1-6000 | head -400 || true
        echo "::endgroup::"
      fi
      [ "$rc" = 137 ] || return 0
      echo "::warning::qemu-$n attempt $attempt was SIGKILLed from outside the harness (rc 137, no [FAIL] line): $(uptime | sed 's/.*load/load/'); memory.events: $(tr '\n' ' ' < /sys/fs/cgroup/memory.events 2>/dev/null) memory.peak: $(cat /sys/fs/cgroup/memory.peak 2>/dev/null)"
      rm -rf "$EVI/$n"; pkill -x qemu-system-x86_64 2>/dev/null; sleep 5
    done
    return 0
  }
  failed_list=""
  for entry in "${MATRIX[@]}"; do
    name=${entry%%|*}; args=${entry#*|}
    echo "::group::QEMU test $name"
    # a ZFS install under TCG needs far more than the other runs (unsquashfs onto ZFS in an emulated aarch64 guest was at 41% after 85 min:
    # patronus runs 726/747): longer hard deadline for those entries (qemu-test.py scales the installer wait to match)
    dl=$DL_QEMU
    if [ "$DL_QEMU" = 5400 ]; then case "$args" in *"--fs zfs"*|*"--mem 8192"*) dl=12600 ;; esac; fi
    with_deadline $dl python3 -u "$HERE/qemu-test.py" --iso "$ISO" --project "$PROJECT" --arch "$ARCH" --init "$INIT" $args --out "$EVI/$name" 2>&1 | tee "$EVI/test-$name.log" | tail -40
    rc=${PIPESTATUS[0]}
    echo "::endgroup::"
    rm -f "$EVI/$name"/target*.qcow2 "$EVI/$name"/answers.img
    if [ "$rc" != 0 ]; then failed_list="$failed_list $name"; [ -z "$failed_step" ] && fail "qemu-$name" "$(grep -E '^\[FAIL\]' "$EVI/test-$name.log" | head -1)"; fi
  done
  if [ "$SB_SIGNED" = 1 ]; then
    name=secureboot
    echo "::group::QEMU test $name (Secure Boot: no key refused, MokManager enrollment, SB off)"
    with_deadline 2400 python3 -u "$HERE/secureboot-test.py" --iso "$ISO" --arch "$ARCH" --out "$EVI/$name" 2>&1 | tee "$EVI/test-$name.log" | tail -40
    rc=${PIPESTATUS[0]}
    echo "::endgroup::"
    if [ "$rc" != 0 ]; then failed_list="$failed_list $name"; [ -z "$failed_step" ] && fail "qemu-$name" "$(grep -E '^\[FAIL\]' "$EVI/test-$name.log" | head -1)"; fi
  elif [ "$ARCH" != riscv64 ] && [ -n "${SECUREBOOT_SIGNING_KEY:-}" ]; then
    fail secureboot "SECUREBOOT_SIGNING_KEY is set but the ISO carries no signed boot chain"
  fi
  if [ "$PROJECT" = horcrux ] || { [ "$PROJECT" = patronus ] && [ "$ARCH" = x86_64 ]; }; then
    # (both products boot an installed system with GRUB: Horcrux ships the menu scripts in sys-apps/horcrux-core, Patronus in net-firewall/patronus;
    # Patronus aarch64 runs under TCG with no time left in the job)
    # UNATTENDED BOOT: the installed system's boot-environment menu (horcrux-be grubcfg) booted by the image's OWN GRUB + kernel under UEFI with
    # no key ever pressed: normal, no grubenv, one-shot next_entry, read-only env block, broken default (fallback), broken trial kernel, every
    # kernel broken (retry then restart), plus a control that must hang without `fallback`. A menu waiting for a key fails the run.
    # Same for installs WITHOUT boot environments (mk-* cases): the image's own grub-mkconfig + /etc/grub.d (sys-apps/horcrux-core's
    # 11_horcrux_unattended) on a staged /boot: broken newest kernel -> previous, saved/next_entry defaults, all broken -> restart,
    # submenu menus, and a stock-scripts control that must hang. --mode all (default) runs both menus.
    name=grub-autoboot
    echo "::group::QEMU test $name"
    with_deadline 4200 python3 -u "$HERE/grub-autoboot-test.py" --iso "$ISO" --arch "$ARCH" --mode all --out "$EVI/$name" 2>&1 | tee "$EVI/test-$name.log" | tail -40
    rc=${PIPESTATUS[0]}
    echo "::endgroup::"
    if [ "$rc" != 0 ]; then failed_list="$failed_list $name"; [ -z "$failed_step" ] && fail "qemu-$name" "$(grep -E '^\[FAIL\]' "$EVI/test-$name.log" | head -1)"; fi
  fi
  if [ "$IMG_DE" != server ] && [ "$ARCH" = x86_64 ]; then
    # encrypted ZFS desktop: passphrase on a USB keyboard at the VGA prompt, GDM greeter ASSERTED on the VGA console, boot-environment
    # rollback, then the portable hibernate flow (one passphrase, hibernate, power off, boot, resume: marker + process survive, scrub clean)
    name=zfs-enc-desktop-hibernate
    echo "::group::QEMU test $name"
    zfs_enc_run "$name" --desktop --hibernate --public
    echo "::endgroup::"
    rm -f "$EVI/$name"/target*.qcow2 "$EVI/$name"/answers.img
    if [ "$rc" != 0 ]; then failed_list="$failed_list $name"; [ -z "$failed_step" ] && fail "qemu-$name" "$(grep -E '^\[FAIL\]' "$EVI/test-$name.log" | head -1)"; fi
  fi
  if [ "$IMG_DE" = server ] && [ "$PROJECT" = horcrux ]; then
    # HEADLESS SERVER GATE (server-first, both architectures): encrypted-ZFS answer-file install, then a REBOOT into the installed system on the SERIAL console only
    # (no VGA, no USB keyboard) with the NIC link down at boot (late network), months-off RTC on every boot, and the same regression gates as the desktop:
    # chrony steps the clock, package-contents-complete, kernel module builds, zfs-kmod dist-kernel rule, shadow reuse, no autologin, web UI reachable,
    # no display stack installed (qlist + emerge -pv @world), nothing personal (public-forbidden.txt), first boot waits for the network and does not show crashed.
    name=server-headless-zfs-enc
    # a branch run (dispatch on a branch that is not on main yet): the installed system syncs the PUBLISHED overlay (main), which lacks the branch's
    # core packages (e.g. a systemd image + main's OpenRC-only horcrux-firstboot = openrc/sysvinit blocker in emerge @world); --overlay-dir puts them back
    ovl=""
    if [ "${GITHUB_REF_TYPE:-}" = branch ] && [ -n "${GITHUB_REF_NAME:-}" ] && [ "${GITHUB_REF_NAME}" != main ]; then ovl="--overlay-dir $(cd "$HERE/../../.." && pwd)/gentoo"; fi
    echo "::group::QEMU test $name"
    zfs_enc_run "$name" --server --public --late-network ${SERVER_LATE_NET_S:-90} $ovl
    echo "::endgroup::"
    rm -f "$EVI/$name"/target*.qcow2 "$EVI/$name"/answers.img
    if [ "$rc" != 0 ]; then failed_list="$failed_list $name"; [ -z "$failed_step" ] && fail "qemu-$name" "$(grep -E '^\[FAIL\]' "$EVI/test-$name.log" | head -1)"; fi
  fi
  if [ "$PROJECT" = horcrux ] && [ "$ARCH" = x86_64 ] && [ -e /dev/kvm ]; then
    # ONE ISO, ANY MACHINE: this stock ISO + a machine profile (tests/fixtures/profiles) = the machine, from binary packages, no image build.
    # Server ISO: the generic server profile. GNOME ISO: a laptop profile on a simulated Apple T2 Mac (hardware layer + firmware read from a
    # synthetic Mac EFI partition) with the REAL T2 kernel installed from the binhost lane amd64/t2 and booted (--real-t2-kernel: no TEST-ONLY t2_kernel = skip). Install wall time is a gate (< 30 min). Branch runs bundle the branch's overlay (not published yet).
    name=machine-profile; prof=generic-server; margs=""
    if [ "$IMG_DE" != server ]; then prof=generic-desktop; margs="--desktop --fake-t2 --real-t2-kernel"; fi
    movl=""
    if [ "${GITHUB_REF_TYPE:-}" = branch ] && [ -n "${GITHUB_REF_NAME:-}" ] && [ "${GITHUB_REF_NAME}" != main ]; then movl="--dev-overlay $(cd "$HERE/../../.." && pwd)/gentoo"; fi
    echo "::group::QEMU test $name ($prof)"
    with_deadline $DL_ZFS python3 -u "$HERE/machine-profile-test.py" --iso "$ISO" --out "$EVI/$name" --profile "$HERE/tests/fixtures/profiles/$prof" $margs $movl 2>&1 | tee "$EVI/test-$name.log" | tail -60
    rc=${PIPESTATUS[0]}
    echo "::endgroup::"
    rm -f "$EVI/$name"/target*.qcow2 "$EVI/$name"/answers.img "$EVI/$name"/mac-esp.img
    if [ "$rc" != 0 ]; then failed_list="$failed_list $name"; [ -z "$failed_step" ] && fail "qemu-$name" "$(grep -E '^\[FAIL\]' "$EVI/test-$name.log" | head -1)"; fi
  fi
  # boot menu + rescue / forensic / live / persistence modes (UEFI + BIOS): see boot-modes-test.py
  if [ "${BOOT_MODES_TESTS:-1}" = 1 ] && [ -z "$VARIANT" ]; then
    for fw in uefi bios; do
      [ $fw = bios ] && [ "${HAS_BIOS:-1}" = 0 ] && continue
      name=boot-modes-$fw; media=usb; [ $fw = bios ] && media=cdrom
      echo "::group::QEMU test $name"
      with_deadline $DL_MODES python3 -u "$HERE/boot-modes-test.py" --iso "$ISO" --project "$PROJECT" --arch "$ARCH" --init "$INIT" --mode $fw --media $media ${NO_MEMTEST:+--no-memtest} --out "$EVI/$name" 2>&1 | tee "$EVI/test-$name.log" | tail -40
      rc=${PIPESTATUS[0]}
      echo "::endgroup::"
      rm -f "$EVI/$name"/*.raw "$EVI/$name"/*.qcow2 "$EVI/$name"/install/target*.qcow2 "$EVI/$name"/install/answers.img
      if [ "$rc" != 0 ]; then failed_list="$failed_list $name"; [ -z "$failed_step" ] && fail "qemu-$name" "$(grep -E '^\[FAIL\]' "$EVI/test-$name.log" | head -1)"; fi
    done
  fi
  # Product gates beyond the shared QEMU matrix (e.g. a vmtest scenario driving the installed system): <project>/ci-gates.sh is SOURCED here with
  # $ISO, $EVI, $HERE, $PROJECT, $ARCH, with_deadline, failed_list and fail() in scope; it appends a failing gate to failed_list and calls fail like
  # the matrix loop above. Horcrux has none.
  if [ -f "$HERE/../project/ci-gates.sh" ] && [ -z "$VARIANT" ]; then . "$HERE/../project/ci-gates.sh"; fi
  TEST_S=$(( $(date +%s)-t ))
fi
# SBOM (CycloneDX JSON) of the TESTED image: the Portage package database read back out of the ISO's own squashfs, plus every Rust
# package of Cargo.lock (licenses from `cargo metadata`, run in a throwaway rust container; best effort: without it the Rust
# components carry name/version/sha256 only). Published next to the ISO and signed (publish-github.sh); no SBOM = no publish.
SBOM_FILE=""
if [ "$result" = pass ]; then
  echo "::group::SBOM"
  REPO_ROOT=$(cd "$HERE/../../.." && pwd)
  SBOM_FILE=$OUT/$(basename "$ISO" .iso).cdx.json
  meta=$OUT/cargo-metadata.json; lock=$REPO_ROOT/Cargo.lock; gen_lock=""; rm -f "$meta"
  # A workspace may gitignore Cargo.lock (Patronus does): the container then resolves one the same way the image build does and
  # hands it back with the metadata, as a tar stream (no bind mounts: the runner's docker may be a different filesystem).
  sbomx=$OUT/sbom-cargo; rm -rf "$sbomx"; mkdir -p "$sbomx"
  if command -v docker >/dev/null && ( cd "$REPO_ROOT" && git archive --format=tar HEAD ) | timeout 900 docker run --rm -i -v live-iso-sbom-cargo:/usr/local/cargo/registry \
       rust:1-bookworm sh -c 'mkdir -p /s && cd /s && tar xf - && { [ -f Cargo.lock ] || cargo generate-lockfile >&2; } && cargo metadata --format-version 1 --locked > /tmp/meta.json && tar cf - -C /tmp meta.json -C /s Cargo.lock' 2> "$EVI/cargo-metadata.log" | tar xf - -C "$sbomx" \
     && [ -s "$sbomx/meta.json" ]; then
    mv "$sbomx/meta.json" "$meta"; [ -f "$lock" ] || { lock=$sbomx/Cargo.lock; gen_lock=1; }
    echo "cargo metadata: $(stat -c %s "$meta") bytes${gen_lock:+ (Cargo.lock generated: not tracked in this repo)}"
  else echo "::warning::cargo metadata failed (Rust components listed without licenses): $(tail -n 2 "$EVI/cargo-metadata.log" 2>/dev/null | tr '\n' ' ')"; rm -f "$meta"; fi
  margs=(); [ -s "$meta" ] && margs=(--cargo-metadata "$meta")
  python3 "$HERE/sbom.py" --from-iso "$ISO" --cargo-lock "$lock" "${margs[@]}" \
      --name "$(basename "$ISO" .iso)" --version "$sha" --project "$PROJECT" -o "$SBOM_FILE" 2>&1 | tee "$EVI/sbom.log" | tail -3
  [ "${PIPESTATUS[0]}" = 0 ] && [ -s "$SBOM_FILE" ] || fail sbom "SBOM generation failed: $(tail -n 1 "$EVI/sbom.log" | cut -c1-200)"
  rm -rf "$meta" "$sbomx"
  echo "::endgroup::"
fi
ACCEL=KVM; [ -e /dev/kvm ] && [ "$ARCH" = "$HOST_ARCH" ] || ACCEL="TCG emulation, no KVM"
[ "$result" = pass ] && summary="PASS ($ARCH, QEMU $ACCEL): ${#MATRIX[@]} QEMU runs (boot, install, reboot; $([ "$ARCH" = x86_64 ] && echo 'UEFI+USB, BIOS+CD; answer-file, serial TUI, VGA TUI, graphical installer driven by keyboard + pointer + AT-SPI, no-answer-file fallback' || echo 'UEFI, USB + optical; answer-file, serial TUI, VGA TUI, graphical installer driven by keyboard + pointer + AT-SPI, no-answer-file fallback')) - installed system refuses password root SSH, accepts the key, serves the web UI on the chosen IP (build ${BUILD_S}s, test ${TEST_S}s)"

# evidence bundle (serial logs, screenshots, result.json - small)
( cd "$EVI" && tar czf "$OUT/$PROJECT-$VPFX$suffix-$ARCH-test-evidence.tgz" --exclude='target*.qcow2' --exclude='*.fd' --exclude='*.qmp' --exclude=test_key . )
cat > "$OUT/status.json" <<JSON
{"project":"$PROJECT","arch":"$ARCH","mode":"$mode","variant":"$VARIANT","image":"$IMG","image_id":"$IMG_ID","result":"$result","failed_step":"$failed_step","summary":$(python3 -c 'import json,sys;print(json.dumps(sys.argv[1]))' "$summary"),"sha":"$sha","run_id":"${GITHUB_RUN_ID:-manual}","run_url":"${GITHUB_SERVER_URL:-}/${GITHUB_REPOSITORY:-}/actions/runs/${GITHUB_RUN_ID:-0}","time":"$(date -u +%FT%TZ)","build_seconds":$BUILD_S,"test_seconds":$TEST_S}
JSON
cat "$OUT/status.json"

# Delivery is the public GitHub release (the product site 302-redirects its stable download URLs there); the ISO is NEVER uploaded to
# Forgejo (owner rule: no downloads/assets on Forgejo). Forgejo only gets the machine-readable status marker (publish.sh status).
# A GitHub publish failure is a real failure of the daily (nothing was delivered), recorded as failed_step=publish.
gh_note=""
if [ -n "${FORGEJO_TOKEN:-}" ] && [ "$result" = pass ]; then
  if [ -z "${GH_RELEASE_TOKEN:-}" ]; then
    fail publish "GH_RELEASE_TOKEN is not set: the ISO cannot be delivered (the GitHub release is the only download location)"
  elif [ -z "${RELEASE_SIGNING_KEY:-}" ]; then
    fail publish "RELEASE_SIGNING_KEY is not set: unsigned images are never published (repo Actions secret)"
  elif [ "$ARCH" != riscv64 ] && [ "${SB_SIGNED:-0}" != 1 ]; then
    fail publish "the ISO has no Secure Boot chain (SECUREBOOT_SIGNING_KEY repo secret missing?): the download pages promise Secure Boot support"
  elif SBOM=$SBOM_FILE bash "$HERE/publish-github.sh" "$mode" "$PROJECT" "$ISO" "$OUT/$PROJECT-$VPFX$suffix-$ARCH-test-evidence.tgz" "$OUT/status.json"; then
    gh_note="GitHub release updated."
  else
    fail publish "GitHub release publish failed (see the job log)"
  fi
  if [ "$result" != pass ]; then
    python3 - "$OUT/status.json" "$failed_step" "$summary" <<'PY' || true
import json, sys
d = json.load(open(sys.argv[1])); d.update(result="fail", failed_step=sys.argv[2], summary=sys.argv[3]); json.dump(d, open(sys.argv[1], "w"))
PY
  fi
fi
if [ -n "${FORGEJO_TOKEN:-}" ] && [ "$mode" = daily ]; then
  bash "$HERE/publish.sh" status "$PROJECT" "" "" "$OUT/status.json" || true
fi
{ echo "### $PROJECT $mode ISO: $result"; echo "$summary"; [ -z "$gh_note" ] || echo "$gh_note"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
# docker hygiene on the shared host (build.sh already removed this build's containers/images; keep the BuildKit cache)
docker image prune -f >/dev/null 2>&1 || true
# honest run status: red when anything failed (the status marker above is already written)
if [ "$result" != pass ]; then echo "::error::Live ISO $mode ($IMG-$ARCH) FAILED at $failed_step: $summary"; exit 1; fi
exit 0
