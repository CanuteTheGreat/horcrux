#!/bin/sh
# Refuse to run mutation testing / chaos (fault-injection) tests on an Amazon EC2 machine.
# Those run only on the colo host (rw-man-01, `runs-on: colo-x86_64`): rw-man-02..05 are EC2
# instances under GuardDuty, which flags mutated code as malware and generated/probed hostnames
# as DGA traffic (2026-10-17). Same check as the Errant tools' ci/mutation.sh and Rustinion's
# scripts/ci/mutants.sh, extended to board_vendor, Xen-era BIOS strings and IMDS. DMI is visible
# inside containers too, so this works from a CI job container or by hand. rw-man-01 reports
# QEMU and has no metadata service, so it passes.
#
# Opt-in since 2026-10-18 (owner: the colo backlog was too big; GuardDuty suppression kept, all
# rw-man-01..05 may run mutation/chaos again): refuses only when CI_COLO_ONLY=1 is set, so setting
# that one variable (workflow env or runner envs:) re-enables the colo-only rule.
#
# Usage: <path>/ec2-guard.sh [what is about to run]   with CI_COLO_ONLY=1: exits 1 on EC2, 0 elsewhere.
what="${1:-mutation/chaos testing}"
if [ "${CI_COLO_ONLY:-0}" != 1 ]; then
  echo "ec2-guard: off (CI_COLO_ONLY unset); $what may run on any rw-man host"
  exit 0
fi
hit=""
for f in sys_vendor board_vendor; do
  if [ "$(cat "/sys/class/dmi/id/$f" 2>/dev/null)" = "Amazon EC2" ]; then hit="DMI $f = Amazon EC2"; fi
done
case "$(cat /sys/class/dmi/id/bios_version 2>/dev/null)" in
  *amazon*) hit="${hit:-DMI bios_version mentions amazon}" ;;
esac
if [ -z "$hit" ] && command -v curl >/dev/null 2>&1; then
  # IMDSv2 answers 401 to a token-less GET, IMDSv1 200; anywhere else nothing answers (000).
  code=$(curl -s -o /dev/null -m 2 -w '%{http_code}' http://169.254.169.254/latest/meta-data/ 2>/dev/null)
  case "$code" in 200|401) hit="EC2 instance metadata service answered (HTTP $code)" ;; esac
fi
if [ -n "$hit" ]; then
  echo "$what refused: this is an EC2 host ($hit); run it on the colo lane (rw-man-01, colo-x86_64)" >&2
  exit 1
fi
echo "ec2-guard: not EC2 ($(cat /sys/class/dmi/id/sys_vendor 2>/dev/null || echo unknown vendor)); $what may run"
