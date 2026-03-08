//! Raw KVM ioctl numbers and C-compatible struct definitions.
//!
//! These are transcribed from `<linux/kvm.h>` and the AArch64 KVM
//! headers.  We define them locally to avoid pulling in a full
//! kernel-headers bindgen dependency.

#![allow(non_camel_case_types, dead_code)]

use std::os::unix::io::RawFd;

// ── ioctl helpers ───────────────────────────────────────────────────────────

/// `_IO(type, nr)` — no-direction ioctl.
const fn io(ty: u32, nr: u32) -> libc::c_ulong {
    ((ty << 8) | nr) as libc::c_ulong
}

/// `_IOW(type, nr, size)` — write ioctl.
const fn iow(ty: u32, nr: u32, size: usize) -> libc::c_ulong {
    (0x4000_0000 | ((size as u32 & 0x3FFF) << 16) | (ty << 8) | nr) as libc::c_ulong
}

/// `_IOR(type, nr, size)` — read ioctl.
const fn ior(ty: u32, nr: u32, size: usize) -> libc::c_ulong {
    (0x8000_0000 | ((size as u32 & 0x3FFF) << 16) | (ty << 8) | nr) as libc::c_ulong
}

/// `_IOWR(type, nr, size)` — read+write ioctl.
const fn iowr(ty: u32, nr: u32, size: usize) -> libc::c_ulong {
    (0xC000_0000 | ((size as u32 & 0x3FFF) << 16) | (ty << 8) | nr) as libc::c_ulong
}

const KVMIO: u32 = 0xAE;

// ── System ioctls (/dev/kvm fd) ─────────────────────────────────────────────

/// Returns the KVM API version (must be 12).
pub const KVM_GET_API_VERSION: libc::c_ulong = io(KVMIO, 0x00);

/// Create a new VM, returns a VM fd.
pub const KVM_CREATE_VM: libc::c_ulong = io(KVMIO, 0x01);

/// Check whether an extension/capability is supported.
pub const KVM_CHECK_EXTENSION: libc::c_ulong = io(KVMIO, 0x03);

/// Returns the size of the mmap region for `KVM_RUN`.
pub const KVM_GET_VCPU_MMAP_SIZE: libc::c_ulong = io(KVMIO, 0x04);

// ── VM ioctls (VM fd) ───────────────────────────────────────────────────────

/// Register a guest physical memory region.
pub const KVM_SET_USER_MEMORY_REGION: libc::c_ulong = iow(
    KVMIO,
    0x46,
    std::mem::size_of::<kvm_userspace_memory_region>(),
);

/// Create a vCPU, returns a vCPU fd.
pub const KVM_CREATE_VCPU: libc::c_ulong = io(KVMIO, 0x41);

/// Set an IRQ level on the in-kernel interrupt controller.
pub const KVM_IRQ_LINE: libc::c_ulong = iow(KVMIO, 0x61, std::mem::size_of::<kvm_irq_level>());

/// Create an in-kernel device (GIC, etc.).
pub const KVM_CREATE_DEVICE: libc::c_ulong =
    iowr(KVMIO, 0xE0, std::mem::size_of::<kvm_create_device>());

/// Set attributes on an in-kernel device.
pub const KVM_SET_DEVICE_ATTR: libc::c_ulong =
    iow(KVMIO, 0xE1, std::mem::size_of::<kvm_device_attr>());

/// Check whether a device attribute is supported.
pub const KVM_HAS_DEVICE_ATTR: libc::c_ulong =
    iow(KVMIO, 0xE2, std::mem::size_of::<kvm_device_attr>());

// ── vCPU ioctls (vCPU fd) ───────────────────────────────────────────────────

/// Run the vCPU until a VM exit.
pub const KVM_RUN: libc::c_ulong = io(KVMIO, 0x80);

/// Read a single register.
pub const KVM_GET_ONE_REG: libc::c_ulong = iow(KVMIO, 0xAB, std::mem::size_of::<kvm_one_reg>());

