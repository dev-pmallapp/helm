//! GICv2 Distributor (GICD) — thin wrapper around shared distributor state.

use std::sync::{Arc, Mutex};

use helm_devices::Device;
use helm_diag::sim_stub;

use super::{GicSharedState, MAX_IRQS, NUM_REGS};

// ── Gicv2Distributor ─────────────────────────────────────────────────────────

/// GICv2 Distributor device.
///
/// When built via [`build_gicv2`](super::build_gicv2) the state is shared
/// with `Gicv2CpuInterface` and an `Arc<AtomicBool>` IRQ line is wired so
/// the CPU step loop sees real interrupt assertions.
///
/// For standalone testing, use `Gicv2Distributor::new(n)` which creates an
/// unshared instance with no IRQ line.
pub struct Gicv2Distributor(pub Arc<Mutex<GicSharedState>>);

impl Gicv2Distributor {
    /// Create a standalone (unshared) distributor for unit tests.
    pub fn new(num_irqs: u32) -> Self {
        let (gicd, _gicc, _line, _shared) = super::build_gicv2(num_irqs);
        gicd
    }

    /// Create from a pre-built shared state (used by `build_gicv2`).
    pub fn from_shared(state: Arc<Mutex<GicSharedState>>) -> Self {
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
        s.highest_pending_for_cpu(0)
            .map(|id| (id, s.dist.priority[id as usize]))
    }

    /// Acknowledge an interrupt: mark it active, clear pending.
    /// (Legacy helper; prefer `cpu_acknowledge` via GICC_IAR.)
    pub fn acknowledge(&mut self, irq: u32) {
        let mut s = self.0.lock().unwrap();
        if irq as usize >= MAX_IRQS {
            return;
        }
        let bit = 1u32 << (irq & 31);
        if irq < 32 {
            s.set_private_active(0, bit);
            s.clear_private_pending(0, bit);
        } else {
            let reg = (irq / 32) as usize;
            s.dist.active[reg] |= bit;
            s.dist.pending[reg] &= !bit;
        }
        // no update_irq_line here — matches original acknowledge() semantics
    }

    /// End-of-interrupt: clear the active bit.
    pub fn eoi(&mut self, irq: u32) {
        self.0.lock().unwrap().cpu_eoi(0, irq);
    }

    /// Legacy: set an IRQ-update callback (no-op; real wiring uses irq_line).
    pub fn set_irq_update_fn(&mut self, _f: Box<dyn FnMut(bool) + Send>) {}
}

// ── Device trait ─────────────────────────────────────────────────────────────

