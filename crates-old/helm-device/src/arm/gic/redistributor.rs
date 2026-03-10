//! GICv3 Redistributor (GICR) — one instance per PE.
//!
//! The redistributor manages SGIs (0-15) and PPIs (16-31) for a single
//! processing element.  It exposes two 64 KB MMIO frames:
//!
//! - **RD\_base** (offset +0x00000): control, type, waker, LPI base.
//! - **SGI\_base** (offset +0x10000): enable, pending, priority, config
//!   for SGIs and PPIs.

use super::common::*;

// ── RD_base frame registers ────────────────────────────────────────────────

/// Redistributor Control Register.
pub const GICR_CTLR: u64 = 0x000;
/// Redistributor Implementer Identification Register.
pub const GICR_IIDR: u64 = 0x004;
/// Redistributor Type Register (64-bit, low word).
pub const GICR_TYPER_LO: u64 = 0x008;
/// Redistributor Type Register (64-bit, high word).
pub const GICR_TYPER_HI: u64 = 0x00C;
/// Redistributor Status Register.
pub const GICR_STATUSR: u64 = 0x010;
/// Redistributor Wake Register.
pub const GICR_WAKER: u64 = 0x014;
/// LPI Configuration Table base (64-bit).
pub const GICR_PROPBASER: u64 = 0x070;
/// LPI Pending Table base (64-bit).
pub const GICR_PENDBASER: u64 = 0x078;

// ── SGI_base frame registers (offsets relative to SGI_base) ────────────────

/// Interrupt Set-Enable Register 0 (SGI/PPI).
pub const GICR_ISENABLER0: u64 = 0x100;
/// Interrupt Clear-Enable Register 0.
pub const GICR_ICENABLER0: u64 = 0x180;
/// Interrupt Set-Pending Register 0.
pub const GICR_ISPENDR0: u64 = 0x200;
/// Interrupt Clear-Pending Register 0.
pub const GICR_ICPENDR0: u64 = 0x280;
/// Interrupt Set-Active Register 0.
pub const GICR_ISACTIVER0: u64 = 0x300;
/// Interrupt Clear-Active Register 0.
pub const GICR_ICACTIVER0: u64 = 0x380;
/// Interrupt Priority Registers (SGI/PPI, 32 bytes).
pub const GICR_IPRIORITYR_BASE: u64 = 0x400;
/// Interrupt Configuration Register 0 (SGIs).
pub const GICR_ICFGR0: u64 = 0xC00;
/// Interrupt Configuration Register 1 (PPIs).
pub const GICR_ICFGR1: u64 = 0xC04;

/// Stride between consecutive redistributors (RD_base + SGI_base).
pub const GICR_STRIDE: u64 = 0x20000;
/// Size of the RD_base frame.
pub const GICR_RD_SIZE: u64 = 0x10000;

/// Per-PE GIC Redistributor state.
pub struct GicRedistributor {
    /// PE index (0-based).
    pub pe_id: u32,
    /// 64-bit affinity value (`Aff3.Aff2.Aff1.Aff0`).
    pub affinity: u64,
    /// Whether this is the last redistributor in the chain.
    pub is_last: bool,
    /// `GICR_WAKER` value.
    pub waker: u32,
    /// SGI/PPI enable bits (IRQs 0-31).
    pub sgi_ppi_enabled: u32,
    /// SGI/PPI pending bits.
    pub sgi_ppi_pending: u32,
    /// SGI/PPI active bits.
    pub sgi_ppi_active: u32,
    /// SGI/PPI priority (one byte per IRQ, 32 entries).
    pub sgi_ppi_priority: [u8; 32],
    /// Interrupt configuration for SGIs.
    pub sgi_config: u32,
    /// Interrupt configuration for PPIs.
    pub ppi_config: u32,
    /// LPI configuration table base address.
    pub prop_baser: u64,
    /// LPI pending table base address.
    pub pend_baser: u64,
}

impl GicRedistributor {
    /// Create a redistributor for the given PE.
    pub fn new(pe_id: u32, is_last: bool) -> Self {
        Self {
            pe_id,
            affinity: pe_id as u64,
            is_last,
            waker: 0x06,
            sgi_ppi_enabled: 0,
            sgi_ppi_pending: 0,
            sgi_ppi_active: 0,
            sgi_ppi_priority: [0; 32],
            sgi_config: 0,
            ppi_config: 0,
            prop_baser: 0,
            pend_baser: 0,
        }
    }

    /// Whether this PE is awake (not in `ProcessorSleep`).
    pub fn is_awake(&self) -> bool {
        self.waker & 0x02 == 0
    }

    /// Check if any SGI/PPI is pending and enabled for this PE.
    pub fn has_pending_sgi_ppi(&self, priority_mask: u8) -> bool {
        self.highest_pending_sgi_ppi(priority_mask).is_some()
    }

    /// Find the highest-priority pending+enabled SGI/PPI on this PE.
    pub fn highest_pending_sgi_ppi(&self, priority_mask: u8) -> Option<u32> {
        let enabled = [self.sgi_ppi_enabled];
        let pending = [self.sgi_ppi_pending];
        highest_pending_in_range(
            &pending,
            &enabled,
            &self.sgi_ppi_priority,
            priority_mask,
            0..32,
        )
    }

    /// Set a SGI/PPI as pending.
    pub fn set_pending(&mut self, irq: u32) {
        if irq < 32 {
            self.sgi_ppi_pending |= 1 << irq;
        }
    }

