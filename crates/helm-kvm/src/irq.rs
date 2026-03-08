//! IRQ injection via KVM.
//!
//! Provides helpers for asserting and de-asserting interrupt lines on
//! the in-kernel interrupt controller (GIC).

use crate::error::{KvmError, Result};
use crate::kvm_sys::{self, kvm_irq_level};
use std::os::unix::io::RawFd;

/// Assert or de-assert an IRQ line on the in-kernel interrupt controller.
///
/// On AArch64 the `irq` field is encoded as:
///
/// ```text
///   bits [31:24]  — unused (0)
///   bits [23:16]  — SPI/PPI type (0=SPI, 1=PPI)
///   bits [15:8]   — vCPU index (for PPI only)
///   bits [7:0]    — IRQ number
/// ```
///
/// For SPI (Shared Peripheral Interrupt), use [`spi_irq`].
/// For PPI (Private Peripheral Interrupt), use [`ppi_irq`].
pub fn irq_line(vm_fd: RawFd, irq: u32, level: bool) -> Result<()> {
    let irq_level = kvm_irq_level {
        irq,
        level: level as u32,
    };
    unsafe { kvm_sys::kvm_ioctl(vm_fd, kvm_sys::KVM_IRQ_LINE, &irq_level as *const _ as u64) }
        .map_err(|e| KvmError::Ioctl {
            name: "KVM_IRQ_LINE",
            source: e,
        })?;
    Ok(())
}

/// Encode a Shared Peripheral Interrupt (SPI) number for `KVM_IRQ_LINE`.
///
/// SPI numbers are in the range 0–987 (GIC SPI IDs 32–1019).
/// `spi_num` is the SPI index (0-based; maps to GIC INTID 32 + spi_num).
pub const fn spi_irq(spi_num: u32) -> u32 {
    spi_num & 0xFF
}

/// Encode a Private Peripheral Interrupt (PPI) number for `KVM_IRQ_LINE`.
///
/// PPIs are per-vCPU.  `ppi_num` is the PPI index (0–15; maps to
/// GIC INTID 16 + ppi_num).  `vcpu_idx` selects the target vCPU.
pub const fn ppi_irq(ppi_num: u32, vcpu_idx: u32) -> u32 {
    (1 << 24) | ((vcpu_idx & 0xFF) << 16) | (ppi_num & 0xFF)
}

/// Assert (raise) an SPI.
pub fn assert_spi(vm_fd: RawFd, spi_num: u32) -> Result<()> {
    irq_line(vm_fd, spi_irq(spi_num), true)
}

/// De-assert (lower) an SPI.
pub fn deassert_spi(vm_fd: RawFd, spi_num: u32) -> Result<()> {
    irq_line(vm_fd, spi_irq(spi_num), false)
}

/// Assert (raise) a PPI on a specific vCPU.
pub fn assert_ppi(vm_fd: RawFd, ppi_num: u32, vcpu_idx: u32) -> Result<()> {
    irq_line(vm_fd, ppi_irq(ppi_num, vcpu_idx), true)
}

/// De-assert (lower) a PPI on a specific vCPU.
pub fn deassert_ppi(vm_fd: RawFd, ppi_num: u32, vcpu_idx: u32) -> Result<()> {
    irq_line(vm_fd, ppi_irq(ppi_num, vcpu_idx), false)
}
