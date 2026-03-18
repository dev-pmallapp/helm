//! GICv2 Distributor (GICD) — thin wrapper around shared `GicState`.

use std::sync::{Arc, Mutex};

use helm_devices::Device;
use helm_debug::sim_stub;

use super::{GicState, MAX_IRQS, NUM_REGS, SPURIOUS_IRQ};

// ── Gicv2Distributor ─────────────────────────────────────────────────────────

/// GICv2 Distributor device.
///
/// When built via [`build_gicv2`](super::build_gicv2) the state is shared
/// with `Gicv2CpuInterface` and an `Arc<AtomicBool>` IRQ line is wired so
/// the CPU step loop sees real interrupt assertions.
///
/// For standalone testing, use `Gicv2Distributor::new(n)` which creates an
/// unshared instance with no IRQ line.
pub struct Gicv2Distributor(pub Arc<Mutex<GicState>>);

impl Gicv2Distributor {
    /// Create a standalone (unshared) distributor for unit tests.
    pub fn new(num_irqs: u32) -> Self {
        Self(Arc::new(Mutex::new(GicState::new(num_irqs))))
    }

    /// Create from a pre-built shared state (used by `build_gicv2`).
    pub fn from_shared(state: Arc<Mutex<GicState>>) -> Self {
        Self(state)
    }

    // ── Helpers preserved for existing API consumers ──────────────────────

    /// Assert a peripheral interrupt (sets it pending).
    pub fn assert_irq(&mut self, irq: u32) {
        self.0.lock().unwrap().assert_irq(irq);
    }

    /// Deassert a peripheral interrupt.
    pub fn deassert_irq(&mut self, irq: u32) {
        self.0.lock().unwrap().deassert_irq(irq);
    }

    /// Return the highest pending+enabled interrupt, or `None`.
    pub fn highest_pending(&self) -> Option<(u32, u8)> {
        let s = self.0.lock().unwrap();
        s.highest_pending().map(|id| (id, s.priority[id as usize]))
    }

    /// Acknowledge an interrupt: mark it active, clear pending.
    /// (Legacy helper; prefer `cpu_acknowledge` via GICC_IAR.)
    pub fn acknowledge(&mut self, irq: u32) {
        let mut s = self.0.lock().unwrap();
        if irq as usize >= MAX_IRQS { return; }
        let reg = (irq / 32) as usize;
        let bit = 1u32 << (irq & 31);
        s.active[reg]  |= bit;
        s.pending[reg] &= !bit;
        // no update_irq_line here — matches original acknowledge() semantics
    }

    /// End-of-interrupt: clear the active bit.
    pub fn eoi(&mut self, irq: u32) {
        self.0.lock().unwrap().cpu_eoi(irq);
    }

    /// Legacy: set an IRQ-update callback (no-op; real wiring uses irq_line).
    pub fn set_irq_update_fn(&mut self, _f: Box<dyn FnMut(bool) + Send>) {}
}

// ── Device trait ─────────────────────────────────────────────────────────────

