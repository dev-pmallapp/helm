//! RISC-V IOMMU — RISC-V I/O Memory Management Unit.
//!
//! Stub implementation with capability registers and bypass translation.
//! Full DDT/page-table walk to be implemented when needed.
//!
//! Register layout follows the RISC-V IOMMU Architecture Specification v1.0.
//!
//! ## Translation policy
//!
//! The DDTP register's `iommu_mode` field (bits [3:0]) determines the
//! translation mode:
//!
//! | Mode | Value | Behaviour in this stub |
//! |------|-------|------------------------|
//! | Off  |   0   | Bypass (hardware reset) |
//! | Bare |   1   | Bypass (spec: no translation) |
//! | 1LVL |   2   | **Fault** (walks not implemented) |
//! | 2LVL |   3   | **Fault** (walks not implemented) |
//! | 3LVL |   4   | **Fault** (walks not implemented) |
//!
//! Modes that imply address translation (1LVL/2LVL/3LVL) must never
//! silently bypass, as that would give the guest a false sense of DMA
//! isolation.

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

// ── DDTP iommu_mode field (bits [3:0]) ──────────────────────────────────────

/// DDTP mode mask: low 4 bits.
const DDTP_MODE_MASK: u64 = 0xF;
/// Off — IOMMU is disabled, all DMA bypasses.
const DDTP_MODE_OFF: u64 = 0;
/// Bare — no translation; IOVA is used as PA (spec-defined bypass).
const DDTP_MODE_BARE: u64 = 1;
/// 1-level device directory table.
const DDTP_MODE_1LVL: u64 = 2;
/// 2-level device directory table.
const DDTP_MODE_2LVL: u64 = 3;
/// 3-level device directory table.
const DDTP_MODE_3LVL: u64 = 4;

// ── Capability values ───────────────────────────────────────────────────────

/// Capabilities: version 1.0, Sv39 support, MSI flat, 44-bit PA.
const CAPABILITIES_VAL: u64 = 1             // version=1
    | (1 << 9)    // Sv39 supported
    | (1 << 10)   // Sv48 supported
    | (1 << 16)   // MSI flat
    | (44 << 32); // PA width = 44
