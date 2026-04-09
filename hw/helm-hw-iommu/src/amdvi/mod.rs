//! AMD-Vi (`IOMMUv2`) — AMD I/O Memory Management Unit.
//!
//! Stub implementation with register identification and bypass translation.
//! Full DTE/page-table walk to be implemented when needed.

use helm_devices::Device;

use crate::common::fault::{IommuFault, IommuTranslateResult};
use crate::common::mem::ByteMem;
use crate::common::tlb::IommuTlb;

// ── Register offsets (AMD IOMMU spec, Table 2) ──────────────────────────────

const DEVTAB_BASE: u64 = 0x0000;
const CMDQ_BASE: u64 = 0x0008;
const EVTLOG_BASE: u64 = 0x0010;
const CONTROL: u64 = 0x0018;
const EXCL_BASE: u64 = 0x0020;
const EXCL_LIMIT: u64 = 0x0028;
const EXT_FEATURE: u64 = 0x0030;
const CAP_HEADER: u64 = 0x0040;

// ── Capability / identification values ──────────────────────────────────────

/// AMD IOMMU capability header: type=0x3 (IOMMU), rev=2.
const CAP_HEADER_VAL: u64 = 0x0000_0002_0000_0003;
/// Extended feature register: basic features.
const EXT_FEATURE_VAL: u64 = 0x0000_0000_0004_032B;
/// Generic "translation unavailable" fault code for the stub path.
const AMDVI_FAULT_UNSUPPORTED: u8 = 0xFF;

// ── AmdViState ──────────────────────────────────────────────────────────────

/// AMD-Vi (`IOMMUv2`) state.
///
/// Contains control registers and TLB cache. Table walks read guest
/// memory via the shared byte-memory contract. Currently a bypass-only stub.
pub struct AmdViState<M: ByteMem> {
    /// MMIO control register.
    pub control: u64,
    /// Device table base address register.
    pub devtab_base: u64,
    /// Command buffer base address register.
    pub cmdq_base: u64,
    /// Event log base address register.
    pub evtlog_base: u64,
    /// Exclusion range base.
    pub excl_base: u64,
    /// Exclusion range limit.
    pub excl_limit: u64,

    /// Software TLB cache.
    pub tlb: IommuTlb,

    /// Guest physical memory for table walks.
    pub mem: M,
}

impl<M: ByteMem> AmdViState<M> {
    /// Create a new AMD-Vi with default (disabled) state.
    pub fn new(mem: M) -> Self {
        Self {
            control: 0,
            devtab_base: 0,
            cmdq_base: 0,
            evtlog_base: 0,
            excl_base: 0,
            excl_limit: 0,
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
        if self.control == 0 && self.devtab_base == 0 {
            return IommuTranslateResult::Bypass;
        }

        IommuTranslateResult::Fault(IommuFault {
            code: AMDVI_FAULT_UNSUPPORTED,
            device_id,
            input_addr: iova,
            is_write,
        })
    }
}

// ── Device trait ────────────────────────────────────────────────────────────

impl<M: ByteMem + Send + 'static> Device for AmdViState<M> {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        match offset {
            CAP_HEADER => CAP_HEADER_VAL,
            EXT_FEATURE => EXT_FEATURE_VAL,
            DEVTAB_BASE => self.devtab_base,
            CMDQ_BASE => self.cmdq_base,
            EVTLOG_BASE => self.evtlog_base,
            CONTROL => self.control,
            EXCL_BASE => self.excl_base,
            EXCL_LIMIT => self.excl_limit,
            _ => {
                log::trace!("AMD-Vi: read from undefined offset {offset:#x}");
                0
            }
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        match offset {
            DEVTAB_BASE => self.devtab_base = val,
            CMDQ_BASE => self.cmdq_base = val,
            EVTLOG_BASE => self.evtlog_base = val,
            CONTROL => self.control = val,
            EXCL_BASE => self.excl_base = val,
            EXCL_LIMIT => self.excl_limit = val,
            _ => {
                log::trace!("AMD-Vi: write to undefined offset {offset:#x} val={val:#x}");
            }
        }
    }

    fn region_size(&self) -> u64 {
        0x4_0000 // 256KB MMIO region
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::fault::IommuTranslateResult;
    use crate::common::mem::TestMem;

    #[test]
    fn region_size_is_256kb() {
        let amdvi = AmdViState::new(TestMem::new(4096));
        assert_eq!(amdvi.region_size(), 0x4_0000);
    }

    #[test]
    fn cap_header_read() {
        let mut amdvi = AmdViState::new(TestMem::new(4096));
        assert_eq!(amdvi.read(CAP_HEADER, 8), CAP_HEADER_VAL);
    }

    #[test]
    fn ext_feature_read() {
        let mut amdvi = AmdViState::new(TestMem::new(4096));
        assert_eq!(amdvi.read(EXT_FEATURE, 8), EXT_FEATURE_VAL);
    }

    #[test]
    fn translate_bypasses_when_disabled() {
        let mut amdvi = AmdViState::new(TestMem::new(4096));
        assert!(matches!(
            amdvi.translate(0, 0x1000, false),
            IommuTranslateResult::Bypass
        ));
    }

    #[test]
    fn translate_faults_when_enabled_but_unimplemented() {
        let mut amdvi = AmdViState::new(TestMem::new(4096));
        amdvi.control = 1;
        amdvi.devtab_base = 0x2000;
        match amdvi.translate(9, 0x1000, true) {
            IommuTranslateResult::Fault(fault) => {
                assert_eq!(fault.code, AMDVI_FAULT_UNSUPPORTED);
                assert_eq!(fault.device_id, 9);
                assert_eq!(fault.input_addr, 0x1000);
                assert!(fault.is_write);
            }
            other => panic!("expected unsupported fault, got {other:?}"),
        }
    }

    #[test]
    fn default_state_is_zero() {
        let amdvi = AmdViState::new(TestMem::new(4096));
        assert_eq!(amdvi.control, 0);
        assert_eq!(amdvi.devtab_base, 0);
        assert_eq!(amdvi.cmdq_base, 0);
        assert_eq!(amdvi.evtlog_base, 0);
    }
}
