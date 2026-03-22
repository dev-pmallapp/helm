//! GICv2 interrupt controller — shared distributor state plus per-CPU interfaces.
//!
//! `build_gicv2()` preserves the existing single-CPU API.
//! `build_gicv2_mp()` exposes the vector-shaped state needed for future SMP work.

pub mod distributor;
pub mod cpu_interface;

pub use distributor::Gicv2Distributor;
pub use cpu_interface::Gicv2CpuInterface;

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "probe")]
use helm_probe::{probe, GicProbes, IrqEvent};
use helm_diag::sim_info;

pub(crate) const MAX_IRQS: usize = 256;
pub(crate) const NUM_REGS: usize = MAX_IRQS / 32;
pub(crate) const SPURIOUS_IRQ: u32 = 1023;

/// Distributor-global GIC state.
pub struct GicDistState {
    pub dist_ctlr: u32,
    pub enabled: [u32; NUM_REGS],
    pub pending: [u32; NUM_REGS],
    pub active: [u32; NUM_REGS],
    pub priority: [u8; MAX_IRQS],
    pub targets: [u8; MAX_IRQS],
    pub config: [u32; MAX_IRQS / 16],
    pub num_irqs: u32,
    physical_level: [u32; NUM_REGS],
}

impl GicDistState {
    fn new(num_irqs: u32) -> Self {
        let mut state = Self {
            dist_ctlr: 0,
            enabled: [0; NUM_REGS],
            pending: [0; NUM_REGS],
            active: [0; NUM_REGS],
            priority: [0u8; MAX_IRQS],
            targets: [0u8; MAX_IRQS],
            config: [0u32; MAX_IRQS / 16],
            num_irqs: num_irqs.min(MAX_IRQS as u32),
            physical_level: [0; NUM_REGS],
        };
        for t in &mut state.targets {
            *t = 1;
        }
        state
    }
}

/// CPU-interface-local GIC state.
pub struct GicCpuState {
    pub cpu_ctlr: u32,
    pub pmr: u32,
    pub bpr: u32,
    pub last_ack: u32,
    pub irq_line: Option<Arc<AtomicBool>>,
    /// SGI/PPI enable bits (IRQs 0-31), banked per CPU.
    pub private_enabled: u32,
    /// SGI/PPI pending bits, banked per CPU.
    pub private_pending: u32,
    /// SGI/PPI active bits, banked per CPU.
    pub private_active: u32,
    /// SGI/PPI priority bytes, banked per CPU.
    pub private_priority: [u8; 32],
    /// SGI/PPI configuration words, banked per CPU.
    pub private_config: [u32; 2],
}

impl GicCpuState {
    fn new(irq_line: Arc<AtomicBool>) -> Self {
        Self {
            cpu_ctlr: 0,
            pmr: 0xFF,
            bpr: 0,
            last_ack: SPURIOUS_IRQ,
            irq_line: Some(irq_line),
            private_enabled: 0,
            private_pending: 0,
            private_active: 0,
            private_priority: [0u8; 32],
            private_config: [0u32; 2],
        }
    }
}

/// Combined GIC state: one distributor plus N CPU interfaces.
pub struct GicSharedState {
    pub dist: GicDistState,
    pub cpus: Vec<GicCpuState>,
    pub active_cpu_idx: usize,
    sgi_log_budget: u32,
    #[cfg(feature = "probe")]
    pub probes: GicProbes,
}

impl GicSharedState {
    pub fn new(num_irqs: u32, num_cpus: usize) -> Self {
        let count = num_cpus.max(1);
        let mut cpus = Vec::with_capacity(count);
        for _ in 0..count {
            cpus.push(GicCpuState::new(Arc::new(AtomicBool::new(false))));
        }
        Self {
            dist: GicDistState::new(num_irqs),
            cpus,
            active_cpu_idx: 0,
            sgi_log_budget: 64,
            #[cfg(feature = "probe")]
            probes: GicProbes::default(),
        }
    }

    fn routes_to_cpu(&self, irq: usize, cpu_idx: usize) -> bool {
        if irq < 32 || cpu_idx >= self.cpus.len() {
            return false;
        }
        cpu_idx < 8 && (self.dist.targets[irq] & (1 << cpu_idx)) != 0
    }

    fn highest_private_pending_for_cpu(&self, cpu_idx: usize) -> Option<(u32, u8)> {
        let cpu = self.cpus.get(cpu_idx)?;
        let pmr = cpu.pmr as u8;
        let mut best: Option<(u32, u8)> = None;
        for irq in 0..32usize {
            let bit = 1u32 << irq;
            if cpu.private_pending & bit == 0
                || cpu.private_enabled & bit == 0
                || cpu.private_active & bit != 0
            {
                continue;
            }
            let prio = cpu.private_priority[irq];
            if prio < pmr && best.map_or(true, |(_, bp)| prio < bp) {
                best = Some((irq as u32, prio));
            }
        }
        best
    }

