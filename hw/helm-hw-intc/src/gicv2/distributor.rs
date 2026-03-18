//! GICv2 Distributor (GICD).
//!
//! Manages SPI (Shared Peripheral Interrupt) configuration:
//! enable, pending, priority, and target CPU routing.
//!
//! # Register map (offsets from distributor base, 4-byte spacing)
//!
//! | Offset range  | Name           | R/W | Description                       |
//! |---------------|----------------|-----|-----------------------------------|
//! | 0x000         | GICD\_CTLR     | RW  | Distributor control               |
//! | 0x004         | GICD\_TYPER    | R   | Interrupt controller type          |
//! | 0x008         | GICD\_IIDR     | R   | Implementer identification         |
//! | 0x100-0x11C   | GICD\_ISENABLER| RW  | Set-enable (write 1 to set)       |
//! | 0x180-0x19C   | GICD\_ICENABLER| RW  | Clear-enable (write 1 to clear)   |
//! | 0x200-0x21C   | GICD\_ISPENDR  | RW  | Set-pending                       |
//! | 0x280-0x29C   | GICD\_ICPENDR  | RW  | Clear-pending                     |
//! | 0x300-0x31C   | GICD\_ISACTIVER| R   | Active status                     |
//! | 0x380-0x39C   | GICD\_ICACTIVER| RW  | Clear-active                      |
//! | 0x400-0x4FC   | GICD\_IPRIORITY| RW  | Priority (8 bits/IRQ, 4 per reg)  |
//! | 0x800-0x8FC   | GICD\_ITARGETS | RW  | CPU targets (8 bits/IRQ, 4/reg)   |
//! | 0xC00-0xC3C   | GICD\_ICFGR    | RW  | Configuration (2 bits/IRQ)        |
//! | 0xFE0-0xFFC   | PID/CID        | R   | Identification registers          |

use helm_devices::Device;

/// Maximum number of SPI interrupt lines.
const MAX_IRQS: usize = 256;
/// Number of 32-bit enable/pending/active registers needed.
const NUM_REGS: usize = MAX_IRQS / 32; // 8

/// GICv2 Distributor.
///
/// Tracks per-interrupt enable, pending, active, priority, target, and
/// configuration state. The distributor is the central routing component
/// that decides which CPU interface receives which interrupt.
pub struct Gicv2Distributor {
    /// GICD\_CTLR: Distributor control (bit 0 = enable forwarding).
    ctlr: u32,
    /// Per-interrupt enable bits (ISENABLERn / ICENABLERn).
    enabled: [u32; NUM_REGS],
    /// Per-interrupt pending bits (ISPENDRn / ICPENDRn).
    pending: [u32; NUM_REGS],
    /// Per-interrupt active bits (ISACTIVERn / ICACTIVERn).
    active: [u32; NUM_REGS],
    /// Per-interrupt priority (8 bits each, IPRIORITYRn).
    priority: [u8; MAX_IRQS],
    /// Per-interrupt target CPU mask (ITARGETSRn).
    targets: [u8; MAX_IRQS],
    /// Per-interrupt configuration (ICFGRn) -- edge/level, 2 bits per IRQ.
    config: [u32; MAX_IRQS / 16], // 16 registers (2 bits per IRQ)
    /// Number of implemented SPI lines (read from TYPER).
    num_irqs: u32,
    /// Callback to signal CPU interface when interrupt state changes.
    irq_update_fn: Option<Box<dyn FnMut(bool) + Send>>,
}

impl Gicv2Distributor {
    /// Create a new distributor supporting up to `num_irqs` SPIs (capped at 256).
    pub fn new(num_irqs: u32) -> Self {
        let mut d = Self {
            ctlr: 0,
            enabled: [0; NUM_REGS],
            pending: [0; NUM_REGS],
            active: [0; NUM_REGS],
            priority: [0; MAX_IRQS],
            targets: [0; MAX_IRQS],
            config: [0; MAX_IRQS / 16],
            num_irqs: num_irqs.min(MAX_IRQS as u32),
            irq_update_fn: None,
        };
        // Default: all priorities to 0 (highest), all targets to CPU 0
        for t in &mut d.targets {
            *t = 1; // CPU 0
        }
        d
    }

