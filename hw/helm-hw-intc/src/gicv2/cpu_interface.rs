//! GICv2 CPU Interface (GICC) — wrapper around per-CPU interface state.

use std::sync::{Arc, Mutex};

use helm_devices::Device;
use helm_diag::sim_stub;

use super::{GicSharedState, SPURIOUS_IRQ};

// ── Gicv2CpuInterface ────────────────────────────────────────────────────────

/// GICv2 CPU Interface device.
///
/// When built via [`build_gicv2`](super::build_gicv2) the state is shared
/// with `Gicv2Distributor` and the IRQ line is updated automatically on
/// every IAR read and EOIR write.
///
/// For standalone testing, use `Gicv2CpuInterface::new()` which creates an
/// unshared instance.
pub struct Gicv2CpuInterface {
    shared: Arc<Mutex<GicSharedState>>,
    cpu_idx: usize,
}

impl Gicv2CpuInterface {
    /// Create a standalone (unshared) CPU interface for unit tests.
    pub fn new() -> Self {
        let (_gicd, mut giccs, _lines, _shared) = super::build_gicv2_mp(256, 1);
        giccs.remove(0)
    }

    /// Create from a pre-built shared state (used by `build_gicv2`).
    pub fn from_shared(state: Arc<Mutex<GicSharedState>>, cpu_idx: usize) -> Self {
        Self {
            shared: state,
            cpu_idx,
        }
    }

    /// Create a banked MMIO CPU interface that follows `shared.active_cpu_idx`.
    pub fn from_banked_shared(state: Arc<Mutex<GicSharedState>>) -> Self {
        Self {
            shared: state,
            cpu_idx: usize::MAX,
        }
    }

    fn selected_cpu_idx(&self, s: &GicSharedState) -> usize {
        if self.cpu_idx == usize::MAX {
            s.active_cpu_idx.min(s.cpus.len().saturating_sub(1))
        } else {
            self.cpu_idx.min(s.cpus.len().saturating_sub(1))
        }
    }

    // ── Legacy helpers (used by existing tests) ───────────────────────────

    /// Update pending state from distributor (standalone-mode helper).
    pub fn update_pending(&mut self, irq_id: u32, priority: u8) {
        let mut s = self.shared.lock().unwrap();
        // set the IRQ pending in the shared state
        if irq_id as usize >= super::MAX_IRQS {
            return;
        }
        s.dist.priority[irq_id as usize] = priority;
        s.dist.pending[(irq_id / 32) as usize] |= 1 << (irq_id & 31);
        s.dist.enabled[(irq_id / 32) as usize] |= 1 << (irq_id & 31);
        s.dist.dist_ctlr = 1; // auto-enable distributor for standalone tests
        s.update_irq_line(self.cpu_idx);
    }

    /// Clear pending signal (standalone-mode helper).
    pub fn clear_pending(&mut self) {
        let mut s = self.shared.lock().unwrap();
        s.dist.pending.iter_mut().for_each(|w| *w = 0);
        s.update_irq_line(self.cpu_idx);
    }

    /// Check if CPU should take an IRQ.
    pub fn has_pending_irq(&self) -> bool {
        let s = self.shared.lock().unwrap();
        let cpu_idx = self.selected_cpu_idx(&s);
        s.cpus[cpu_idx].cpu_ctlr & 1 != 0 && s.highest_pending_for_cpu(cpu_idx).is_some()
    }
}

impl Default for Gicv2CpuInterface {
    fn default() -> Self {
        Self::new()
    }
}

// ── Device trait ─────────────────────────────────────────────────────────────