    /// Clear a SGI/PPI pending bit.
    pub fn clear_pending(&mut self, irq: u32) {
        if irq < 32 {
            self.sgi_ppi_pending &= !(1 << irq);
        }
    }

    /// Set a SGI/PPI as active.
    pub fn set_active(&mut self, irq: u32) {
        if irq < 32 {
            self.sgi_ppi_active |= 1 << irq;
        }
    }

    /// Clear a SGI/PPI active bit.
    pub fn clear_active(&mut self, irq: u32) {
        if irq < 32 {
            self.sgi_ppi_active &= !(1 << irq);
        }
    }

    /// Handle a read in the RD\_base frame.
    pub fn read_rd(&self, offset: u64) -> u32 {
        match offset {
            GICR_CTLR => 0,
            GICR_IIDR => 0x0300_043B,
            GICR_TYPER_LO => self.typer_lo(),
            GICR_TYPER_HI => self.typer_hi(),
            GICR_STATUSR => 0,
            GICR_WAKER => self.waker,
            GICR_PROPBASER => self.prop_baser as u32,
            0x074 => (self.prop_baser >> 32) as u32,
            GICR_PENDBASER => self.pend_baser as u32,
            0x07C => (self.pend_baser >> 32) as u32,
            _ => 0,
        }
    }

    /// Handle a write in the RD\_base frame.
    pub fn write_rd(&mut self, offset: u64, value: u32) {
        match offset {
            GICR_WAKER => {
                let sleep = value & 0x02;
                self.waker = sleep | if sleep != 0 { 0x04 } else { 0x00 };
            }
            GICR_PROPBASER => {
                self.prop_baser = (self.prop_baser & 0xFFFF_FFFF_0000_0000) | value as u64;
            }
            0x074 => {
                self.prop_baser =
                    (self.prop_baser & 0x0000_0000_FFFF_FFFF) | ((value as u64) << 32);
            }
            GICR_PENDBASER => {
                self.pend_baser = (self.pend_baser & 0xFFFF_FFFF_0000_0000) | value as u64;
            }
            0x07C => {
                self.pend_baser =
                    (self.pend_baser & 0x0000_0000_FFFF_FFFF) | ((value as u64) << 32);
            }
            _ => {}
        }
    }

    /// Handle a read in the SGI\_base frame (offset relative to SGI\_base).
    pub fn read_sgi(&self, offset: u64) -> u32 {
        match offset {
            GICR_ISENABLER0 | GICR_ICENABLER0 => self.sgi_ppi_enabled,
            GICR_ISPENDR0 | GICR_ICPENDR0 => self.sgi_ppi_pending,
            GICR_ISACTIVER0 | GICR_ICACTIVER0 => self.sgi_ppi_active,
            off if (GICR_IPRIORITYR_BASE..GICR_IPRIORITYR_BASE + 32).contains(&off) => {
                byte_array_read4(
                    &self.sgi_ppi_priority,
                    (off - GICR_IPRIORITYR_BASE) as usize,
                )
            }
            GICR_ICFGR0 => self.sgi_config,
            GICR_ICFGR1 => self.ppi_config,
            _ => 0,
        }
    }

    /// Handle a write in the SGI\_base frame (offset relative to SGI\_base).
    pub fn write_sgi(&mut self, offset: u64, value: u32) {
        match offset {
            GICR_ISENABLER0 => self.sgi_ppi_enabled |= value,
            GICR_ICENABLER0 => self.sgi_ppi_enabled &= !value,
            GICR_ISPENDR0 => self.sgi_ppi_pending |= value,
            GICR_ICPENDR0 => self.sgi_ppi_pending &= !value,
            GICR_ISACTIVER0 => self.sgi_ppi_active |= value,
            GICR_ICACTIVER0 => self.sgi_ppi_active &= !value,
            off if (GICR_IPRIORITYR_BASE..GICR_IPRIORITYR_BASE + 32).contains(&off) => {
                byte_array_write4(
                    &mut self.sgi_ppi_priority,
                    (off - GICR_IPRIORITYR_BASE) as usize,
                    value,
                );
            }
            GICR_ICFGR0 => self.sgi_config = value,
            GICR_ICFGR1 => self.ppi_config = value,
            _ => {}
        }
    }

    /// Reset to power-on state.
    pub fn reset(&mut self) {
        self.waker = 0x06;
        self.sgi_ppi_enabled = 0;
        self.sgi_ppi_pending = 0;
        self.sgi_ppi_active = 0;
        self.sgi_ppi_priority = [0; 32];
        self.sgi_config = 0;
        self.ppi_config = 0;
        self.prop_baser = 0;
        self.pend_baser = 0;
    }

    /// Build the low 32 bits of `GICR_TYPER`.
    fn typer_lo(&self) -> u32 {
        let mut val = 0u32;
        val |= (self.affinity as u32 & 0xFF) << 24;
        val |= (self.pe_id & 0xFFFF) << 8;
        if self.is_last {
            val |= 1 << 4;
        }
        val
    }

    /// Build the high 32 bits of `GICR_TYPER`.
    fn typer_hi(&self) -> u32 {
        let aff1 = ((self.affinity >> 8) & 0xFF) as u32;
        let aff2 = ((self.affinity >> 16) & 0xFF) as u32;
        let aff3 = ((self.affinity >> 24) & 0xFF) as u32;
        (aff3 << 24) | (aff2 << 16) | (aff1 << 8)
    }
}