impl Device for Gicv2Distributor {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        let s = self.0.lock().unwrap();
        let active_cpu_idx = s.active_cpu_idx.min(s.cpus.len().saturating_sub(1));
        match offset {
            0x000 => u64::from(s.dist.dist_ctlr),
            0x004 => {
                let it_lines = if s.dist.num_irqs > 32 {
                    (s.dist.num_irqs / 32) - 1
                } else {
                    0
                };
                u64::from(it_lines)
            }
            0x008 => 0x0102_0043,
            o @ 0x080..=0x09C => {
                let n = ((o - 0x080) / 4) as usize;
                u64::from(if n == 0 {
                    s.private_group_for_cpu(active_cpu_idx)
                } else if n < NUM_REGS {
                    s.dist.group[n]
                } else {
                    0
                })
            }
            o @ 0x100..=0x11C => {
                let n = ((o - 0x100) / 4) as usize;
                u64::from(if n == 0 {
                    s.private_enabled_for_cpu(active_cpu_idx)
                } else if n < NUM_REGS {
                    s.dist.enabled[n]
                } else {
                    0
                })
            }
            o @ 0xF10..=0xF1C => {
                let group = ((o - 0xF10) / 4) as u32;
                let mut val = 0u32;
                let pending = s.private_pending_for_cpu(active_cpu_idx);
                for lane in 0..4u32 {
                    let sgi = group * 4 + lane;
                    if pending & (1u32 << sgi) != 0 {
                        val |= 0xFF << (lane * 8);
                    }
                }
                u64::from(val)
            }
            o @ 0xF20..=0xF2C => {
                let group = ((o - 0xF20) / 4) as u32;
                let mut val = 0u32;
                let pending = s.private_pending_for_cpu(active_cpu_idx);
                for lane in 0..4u32 {
                    let sgi = group * 4 + lane;
                    if pending & (1u32 << sgi) != 0 {
                        val |= 0xFF << (lane * 8);
                    }
                }
                u64::from(val)
            }
            o @ 0x180..=0x19C => {
                let n = ((o - 0x180) / 4) as usize;
                u64::from(if n == 0 {
                    s.private_enabled_for_cpu(active_cpu_idx)
                } else if n < NUM_REGS {
                    s.dist.enabled[n]
                } else {
                    0
                })
            }
            o @ 0x200..=0x21C => {
                let n = ((o - 0x200) / 4) as usize;
                u64::from(if n == 0 {
                    s.private_pending_for_cpu(active_cpu_idx)
                } else if n < NUM_REGS {
                    s.dist.pending[n]
                } else {
                    0
                })
            }
            o @ 0x280..=0x29C => {
                let n = ((o - 0x280) / 4) as usize;
                u64::from(if n == 0 {
                    s.private_pending_for_cpu(active_cpu_idx)
                } else if n < NUM_REGS {
                    s.dist.pending[n]
                } else {
                    0
                })
            }
            o @ 0x300..=0x31C => {
                let n = ((o - 0x300) / 4) as usize;
                u64::from(if n == 0 {
                    s.private_active_for_cpu(active_cpu_idx)
                } else if n < NUM_REGS {
                    s.dist.active[n]
                } else {
                    0
                })
            }
            o @ 0x400..=0x4FC => {
                let base = (o - 0x400) as usize;
                let mut val = 0u32;
                for i in 0..4 {
                    let idx = base + i;
                    if idx < 32 {
                        val |= u32::from(s.cpus[active_cpu_idx].private_priority[idx]) << (i * 8);
                    } else if idx < MAX_IRQS {
                        val |= u32::from(s.dist.priority[idx]) << (i * 8);
                    }
                }
                u64::from(val)
            }
            o @ 0x800..=0x8FC => {
                let base = (o - 0x800) as usize;
                let mut val = 0u32;
                for i in 0..4 {
                    let idx = base + i;
                    if idx < 32 {
                        val |= u32::from(1u8 << active_cpu_idx.min(7)) << (i * 8);
                    } else if idx < MAX_IRQS {
                        val |= u32::from(s.dist.targets[idx]) << (i * 8);
                    }
                }
                u64::from(val)
            }
            o @ 0xC00..=0xC3C => {
                let n = ((o - 0xC00) / 4) as usize;
                u64::from(if n < 2 {
                    s.cpus[active_cpu_idx].private_config[n]
                } else {
                    s.dist.config.get(n).copied().unwrap_or(0)
                })
            }
            // PID/CID
            0xFE0 => 0x90,
            0xFE4 => 0xB4,
            0xFE8 => 0x2B,
            0xFEC => 0x00,
            0xFF0 => 0x0D,
            0xFF4 => 0xF0,
            0xFF8 => 0x05,
            0xFFC => 0xB1,
            _ => {
                sim_stub!(
                    component = "gicv2-gicd",
                    "read unhandled offset={offset:#x} -> 0"
                );
                0
            }
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let val32 = val as u32;
        let mut s = self.0.lock().unwrap();
        let active_cpu_idx = s.active_cpu_idx.min(s.cpus.len().saturating_sub(1));
        match offset {
            0x000 => {
                s.dist.dist_ctlr = val32 & 1;
                s.update_all_irq_lines();
            }
            o @ 0x080..=0x09C => {
                let n = ((o - 0x080) / 4) as usize;
                if n == 0 {
                    if let Some(cpu) = s.cpus.get_mut(active_cpu_idx) {
                        cpu.private_group = val32;
                    }
                } else if n < NUM_REGS {
                    s.dist.group[n] = val32;
                }
            }
            o @ 0x100..=0x11C => {
                let n = ((o - 0x100) / 4) as usize;
                if n == 0 {
                    if let Some(cpu) = s.cpus.get_mut(active_cpu_idx) {
                        cpu.private_enabled |= val32;
                    }
                    s.update_irq_line(active_cpu_idx);
                } else if n < NUM_REGS {
                    s.dist.enabled[n] |= val32;
                    s.update_all_irq_lines();
                }
            }
            o @ 0x180..=0x19C => {
                let n = ((o - 0x180) / 4) as usize;
                if n == 0 {
                    if let Some(cpu) = s.cpus.get_mut(active_cpu_idx) {
                        cpu.private_enabled &= !val32;
                    }
                    s.update_irq_line(active_cpu_idx);
                } else if n < NUM_REGS {
                    s.dist.enabled[n] &= !val32;
                    s.update_all_irq_lines();
                }
            }
            o @ 0x200..=0x21C => {
                let n = ((o - 0x200) / 4) as usize;
                if n == 0 {
                    s.set_private_pending(active_cpu_idx, val32);
                    s.update_irq_line(active_cpu_idx);
                } else if n < NUM_REGS {
                    s.dist.pending[n] |= val32;
                    s.update_all_irq_lines();
                }
            }
            o @ 0x280..=0x29C => {
                let n = ((o - 0x280) / 4) as usize;
                if n == 0 {
                    s.clear_private_pending(active_cpu_idx, val32);
                    s.update_irq_line(active_cpu_idx);
                } else if n < NUM_REGS {
                    s.dist.pending[n] &= !val32;
                    s.update_all_irq_lines();
                }
            }
            o @ 0x380..=0x39C => {
                let n = ((o - 0x380) / 4) as usize;
                if n == 0 {
                    s.clear_private_active(active_cpu_idx, val32);
                } else if n < NUM_REGS {
                    s.dist.active[n] &= !val32;
                }
            }
            o @ 0x400..=0x4FC => {
                let base = (o - 0x400) as usize;
                for i in 0..4 {
                    let idx = base + i;
                    if idx < 32 {
                        s.cpus[active_cpu_idx].private_priority[idx] =
                            ((val32 >> (i * 8)) & 0xFF) as u8;
                    } else if idx < MAX_IRQS {
                        s.dist.priority[idx] = ((val32 >> (i * 8)) & 0xFF) as u8;
                    }
                }
            }
            o @ 0x800..=0x8FC => {
                let base = (o - 0x800) as usize;
                for i in 0..4 {
                    let idx = base + i;
                    if idx >= 32 && idx < MAX_IRQS {
                        s.dist.targets[idx] = ((val32 >> (i * 8)) & 0xFF) as u8;
                    }
                }
                s.update_all_irq_lines();
            }
            o @ 0xC00..=0xC3C => {
                let n = ((o - 0xC00) / 4) as usize;
                if n < 2 {
                    s.cpus[active_cpu_idx].private_config[n] = val32;
                } else if n < s.dist.config.len() {
                    s.dist.config[n] = val32;
                }
            }
            0xF00 => {
                let sgintid = val32 & 0xF;
                let target_mask = ((val32 >> 16) & 0xFF) as u8;
                let target_filter = (val32 >> 24) & 0x3;
                s.generate_sgi(active_cpu_idx, sgintid, target_mask, target_filter);
            }
            o @ 0xF10..=0xF1C => {
                let group = ((o - 0xF10) / 4) as u32;
                let mut clear_mask = 0u32;
                for lane in 0..4u32 {
                    if ((val32 >> (lane * 8)) & 0xFF) != 0 {
                        clear_mask |= 1u32 << (group * 4 + lane);
                    }
                }
                s.clear_private_pending(active_cpu_idx, clear_mask);
                s.update_irq_line(active_cpu_idx);
            }
            o @ 0xF20..=0xF2C => {
                let group = ((o - 0xF20) / 4) as u32;
                let mut set_mask = 0u32;
                for lane in 0..4u32 {
                    if ((val32 >> (lane * 8)) & 0xFF) != 0 {
                        set_mask |= 1u32 << (group * 4 + lane);
                    }
                }
                s.set_private_pending(active_cpu_idx, set_mask);
                s.update_irq_line(active_cpu_idx);
            }
            _ => {
                sim_stub!(
                    component = "gicv2-gicd",
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
    use std::sync::Arc;

    #[test]
    fn typer_reports_irq_count() {
        let mut gicd = Gicv2Distributor::new(128);
        assert_eq!(gicd.read(0x004, 4), 3); // (128/32) - 1 = 3
    }

    #[test]
    fn enable_and_pending() {
        let mut gicd = Gicv2Distributor::new(128);
        gicd.write(0x000, 4, 1); // GICD_CTLR enable
        gicd.write(0x104, 4, 0x2); // ISENABLER[1] bit 1 = IRQ 33
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
        assert_eq!(id, 33); // priority 0x10 < 0x80
        assert_eq!(prio, 0x10);
    }

    #[test]
    fn private_ppi_state_is_banked_per_cpu() {
        let (mut gicd, mut giccs, _lines, shared) = super::super::build_gicv2_mp(128, 2);

        gicd.write(0x000, 4, 1);
        giccs[0].write(0x000, 4, 1);
        giccs[0].write(0x004, 4, 0xFF);
        giccs[1].write(0x000, 4, 1);
        giccs[1].write(0x004, 4, 0xFF);

        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(0);
        }
        gicd.write(0x100, 4, 1 << 30);
        gicd.write(0x200, 4, 1 << 30);

        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(1);
        }
        gicd.write(0x100, 4, 1 << 30);

        let mut banked = super::super::Gicv2CpuInterface::from_banked_shared(Arc::clone(&shared));
        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(0);
        }
        assert_eq!(banked.read(0x00C, 4), 30);
        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(1);
        }
        assert_eq!(banked.read(0x00C, 4), super::super::SPURIOUS_IRQ as u64);
    }