/// Write a single register.
pub const KVM_SET_ONE_REG: libc::c_ulong = iow(KVMIO, 0xAC, std::mem::size_of::<kvm_one_reg>());

/// Preferred target CPU type for `KVM_ARM_VCPU_INIT`.
pub const KVM_ARM_PREFERRED_TARGET: libc::c_ulong =
    ior(KVMIO, 0xAF, std::mem::size_of::<kvm_vcpu_init>());

/// Initialise a vCPU with the given target and feature bitmap.
pub const KVM_ARM_VCPU_INIT: libc::c_ulong = iow(KVMIO, 0xAE, std::mem::size_of::<kvm_vcpu_init>());

// ── Capabilities ────────────────────────────────────────────────────────────

pub const KVM_CAP_USER_MEMORY: u32 = 3;
pub const KVM_CAP_IRQCHIP: u32 = 0;
pub const KVM_CAP_ARM_EL1_32BIT: u32 = 105;
pub const KVM_CAP_ARM_PMU_V3: u32 = 126;
pub const KVM_CAP_ARM_PSCI_0_2: u32 = 102;
pub const KVM_CAP_ONE_REG: u32 = 70;
pub const KVM_CAP_ARM_VM_IPA_SIZE: u32 = 165;

// ── KVM exit reasons ────────────────────────────────────────────────────────

pub const KVM_EXIT_UNKNOWN: u32 = 0;
pub const KVM_EXIT_EXCEPTION: u32 = 1;
pub const KVM_EXIT_IO: u32 = 2;
pub const KVM_EXIT_HYPERCALL: u32 = 3;
pub const KVM_EXIT_DEBUG: u32 = 4;
pub const KVM_EXIT_HLT: u32 = 5;
pub const KVM_EXIT_MMIO: u32 = 6;
pub const KVM_EXIT_IRQ_WINDOW_OPEN: u32 = 7;
pub const KVM_EXIT_SHUTDOWN: u32 = 8;
pub const KVM_EXIT_FAIL_ENTRY: u32 = 9;
pub const KVM_EXIT_INTR: u32 = 10;
pub const KVM_EXIT_INTERNAL_ERROR: u32 = 17;
pub const KVM_EXIT_SYSTEM_EVENT: u32 = 24;

// ── AArch64 register encoding ───────────────────────────────────────────────

/// KVM register space — ARM64.
pub const KVM_REG_ARM64: u64 = 0x6000_0000_0000_0000;
/// 64-bit register width tag.
pub const KVM_REG_SIZE_U64: u64 = 0x0030_0000_0000_0000;
/// 32-bit register width tag.
pub const KVM_REG_SIZE_U32: u64 = 0x0020_0000_0000_0000;
/// Core register group.
pub const KVM_REG_ARM_CORE: u64 = 0x0000_0000_0001_0000;
/// System register group (op0/op1/crn/crm/op2 encoded).
pub const KVM_REG_ARM64_SYSREG: u64 = 0x0000_0000_0013_0000;

/// Build a core-register ID for a 64-bit register at the given u32
/// offset within `struct kvm_regs`.
pub const fn arm64_core_reg(offset: u64) -> u64 {
    KVM_REG_ARM64 | KVM_REG_SIZE_U64 | KVM_REG_ARM_CORE | offset
}

/// Build a system register ID from (op0, op1, crn, crm, op2).
pub const fn arm64_sys_reg(op0: u64, op1: u64, crn: u64, crm: u64, op2: u64) -> u64 {
    KVM_REG_ARM64
        | KVM_REG_SIZE_U64
        | KVM_REG_ARM64_SYSREG
        | ((op0 & 3) << 14)
        | ((op1 & 7) << 11)
        | ((crn & 0xF) << 7)
        | ((crm & 0xF) << 3)
        | (op2 & 7)
}

// Core register offsets (in u32 units within `struct kvm_regs`).
// X0–X30 each occupy 2 u32 slots.