    fn private_pending_for_cpu(&self, cpu_idx: usize) -> u32 {
        self.cpus.get(cpu_idx).map_or(0, |cpu| cpu.private_pending)
    }

    fn private_enabled_for_cpu(&self, cpu_idx: usize) -> u32 {
        self.cpus.get(cpu_idx).map_or(0, |cpu| cpu.private_enabled)
    }

    fn private_active_for_cpu(&self, cpu_idx: usize) -> u32 {
        self.cpus.get(cpu_idx).map_or(0, |cpu| cpu.private_active)
    }

    fn set_private_pending(&mut self, cpu_idx: usize, mask: u32) {
        if let Some(cpu) = self.cpus.get_mut(cpu_idx) {
            cpu.private_pending |= mask;
        }
    }

    fn clear_private_pending(&mut self, cpu_idx: usize, mask: u32) {
        if let Some(cpu) = self.cpus.get_mut(cpu_idx) {
            cpu.private_pending &= !mask;
        }
    }

    fn set_private_active(&mut self, cpu_idx: usize, mask: u32) {
        if let Some(cpu) = self.cpus.get_mut(cpu_idx) {
            cpu.private_active |= mask;
        }
    }

    fn clear_private_active(&mut self, cpu_idx: usize, mask: u32) {
        if let Some(cpu) = self.cpus.get_mut(cpu_idx) {
            cpu.private_active &= !mask;
        }
    }

    pub fn update_irq_line(&self, cpu_idx: usize) {
        if let Some(cpu) = self.cpus.get(cpu_idx) {
            if let Some(ref line) = cpu.irq_line {
                let should_raise = self.dist.dist_ctlr & 1 != 0
                    && cpu.cpu_ctlr & 1 != 0
                    && self.highest_pending_for_cpu(cpu_idx).is_some();
                line.store(should_raise, Ordering::Release);
            }
        }
    }

    pub fn update_all_irq_lines(&self) {
        for cpu_idx in 0..self.cpus.len() {
            self.update_irq_line(cpu_idx);
        }
    }

    pub fn highest_pending_for_cpu(&self, cpu_idx: usize) -> Option<u32> {
        if self.dist.dist_ctlr & 1 == 0 || cpu_idx >= self.cpus.len() {
            return None;
        }
        let mut best = self.highest_private_pending_for_cpu(cpu_idx);
        let pmr = self.cpus[cpu_idx].pmr as u8;
        for irq in 32..self.dist.num_irqs as usize {
            let reg = irq / 32;
            let bit = 1u32 << (irq & 31);
            if self.dist.pending[reg] & bit == 0
                || self.dist.enabled[reg] & bit == 0
                || self.dist.active[reg] & bit != 0
                || !self.routes_to_cpu(irq, cpu_idx)
            {
                continue;
            }
            let prio = self.dist.priority[irq];
            if prio < pmr && best.map_or(true, |(_, bp)| prio < bp) {
                best = Some((irq as u32, prio));
            }
        }
        best.map(|(id, _)| id)
    }

    pub fn set_active_cpu(&mut self, cpu_idx: usize) {
        self.active_cpu_idx = cpu_idx.min(self.cpus.len().saturating_sub(1));
    }

    pub fn assert_irq(&mut self, irq: u32) {
        if irq as usize >= MAX_IRQS {
            return;
        }
        if irq < 32 {
            self.set_private_pending(self.active_cpu_idx, 1u32 << irq);
            self.update_irq_line(self.active_cpu_idx);
            #[cfg(feature = "probe")]
            probe!(self.probes.irq_asserted, IrqEvent { irq_id: irq, asserted: true });
            return;
        }
        let reg = (irq / 32) as usize;
        let bit = 1u32 << (irq & 31);
        self.dist.physical_level[reg] |= bit;
        self.dist.pending[reg] |= bit;
        self.update_all_irq_lines();
        #[cfg(feature = "probe")]
        probe!(self.probes.irq_asserted, IrqEvent { irq_id: irq, asserted: true });
    }

    pub fn deassert_irq(&mut self, irq: u32) {
        if irq as usize >= MAX_IRQS {
            return;
        }
        if irq < 32 {
            self.clear_private_pending(self.active_cpu_idx, 1u32 << irq);
            self.update_irq_line(self.active_cpu_idx);
            #[cfg(feature = "probe")]
            probe!(self.probes.irq_deasserted, IrqEvent { irq_id: irq, asserted: false });
            return;
        }
        let reg = (irq / 32) as usize;
        let bit = 1u32 << (irq & 31);
        self.dist.physical_level[reg] &= !bit;
        self.dist.pending[reg] &= !bit;
        self.update_all_irq_lines();
        #[cfg(feature = "probe")]
        probe!(self.probes.irq_deasserted, IrqEvent { irq_id: irq, asserted: false });
    }