    #[test]
    fn sgir_targets_secondary_cpu() {
        let (mut gicd, mut giccs, _lines, shared) = super::super::build_gicv2_mp(128, 2);

        gicd.write(0x000, 4, 1);
        giccs[0].write(0x000, 4, 1);
        giccs[0].write(0x004, 4, 0xFF);
        giccs[1].write(0x000, 4, 1);
        giccs[1].write(0x004, 4, 0xFF);

        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(1);
        }
        gicd.write(0x100, 4, 1 << 5);

        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(0);
        }
        gicd.write(0xF00, 4, u64::from((0b10u32 << 16) | 5));

        let mut banked = super::super::Gicv2CpuInterface::from_banked_shared(Arc::clone(&shared));
        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(0);
        }
        assert_eq!(banked.read(0x00C, 4), super::super::SPURIOUS_IRQ as u64);
        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(1);
        }
        assert_eq!(banked.read(0x00C, 4), 5);
    }

    #[test]
    fn igroupr_is_banked_for_private_irqs_and_shared_for_spis() {
        let (mut gicd, _giccs, _lines, shared) = super::super::build_gicv2_mp(128, 2);

        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(0);
        }
        gicd.write(0x080, 4, 0xA5A5_5A5A);
        gicd.write(0x084, 4, 0x1122_3344);

        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(1);
        }
        assert_eq!(gicd.read(0x080, 4), 0, "IGROUPR0 is banked per CPU");
        assert_eq!(gicd.read(0x084, 4), 0x1122_3344, "IGROUPR1 is shared");
        gicd.write(0x080, 4, 0x55AA_55AA);

        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(0);
        }
        assert_eq!(gicd.read(0x080, 4), 0xA5A5_5A5A);
        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(1);
        }
        assert_eq!(gicd.read(0x080, 4), 0x55AA_55AA);
    }

    #[test]
    fn cpendsgir_clears_sgi_pending_without_stubbed_offsets() {
        let (mut gicd, mut giccs, _lines, shared) = super::super::build_gicv2_mp(128, 1);

        gicd.write(0x000, 4, 1);
        giccs[0].write(0x000, 4, 1);
        giccs[0].write(0x004, 4, 0xFF);
        gicd.write(0x100, 4, 0xF);
        gicd.write(0xF20, 4, 0xFFFF_FFFF);
        assert_eq!(gicd.read(0x200, 4) & 0xF, 0xF, "SPENDSGIR sets SGI pending");
        assert_eq!(gicd.read(0xF10, 4), 0xFFFF_FFFF, "CPENDSGIR reports pending SGIs");

        {
            let mut s = shared.lock().unwrap();
            s.set_active_cpu(0);
        }
        gicd.write(0xF10, 4, 0xFFFF_0000);
        assert_eq!(gicd.read(0x200, 4) & 0xF, 0x3, "selected SGIs are cleared by byte lane");
        gicd.write(0xF10, 4, 0x0000_FFFF);
        assert_eq!(gicd.read(0x200, 4) & 0xF, 0, "selected SGIs are cleared");
        assert_eq!(gicd.read(0xF10, 4), 0, "CPENDSGIR reads back clear after write");
    }
}