    /// Set the callback invoked when interrupt state changes.
    ///
    /// The callback receives `true` when at least one enabled pending
    /// interrupt exists, `false` otherwise.
    pub fn set_irq_update_fn(&mut self, f: Box<dyn FnMut(bool) + Send>) {
        self.irq_update_fn = Some(f);
    }

    /// Assert an interrupt (SPI). Called by device `InterruptSink`.
    pub fn assert_irq(&mut self, irq: u32) {
        if irq as usize >= MAX_IRQS {
            return;
        }
        let reg = (irq / 32) as usize;
        let bit = 1u32 << (irq & 31);
        self.pending[reg] |= bit;
        self.signal_update();
    }

    /// Deassert an interrupt.
    pub fn deassert_irq(&mut self, irq: u32) {
        if irq as usize >= MAX_IRQS {
            return;
        }
        let reg = (irq / 32) as usize;
        let bit = 1u32 << (irq & 31);
        self.pending[reg] &= !bit;
        self.signal_update();
    }

    /// Find the highest-priority pending & enabled interrupt.
    ///
    /// Returns `(irq_id, priority)` or `None` if no pending interrupt
    /// qualifies (either the distributor is disabled or nothing is pending
    /// and enabled).
    pub fn highest_pending(&self) -> Option<(u32, u8)> {
        if self.ctlr & 1 == 0 {
            return None; // distributor disabled
        }
        let mut best_irq = None;
        let mut best_prio = 0xFFu8;
        for irq in 0..self.num_irqs as usize {
            let reg = irq / 32;
            let bit = 1u32 << (irq & 31);
            if self.pending[reg] & bit != 0
                && self.enabled[reg] & bit != 0
                && self.active[reg] & bit == 0
                && self.priority[irq] < best_prio
            {
                best_prio = self.priority[irq];
                best_irq = Some((irq as u32, best_prio));
            }
        }
        best_irq
    }

    /// Acknowledge interrupt: mark active, clear pending.
    pub fn acknowledge(&mut self, irq: u32) {
        if irq as usize >= MAX_IRQS {
            return;
        }
        let reg = (irq / 32) as usize;
        let bit = 1u32 << (irq & 31);
        self.active[reg] |= bit;
        self.pending[reg] &= !bit;
    }

    /// End of interrupt: deactivate.
    pub fn eoi(&mut self, irq: u32) {
        if irq as usize >= MAX_IRQS {
            return;
        }
        let reg = (irq / 32) as usize;
        let bit = 1u32 << (irq & 31);
        self.active[reg] &= !bit;
        self.signal_update();
    }

    /// Notify the CPU interface callback of current pending state.
    fn signal_update(&mut self) {
        if let Some(f) = &mut self.irq_update_fn {
            let has_pending = self.ctlr & 1 != 0 && {
                // Inline check: any pending & enabled & !active?
                let mut found = false;
                for irq in 0..MAX_IRQS {
                    let reg = irq / 32;
                    let bit = 1u32 << (irq & 31);
                    if self.pending[reg] & bit != 0
                        && self.enabled[reg] & bit != 0
                        && self.active[reg] & bit == 0
                    {
                        found = true;
                        break;
                    }
                }
                found
            };
            f(has_pending);
        }
    }
}