impl Device for Gicv2CpuInterface {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        let mut s = self.shared.lock().unwrap();
        let cpu_idx = self.selected_cpu_idx(&s);
        match offset {
            // GICC_CTLR: dual views (IHI0048B §4.4.1).
            // Secure: full register (EnableGrp0[0], EnableGrp1[1], AckCtl[2], FIQEn[3], ...).
            // Non-Secure: only EnableGrp1[0] (mapped from S-bit1) and EOImodeNS[9].
            0x000 => {
                let ctlr = s.cpus[cpu_idx].cpu_ctlr;
                if s.current_is_secure {
                    u64::from(ctlr)
                } else {
                    u64::from(ctlr & 0x0000_0201)
                }
            }
            0x004 => u64::from(s.cpus[cpu_idx].pmr),
            0x008 => u64::from(s.cpus[cpu_idx].bpr),
            // GICC_IAR — acknowledge; this is the hot path during IRQ delivery
            0x00C => u64::from(s.cpu_acknowledge(cpu_idx)),
            0x010 => 0, // EOIR read returns 0
            0x014 => {
                // GICC_RPR — running priority
                // RPR reflects the top of the active-priority stack so nested
                // interrupts restore the preempted priority after EOIR.
                u64::from(s.cpus[cpu_idx].running_pri)
            }
            0x018 => {
                // GICC_HPPIR — highest priority pending
                u64::from(s.highest_pending_for_cpu(cpu_idx).unwrap_or(SPURIOUS_IRQ))
            }
            // GICC_ABPR (0x01C): Aliased BPR — Non-Secure BPR for Group 1 interrupts.
            // Without TrustZone, mirror BPR with minimum value of 1 per spec.
            0x01C => u64::from(s.cpus[cpu_idx].bpr.max(1)),
            // GICC_AIAR (0x020): Aliased IAR — acknowledge Group 1 interrupt.
            0x020 => u64::from(s.cpu_acknowledge(cpu_idx)),
            // GICC_AHPPIR (0x028): Aliased HPPIR for Group 1.
            0x028 => u64::from(s.highest_pending_for_cpu(cpu_idx).unwrap_or(SPURIOUS_IRQ)),
            0x00FC => 0x0102_0043, // GICC_IIDR
            // GICC_APR0..3 (0xD0–0xDC) and GICC_NSAPR0..3 (0xE0–0xEC): RAZ
            0x0D0..=0x0EC => 0,
            // GICC_DIR (0x1000): write-only, reads RAZ
            0x1000 => 0,
            // All other offsets within the 64 KB region: RAZ silently.
            // This covers optional/unimplemented/implementation-defined registers
            // (0x01C–0x0CF, 0x0ED–0x0FB, 0x0FD–0x0FF, 0x101–0x0FFF, 0x1001–0xFFFF)
            // that guests may probe (Linux SMP scan, firmware identity reads).
            0x01C..=0xFFFF => 0,
            _ => {
                sim_stub!(
                    component = "gicv2-gicc",
                    "read unhandled offset={offset:#x} -> 0"
                );
                0
            }
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let val32 = val as u32;
        let mut s = self.shared.lock().unwrap();
        let cpu_idx = self.selected_cpu_idx(&s);
        match offset {
            // GICC_CTLR write: Secure updates full register; Non-Secure may only
            // update EnableGrp1[0] and EOImodeNS[9] (IHI0048B §4.4.1).
            0x000 => {
                if s.current_is_secure {
                    s.cpus[cpu_idx].cpu_ctlr = val32;
                } else {
                    let ns_mask = 0x0000_0201u32;
                    let ctlr = &mut s.cpus[cpu_idx].cpu_ctlr;
                    *ctlr = (*ctlr & !ns_mask) | (val32 & ns_mask);
                }
                s.update_irq_line(cpu_idx);
            }
            0x004 => {
                s.cpus[cpu_idx].pmr = val32 & 0xFF;
                s.update_irq_line(cpu_idx);
            }
            0x008 => {
                s.cpus[cpu_idx].bpr = val32 & 0x7;
            }
            // GICC_EOIR — end of interrupt, deactivate
            0x010 => {
                s.cpu_eoi(cpu_idx, val32);
            }
            0x014 => {} // APR0 — ignore (no active priority stack exposed)
            // GICC_ABPR (0x01C): Aliased BPR write.
            0x01C => {
                s.cpus[cpu_idx].bpr = (val32 & 0x7).max(1);
            }
            // GICC_AEOIR (0x024): Aliased EOIR — deactivate Group 1 interrupt.
            0x024 => {
                s.cpu_eoi(cpu_idx, val32);
            }
            // GICC_DIR (0x1000) — deactivate interrupt (priority drop mode)
            0x1000 => {
                s.cpu_eoi(cpu_idx, val32);
            }
            // APR/NSAPR/implementation-defined/reserved — writes ignored silently
            0x01C..=0x0FF | 0x101..=0xFFFF => {}
            _ => {
                sim_stub!(
                    component = "gicv2-gicc",
                    "write unhandled offset={offset:#x} val={val:#x} (ignored)"
                );
            }
        }
    }

