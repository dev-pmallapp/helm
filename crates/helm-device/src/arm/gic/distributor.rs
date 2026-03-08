//! GIC Distributor (GICD) — shared register state for GICv2 and GICv3.
//!
//! The distributor manages SPI (Shared Peripheral Interrupt) enable,
//! pending, active, priority, target/routing, and configuration state.

use super::common::*;
use super::GicVersion;

// ── GICD register offsets ───────────────────────────────────────────────────

/// Distributor Control Register.
pub const GICD_CTLR: u64 = 0x000;
/// Interrupt Controller Type Register.
pub const GICD_TYPER: u64 = 0x004;
/// Distributor Implementer Identification Register.
pub const GICD_IIDR: u64 = 0x008;
/// Interrupt Set-Enable Registers (base).
pub const GICD_ISENABLER_BASE: u64 = 0x100;
/// Interrupt Clear-Enable Registers (base).
pub const GICD_ICENABLER_BASE: u64 = 0x180;
/// Interrupt Set-Pending Registers (base).
pub const GICD_ISPENDR_BASE: u64 = 0x200;
/// Interrupt Clear-Pending Registers (base).
pub const GICD_ICPENDR_BASE: u64 = 0x280;
/// Interrupt Set-Active Registers (base).
pub const GICD_ISACTIVER_BASE: u64 = 0x300;
/// Interrupt Clear-Active Registers (base).
pub const GICD_ICACTIVER_BASE: u64 = 0x380;
/// Interrupt Priority Registers (base).
pub const GICD_IPRIORITYR_BASE: u64 = 0x400;
/// Interrupt Processor Targets Registers (base, GICv2 only).
pub const GICD_ITARGETSR_BASE: u64 = 0x800;
/// Interrupt Configuration Registers (base).
pub const GICD_ICFGR_BASE: u64 = 0xC00;
/// Interrupt Group Modifier Registers (base, GICv3 only).
pub const GICD_IGRPMODR_BASE: u64 = 0xD00;
/// Interrupt Routing Registers (base, GICv3 only, 64-bit per SPI).
pub const GICD_IROUTER_BASE: u64 = 0x6100;
/// Peripheral ID2 Register.
pub const GICD_PIDR2: u64 = 0xFFE8;

/// GIC Distributor state.
pub struct GicDistributor {
    /// GIC architecture version that governs register semantics.
    pub version: GicVersion,
    /// Number of supported IRQs (rounded up to multiple of 32, max 1020).
    pub num_irqs: u32,
    /// GICD_CTLR value.
    pub ctrl: u32,
    /// Enable bitmap (1 bit per IRQ).
    pub enabled: Vec<u32>,
    /// Pending bitmap.
    pub pending: Vec<u32>,
    /// Active bitmap.
    pub active: Vec<u32>,
    /// Priority (8 bits per IRQ).
    pub priority: Vec<u8>,
    /// Target CPU mask (8 bits per IRQ) — used by GICv2.
    pub targets: Vec<u8>,
    /// Interrupt configuration (2 bits per IRQ).
    pub config: Vec<u32>,
    /// Group modifier registers — GICv3 only.
    pub group_mod: Vec<u32>,
    /// Affinity routing (64 bits per SPI) — GICv3 only.
    pub irouter: Vec<u64>,
}

impl GicDistributor {
    /// Create a distributor for the given GIC version and IRQ count.
    pub fn new(version: GicVersion, num_irqs: u32) -> Self {
        let nirq = (num_irqs.div_ceil(32) * 32).min(MAX_IRQS) as usize;
        let num_spis = nirq.saturating_sub(SPI_START as usize);
        Self {
            version,
            num_irqs: nirq as u32,
            ctrl: 0,
            enabled: vec![0u32; nirq / 32],
            pending: vec![0u32; nirq / 32],
            active: vec![0u32; nirq / 32],
            priority: vec![0u8; nirq],
            targets: vec![1u8; nirq],
            config: vec![0u32; nirq / 16],
            group_mod: if version.is_v3_or_later() {
                vec![0u32; nirq / 32]
            } else {
                Vec::new()
            },
            irouter: if version.is_v3_or_later() {
                vec![0u64; num_spis]
            } else {
                Vec::new()
            },
        }
    }

    /// Whether the distributor is enabled.
    pub fn is_enabled(&self) -> bool {
        self.ctrl & 1 != 0
    }

    /// Whether Affinity Routing Enable is set (GICv3 `ARE_S` / `ARE_NS`).
    pub fn are_enabled(&self) -> bool {
        self.version.is_v3_or_later() && (self.ctrl & (1 << 4)) != 0
    }

