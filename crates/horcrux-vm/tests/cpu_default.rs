//! The run-time default CPU model of new VMs (own process: the setting is
//! process-global).
use horcrux_common::{VmArchitecture, VmConfig, VmHypervisor, VmStatus};
use horcrux_vm::vm::defaults::{self, Source};

fn vm() -> VmConfig {
    VmConfig {
        id: "d1".into(),
        name: "d1".into(),
        hypervisor: VmHypervisor::Qemu,
        memory: 256,
        cpus: 2,
        disk_size: 1,
        status: VmStatus::Stopped,
        architecture: VmArchitecture::X86_64,
        disks: vec![],
        display: Default::default(),
        hardware: Default::default(),
    }
}

#[test]
fn a_change_of_the_default_only_reaches_vms_created_afterwards() {
    defaults::set_cpu_model_override(None);
    assert_eq!(
        defaults::cpu_model(),
        ("x86-64-v2-AES".to_string(), Source::Builtin)
    );
    let mut first = vm();
    defaults::apply_create_defaults(&mut first);

    defaults::set_cpu_model_override(Some("x86-64-v3".into()));
    assert_eq!(
        defaults::cpu_model(),
        ("x86-64-v3".to_string(), Source::Api)
    );
    let mut second = vm();
    defaults::apply_create_defaults(&mut second);
    assert_eq!(first.hardware.cpu_type.as_deref(), Some("x86-64-v2-AES"));
    assert_eq!(second.hardware.cpu_type.as_deref(), Some("x86-64-v3"));

    // an implausible value is not accepted as an override
    defaults::set_cpu_model_override(Some("bad model;rm".into()));
    assert_eq!(defaults::cpu_model().1, Source::Builtin);
    defaults::set_cpu_model_override(None);
}