impl Device for Gicv2Distributor {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        match offset {
            // GICD_CTLR
            0x000 => u64::from(self.ctlr),
            // GICD_TYPER: IT_Lines = (num_irqs / 32) - 1, CPUNumber = 0
            0x004 => {
                let it_lines = if self.num_irqs > 32 {
                    (self.num_irqs / 32) - 1
                } else {
                    0
                };
                u64::from(it_lines)
            }
            // GICD_IIDR: implementer
            0x008 => 0x0102_0043,
            // GICD_ISENABLERn (0x100-0x11C)
            o @ 0x100..=0x11C => {
                let n = ((o - 0x100) / 4) as usize;
                if n < NUM_REGS {
                    u64::from(self.enabled[n])
                } else {
                    0
                }
            }
            // GICD_ICENABLERn reads back enable state (same as ISENABLER)
            o @ 0x180..=0x19C => {
                let n = ((o - 0x180) / 4) as usize;
                if n < NUM_REGS {
                    u64::from(self.enabled[n])
                } else {
                    0
                }
            }
            // GICD_ISPENDRn (0x200-0x21C)
            o @ 0x200..=0x21C => {
                let n = ((o - 0x200) / 4) as usize;
                if n < NUM_REGS {
                    u64::from(self.pending[n])
                } else {
                    0
                }
            }
            // GICD_ICPENDRn (0x280-0x29C)
            o @ 0x280..=0x29C => {
                let n = ((o - 0x280) / 4) as usize;
                if n < NUM_REGS {
                    u64::from(self.pending[n])
                } else {
                    0
                }
            }
            // GICD_ISACTIVERn (0x300-0x31C)
            o @ 0x300..=0x31C => {
                let n = ((o - 0x300) / 4) as usize;
                if n < NUM_REGS {
                    u64::from(self.active[n])
                } else {
                    0
                }
            }
            // GICD_IPRIORITYRn (0x400-0x4FC) -- 4 priorities per register
            o @ 0x400..=0x4FC => {
                let base = (o - 0x400) as usize;
                let mut val = 0u32;
                for i in 0..4 {
                    let idx = base + i;
                    if idx < MAX_IRQS {
                        val |= u32::from(self.priority[idx]) << (i * 8);
                    }
                }
                u64::from(val)
            }
            // GICD_ITARGETSRn (0x800-0x8FC) -- 4 targets per register
            o @ 0x800..=0x8FC => {
                let base = (o - 0x800) as usize;
                let mut val = 0u32;
                for i in 0..4 {
                    let idx = base + i;
                    if idx < MAX_IRQS {
                        val |= u32::from(self.targets[idx]) << (i * 8);
                    }
                }
                u64::from(val)
            }
            // GICD_ICFGRn (0xC00-0xC3C) -- 16 IRQs per register (2 bits each)
            o @ 0xC00..=0xC3C => {
                let n = ((o - 0xC00) / 4) as usize;
                if n < self.config.len() {
                    u64::from(self.config[n])
                } else {
                    0
                }
            }
            // PID/CID (identification at 0xFE0-0xFFC)
            0xFE0 => 0x90, // GICD PID0
            0xFE4 => 0xB4, // PID1
            0xFE8 => 0x2B, // PID2 (GICv2)
            0xFEC => 0x00, // PID3
            0xFF0 => 0x0D, // CID0
            0xFF4 => 0xF0, // CID1
            0xFF8 => 0x05, // CID2
            0xFFC => 0xB1, // CID3
            _ => {
                log::trace!("GICD read: unhandled offset {offset:#x}");
                0
            }
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let val32 = val as u32;
        match offset {
            // GICD_CTLR
            0x000 => {
                self.ctlr = val32 & 1;
                self.signal_update();
            }
            // GICD_ISENABLERn -- set-enable (write 1 to set)
            o @ 0x100..=0x11C => {
                let n = ((o - 0x100) / 4) as usize;
                if n < NUM_REGS {
                    self.enabled[n] |= val32;
                }
            }
            // GICD_ICENABLERn -- clear-enable (write 1 to clear)
            o @ 0x180..=0x19C => {
                let n = ((o - 0x180) / 4) as usize;
                if n < NUM_REGS {
                    self.enabled[n] &= !val32;
                }
            }
            // GICD_ISPENDRn -- set-pending
            o @ 0x200..=0x21C => {
                let n = ((o - 0x200) / 4) as usize;
                if n < NUM_REGS {
                    self.pending[n] |= val32;
                }
                self.signal_update();
            }
            // GICD_ICPENDRn -- clear-pending
            o @ 0x280..=0x29C => {
                let n = ((o - 0x280) / 4) as usize;
                if n < NUM_REGS {
                    self.pending[n] &= !val32;
                }
                self.signal_update();
            }
            // GICD_ICACTIVERn -- clear active
            o @ 0x380..=0x39C => {
                let n = ((o - 0x380) / 4) as usize;
                if n < NUM_REGS {
                    self.active[n] &= !val32;
                }
            }
            // GICD_IPRIORITYRn
            o @ 0x400..=0x4FC => {
                let base = (o - 0x400) as usize;
                for i in 0..4 {
                    let idx = base + i;
                    if idx < MAX_IRQS {
                        self.priority[idx] = ((val32 >> (i * 8)) & 0xFF) as u8;
                    }
                }
            }
            // GICD_ITARGETSRn
            o @ 0x800..=0x8FC => {
                let base = (o - 0x800) as usize;
                for i in 0..4 {
                    let idx = base + i;
                    if idx < MAX_IRQS {
                        self.targets[idx] = ((val32 >> (i * 8)) & 0xFF) as u8;
                    }
                }
            }
            // GICD_ICFGRn
            o @ 0xC00..=0xC3C => {
                let n = ((o - 0xC00) / 4) as usize;
                if n < self.config.len() {
                    self.config[n] = val32;
                }
            }
            _ => {
                log::trace!("GICD write: unhandled offset {offset:#x} val={val:#x}");
            }
        }
    }