    pub fn cpu_acknowledge(&mut self, cpu_idx: usize) -> u32 {
        if let Some(irq) = self.highest_pending_for_cpu(cpu_idx) {
            let bit = 1u32 << (irq & 31);
            if irq < 32 {
                self.clear_private_pending(cpu_idx, bit);
                self.set_private_active(cpu_idx, bit);
            } else {
                let reg = (irq / 32) as usize;
                self.dist.pending[reg] &= !bit;
                self.dist.active[reg] |= bit;
            }
            self.cpus[cpu_idx].last_ack = irq;
            self.update_irq_line(cpu_idx);
            irq
        } else {
            SPURIOUS_IRQ
        }
    }

    pub fn cpu_eoi(&mut self, cpu_idx: usize, irq: u32) {
        if irq as usize >= MAX_IRQS || cpu_idx >= self.cpus.len() {
            return;
        }
        let bit = 1u32 << (irq & 31);
        if irq < 32 {
            self.clear_private_active(cpu_idx, bit);
            self.update_irq_line(cpu_idx);
        } else {
            let reg = (irq / 32) as usize;
            self.dist.active[reg] &= !bit;
            if self.dist.physical_level[reg] & bit != 0 {
                self.dist.pending[reg] |= bit;
            }
            self.update_all_irq_lines();
        }
        self.cpus[cpu_idx].last_ack = SPURIOUS_IRQ;
        #[cfg(feature = "probe")]
        probe!(self.probes.eoi, IrqEvent { irq_id: irq, asserted: false });
    }

    pub fn generate_sgi(&mut self, source_cpu_idx: usize, sgintid: u32, target_mask: u8, target_filter: u32) {
        if sgintid >= 16 || source_cpu_idx >= self.cpus.len() {
            return;
        }
        let bit = 1u32 << sgintid;
        let cpu_count = self.cpus.len();
        let mut targets = Vec::new();
        match target_filter {
            0b00 => {
                for cpu_idx in 0..cpu_count.min(8) {
                    if (target_mask & (1u8 << cpu_idx)) != 0 {
                        self.set_private_pending(cpu_idx, bit);
                        targets.push(cpu_idx);
                    }
                }
            }
            0b01 => {
                for cpu_idx in 0..cpu_count {
                    if cpu_idx != source_cpu_idx {
                        self.set_private_pending(cpu_idx, bit);
                        targets.push(cpu_idx);
                    }
                }
            }
            0b10 => {
                self.set_private_pending(source_cpu_idx, bit);
                targets.push(source_cpu_idx);
            }
            _ => return,
        }
        if self.sgi_log_budget > 0 {
            self.sgi_log_budget -= 1;
            sim_info!(
                component = "gicv2-smp",
                "cpu{} SGI{} targets={:?} filter={} mask={:#04x}",
                source_cpu_idx,
                sgintid,
                targets,
                target_filter,
                target_mask
            );
        }
        self.update_all_irq_lines();
    }
}

/// Backward-compatible single-CPU builder.
pub fn build_gicv2(
    num_irqs: u32,
) -> (
    Gicv2Distributor,
    Gicv2CpuInterface,
    Arc<AtomicBool>,
    Arc<Mutex<GicSharedState>>,
) {
    let (gicd, mut giccs, mut irq_lines, shared) = build_gicv2_mp(num_irqs, 1);
    (gicd, giccs.remove(0), irq_lines.remove(0), shared)
}

/// Multicore-ready builder returning one CPU interface and IRQ line per CPU.
pub fn build_gicv2_mp(
    num_irqs: u32,
    num_cpus: usize,
) -> (
    Gicv2Distributor,
    Vec<Gicv2CpuInterface>,
    Vec<Arc<AtomicBool>>,
    Arc<Mutex<GicSharedState>>,
) {
    let state = GicSharedState::new(num_irqs, num_cpus);
    let irq_lines: Vec<Arc<AtomicBool>> = state
        .cpus
        .iter()
        .filter_map(|cpu| cpu.irq_line.as_ref().cloned())
        .collect();
    let shared = Arc::new(Mutex::new(state));
    let cpu_count = irq_lines.len();
    let mut giccs = Vec::with_capacity(cpu_count);
    for cpu_idx in 0..cpu_count {
        giccs.push(Gicv2CpuInterface::from_shared(Arc::clone(&shared), cpu_idx));
    }
    (Gicv2Distributor::from_shared(Arc::clone(&shared)), giccs, irq_lines, shared)
}

/// Interrupt sink that routes a device line into a shared GIC INTID.
pub struct GicSink {
    gic: Arc<Mutex<GicSharedState>>,
    pub intid: u32,
}

impl GicSink {
    pub fn new(gic: Arc<Mutex<GicSharedState>>, intid: u32) -> Self {
        Self { gic, intid }
    }
}

impl helm_devices::InterruptSink for GicSink {
    fn on_assert(&self, _wire_id: helm_devices::WireId) {
        self.gic.lock().unwrap().assert_irq(self.intid);
    }

    fn on_deassert(&self, _wire_id: helm_devices::WireId) {
        self.gic.lock().unwrap().deassert_irq(self.intid);
    }
}
