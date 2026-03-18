//! GICv2 interrupt controller — shared state, distributor, and CPU interface.
//!
//! `build_gicv2(num_irqs)` is the primary entry point. It returns two device
//! objects that share the same `GicState` via `Arc<Mutex<>>`, and an
//! `Arc<AtomicBool>` IRQ line that the CPU polls each step.

pub mod distributor;
pub mod cpu_interface;

pub use distributor::Gicv2Distributor;
pub use cpu_interface::Gicv2CpuInterface;

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

// ── Constants ─────────────────────────────────────────────────────────────────

pub(crate) const MAX_IRQS: usize = 256;
pub(crate) const NUM_REGS: usize = MAX_IRQS / 32;
pub(crate) const SPURIOUS_IRQ: u32 = 1023;

// ── GicState — all GICD + GICC register state in one place ───────────────────

/// Combined GICD + GICC state shared between the two device objects.
pub struct GicState {
    // ── GICD ────────────────────────────────────────────────────────────────
    pub dist_ctlr: u32,
    pub enabled:   [u32; NUM_REGS],
    pub pending:   [u32; NUM_REGS],
    pub active:    [u32; NUM_REGS],
    pub priority:  [u8; MAX_IRQS],
    pub targets:   [u8; MAX_IRQS],
    pub config:    [u32; MAX_IRQS / 16],
    pub num_irqs:  u32,
    // ── GICC ────────────────────────────────────────────────────────────────
    pub cpu_ctlr:  u32,
    pub pmr:       u32,
    pub bpr:       u32,
    pub last_ack:  u32,
    // ── IRQ line to CPU ─────────────────────────────────────────────────────
    pub irq_line:  Option<Arc<AtomicBool>>,
}

impl GicState {
    pub fn new(num_irqs: u32) -> Self {
        let mut s = Self {
            dist_ctlr: 0,
            enabled:   [0; NUM_REGS],
            pending:   [0; NUM_REGS],
            active:    [0; NUM_REGS],
            priority:  [0u8; MAX_IRQS],
            targets:   [0u8; MAX_IRQS],
            config:    [0u32; MAX_IRQS / 16],
            num_irqs:  num_irqs.min(MAX_IRQS as u32),
            cpu_ctlr:  0,
            pmr:       0xFF,
            bpr:       0,
            last_ack:  SPURIOUS_IRQ,
            irq_line:  None,
        };
        for t in &mut s.targets { *t = 1; }
        s
    }

    /// Recompute and apply the IRQ line level.
    pub fn update_irq_line(&self) {
        if let Some(ref line) = self.irq_line {
            let should_raise = self.dist_ctlr & 1 != 0
                && self.cpu_ctlr & 1 != 0
                && self.highest_pending().is_some();
            line.store(should_raise, Ordering::Release);
        }
    }

    /// Find the highest-priority pending+enabled interrupt whose priority
    /// passes the CPU PMR. Returns the INTID, or `None`.
    pub fn highest_pending(&self) -> Option<u32> {
        if self.dist_ctlr & 1 == 0 { return None; }
        let pmr = self.pmr as u8;
        let mut best: Option<(u32, u8)> = None;
        for irq in 0..self.num_irqs as usize {
            let reg = irq / 32;
            let bit = 1u32 << (irq & 31);
            if self.pending[reg] & bit != 0
                && self.enabled[reg] & bit != 0
                && self.active[reg] & bit == 0
            {
                let prio = self.priority[irq];
                if prio < pmr && best.map_or(true, |(_, bp)| prio < bp) {
                    best = Some((irq as u32, prio));
                }
            }
        }
        best.map(|(id, _)| id)
    }

    /// Assert a peripheral interrupt.
    pub fn assert_irq(&mut self, irq: u32) {
        if irq as usize >= MAX_IRQS { return; }
        self.pending[(irq / 32) as usize] |= 1 << (irq & 31);
        self.update_irq_line();
    }

    /// Deassert a peripheral interrupt.
    pub fn deassert_irq(&mut self, irq: u32) {
        if irq as usize >= MAX_IRQS { return; }
        self.pending[(irq / 32) as usize] &= !(1 << (irq & 31));
        self.update_irq_line();
    }

    /// GICC_IAR read: acknowledge the highest pending interrupt.
    /// Moves it from pending -> active, updates the IRQ line, returns INTID.
    pub fn cpu_acknowledge(&mut self) -> u32 {
        if let Some(irq) = self.highest_pending() {
            let reg = (irq / 32) as usize;
            let bit = 1u32 << (irq & 31);
            self.pending[reg] &= !bit;
            self.active[reg]  |= bit;
            self.last_ack = irq;
            self.update_irq_line();
            irq
        } else {
            SPURIOUS_IRQ
        }
    }

    /// GICC_EOIR write: deactivate an acknowledged interrupt.
    pub fn cpu_eoi(&mut self, irq: u32) {
        if irq as usize >= MAX_IRQS { return; }
        self.active[(irq / 32) as usize] &= !(1 << (irq & 31));
        self.last_ack = SPURIOUS_IRQ;
        self.update_irq_line();
    }
}

// ── build_gicv2 ───────────────────────────────────────────────────────────────

/// Build a GICv2 distributor + CPU interface that share state.
///
/// Returns:
/// - `Gicv2Distributor`      — maps to GICD base (4 KiB MMIO)
/// - `Gicv2CpuInterface`     — maps to GICC base (4 KiB MMIO)
/// - `Arc<AtomicBool>`       — IRQ line; `true` = IRQ asserted to CPU
/// - `Arc<Mutex<GicState>>`  — shared GIC state for asserting device IRQs
pub fn build_gicv2(num_irqs: u32) -> (Gicv2Distributor, Gicv2CpuInterface, Arc<AtomicBool>, Arc<Mutex<GicState>>) {
    let irq_line = Arc::new(AtomicBool::new(false));
    let mut state = GicState::new(num_irqs);
    state.irq_line = Some(Arc::clone(&irq_line));
    let shared = Arc::new(Mutex::new(state));
    (
        Gicv2Distributor::from_shared(Arc::clone(&shared)),
        Gicv2CpuInterface::from_shared(Arc::clone(&shared)),
        irq_line,
        shared,
    )
}