/// Register ID for `Xn` (n = 0..30).
pub const fn reg_id_xn(n: u64) -> u64 {
    arm64_core_reg(n * 2)
}

/// Register ID for `SP` (stack pointer).
pub const REG_ID_SP: u64 = arm64_core_reg(62);
/// Register ID for `PC` (program counter).
pub const REG_ID_PC: u64 = arm64_core_reg(64);
/// Register ID for `PSTATE`.
pub const REG_ID_PSTATE: u64 = arm64_core_reg(66);
/// Register ID for `SP_EL1`.
pub const REG_ID_SP_EL1: u64 = arm64_core_reg(68);

// Common system registers.

/// SCTLR_EL1 — System Control Register (EL1).
pub const SYSREG_SCTLR_EL1: u64 = arm64_sys_reg(3, 0, 1, 0, 0);
/// TCR_EL1 — Translation Control Register (EL1).
pub const SYSREG_TCR_EL1: u64 = arm64_sys_reg(3, 0, 2, 0, 2);
/// TTBR0_EL1 — Translation Table Base Register 0 (EL1).
pub const SYSREG_TTBR0_EL1: u64 = arm64_sys_reg(3, 0, 2, 0, 0);
/// TTBR1_EL1 — Translation Table Base Register 1 (EL1).
pub const SYSREG_TTBR1_EL1: u64 = arm64_sys_reg(3, 0, 2, 0, 1);
/// MAIR_EL1 — Memory Attribute Indirection Register (EL1).
pub const SYSREG_MAIR_EL1: u64 = arm64_sys_reg(3, 0, 10, 2, 0);
/// VBAR_EL1 — Vector Base Address Register (EL1).
pub const SYSREG_VBAR_EL1: u64 = arm64_sys_reg(3, 0, 12, 0, 0);
/// ELR_EL1 — Exception Link Register (EL1).
pub const SYSREG_ELR_EL1: u64 = arm64_sys_reg(3, 0, 4, 0, 1);
/// SPSR_EL1 — Saved Program Status Register (EL1).
pub const SYSREG_SPSR_EL1: u64 = arm64_sys_reg(3, 0, 4, 0, 0);
/// ESR_EL1 — Exception Syndrome Register (EL1).
pub const SYSREG_ESR_EL1: u64 = arm64_sys_reg(3, 0, 5, 2, 0);
/// FAR_EL1 — Fault Address Register (EL1).
pub const SYSREG_FAR_EL1: u64 = arm64_sys_reg(3, 0, 6, 0, 0);
/// CNTV_CTL_EL0 — Counter-timer Virtual Timer Control.
pub const SYSREG_CNTV_CTL_EL0: u64 = arm64_sys_reg(3, 3, 14, 3, 1);
/// CNTV_CVAL_EL0 — Counter-timer Virtual Timer Compare Value.
pub const SYSREG_CNTV_CVAL_EL0: u64 = arm64_sys_reg(3, 3, 14, 3, 2);
/// CNTP_CTL_EL0 — Counter-timer Physical Timer Control.
pub const SYSREG_CNTP_CTL_EL0: u64 = arm64_sys_reg(3, 3, 14, 2, 1);
/// CNTP_CVAL_EL0 — Counter-timer Physical Timer Compare Value.
pub const SYSREG_CNTP_CVAL_EL0: u64 = arm64_sys_reg(3, 3, 14, 2, 2);

// ── In-kernel device types ──────────────────────────────────────────────────

/// GICv2 device type for `KVM_CREATE_DEVICE`.
pub const KVM_DEV_TYPE_ARM_VGIC_V2: u32 = 5;
/// GICv3 device type for `KVM_CREATE_DEVICE`.
pub const KVM_DEV_TYPE_ARM_VGIC_V3: u32 = 7;