    /// Check whether a given IRQ is enabled.
    pub fn is_irq_enabled(&self, irq: u32) -> bool {
        bitmap_is_set(&self.enabled, irq)
    }

    /// Check whether a given IRQ is pending.
    pub fn is_irq_pending(&self, irq: u32) -> bool {
        bitmap_is_set(&self.pending, irq)
    }

    /// Set an IRQ as pending.
    pub fn set_pending(&mut self, irq: u32) {
        bitmap_set(&mut self.pending, irq);
    }

    /// Clear an IRQ's pending bit.
    pub fn clear_pending(&mut self, irq: u32) {
        bitmap_clear(&mut self.pending, irq);
    }

    /// Set an IRQ as active.
    pub fn set_active(&mut self, irq: u32) {
        bitmap_set(&mut self.active, irq);
    }

    /// Clear an IRQ's active bit.
    pub fn clear_active(&mut self, irq: u32) {
        bitmap_clear(&mut self.active, irq);
    }

    /// Find the highest-priority pending+enabled SPI (INTID 32+).
    pub fn highest_pending_spi(&self, priority_mask: u8) -> Option<u32> {
        highest_pending_in_range(
            &self.pending,
            &self.enabled,
            &self.priority,
            priority_mask,
            SPI_START..self.num_irqs,
        )
    }

    /// Resolve the target PE for an SPI.
    ///
    /// With GICv3 affinity routing (`ARE=1`) this reads `GICD_IROUTER`;
    /// with GICv2 it reads the `GICD_ITARGETSR` byte.
    pub fn spi_target_pe(&self, irq: u32) -> u32 {
        if self.version.is_v3_or_later() && irq >= SPI_START {
            let idx = (irq - SPI_START) as usize;
            self.irouter
                .get(idx)
                .map(|r| (r & 0xFF) as u32)
                .unwrap_or(0)
        } else {
            self.targets.get(irq as usize).copied().unwrap_or(0) as u32
        }
    }

    /// Handle a GICD register read.
    pub fn read(&self, offset: u64) -> u32 {
        match offset {
            GICD_CTLR => self.ctrl,
            GICD_TYPER => self.read_typer(),
            GICD_IIDR => self.read_iidr(),
            GICD_PIDR2 if self.version.is_v3_or_later() => 0x3 << 4,
            off if in_range(off, GICD_ISENABLER_BASE, 0x80) => {
                bitmap_read_word(&self.enabled, ((off - GICD_ISENABLER_BASE) / 4) as usize)
            }
            off if in_range(off, GICD_ICENABLER_BASE, 0x80) => {
                bitmap_read_word(&self.enabled, ((off - GICD_ICENABLER_BASE) / 4) as usize)
            }
            off if in_range(off, GICD_ISPENDR_BASE, 0x80) => {
                bitmap_read_word(&self.pending, ((off - GICD_ISPENDR_BASE) / 4) as usize)
            }
            off if in_range(off, GICD_ICPENDR_BASE, 0x80) => {
                bitmap_read_word(&self.pending, ((off - GICD_ICPENDR_BASE) / 4) as usize)
            }
            off if in_range(off, GICD_ISACTIVER_BASE, 0x80) => {
                bitmap_read_word(&self.active, ((off - GICD_ISACTIVER_BASE) / 4) as usize)
            }
            off if in_range(off, GICD_ICACTIVER_BASE, 0x80) => {
                bitmap_read_word(&self.active, ((off - GICD_ICACTIVER_BASE) / 4) as usize)
            }
            off if in_range(off, GICD_IPRIORITYR_BASE, 0x400) => {
                byte_array_read4(&self.priority, (off - GICD_IPRIORITYR_BASE) as usize)
            }
            off if in_range(off, GICD_ITARGETSR_BASE, 0x400) => {
                if self.are_enabled() {
                    0
                } else {
                    byte_array_read4(&self.targets, (off - GICD_ITARGETSR_BASE) as usize)
                }
            }
            off if in_range(off, GICD_ICFGR_BASE, 0x100) => self
                .config
                .get(((off - GICD_ICFGR_BASE) / 4) as usize)
                .copied()
                .unwrap_or(0),
            off if in_range(off, GICD_IGRPMODR_BASE, 0x80) && self.version.is_v3_or_later() => self
                .group_mod
                .get(((off - GICD_IGRPMODR_BASE) / 4) as usize)
                .copied()
                .unwrap_or(0),
            off if in_range(off, GICD_IROUTER_BASE, 0x2000) && self.version.is_v3_or_later() => {
                let irq_idx = ((off - GICD_IROUTER_BASE) / 8) as usize;
                let low_half = (off - GICD_IROUTER_BASE).is_multiple_of(8);
                self.irouter.get(irq_idx).map_or(0, |&route| {
                    if low_half {
                        route as u32
                    } else {
                        (route >> 32) as u32
                    }
                })
            }
            _ => 0,
        }
    }