/// Fault code: DDT/page-table walks are not yet implemented in this stub.
const RISCV_IOMMU_FAULT_TRANSLATION_NOT_IMPL: u8 = 0x01;

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

    /// Returns the `iommu_mode` field from the DDTP register (bits [3:0]).
    #[inline]
    pub fn ddtp_mode(&self) -> u64 {
        self.ddtp & DDTP_MODE_MASK
    }

    /// Returns `true` when the DDTP `iommu_mode` selects a translation
    /// mode that implies the IOMMU is actively performing address
    /// translation (1LVL, 2LVL, or 3LVL).
    #[inline]
    pub fn is_translation_enabled(&self) -> bool {
        matches!(
            self.ddtp_mode(),
            DDTP_MODE_1LVL | DDTP_MODE_2LVL | DDTP_MODE_3LVL
        )
    }

    /// Translate a DMA address.
    ///
    /// * **Off** (`iommu_mode` = 0) or **Bare** (`iommu_mode` = 1) --
    ///   bypass; IOVA passes through as PA.  Off is the hardware-reset
    ///   default; Bare is a spec-defined "no translation" mode.
    /// * **1LVL / 2LVL / 3LVL** (`iommu_mode` = 2/3/4) -- the stub does
    ///   not implement DDT/page-table walks.  Instead of silently bypassing
    ///   (which would give the guest a false sense of DMA isolation), return
    ///   an explicit translation fault.
    /// * **Reserved modes** (5-15) -- fault, since the behaviour is
    ///   undefined.
    pub fn translate(
        &mut self,
        device_id: u32,
        iova: u64,
        is_write: bool,
    ) -> IommuTranslateResult {
        let mode = self.ddtp_mode();
        match mode {
            DDTP_MODE_OFF | DDTP_MODE_BARE => IommuTranslateResult::Bypass,
            DDTP_MODE_1LVL | DDTP_MODE_2LVL | DDTP_MODE_3LVL => {
                log::warn!(
                    "RISC-V IOMMU: translate() called with iommu_mode={mode} but DDT walk \
                     not implemented (device_id={device_id}, iova={iova:#x}, \
                     write={is_write}) -- faulting"
                );
                IommuTranslateResult::Fault(IommuFault {
                    code: RISCV_IOMMU_FAULT_TRANSLATION_NOT_IMPL,
                    device_id,
                    input_addr: iova,
                    is_write,
                })
            }
            _ => {
                // Reserved iommu_mode values (5-15).
                log::warn!(
                    "RISC-V IOMMU: reserved iommu_mode={mode} in DDTP -- faulting"
                );
                IommuTranslateResult::Fault(IommuFault {
                    code: RISCV_IOMMU_FAULT_TRANSLATION_NOT_IMPL,
                    device_id,
                    input_addr: iova,
                    is_write,
                })
            }
        }
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
                log::warn!("RISC-V IOMMU: read from undefined register offset {offset:#x}");
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
                log::warn!("RISC-V IOMMU: write to undefined register offset {offset:#x} val={val:#x}");
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
    fn default_state_is_zero() {
        let iommu = RiscvIommuState::new(TestMem::new(4096));
        assert_eq!(iommu.fctl, 0);
        assert_eq!(iommu.ddtp, 0);
        assert_eq!(iommu.cqb, 0);
        assert_eq!(iommu.fqb, 0);
        assert_eq!(iommu.ddtp_mode(), DDTP_MODE_OFF);
        assert!(!iommu.is_translation_enabled());
    }

    // ── Disabled / bypass modes ────────────────────────────────────────────

    #[test]
    fn translate_bypasses_when_mode_off() {
        // Default-reset: ddtp=0 => iommu_mode=Off.
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        assert_eq!(iommu.ddtp_mode(), DDTP_MODE_OFF);
        assert!(!iommu.is_translation_enabled());
        assert!(matches!(
            iommu.translate(0, 0x1000, false),
            IommuTranslateResult::Bypass
        ));
    }

    #[test]
    fn translate_bypasses_when_mode_bare() {
        // Bare mode: iommu_mode=1 is a spec-defined no-translation mode.
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        iommu.ddtp = DDTP_MODE_BARE; // mode=1 in low bits
        assert_eq!(iommu.ddtp_mode(), DDTP_MODE_BARE);
        assert!(!iommu.is_translation_enabled());
        assert!(matches!(
            iommu.translate(7, 0x3000, false),
            IommuTranslateResult::Bypass
        ));
    }

    #[test]
    fn translate_bypasses_bare_even_with_high_bits_set() {
        // DDTP has a PPN in high bits but mode is still Bare.
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        iommu.ddtp = 0xDEAD_0000 | DDTP_MODE_BARE;
        assert_eq!(iommu.ddtp_mode(), DDTP_MODE_BARE);
        assert!(matches!(
            iommu.translate(0, 0x5000, true),
            IommuTranslateResult::Bypass
        ));
    }

    // ── Enabled / fault modes ──────────────────────────────────────────────

    #[test]
    fn translate_faults_when_mode_1lvl() {
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        iommu.ddtp = 0x4000 | DDTP_MODE_1LVL;
        assert!(iommu.is_translation_enabled());
        match iommu.translate(11, 0x2000, true) {
            IommuTranslateResult::Fault(fault) => {
                assert_eq!(fault.code, RISCV_IOMMU_FAULT_TRANSLATION_NOT_IMPL);
                assert_eq!(fault.device_id, 11);
                assert_eq!(fault.input_addr, 0x2000);
                assert!(fault.is_write);
            }
            other => panic!("expected translation-not-impl fault, got {other:?}"),
        }
    }

    #[test]
    fn translate_faults_when_mode_2lvl() {
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        iommu.ddtp = 0x8000 | DDTP_MODE_2LVL;
        assert!(iommu.is_translation_enabled());
        assert!(matches!(
            iommu.translate(0, 0x6000, false),
            IommuTranslateResult::Fault(_)
        ));
    }

    #[test]
    fn translate_faults_when_mode_3lvl() {
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        iommu.ddtp = 0xC000 | DDTP_MODE_3LVL;
        assert!(iommu.is_translation_enabled());
        assert!(matches!(
            iommu.translate(2, 0x7000, true),
            IommuTranslateResult::Fault(_)
        ));
    }

    #[test]
    fn translate_faults_for_reserved_mode() {
        // Mode values 5-15 are reserved; they must fault, not bypass.
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        iommu.ddtp = 0x1000 | 5; // reserved mode 5
        assert!(matches!(
            iommu.translate(0, 0x9000, false),
            IommuTranslateResult::Fault(_)
        ));
    }

    #[test]
    fn translate_faults_for_read_and_write() {
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        iommu.ddtp = 0x4000 | DDTP_MODE_1LVL;

        // Read
        match iommu.translate(3, 0xA000, false) {
            IommuTranslateResult::Fault(f) => assert!(!f.is_write),
            other => panic!("expected fault on read, got {other:?}"),
        }
        // Write
        match iommu.translate(3, 0xA000, true) {
            IommuTranslateResult::Fault(f) => assert!(f.is_write),
            other => panic!("expected fault on write, got {other:?}"),
        }
    }

    // ── Register read/write round-trip ─────────────────────────────────────

    #[test]
    fn ddtp_register_readback() {
        let mut iommu = RiscvIommuState::new(TestMem::new(4096));
        iommu.write(DDTP, 8, 0xBEEF);
        assert_eq!(iommu.read(DDTP, 8), 0xBEEF);
    }
}
