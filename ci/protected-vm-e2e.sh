#!/bin/bash
# Real-kernel proof that a protected gaming VM's RAM is unreadable from host
# userspace (docs/design/gaming-vm.md section 12).
#
# Runs horcrux-vm's ignored `protected_memfd` test on a kernel with /dev/kvm,
# KVM_CAP_GUEST_MEMFD and the KVM_X86_SW_PROTECTED_VM machine type (Linux
# 6.8+). The test creates a sw-protected VM + guest_memfd and asserts that
# mapping that guest RAM from host userspace is refused by the kernel.
#
#   cargo test -p horcrux-vm --test protected_memfd --no-run
#   ci/protected-vm-e2e.sh target/debug/deps/protected_memfd-<hash>
set -euo pipefail
BIN=$(readlink -f "$1")
test -x "$BIN"

echo "# kernel: $(uname -r)"
if [ ! -e /dev/kvm ]; then
    echo "SKIP: no /dev/kvm on this runner"
    exit 0
fi
ls -l /dev/kvm

# The test opens /dev/kvm directly; no nested VM is needed. Run it (as root in
# the CI container) with output shown.
"$BIN" --ignored --nocapture guest_memfd_is_unreadable_from_host_userspace 2>&1 | tee out.log

# Fail the gate if the kernel claimed the capability but the read was NOT
# refused (the test asserts this, so a non-zero exit already fails; this is a
# belt-and-suspenders check on the log wording).
if grep -q "host userspace was ABLE to map guest RAM" out.log; then
    echo "FAIL: guest RAM was readable from host userspace"
    exit 1
fi
echo "ok - protected-vm e2e finished"