    /// Handle a GICD register write.
    pub fn write(&mut self, offset: u64, value: u32) {
        match offset {
            GICD_CTLR => {
                if self.version.is_v3_or_later() {
                    self.ctrl = value & 0x77;
                } else {
                    self.ctrl = value & 1;
                }
            }
            off if in_range(off, GICD_ISENABLER_BASE, 0x80) => {
                bitmap_or_word(
                    &mut self.enabled,
                    ((off - GICD_ISENABLER_BASE) / 4) as usize,
                    value,
                );
            }
            off if in_range(off, GICD_ICENABLER_BASE, 0x80) => {
                bitmap_andnot_word(
                    &mut self.enabled,
                    ((off - GICD_ICENABLER_BASE) / 4) as usize,
                    value,
                );
            }
            off if in_range(off, GICD_ISPENDR_BASE, 0x80) => {
                bitmap_or_word(
                    &mut self.pending,
                    ((off - GICD_ISPENDR_BASE) / 4) as usize,
                    value,
                );
            }
            off if in_range(off, GICD_ICPENDR_BASE, 0x80) => {
                bitmap_andnot_word(
                    &mut self.pending,
                    ((off - GICD_ICPENDR_BASE) / 4) as usize,
                    value,
                );
            }
            off if in_range(off, GICD_ISACTIVER_BASE, 0x80) => {
                bitmap_or_word(
                    &mut self.active,
                    ((off - GICD_ISACTIVER_BASE) / 4) as usize,
                    value,
                );
            }
            off if in_range(off, GICD_ICACTIVER_BASE, 0x80) => {
                bitmap_andnot_word(
                    &mut self.active,
                    ((off - GICD_ICACTIVER_BASE) / 4) as usize,
                    value,
                );
            }
            off if in_range(off, GICD_IPRIORITYR_BASE, 0x400) => {
                byte_array_write4(
                    &mut self.priority,
                    (off - GICD_IPRIORITYR_BASE) as usize,
                    value,
                );
            }
            off if in_range(off, GICD_ITARGETSR_BASE, 0x400) => {
                if !self.are_enabled() {
                    byte_array_write4(
                        &mut self.targets,
                        (off - GICD_ITARGETSR_BASE) as usize,
                        value,
                    );
                }
            }
            off if in_range(off, GICD_ICFGR_BASE, 0x100) => {
                let idx = ((off - GICD_ICFGR_BASE) / 4) as usize;
                if let Some(slot) = self.config.get_mut(idx) {
                    *slot = value;
                }
            }
            off if in_range(off, GICD_IGRPMODR_BASE, 0x80) && self.version.is_v3_or_later() => {
                let idx = ((off - GICD_IGRPMODR_BASE) / 4) as usize;
                if let Some(slot) = self.group_mod.get_mut(idx) {
                    *slot = value;
                }
            }
            off if in_range(off, GICD_IROUTER_BASE, 0x2000) && self.version.is_v3_or_later() => {
                let irq_idx = ((off - GICD_IROUTER_BASE) / 8) as usize;
                let low_half = (off - GICD_IROUTER_BASE).is_multiple_of(8);
                if let Some(route) = self.irouter.get_mut(irq_idx) {
                    if low_half {
                        *route = (*route & 0xFFFF_FFFF_0000_0000) | value as u64;
                    } else {
                        *route = (*route & 0x0000_0000_FFFF_FFFF) | ((value as u64) << 32);
                    }
                }
            }
            _ => {}
        }
    }

    /// Reset to power-on state.
    pub fn reset(&mut self) {
        self.ctrl = 0;
        self.enabled.iter_mut().for_each(|w| *w = 0);
        self.pending.iter_mut().for_each(|w| *w = 0);
        self.active.iter_mut().for_each(|w| *w = 0);
        self.priority.iter_mut().for_each(|b| *b = 0);
        self.targets.iter_mut().for_each(|b| *b = 1);
        self.config.iter_mut().for_each(|w| *w = 0);
        self.group_mod.iter_mut().for_each(|w| *w = 0);
        self.irouter.iter_mut().for_each(|r| *r = 0);
    }

    fn read_typer(&self) -> u32 {
        (self.num_irqs / 32).saturating_sub(1) & 0x1F
    }

    fn read_iidr(&self) -> u32 {
        if self.version.is_v3_or_later() {
            0x0300_043B
        } else {
            0x0200_043B
        }
    }
}
