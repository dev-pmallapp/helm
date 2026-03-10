use crate::kvm_sys;

#[test]
fn kvm_api_version_constant() {
    // Sanity: the ioctl number should be non-zero.
    assert_ne!(kvm_sys::KVM_GET_API_VERSION, 0);
}

#[test]
fn kvm_create_vm_constant() {
    assert_ne!(kvm_sys::KVM_CREATE_VM, 0);
}

#[test]
fn kvm_run_constant() {
    assert_ne!(kvm_sys::KVM_RUN, 0);
}

#[test]
fn memory_region_struct_size() {
    // kvm_userspace_memory_region = 4+4+8+8+8 = 32 bytes
    assert_eq!(
        std::mem::size_of::<kvm_sys::kvm_userspace_memory_region>(),
        32
    );
}

#[test]
fn irq_level_struct_size() {
    // kvm_irq_level = 4+4 = 8 bytes
    assert_eq!(std::mem::size_of::<kvm_sys::kvm_irq_level>(), 8);
}

#[test]
fn vcpu_init_struct_size() {
    // kvm_vcpu_init = 4 + 7*4 = 32 bytes
    assert_eq!(std::mem::size_of::<kvm_sys::kvm_vcpu_init>(), 32);
}

#[test]
fn create_device_struct_size() {
    // kvm_create_device = 4+4+4 = 12 bytes
    assert_eq!(std::mem::size_of::<kvm_sys::kvm_create_device>(), 12);
}

#[test]
fn one_reg_struct_size() {
    // kvm_one_reg = 8+8 = 16 bytes
    assert_eq!(std::mem::size_of::<kvm_sys::kvm_one_reg>(), 16);
}