/// Device attribute group: GIC distributor.
pub const KVM_DEV_ARM_VGIC_GRP_ADDR: u32 = 0;
/// Device attribute group: GIC initialisation.
pub const KVM_DEV_ARM_VGIC_GRP_CTRL: u32 = 4;
/// Number of IRQs attribute.
pub const KVM_DEV_ARM_VGIC_GRP_NR_IRQS: u32 = 3;

/// Address type: GICv2 distributor.
pub const KVM_VGIC_V2_ADDR_TYPE_DIST: u64 = 0;
/// Address type: GICv2 CPU interface.
pub const KVM_VGIC_V2_ADDR_TYPE_CPU: u64 = 1;
/// Address type: GICv3 distributor.
pub const KVM_VGIC_V3_ADDR_TYPE_DIST: u64 = 0;
/// Address type: GICv3 redistributor.
pub const KVM_VGIC_V3_ADDR_TYPE_REDIST: u64 = 1;

/// Control attribute: initialise the VGIC.
pub const KVM_DEV_ARM_VGIC_CTRL_INIT: u64 = 0;

// ── KVM_ARM_VCPU feature bits ───────────────────────────────────────────────

/// PSCI v0.2 support.
pub const KVM_ARM_VCPU_PSCI_0_2: u32 = 1;
/// Power-off the vCPU at creation (secondary cores).
pub const KVM_ARM_VCPU_POWER_OFF: u32 = 0;

// ── C-compatible structs ────────────────────────────────────────────────────

/// Guest physical memory region descriptor.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct kvm_userspace_memory_region {
    pub slot: u32,
    pub flags: u32,
    pub guest_phys_addr: u64,
    pub memory_size: u64,
    pub userspace_addr: u64,
}

/// Single-register access descriptor.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct kvm_one_reg {
    pub id: u64,
    pub addr: u64,
}

/// IRQ level descriptor for `KVM_IRQ_LINE`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct kvm_irq_level {
    pub irq: u32,
    pub level: u32,
}

/// vCPU initialisation descriptor for `KVM_ARM_VCPU_INIT`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct kvm_vcpu_init {
    pub target: u32,
    pub features: [u32; 7],
}

/// Device creation descriptor for `KVM_CREATE_DEVICE`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct kvm_create_device {
    pub type_: u32,
    pub fd: u32,
    pub flags: u32,
}

/// Device attribute descriptor.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct kvm_device_attr {
    pub flags: u32,
    pub group: u32,
    pub attr: u64,
    pub addr: u64,
}

// ── MMIO exit data layout ───────────────────────────────────────────────────
// These offsets are relative to the start of `struct kvm_run`.

/// Offset of `exit_reason` within `kvm_run`.
pub const KVM_RUN_EXIT_REASON_OFFSET: usize = 8;

/// Offset of the MMIO sub-struct within `kvm_run`.
/// On both x86-64 and AArch64 Linux, the mmio union member starts
/// at offset 16 within the anonymous `union { ... }` which itself
/// starts at a fixed point in `kvm_run`.  The actual offset depends
/// on kernel version; we use the standard value.
pub const KVM_RUN_MMIO_OFFSET: usize = 64;

/// Layout of the MMIO exit data.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct kvm_run_mmio {
    pub phys_addr: u64,
    pub data: [u8; 8],
    pub len: u32,
    pub is_write: u8,
}

/// Offset of the `internal.suberror` field.
pub const KVM_RUN_INTERNAL_OFFSET: usize = 64;

/// Offset of the `system_event.type` field.
pub const KVM_RUN_SYSTEM_EVENT_OFFSET: usize = 64;

// ── Safe ioctl wrapper ──────────────────────────────────────────────────────

/// Perform a raw ioctl.  Returns the result or an OS error.
pub(crate) unsafe fn kvm_ioctl(
    fd: RawFd,
    request: libc::c_ulong,
    arg: u64,
) -> std::io::Result<i32> {
    let ret = libc::ioctl(fd, request, arg);
    if ret < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}
