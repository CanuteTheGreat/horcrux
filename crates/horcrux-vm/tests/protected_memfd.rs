//! Real-kernel proof that a protected gaming VM's RAM cannot be read from host
//! userspace (docs/design/gaming-vm.md section 12).
//!
//! `guest_memfd` is KVM's private guest memory: pages owned by a guest are not
//! mapped into host userspace at all, so the VMM process (QEMU) — and anything
//! that can read the VMM's address space — cannot see guest RAM. This test
//! creates a throw-away `KVM_X86_SW_PROTECTED_VM`, allocates a `guest_memfd`,
//! and asserts that mapping it from host userspace is refused by the kernel.
//!
//! Ignored by default: it needs `/dev/kvm` on a Linux 6.8+ kernel that offers
//! `KVM_CAP_GUEST_MEMFD` and the `KVM_X86_SW_PROTECTED_VM` type (see
//! `ci/protected-vm-e2e.sh`). On a kernel/host without those it SKIPS loudly
//! (prints the reason and returns ok) rather than passing silently.
//!
//!   cargo test -p horcrux-vm --test protected_memfd -- --ignored --nocapture
use horcrux_vm::vm::protected::{probe_guest_memfd, GuestMemfdProbe};
use std::path::Path;

#[test]
#[ignore = "needs /dev/kvm with guest_memfd + sw-protected VM type (real kernel)"]
fn guest_memfd_is_unreadable_from_host_userspace() {
    let probe: GuestMemfdProbe = probe_guest_memfd(Path::new("/dev/kvm"));
    println!("probe: {probe:?}");

    if !probe.kvm_accessible {
        println!(
            "SKIP: /dev/kvm not accessible ({}); run on the kvm-x86_64 runner",
            probe.detail
        );
        return;
    }
    if !probe.cap_guest_memfd || !probe.sw_protected_vm_type {
        println!(
            "SKIP: this kernel lacks guest_memfd and/or the sw-protected VM type \
             (needs Linux 6.8+): {}",
            probe.detail
        );
        return;
    }

    // The capability is present, so the VM + guest_memfd must have been created
    // and the host-userspace mapping must have been refused.
    assert!(
        probe.created,
        "guest_memfd was not created: {}",
        probe.detail
    );
    assert_eq!(
        probe.host_mmap_refused,
        Some(true),
        "host userspace was ABLE to map guest RAM — not private! detail: {}",
        probe.detail
    );
    println!(
        "ok - host userspace cannot map or read guest_memfd RAM: {}",
        probe.detail
    );
}
