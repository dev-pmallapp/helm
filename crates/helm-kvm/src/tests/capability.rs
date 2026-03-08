use crate::capability::KvmCaps;

#[test]
fn caps_default_fields() {
    let caps = KvmCaps {
        api_version: 12,
        user_memory: true,
        one_reg: true,
        arm_el1_32bit: false,
        arm_psci_0_2: true,
        arm_pmu_v3: false,
        arm_vm_ipa_size: 40,
        vcpu_mmap_size: 4096,
    };
    assert_eq!(caps.api_version, 12);
    assert!(caps.user_memory);
    assert!(caps.one_reg);
    assert!(!caps.arm_el1_32bit);
    assert!(caps.arm_psci_0_2);
    assert!(!caps.arm_pmu_v3);
    assert_eq!(caps.arm_vm_ipa_size, 40);
    assert_eq!(caps.vcpu_mmap_size, 4096);
}