    fn region_size(&self) -> u64 {
        0x1_0000
    } // 64KB — matches arm-virt DTB
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
        gicc.write(0x000, 4, 1); // enable GICC
        gicc.write(0x004, 4, 0xFF); // PMR = 0xFF (allow all)
        gicc.update_pending(33, 0x10);
        assert!(gicc.has_pending_irq());
        assert_eq!(gicc.read(0x00C, 4), 33);
    }

    #[test]
    fn priority_masking_blocks_low_priority() {
        let mut gicc = Gicv2CpuInterface::new();
        gicc.write(0x000, 4, 1); // enable GICC
        gicc.write(0x004, 4, 0x10); // PMR = 0x10
        gicc.update_pending(33, 0x20); // priority 0x20 >= PMR → blocked
        assert!(!gicc.has_pending_irq());
    }

    #[test]
    fn banked_cpu_interface_follows_active_cpu() {
        let (_gicd, _giccs, _lines, shared) = super::super::build_gicv2_mp(128, 2);
        let mut banked = Gicv2CpuInterface::from_banked_shared(Arc::clone(&shared));

        {
            let mut s = shared.lock().unwrap();
            s.dist.dist_ctlr = 1;
            s.cpus[1].cpu_ctlr = 1;
            s.cpus[1].pmr = 0xFF;
            s.dist.enabled[(33 / 32) as usize] |= 1 << (33 & 31);
            s.dist.pending[(33 / 32) as usize] |= 1 << (33 & 31);
            s.dist.targets[33] = 0b10;
            s.set_active_cpu(0);
        }
        assert_eq!(banked.read(0x00C, 4), SPURIOUS_IRQ as u64);

        {
            let mut s = shared.lock().unwrap();
            s.dist.pending[(33 / 32) as usize] |= 1 << (33 & 31);
            s.set_active_cpu(1);
        }
        assert_eq!(banked.read(0x00C, 4), 33);
    }

    #[test]
    fn running_priority_uses_banked_private_irq_priority() {
        let (_gicd, _giccs, _lines, shared) = super::super::build_gicv2_mp(128, 1);
        let mut gicc = Gicv2CpuInterface::from_shared(Arc::clone(&shared), 0);

        {
            let mut s = shared.lock().unwrap();
            s.dist.dist_ctlr = 1;
            s.cpus[0].cpu_ctlr = 1;
            s.cpus[0].pmr = 0xFF;
            s.cpus[0].private_enabled |= 1 << 7;
            s.cpus[0].private_pending |= 1 << 7;
            s.cpus[0].private_priority[7] = 0x23;
            s.dist.priority[7] = 0x91;
        }

        assert_eq!(gicc.read(0x00C, 4), 7);
        assert_eq!(gicc.read(0x014, 4), 0x23);
    }

    #[test]
    fn running_priority_tracks_nested_preemption_until_matching_eoi() {
        let (_gicd, _giccs, _lines, shared) = super::super::build_gicv2_mp(128, 1);
        let mut gicc = Gicv2CpuInterface::from_shared(Arc::clone(&shared), 0);

        {
            let mut s = shared.lock().unwrap();
            s.dist.dist_ctlr = 1;
            s.cpus[0].cpu_ctlr = 1;
            s.cpus[0].pmr = 0xFF;
            s.cpus[0].private_enabled |= (1 << 5) | (1 << 6) | (1 << 7);
            s.cpus[0].private_priority[5] = 0x40;
            s.cpus[0].private_priority[6] = 0x20;
            s.cpus[0].private_priority[7] = 0x60;
            s.cpus[0].private_pending |= 1 << 5;
        }

        assert_eq!(gicc.read(0x00C, 4), 5);
        assert_eq!(gicc.read(0x014, 4), 0x40);

        {
            let mut s = shared.lock().unwrap();
            s.cpus[0].private_pending |= 1 << 7;
            s.update_irq_line(0);
        }
        assert!(
            !gicc.has_pending_irq(),
            "lower-priority IRQ must not preempt the running interrupt"
        );

        {
            let mut s = shared.lock().unwrap();
            s.cpus[0].private_pending |= 1 << 6;
            s.update_irq_line(0);
        }
        assert!(gicc.has_pending_irq());
        assert_eq!(gicc.read(0x00C, 4), 6);
        assert_eq!(gicc.read(0x014, 4), 0x20);

        gicc.write(0x010, 4, 6);
        assert_eq!(gicc.read(0x014, 4), 0x40);
        assert!(!gicc.has_pending_irq());

        gicc.write(0x010, 4, 5);
        assert_eq!(gicc.read(0x014, 4), 0xFF);
        assert!(gicc.has_pending_irq());
        assert_eq!(gicc.read(0x00C, 4), 7);
        assert_eq!(gicc.read(0x014, 4), 0x60);
    }
}
