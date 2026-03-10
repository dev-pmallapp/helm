//! VM exit reason types.
//!
//! Each variant corresponds to a `KVM_EXIT_*` reason read from the
//! `kvm_run` structure after `KVM_RUN` returns.

use crate::kvm_sys;

/// Reason the vCPU stopped executing.
#[derive(Debug, Clone)]
pub enum VmExit {
    /// Guest performed an MMIO access to an address not backed by a
    /// `KVM_SET_USER_MEMORY_REGION`.
    Mmio {
        addr: u64,
        data: [u8; 8],
        len: u32,
        is_write: bool,
    },
    /// An interrupt window opened — used when the host requested
    /// notification via `KVM_CAP_IRQ_WINDOW`.
    IrqWindowOpen,
    /// The guest issued a `HLT` instruction (or `WFI` on AArch64).
    Hlt,
    /// The guest initiated a shutdown (PSCI `SYSTEM_OFF` or triple-fault).
    Shutdown,
    /// A host signal interrupted `KVM_RUN` (e.g. `SIGALRM`).
    Intr,
    /// The guest triggered `KVM_EXIT_FAIL_ENTRY` — usually a bad
    /// initial register state.
    FailEntry { hardware_entry_failure_reason: u64 },
    /// An unrecoverable internal error inside KVM.
    InternalError { suberror: u32 },
    /// System event (reset, shutdown, crash).
    SystemEvent { event_type: u32 },
    /// A debug exception (single-step, HW breakpoint).
    Debug,
    /// A hypercall was issued by the guest (PSCI calls on AArch64).
    Hypercall,
    /// Exit reason not (yet) handled by HELM.
    Unknown(u32),
}

impl VmExit {
    /// Returns `true` if this exit requires the vCPU to stop permanently.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            VmExit::Shutdown | VmExit::FailEntry { .. } | VmExit::InternalError { .. }
        )
    }
}

/// Decode the exit reason from a raw `kvm_run` pointer.
///
/// # Safety
/// `kvm_run_ptr` must point to a valid, mapped `kvm_run` structure
/// returned by `mmap` on the vCPU fd.
pub(crate) unsafe fn decode_exit(kvm_run_ptr: *const u8) -> VmExit {
    let exit_reason = read_u32(kvm_run_ptr, kvm_sys::KVM_RUN_EXIT_REASON_OFFSET);

    match exit_reason {
        kvm_sys::KVM_EXIT_MMIO => {
            let mmio_ptr = kvm_run_ptr.add(kvm_sys::KVM_RUN_MMIO_OFFSET);
            let mmio = &*(mmio_ptr as *const kvm_sys::kvm_run_mmio);
            VmExit::Mmio {
                addr: mmio.phys_addr,
                data: mmio.data,
                len: mmio.len,
                is_write: mmio.is_write != 0,
            }
        }
        kvm_sys::KVM_EXIT_HLT => VmExit::Hlt,
        kvm_sys::KVM_EXIT_SHUTDOWN => VmExit::Shutdown,
        kvm_sys::KVM_EXIT_INTR => VmExit::Intr,
        kvm_sys::KVM_EXIT_IRQ_WINDOW_OPEN => VmExit::IrqWindowOpen,
        kvm_sys::KVM_EXIT_FAIL_ENTRY => {
            let reason = read_u64(kvm_run_ptr, kvm_sys::KVM_RUN_INTERNAL_OFFSET);
            VmExit::FailEntry {
                hardware_entry_failure_reason: reason,
            }
        }
        kvm_sys::KVM_EXIT_INTERNAL_ERROR => {
            let sub = read_u32(kvm_run_ptr, kvm_sys::KVM_RUN_INTERNAL_OFFSET);
            VmExit::InternalError { suberror: sub }
        }
        kvm_sys::KVM_EXIT_SYSTEM_EVENT => {
            let etype = read_u32(kvm_run_ptr, kvm_sys::KVM_RUN_SYSTEM_EVENT_OFFSET);
            VmExit::SystemEvent { event_type: etype }
        }
        kvm_sys::KVM_EXIT_DEBUG => VmExit::Debug,
        kvm_sys::KVM_EXIT_HYPERCALL => VmExit::Hypercall,
        other => VmExit::Unknown(other),
    }
}

/// Write MMIO response data back into the `kvm_run` MMIO data field.
///
/// # Safety
/// `kvm_run_ptr` must point to a valid `kvm_run` structure and the
/// last exit must have been `KVM_EXIT_MMIO` with `is_write == 0`.
pub(crate) unsafe fn set_mmio_response(kvm_run_ptr: *mut u8, value: u64, len: u32) {
    let mmio_ptr = kvm_run_ptr.add(kvm_sys::KVM_RUN_MMIO_OFFSET);
    let mmio = &mut *(mmio_ptr as *mut kvm_sys::kvm_run_mmio);
    let bytes = value.to_le_bytes();
    let n = (len as usize).min(8);
    mmio.data[..n].copy_from_slice(&bytes[..n]);
}

unsafe fn read_u32(base: *const u8, offset: usize) -> u32 {
    let ptr = base.add(offset) as *const u32;
    ptr.read_unaligned()
}

unsafe fn read_u64(base: *const u8, offset: usize) -> u64 {
    let ptr = base.add(offset) as *const u64;
    ptr.read_unaligned()
}
