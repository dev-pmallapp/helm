//! KVM capability probing.
//!
//! Wraps `KVM_CHECK_EXTENSION` to discover which features the host
//! KVM supports.

use crate::error::{KvmError, Result};
use crate::kvm_sys;
use std::os::unix::io::RawFd;

/// The set of KVM capabilities that HELM cares about.
#[derive(Debug, Clone)]
pub struct KvmCaps {
    /// The KVM API version (must be 12).
    pub api_version: i32,
    /// `KVM_CAP_USER_MEMORY` — user-space memory regions.
    pub user_memory: bool,
    /// `KVM_CAP_ONE_REG` — per-register get/set.
    pub one_reg: bool,
    /// `KVM_CAP_ARM_EL1_32BIT` — AArch32 EL1 support.
    pub arm_el1_32bit: bool,
    /// `KVM_CAP_ARM_PSCI_0_2` — PSCI v0.2.
    pub arm_psci_0_2: bool,
    /// `KVM_CAP_ARM_PMU_V3` — PMU v3 (perf counters).
    pub arm_pmu_v3: bool,
    /// Maximum guest IPA size in bits (0 = default 40-bit).
    pub arm_vm_ipa_size: u32,
    /// Size in bytes of the `kvm_run` mmap region.
    pub vcpu_mmap_size: usize,
}

/// Probe `/dev/kvm` capabilities.
///
/// `kvm_fd` must be an open file descriptor to `/dev/kvm`.
pub fn probe(kvm_fd: RawFd) -> Result<KvmCaps> {
    let api = check_api_version(kvm_fd)?;
    Ok(KvmCaps {
        api_version: api,
        user_memory: check_extension(kvm_fd, kvm_sys::KVM_CAP_USER_MEMORY)? > 0,
        one_reg: check_extension(kvm_fd, kvm_sys::KVM_CAP_ONE_REG)? > 0,
        arm_el1_32bit: check_extension(kvm_fd, kvm_sys::KVM_CAP_ARM_EL1_32BIT)? > 0,
        arm_psci_0_2: check_extension(kvm_fd, kvm_sys::KVM_CAP_ARM_PSCI_0_2)? > 0,
        arm_pmu_v3: check_extension(kvm_fd, kvm_sys::KVM_CAP_ARM_PMU_V3)? > 0,
        arm_vm_ipa_size: check_extension(kvm_fd, kvm_sys::KVM_CAP_ARM_VM_IPA_SIZE)? as u32,
        vcpu_mmap_size: get_vcpu_mmap_size(kvm_fd)?,
    })
}

/// Verify `KVM_GET_API_VERSION == 12`.
fn check_api_version(kvm_fd: RawFd) -> Result<i32> {
    let ver =
        unsafe { kvm_sys::kvm_ioctl(kvm_fd, kvm_sys::KVM_GET_API_VERSION, 0) }.map_err(|e| {
            KvmError::Ioctl {
                name: "KVM_GET_API_VERSION",
                source: e,
            }
        })?;
    if ver != 12 {
        return Err(KvmError::Unavailable(format!(
            "expected KVM API version 12, got {ver}"
        )));
    }
    Ok(ver)
}

/// `KVM_CHECK_EXTENSION` — returns the extension value (0 = unsupported).
fn check_extension(kvm_fd: RawFd, cap: u32) -> Result<i32> {
    unsafe { kvm_sys::kvm_ioctl(kvm_fd, kvm_sys::KVM_CHECK_EXTENSION, cap as u64) }.map_err(|e| {
        KvmError::Ioctl {
            name: "KVM_CHECK_EXTENSION",
            source: e,
        }
    })
}

/// `KVM_GET_VCPU_MMAP_SIZE` — size of the `kvm_run` mmap region.
fn get_vcpu_mmap_size(kvm_fd: RawFd) -> Result<usize> {
    let size =
        unsafe { kvm_sys::kvm_ioctl(kvm_fd, kvm_sys::KVM_GET_VCPU_MMAP_SIZE, 0) }.map_err(|e| {
            KvmError::Ioctl {
                name: "KVM_GET_VCPU_MMAP_SIZE",
                source: e,
            }
        })?;
    if size <= 0 {
        return Err(KvmError::Unavailable(
            "KVM_GET_VCPU_MMAP_SIZE returned 0".into(),
        ));
    }
    Ok(size as usize)
}
