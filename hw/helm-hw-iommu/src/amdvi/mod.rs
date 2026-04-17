//! AMD-Vi (`IOMMUv2`) — AMD I/O Memory Management Unit.
//!
//! Stub implementation with register identification and bypass translation.
//! Full DTE/page-table walk to be implemented when needed.
//!
//! ## Translation policy
//!
//! When the IOMMU is **disabled** (`IommuEn` bit clear in the CONTROL
//! register), `translate()` returns `Bypass` — all DMA passes through
//! without address translation.  This matches real hardware behaviour on
//! reset.
//!
//! When the IOMMU is **enabled** (`IommuEn` bit set), the stub does not
//! yet implement DTE/page-table walks.  Rather than silently bypassing
//! (which would give the guest a false sense of DMA isolation), the stub
//! returns an explicit `Fault` with code `AMDVI_FAULT_TRANSLATION_NOT_IMPL`.

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

// ── CONTROL register bits (AMD IOMMU spec, section 2.4) ─────────────────────

/// Bit 0 of the CONTROL register — enables the IOMMU.
const CONTROL_IOMMU_EN: u64 = 1 << 0;

// ── Capability / identification values ──────────────────────────────────────

/// AMD IOMMU capability header: type=0x3 (IOMMU), rev=2.
const CAP_HEADER_VAL: u64 = 0x0000_0002_0000_0003;
/// Extended feature register: basic features.
const EXT_FEATURE_VAL: u64 = 0x0000_0000_0004_032B;
/// Fault code: translation walks are not yet implemented in this stub.
const AMDVI_FAULT_TRANSLATION_NOT_IMPL: u8 = 0x01;

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

    /// Returns `true` when the guest has set the `IommuEn` bit (bit 0) of
    /// the CONTROL register, indicating that the IOMMU should perform
    /// address translation on DMA traffic.
    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.control & CONTROL_IOMMU_EN != 0
    }

    /// Translate a DMA address.
    ///
    /// * **Disabled** (`IommuEn` bit clear) -- bypass; IOVA passes through
    ///   as PA.  This is the correct hardware-reset behaviour.
    /// * **Enabled** (`IommuEn` bit set) -- the stub does not implement
    ///   DTE/page-table walks.  Instead of silently bypassing (which would
    ///   give the guest a false sense of DMA isolation), return an explicit
    ///   translation fault.
    pub fn translate(
        &mut self,
        device_id: u32,
        iova: u64,
        is_write: bool,
    ) -> IommuTranslateResult {
        if !self.is_enabled() {
            return IommuTranslateResult::Bypass;
        }

        // The IOMMU is enabled but we have no DTE/page-table walk yet.
        log::warn!(
            "AMD-Vi: translate() called with IommuEn set but DTE walk not implemented \
             (device_id={device_id}, iova={iova:#x}, write={is_write}) -- faulting"
        );
        IommuTranslateResult::Fault(IommuFault {
            code: AMDVI_FAULT_TRANSLATION_NOT_IMPL,
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
                log::warn!("AMD-Vi: read from undefined register offset {offset:#x}");
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
                log::warn!("AMD-Vi: write to undefined register offset {offset:#x} val={val:#x}");
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
    fn default_state_is_zero() {
        let amdvi = AmdViState::new(TestMem::new(4096));
        assert_eq!(amdvi.control, 0);
        assert_eq!(amdvi.devtab_base, 0);
        assert_eq!(amdvi.cmdq_base, 0);
        assert_eq!(amdvi.evtlog_base, 0);
        assert!(!amdvi.is_enabled());
    }

    // ── Disabled bypass ────────────────────────────────────────────────────

    #[test]
    fn translate_bypasses_when_disabled_default() {
        let mut amdvi = AmdViState::new(TestMem::new(4096));
        // Default-reset state: IommuEn clear, everything zero.
        assert!(!amdvi.is_enabled());
        assert!(matches!(
            amdvi.translate(0, 0x1000, false),
            IommuTranslateResult::Bypass
        ));
    }

    #[test]
    fn translate_bypasses_when_devtab_set_but_iommu_disabled() {
        // Programming the device-table base without setting IommuEn should
        // still bypass -- the IOMMU is not yet armed.
        let mut amdvi = AmdViState::new(TestMem::new(4096));
        amdvi.devtab_base = 0x2000;
        assert!(!amdvi.is_enabled());
        assert!(matches!(
            amdvi.translate(5, 0x3000, false),
            IommuTranslateResult::Bypass
        ));
    }

    #[test]
    fn translate_bypasses_when_non_enable_control_bits_set() {
        // Setting other CONTROL bits (e.g. bit 4) without IommuEn should
        // still count as disabled.
        let mut amdvi = AmdViState::new(TestMem::new(4096));
        amdvi.control = 0x10; // some other bit, NOT IommuEn
        assert!(!amdvi.is_enabled());
        assert!(matches!(
            amdvi.translate(1, 0x4000, true),
            IommuTranslateResult::Bypass
        ));
    }

    // ── Enabled fault ──────────────────────────────────────────────────────

    #[test]
    fn translate_faults_when_iommu_enabled() {
        // IommuEn set -- the stub must NOT silently bypass.
        let mut amdvi = AmdViState::new(TestMem::new(4096));
        amdvi.control = CONTROL_IOMMU_EN;
        assert!(amdvi.is_enabled());
        match amdvi.translate(9, 0x1000, true) {
            IommuTranslateResult::Fault(fault) => {
                assert_eq!(fault.code, AMDVI_FAULT_TRANSLATION_NOT_IMPL);
                assert_eq!(fault.device_id, 9);
                assert_eq!(fault.input_addr, 0x1000);
                assert!(fault.is_write);
            }
            other => panic!("expected translation-not-impl fault, got {other:?}"),
        }
    }

    #[test]
    fn translate_faults_when_iommu_enabled_with_other_bits() {
        // IommuEn plus additional CONTROL bits -- still enabled.
        let mut amdvi = AmdViState::new(TestMem::new(4096));
        amdvi.control = CONTROL_IOMMU_EN | 0x30;
        amdvi.devtab_base = 0x8000;
        assert!(amdvi.is_enabled());
        assert!(matches!(
            amdvi.translate(0, 0x5000, false),
            IommuTranslateResult::Fault(_)
        ));
    }

    #[test]
    fn translate_faults_for_read_and_write() {
        let mut amdvi = AmdViState::new(TestMem::new(4096));
        amdvi.control = CONTROL_IOMMU_EN;

        // Read
        match amdvi.translate(3, 0xA000, false) {
            IommuTranslateResult::Fault(f) => assert!(!f.is_write),
            other => panic!("expected fault on read, got {other:?}"),
        }
        // Write
        match amdvi.translate(3, 0xA000, true) {
            IommuTranslateResult::Fault(f) => assert!(f.is_write),
            other => panic!("expected fault on write, got {other:?}"),
        }
    }

    // ── Register read/write round-trip ─────────────────────────────────────

    #[test]
    fn control_register_readback() {
        let mut amdvi = AmdViState::new(TestMem::new(4096));
        amdvi.write(CONTROL, 8, 0xDEAD);
        assert_eq!(amdvi.read(CONTROL, 8), 0xDEAD);
    }
}
