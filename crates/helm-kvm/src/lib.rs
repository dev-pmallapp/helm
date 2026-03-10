//! # helm-kvm
//!
//! KVM backend for HELM — near-native guest CPU execution via
//! `/dev/kvm` on Linux AArch64 hosts.
//!
//! This crate wraps the KVM ioctl interface and provides:
//!
//! - [`KvmVm`] — virtual machine lifecycle (create, memory, vCPUs)
//! - [`KvmVcpu`] — register access, run loop, MMIO response
//! - [`GuestMemory`] / [`GuestMemoryRegion`] — mmap-backed guest RAM
//! - [`KvmGic`] — in-kernel GIC setup
//! - [`irq`] — IRQ injection helpers
//! - [`VmExit`] — decoded VM exit reasons
//!
//! # Example (sketch)
//!
//! ```rust,ignore
//! use helm_kvm::{KvmVm, GicConfig, VmExit};
//!
//! let mut vm = KvmVm::new()?;
//! vm.add_memory(0x4000_0000, 256 * 1024 * 1024)?; // 256 MB RAM
//!
//! let mut vcpu = vm.create_vcpu()?;
//! let target = vm.preferred_target()?;
//! vcpu.init(&target)?;
//! vcpu.set_pc(0x4000_0000)?;
//!
//! vm.setup_gic(GicConfig::default())?;
//!
//! loop {
//!     match vcpu.run()? {
//!         VmExit::Mmio { addr, data, len, is_write } => {
//!             // dispatch to device bus
//!         }
//!         VmExit::Shutdown => break,
//!         _ => {}
//!     }
//! }
//! ```

pub mod capability;
pub mod error;
pub mod exit;
pub mod gic;
pub mod irq;
pub mod kvm_sys;
pub mod memory;
pub mod vcpu;
pub mod vm;

pub use capability::KvmCaps;
pub use error::{KvmError, Result};
pub use exit::VmExit;
pub use gic::{GicConfig, GicVersion, KvmGic};
pub use irq::{assert_ppi, assert_spi, deassert_ppi, deassert_spi, ppi_irq, spi_irq};
pub use memory::{GuestMemory, GuestMemoryRegion};
pub use vcpu::{CoreRegs, KvmVcpu, SysRegs};
pub use vm::KvmVm;

#[cfg(test)]
mod tests;
