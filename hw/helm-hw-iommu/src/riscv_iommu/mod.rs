//! RISC-V IOMMU — RISC-V I/O Memory Management Unit.
//!
//! Stub implementation with capability registers and bypass translation.
//! Full DDT/page-table walk to be implemented when needed.
//!
//! Register layout follows the RISC-V IOMMU Architecture Specification v1.0.

use helm_devices::Device;

use crate::common::fault::{IommuFault, IommuTranslateResult};
use crate::common::mem::ByteMem;
use crate::common::tlb::IommuTlb;

// ── Register offsets (RISC-V IOMMU spec, section 5) ─────────────────────────

const CAPABILITIES: u64 = 0x0000;
const FCTL: u64 = 0x0008;
const DDTP: u64 = 0x0010;
const CQB: u64 = 0x0018;
const CQH: u64 = 0x0020;
const CQT: u64 = 0x0028;
const FQB: u64 = 0x0030;
const FQH: u64 = 0x0038;
const FQT: u64 = 0x0040;
const IOCNTOVF: u64 = 0x0058;
const IOCNTINH: u64 = 0x0060;
const IOHPMCYCLES: u64 = 0x0068;

// ── Capability values ───────────────────────────────────────────────────────

/// Capabilities: version 1.0, Sv39 support, MSI flat, 44-bit PA.
const CAPABILITIES_VAL: u64 = 1             // version=1
    | (1 << 9)    // Sv39 supported
    | (1 << 10)   // Sv48 supported
    | (1 << 16)   // MSI flat
    | (44 << 32); // PA width = 44
/// Generic "translation unavailable" fault code for the stub path.
const RISCV_IOMMU_FAULT_UNSUPPORTED: u8 = 0xFF;

// ── RiscvIommuState ─────────────────────────────────────────────────────────

/// RISC-V IOMMU state.
///
/// Contains capability and control registers plus TLB cache. Currently
/// a bypass-only stub.
pub struct RiscvIommuState<M: ByteMem> {
    /// Feature control register.
    pub fctl: u64,
    /// Device Directory Table Pointer.
    pub ddtp: u64,
    /// Command queue base.
    pub cqb: u64,
    /// Command queue head.
    pub cqh: u64,
    /// Command queue tail.
    pub cqt: u64,
    /// Fault queue base.
    pub fqb: u64,
    /// Fault queue head.
    pub fqh: u64,
    /// Fault queue tail.
    pub fqt: u64,

    /// Software TLB cache.
    pub tlb: IommuTlb,

    /// Guest physical memory for table walks.
    pub mem: M,
}

impl<M: ByteMem> RiscvIommuState<M> {
    /// Create a new RISC-V IOMMU with default (disabled) state.
    pub fn new(mem: M) -> Self {
        Self {
            fctl: 0,
            ddtp: 0,
            cqb: 0,
            cqh: 0,
            cqt: 0,
            fqb: 0,
            fqh: 0,
            fqt: 0,
            tlb: IommuTlb::new(),
            mem,
        }
    }

    /// Translate a DMA address.
    ///
    /// Default-reset state bypasses to match an unconfigured IOMMU. Once the
    /// guest enables/configures the unit, the still-unimplemented walk path
    /// must fault rather than silently claim isolation.
    pub fn translate(
        &mut self,
        device_id: u32,
        iova: u64,
        is_write: bool,
    ) -> IommuTranslateResult {
        if self.fctl == 0 && self.ddtp == 0 {
            return IommuTranslateResult::Bypass;
        }

        IommuTranslateResult::Fault(IommuFault {
            code: RISCV_IOMMU_FAULT_UNSUPPORTED,
            device_id,
            input_addr: iova,
            is_write,
        })
    }
}

// ── Device trait ────────────────────────────────────────────────────────────

impl<M: ByteMem + Send + 'static> Device for RiscvIommuState<M> {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        match offset {
            CAPABILITIES => CAPABILITIES_VAL,
            FCTL => self.fctl,
            DDTP => self.ddtp,
            CQB => self.cqb,
            CQH => self.cqh,
            CQT => self.cqt,
            FQB => self.fqb,
            FQH => self.fqh,
            FQT => self.fqt,
            IOCNTOVF | IOCNTINH | IOHPMCYCLES => 0,
            _ => {
                log::trace!("RISC-V IOMMU: read from undefined offset {offset:#x}");
                0
            }
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        match offset {
            FCTL => self.fctl = val,
            DDTP => self.ddtp = val,
            CQB => self.cqb = val,
            CQH => self.cqh = val,
            CQT => self.cqt = val,
            FQB => self.fqb = val,
            FQH => self.fqh = val,
            FQT => self.fqt = val,
            _ => {
                log::trace!("RISC-V IOMMU: write to undefined offset {offset:#x} val={val:#x}");
            }
        }
    }

    fn region_size(&self) -> u64 {
        0x1000 // 4KB MMIO region
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::fault::IommuTranslateResult;
    use crate::common::mem::TestMem;

    #[test]
    fn region_size_is_4kb() {
        let iommu = RiscvIommuState::new(TestMem::new(4096));
        assert_eq!(iommu.region_size(), 0x1000);
    }

    #[test]
    fn capabilities_read() {
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        let caps = iommu.read(CAPABILITIES, 8);
        // Version bit set
        assert_ne!(caps & 0x1, 0);
        // Sv39 supported
        assert_ne!(caps & (1 << 9), 0);
    }

    #[test]
    fn fctl_write_readback() {
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        iommu.write(FCTL, 8, 0x42);
        assert_eq!(iommu.read(FCTL, 8), 0x42);
    }

    #[test]
    fn translate_bypasses_when_disabled() {
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        assert!(matches!(
            iommu.translate(0, 0x1000, false),
            IommuTranslateResult::Bypass
        ));
    }

    #[test]
    fn translate_faults_when_enabled_but_unimplemented() {
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        iommu.fctl = 1;
        iommu.ddtp = 0x4000;
        match iommu.translate(11, 0x2000, true) {
            IommuTranslateResult::Fault(fault) => {
                assert_eq!(fault.code, RISCV_IOMMU_FAULT_UNSUPPORTED);
                assert_eq!(fault.device_id, 11);
                assert_eq!(fault.input_addr, 0x2000);
                assert!(fault.is_write);
            }
            other => panic!("expected unsupported fault, got {other:?}"),
        }
    }

    #[test]
    fn default_state_is_zero() {
        let iommu = RiscvIommuState::new(TestMem::new(4096));
        assert_eq!(iommu.fctl, 0);
        assert_eq!(iommu.ddtp, 0);
        assert_eq!(iommu.cqb, 0);
        assert_eq!(iommu.fqb, 0);
    }
}