    fn region_size(&self) -> u64 {
        0x1000 // 4 KiB
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_devices::Device;

    #[test]
    fn typer_reports_irq_count() {
        let mut gicd = Gicv2Distributor::new(128);
        // GICD_TYPER at offset 0x004: IT_Lines = (128/32)-1 = 3
        assert_eq!(gicd.read(0x004, 4), 3);
    }

    #[test]
    fn enable_and_pending() {
        let mut gicd = Gicv2Distributor::new(128);
        // Enable distributor
        gicd.write(0x000, 4, 1);
        // Enable IRQ 33 (reg 1 = IRQs 32-63, bit 1)
        gicd.write(0x104, 4, 0x2); // ISENABLERn[1] bit 1
        // Assert IRQ 33
        gicd.assert_irq(33);
        // Check pending
        assert_ne!(gicd.read(0x204, 4) & 0x2, 0);
        // Highest pending should be IRQ 33
        let (id, _prio) = gicd.highest_pending().unwrap();
        assert_eq!(id, 33);
    }

    #[test]
    fn acknowledge_and_eoi() {
        let mut gicd = Gicv2Distributor::new(128);
        gicd.write(0x000, 4, 1);
        gicd.write(0x104, 4, 0x2); // enable IRQ 33
        gicd.assert_irq(33);
        // Acknowledge
        gicd.acknowledge(33);
        // Should be active now, not pending
        assert!(gicd.highest_pending().is_none());
        assert_ne!(gicd.read(0x304, 4) & 0x2, 0); // active
        // EOI
        gicd.eoi(33);
        assert_eq!(gicd.read(0x304, 4) & 0x2, 0); // no longer active
    }

    #[test]
    fn priority_ordering() {
        let mut gicd = Gicv2Distributor::new(128);
        gicd.write(0x000, 4, 1);
        // Enable IRQ 32 and 33
        gicd.write(0x104, 4, 0x3);
        // Set priorities: IRQ 32 = 0x80, IRQ 33 = 0x10 (lower = higher priority)
        gicd.priority[32] = 0x80;
        gicd.priority[33] = 0x10;
        gicd.assert_irq(32);
        gicd.assert_irq(33);
        // Highest priority should be IRQ 33 (priority 0x10)
        let (id, prio) = gicd.highest_pending().unwrap();
        assert_eq!(id, 33);
        assert_eq!(prio, 0x10);
    }
}
