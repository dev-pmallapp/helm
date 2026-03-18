//! GICv2 CPU Interface (GICC).
//!
//! Per-CPU interrupt interface: acknowledge, end-of-interrupt,
//! priority masking, and IRQ output to the processor.
//!
//! # Register map (offsets from CPU interface base, 4-byte spacing)
//!
//! | Offset | Name        | R/W | Description                          |
//! |--------|-------------|-----|--------------------------------------|
//! | 0x000  | GICC\_CTLR  | RW  | CPU interface control                |
//! | 0x004  | GICC\_PMR   | RW  | Priority mask register               |
//! | 0x008  | GICC\_BPR   | RW  | Binary point register                |
//! | 0x00C  | GICC\_IAR   | R   | Interrupt acknowledge register       |
//! | 0x010  | GICC\_EOIR  | W   | End of interrupt register            |
//! | 0x014  | GICC\_RPR   | R   | Running priority register            |
//! | 0x018  | GICC\_HPPIR | R   | Highest priority pending interrupt   |
//! | 0x00FC | GICC\_IIDR  | R   | Implementer identification           |

use helm_devices::Device;

/// Spurious interrupt ID returned when no interrupt is pending.
const SPURIOUS_IRQ: u32 = 1023;

/// GICv2 CPU Interface.
///
/// Tracks the per-CPU state needed to acknowledge interrupts and
/// filter them by priority. Works in tandem with [`Gicv2Distributor`]
/// which manages the global routing state.
///
/// [`Gicv2Distributor`]: crate::Gicv2Distributor
pub struct Gicv2CpuInterface {
    /// GICC\_CTLR: CPU interface control (bit 0 = enable).
    ctlr: u32,
    /// GICC\_PMR: Priority mask -- IRQs with priority >= PMR are blocked.
    pmr: u32,
    /// GICC\_BPR: Binary point register.
    bpr: u32,
    /// Last acknowledged IRQ (for IAR reads).
    last_ack: u32,
    /// Whether an IRQ is signaled to the CPU.
    pub irq_pending: bool,
    /// Highest pending IRQ id (set by distributor callback).
    pub pending_irq_id: u32,
    /// Highest pending priority.
    pub pending_priority: u8,
}

impl Gicv2CpuInterface {
    /// Create a new CPU interface in its reset state (disabled, spurious).
    pub fn new() -> Self {
        Self {
            ctlr: 0,
            pmr: 0,
            bpr: 0,
            last_ack: SPURIOUS_IRQ,
            irq_pending: false,
            pending_irq_id: SPURIOUS_IRQ,
            pending_priority: 0xFF,
        }
    }

    /// Update pending state from distributor.
    ///
    /// Called when the distributor determines the highest-priority pending
    /// interrupt for this CPU.
    pub fn update_pending(&mut self, irq_id: u32, priority: u8) {
        self.pending_irq_id = irq_id;
        self.pending_priority = priority;
        // Signal IRQ if enabled and priority passes mask
        self.irq_pending =
            self.ctlr & 1 != 0 && u32::from(priority) < self.pmr;
    }

    /// Clear pending signal (no interrupt waiting).
    pub fn clear_pending(&mut self) {
        self.pending_irq_id = SPURIOUS_IRQ;
        self.pending_priority = 0xFF;
        self.irq_pending = false;
    }

    /// Check if CPU should take an IRQ.
    pub fn has_pending_irq(&self) -> bool {
        self.irq_pending
    }
}

impl Default for Gicv2CpuInterface {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for Gicv2CpuInterface {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        match offset {
            // GICC_CTLR
            0x000 => u64::from(self.ctlr),
            // GICC_PMR
            0x004 => u64::from(self.pmr),
            // GICC_BPR
            0x008 => u64::from(self.bpr),
            // GICC_IAR -- Interrupt Acknowledge
            0x00C => {
                if self.pending_irq_id != SPURIOUS_IRQ {
                    self.last_ack = self.pending_irq_id;
                    u64::from(self.pending_irq_id)
                } else {
                    u64::from(SPURIOUS_IRQ)
                }
            }
            // GICC_EOIR -- End of Interrupt (read returns 0)
            0x010 => 0,
            // GICC_RPR -- Running Priority
            0x014 => u64::from(self.pending_priority),
            // GICC_HPPIR -- Highest Priority Pending
            0x018 => u64::from(self.pending_irq_id),
            // GICC_IIDR
            0x00FC => 0x0102_0043,
            _ => {
                log::trace!("GICC read: unhandled offset {offset:#x}");
                0
            }
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let val32 = val as u32;
        match offset {
            // GICC_CTLR
            0x000 => {
                self.ctlr = val32 & 1;
                // Re-evaluate pending status
                if self.ctlr & 1 != 0
                    && u32::from(self.pending_priority) < self.pmr
                {
                    self.irq_pending = true;
                } else if self.ctlr & 1 == 0 {
                    self.irq_pending = false;
                }
            }
            // GICC_PMR
            0x004 => {
                self.pmr = val32 & 0xFF;
                // Re-evaluate
                self.irq_pending = self.ctlr & 1 != 0
                    && u32::from(self.pending_priority) < self.pmr
                    && self.pending_irq_id != SPURIOUS_IRQ;
            }
            // GICC_BPR
            0x008 => self.bpr = val32 & 0x7,
            // GICC_EOIR -- End of Interrupt
            0x010 => {
                // val32 is the IRQ ID being completed.
                // The engine should call distributor.eoi() with this IRQ ID.
                let _ = val32;
                self.last_ack = SPURIOUS_IRQ;
            }
            _ => {
                log::trace!("GICC write: unhandled offset {offset:#x} val={val:#x}");
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
    fn cpu_interface_defaults_to_spurious() {
        let mut gicc = Gicv2CpuInterface::new();
        assert_eq!(gicc.read(0x00C, 4), 1023); // IAR = spurious
    }

    #[test]
    fn cpu_interface_enable_and_mask() {
        let mut gicc = Gicv2CpuInterface::new();
        // Enable CPU interface
        gicc.write(0x000, 4, 1);
        // Set PMR to 0xFF (allow all)
        gicc.write(0x004, 4, 0xFF);
        // Update with pending IRQ
        gicc.update_pending(33, 0x10);
        assert!(gicc.has_pending_irq());
        // Read IAR
        assert_eq!(gicc.read(0x00C, 4), 33);
    }

    #[test]
    fn priority_masking_blocks_low_priority() {
        let mut gicc = Gicv2CpuInterface::new();
        gicc.write(0x000, 4, 1); // enable
        gicc.write(0x004, 4, 0x10); // PMR = 0x10 (only priorities < 0x10 pass)
        gicc.update_pending(33, 0x20); // priority 0x20 > PMR
        assert!(!gicc.has_pending_irq()); // blocked
    }
}
