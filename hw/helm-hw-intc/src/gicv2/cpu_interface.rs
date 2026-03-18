//! GICv2 CPU Interface (GICC) — thin wrapper around shared `GicState`.

use std::sync::{Arc, Mutex};

use helm_devices::Device;
use helm_debug::sim_stub;

use super::{GicState, SPURIOUS_IRQ};

// ── Gicv2CpuInterface ────────────────────────────────────────────────────────

/// GICv2 CPU Interface device.
///
/// When built via [`build_gicv2`](super::build_gicv2) the state is shared
/// with `Gicv2Distributor` and the IRQ line is updated automatically on
/// every IAR read and EOIR write.
///
/// For standalone testing, use `Gicv2CpuInterface::new()` which creates an
/// unshared instance.
pub struct Gicv2CpuInterface(pub Arc<Mutex<GicState>>);

impl Gicv2CpuInterface {
    /// Create a standalone (unshared) CPU interface for unit tests.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(GicState::new(256))))
    }

    /// Create from a pre-built shared state (used by `build_gicv2`).
    pub fn from_shared(state: Arc<Mutex<GicState>>) -> Self {
        Self(state)
    }

    // ── Legacy helpers (used by existing tests) ───────────────────────────

    /// Update pending state from distributor (standalone-mode helper).
    pub fn update_pending(&mut self, irq_id: u32, priority: u8) {
        let mut s = self.0.lock().unwrap();
        // set the IRQ pending in the shared state
        if irq_id as usize >= super::MAX_IRQS { return; }
        s.priority[irq_id as usize] = priority;
        s.pending[(irq_id / 32) as usize] |= 1 << (irq_id & 31);
        s.enabled[(irq_id / 32) as usize] |= 1 << (irq_id & 31);
        s.dist_ctlr = 1; // auto-enable distributor for standalone tests
        s.update_irq_line();
    }

    /// Clear pending signal (standalone-mode helper).
    pub fn clear_pending(&mut self) {
        let mut s = self.0.lock().unwrap();
        s.pending.iter_mut().for_each(|w| *w = 0);
        s.update_irq_line();
    }

    /// Check if CPU should take an IRQ.
    pub fn has_pending_irq(&self) -> bool {
        let s = self.0.lock().unwrap();
        s.cpu_ctlr & 1 != 0 && s.highest_pending().is_some()
    }
}

impl Default for Gicv2CpuInterface {
    fn default() -> Self { Self::new() }
}

// ── Device trait ─────────────────────────────────────────────────────────────

impl Device for Gicv2CpuInterface {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        let mut s = self.0.lock().unwrap();
        match offset {
            0x000 => u64::from(s.cpu_ctlr),
            0x004 => u64::from(s.pmr),
            0x008 => u64::from(s.bpr),
            // GICC_IAR — acknowledge; this is the hot path during IRQ delivery
            0x00C => u64::from(s.cpu_acknowledge()),
            0x010 => 0, // EOIR read returns 0
            0x014 => {  // GICC_RPR — running priority
                u64::from(s.last_ack.saturating_sub(1) as u8)
            }
            0x018 => {  // GICC_HPPIR — highest priority pending
                u64::from(s.highest_pending().unwrap_or(SPURIOUS_IRQ))
            }
            0x00FC => 0x0102_0043, // GICC_IIDR
            _ => {
                sim_stub!(component="gicv2-gicc", "read unhandled offset={offset:#x} -> 0");
                0
            }
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let val32 = val as u32;
        let mut s = self.0.lock().unwrap();
        match offset {
            0x000 => {
                s.cpu_ctlr = val32 & 1;
                s.update_irq_line();
            }
            0x004 => {
                s.pmr = val32 & 0xFF;
                s.update_irq_line();
            }
            0x008 => { s.bpr = val32 & 0x7; }
            // GICC_EOIR — end of interrupt, deactivate
            0x010 => { s.cpu_eoi(val32); }
            0x014 => {} // GICC_AIAR / APR — ignore
            _ => {
                sim_stub!(component="gicv2-gicc", "write unhandled offset={offset:#x} val={val:#x} (ignored)");
            }
        }
    }

    fn region_size(&self) -> u64 { 0x1000 }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use helm_devices::Device;

    #[test]
    fn cpu_interface_defaults_to_spurious() {
        let mut gicc = Gicv2CpuInterface::new();
        assert_eq!(gicc.read(0x00C, 4), 1023);
    }

    #[test]
    fn cpu_interface_enable_and_mask() {
        let mut gicc = Gicv2CpuInterface::new();
        gicc.write(0x000, 4, 1);   // enable GICC
        gicc.write(0x004, 4, 0xFF); // PMR = 0xFF (allow all)
        gicc.update_pending(33, 0x10);
        assert!(gicc.has_pending_irq());
        assert_eq!(gicc.read(0x00C, 4), 33);
    }

    #[test]
    fn priority_masking_blocks_low_priority() {
        let mut gicc = Gicv2CpuInterface::new();
        gicc.write(0x000, 4, 1);    // enable GICC
        gicc.write(0x004, 4, 0x10); // PMR = 0x10
        gicc.update_pending(33, 0x20); // priority 0x20 >= PMR → blocked
        assert!(!gicc.has_pending_irq());
    }
}