impl Device for Gicv2Distributor {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        let s = self.0.lock().unwrap();
        match offset {
            0x000 => u64::from(s.dist_ctlr),
            0x004 => {
                let it_lines = if s.num_irqs > 32 { (s.num_irqs / 32) - 1 } else { 0 };
                u64::from(it_lines)
            }
            0x008 => 0x0102_0043,
            o @ 0x100..=0x11C => {
                let n = ((o - 0x100) / 4) as usize;
                u64::from(if n < NUM_REGS { s.enabled[n] } else { 0 })
            }
            o @ 0x180..=0x19C => {
                let n = ((o - 0x180) / 4) as usize;
                u64::from(if n < NUM_REGS { s.enabled[n] } else { 0 })
            }
            o @ 0x200..=0x21C => {
                let n = ((o - 0x200) / 4) as usize;
                u64::from(if n < NUM_REGS { s.pending[n] } else { 0 })
            }
            o @ 0x280..=0x29C => {
                let n = ((o - 0x280) / 4) as usize;
                u64::from(if n < NUM_REGS { s.pending[n] } else { 0 })
            }
            o @ 0x300..=0x31C => {
                let n = ((o - 0x300) / 4) as usize;
                u64::from(if n < NUM_REGS { s.active[n] } else { 0 })
            }
            o @ 0x400..=0x4FC => {
                let base = (o - 0x400) as usize;
                let mut val = 0u32;
                for i in 0..4 {
                    let idx = base + i;
                    if idx < MAX_IRQS { val |= u32::from(s.priority[idx]) << (i * 8); }
                }
                u64::from(val)
            }
            o @ 0x800..=0x8FC => {
                let base = (o - 0x800) as usize;
                let mut val = 0u32;
                for i in 0..4 {
                    let idx = base + i;
                    if idx < MAX_IRQS { val |= u32::from(s.targets[idx]) << (i * 8); }
                }
                u64::from(val)
            }
            o @ 0xC00..=0xC3C => {
                let n = ((o - 0xC00) / 4) as usize;
                u64::from(s.config.get(n).copied().unwrap_or(0))
            }
            // PID/CID
            0xFE0 => 0x90, 0xFE4 => 0xB4, 0xFE8 => 0x2B, 0xFEC => 0x00,
            0xFF0 => 0x0D, 0xFF4 => 0xF0, 0xFF8 => 0x05, 0xFFC => 0xB1,
            _ => {
                sim_stub!(component="gicv2-gicd", "read unhandled offset={offset:#x} -> 0");
                0
            }
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let val32 = val as u32;
        let mut s = self.0.lock().unwrap();
        match offset {
            0x000 => {
                s.dist_ctlr = val32 & 1;
                s.update_irq_line();
            }
            o @ 0x100..=0x11C => {
                let n = ((o - 0x100) / 4) as usize;
                if n < NUM_REGS { s.enabled[n] |= val32; s.update_irq_line(); }
            }
            o @ 0x180..=0x19C => {
                let n = ((o - 0x180) / 4) as usize;
                if n < NUM_REGS { s.enabled[n] &= !val32; s.update_irq_line(); }
            }
            o @ 0x200..=0x21C => {
                let n = ((o - 0x200) / 4) as usize;
                if n < NUM_REGS { s.pending[n] |= val32; s.update_irq_line(); }
            }
            o @ 0x280..=0x29C => {
                let n = ((o - 0x280) / 4) as usize;
                if n < NUM_REGS { s.pending[n] &= !val32; s.update_irq_line(); }
            }
            o @ 0x380..=0x39C => {
                let n = ((o - 0x380) / 4) as usize;
                if n < NUM_REGS { s.active[n] &= !val32; }
            }
            o @ 0x400..=0x4FC => {
                let base = (o - 0x400) as usize;
                for i in 0..4 {
                    let idx = base + i;
                    if idx < MAX_IRQS {
                        s.priority[idx] = ((val32 >> (i * 8)) & 0xFF) as u8;
                    }
                }
            }
            o @ 0x800..=0x8FC => {
                let base = (o - 0x800) as usize;
                for i in 0..4 {
                    let idx = base + i;
                    if idx < MAX_IRQS {
                        s.targets[idx] = ((val32 >> (i * 8)) & 0xFF) as u8;
                    }
                }
            }
            o @ 0xC00..=0xC3C => {
                let n = ((o - 0xC00) / 4) as usize;
                if n < s.config.len() { s.config[n] = val32; }
            }
            _ => { sim_stub!(component="gicv2-gicd", "write unhandled offset={offset:#x} val={val:#x} (ignored)"); }
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
    fn typer_reports_irq_count() {
        let mut gicd = Gicv2Distributor::new(128);
        assert_eq!(gicd.read(0x004, 4), 3); // (128/32) - 1 = 3
    }

    #[test]
    fn enable_and_pending() {
        let mut gicd = Gicv2Distributor::new(128);
        gicd.write(0x000, 4, 1);           // GICD_CTLR enable
        gicd.write(0x104, 4, 0x2);         // ISENABLER[1] bit 1 = IRQ 33
        gicd.assert_irq(33);
        assert_ne!(gicd.read(0x204, 4) & 0x2, 0);
        let (id, _prio) = gicd.highest_pending().unwrap();
        assert_eq!(id, 33);
    }

    #[test]
    fn acknowledge_and_eoi() {
        let mut gicd = Gicv2Distributor::new(128);
        gicd.write(0x000, 4, 1);
        gicd.write(0x104, 4, 0x2);
        gicd.assert_irq(33);
        gicd.acknowledge(33);
        assert!(gicd.highest_pending().is_none());
        assert_ne!(gicd.read(0x304, 4) & 0x2, 0); // ISACTIVER
        gicd.eoi(33);
        assert_eq!(gicd.read(0x304, 4) & 0x2, 0);
    }

    #[test]
    fn priority_ordering() {
        let mut gicd = Gicv2Distributor::new(128);
        gicd.write(0x000, 4, 1);
        gicd.write(0x104, 4, 0x3); // enable IRQ 32 and 33
        // Set priorities via GICD_IPRIORITYRn register writes:
        // IRQ 32 and 33 both live in register at offset 0x420 (IRQs 32-35)
        // byte 0 = IRQ 32 (0x80), byte 1 = IRQ 33 (0x10)
        gicd.write(0x420, 4, 0x0000_1080);
        gicd.assert_irq(32);
        gicd.assert_irq(33);
        let (id, prio) = gicd.highest_pending().unwrap();
        assert_eq!(id, 33);    // priority 0x10 < 0x80
        assert_eq!(prio, 0x10);
    }
}
