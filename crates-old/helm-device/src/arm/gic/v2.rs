//! GIC (Generic Interrupt Controller) — ARM IHI0048B.
//!
//! Stub implementation of GICv2 distributor + CPU interface.
//! Sufficient for basic interrupt routing in simulation.

use crate::device::{Device, DeviceEvent};
use crate::irq::InterruptController;
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::{HelmResult, IrqSignal};

const GICD_CTLR: u64 = 0x000;
const GICD_TYPER: u64 = 0x004;
const GICD_IIDR: u64 = 0x008;
const GICD_ISENABLER_BASE: u64 = 0x100;
const GICD_ICENABLER_BASE: u64 = 0x180;
const GICD_ISPENDR_BASE: u64 = 0x200;
const GICD_ICPENDR_BASE: u64 = 0x280;
const GICD_IPRIORITYR_BASE: u64 = 0x400;
const GICD_ITARGETSR_BASE: u64 = 0x800;
const GICD_ICFGR_BASE: u64 = 0xC00;

const GICC_CTLR: u64 = 0x000;
const GICC_PMR: u64 = 0x004;
const GICC_IAR: u64 = 0x00C;
const GICC_EOIR: u64 = 0x010;

/// GICv2 distributor + CPU interface.
pub struct Gic {
    dev_name: String,
    region: MemRegion,
    /// Number of supported IRQs (must be multiple of 32, max 1020).
    pub num_irqs: u32,
    /// Distributor control register.
    dist_ctrl: u32,
    /// Enable bits (1 bit per IRQ).
    enabled: Vec<u32>,
    /// Pending bits (1 bit per IRQ).
    pending: Vec<u32>,
    /// Priority (8 bits per IRQ).
    priority: Vec<u8>,
    /// Target CPU mask (8 bits per IRQ, only CPU0 in single-core).
    targets: Vec<u8>,
    /// CPU interface control.
    cpu_ctrl: u32,
    /// Priority mask register.
    priority_mask: u32,
    /// Last acknowledged IRQ (for EOIR tracking).
    last_ack: Option<u32>,
    /// Optional signal raised when any IRQ is pending for the CPU.
    irq_signal: Option<IrqSignal>,
    /// Interrupt configuration (2 bits per IRQ: edge/level).
    config: Vec<u32>,
}

impl Gic {
    /// Create a new GICv2 supporting `num_irqs` interrupt lines.
    pub fn new(name: impl Into<String>, num_irqs: u32) -> Self {
        let nirq = (num_irqs.div_ceil(32) * 32).min(1020) as usize;
        let n = name.into();
        Self {
            region: MemRegion {
                name: n.clone(),
                base: 0,
                size: 0x20000,
                kind: crate::region::RegionKind::Io,
                priority: 0,
            },
            dev_name: n,
            num_irqs: nirq as u32,
            dist_ctrl: 0,
            enabled: vec![0u32; nirq / 32],
            pending: vec![0u32; nirq / 32],
            priority: vec![0u8; nirq],
            targets: vec![1u8; nirq],
            cpu_ctrl: 0,
            priority_mask: 0xFF,
            last_ack: None,
            irq_signal: None,
            config: vec![0u32; nirq / 16],
        }
    }

    /// Attach an IRQ signal that will be raised/lowered as pending state changes.
    pub fn set_irq_signal(&mut self, signal: IrqSignal) {
        self.irq_signal = Some(signal);
    }

    fn update_irq_signal(&self) {
        if let Some(ref sig) = self.irq_signal {
            if self.dist_ctrl & 1 != 0 && self.cpu_ctrl & 1 != 0 && self.highest_pending().is_some()
            {
                sig.raise();
            } else {
                sig.lower();
            }
        }
    }

    fn is_enabled(&self, irq: u32) -> bool {
        let idx = (irq / 32) as usize;
        let bit = irq % 32;
        idx < self.enabled.len() && self.enabled[idx] & (1 << bit) != 0
    }

    fn is_pending(&self, irq: u32) -> bool {
        let idx = (irq / 32) as usize;
        let bit = irq % 32;
        idx < self.pending.len() && self.pending[idx] & (1 << bit) != 0
    }

    fn set_pending(&mut self, irq: u32) {
        let idx = (irq / 32) as usize;
        let bit = irq % 32;
        if idx < self.pending.len() {
            self.pending[idx] |= 1 << bit;
        }
    }

    fn clear_pending(&mut self, irq: u32) {
        let idx = (irq / 32) as usize;
        let bit = irq % 32;
        if idx < self.pending.len() {
            self.pending[idx] &= !(1 << bit);
        }
    }

    fn highest_pending(&self) -> Option<u32> {
        let mut best: Option<(u32, u8)> = None;
        for irq in 0..self.num_irqs {
            if self.is_pending(irq) && self.is_enabled(irq) {
                let prio = self.priority.get(irq as usize).copied().unwrap_or(0xFF);
                if prio < self.priority_mask as u8 && best.is_none_or(|(_, bp)| prio < bp) {
                    best = Some((irq, prio));
                }
            }
        }
        best.map(|(irq, _)| irq)
    }

    fn handle_dist_read(&self, offset: u64) -> u32 {
        match offset {
            GICD_CTLR => self.dist_ctrl,
            GICD_TYPER => (self.num_irqs / 32).saturating_sub(1) & 0x1F,
            GICD_IIDR => 0x0200_043B,
            o if (GICD_ISENABLER_BASE..GICD_ISENABLER_BASE + 0x80).contains(&o) => {
                let idx = ((o - GICD_ISENABLER_BASE) / 4) as usize;
                self.enabled.get(idx).copied().unwrap_or(0)
            }
            o if (GICD_ISPENDR_BASE..GICD_ISPENDR_BASE + 0x80).contains(&o) => {
                let idx = ((o - GICD_ISPENDR_BASE) / 4) as usize;
                self.pending.get(idx).copied().unwrap_or(0)
            }
            o if (GICD_IPRIORITYR_BASE..GICD_IPRIORITYR_BASE + 0x400).contains(&o) => {
                let base_irq = (o - GICD_IPRIORITYR_BASE) as usize;
                let mut val = 0u32;
                for i in 0..4 {
                    if base_irq + i < self.priority.len() {
                        val |= (self.priority[base_irq + i] as u32) << (i * 8);
                    }
                }
                val
            }
            o if (GICD_ITARGETSR_BASE..GICD_ITARGETSR_BASE + 0x400).contains(&o) => {
                let base_irq = (o - GICD_ITARGETSR_BASE) as usize;
                let mut val = 0u32;
                for i in 0..4 {
                    if base_irq + i < self.targets.len() {
                        val |= (self.targets[base_irq + i] as u32) << (i * 8);
                    }
                }
                val
            }
            o if (GICD_ICFGR_BASE..GICD_ICFGR_BASE + 0x100).contains(&o) => {
                let idx = ((o - GICD_ICFGR_BASE) / 4) as usize;
                self.config.get(idx).copied().unwrap_or(0)
            }
            _ => 0,
        }
    }

    fn handle_dist_write(&mut self, offset: u64, value: u32) {
        match offset {
            GICD_CTLR => self.dist_ctrl = value & 1,
            o if (GICD_ISENABLER_BASE..GICD_ISENABLER_BASE + 0x80).contains(&o) => {
                let idx = ((o - GICD_ISENABLER_BASE) / 4) as usize;
                if idx < self.enabled.len() {
                    self.enabled[idx] |= value;
                }
            }
            o if (GICD_ICENABLER_BASE..GICD_ICENABLER_BASE + 0x80).contains(&o) => {
                let idx = ((o - GICD_ICENABLER_BASE) / 4) as usize;
                if idx < self.enabled.len() {
                    self.enabled[idx] &= !value;
                }
            }
            o if (GICD_ISPENDR_BASE..GICD_ISPENDR_BASE + 0x80).contains(&o) => {
                let idx = ((o - GICD_ISPENDR_BASE) / 4) as usize;
                if idx < self.pending.len() {
                    self.pending[idx] |= value;
                }
            }
            o if (GICD_ICPENDR_BASE..GICD_ICPENDR_BASE + 0x80).contains(&o) => {
                let idx = ((o - GICD_ICPENDR_BASE) / 4) as usize;
                if idx < self.pending.len() {
                    self.pending[idx] &= !value;
                }
            }
            o if (GICD_IPRIORITYR_BASE..GICD_IPRIORITYR_BASE + 0x400).contains(&o) => {
                let base_irq = (o - GICD_IPRIORITYR_BASE) as usize;
                for i in 0..4 {
                    if base_irq + i < self.priority.len() {
                        self.priority[base_irq + i] = (value >> (i * 8)) as u8;
                    }
                }
            }
            o if (GICD_ITARGETSR_BASE..GICD_ITARGETSR_BASE + 0x400).contains(&o) => {
                let base_irq = (o - GICD_ITARGETSR_BASE) as usize;
                for i in 0..4 {
                    if base_irq + i < self.targets.len() {
                        self.targets[base_irq + i] = (value >> (i * 8)) as u8;
                    }
                }
            }
            o if (GICD_ICFGR_BASE..GICD_ICFGR_BASE + 0x100).contains(&o) => {
                let idx = ((o - GICD_ICFGR_BASE) / 4) as usize;
                if idx < self.config.len() {
                    self.config[idx] = value;
                }
            }
            _ => {}
        }
        self.update_irq_signal();
    }

    fn handle_cpu_read(&mut self, offset: u64) -> u32 {
        match offset {
            GICC_CTLR => self.cpu_ctrl,
            GICC_PMR => self.priority_mask,
            GICC_IAR => {
                if let Some(irq) = self.highest_pending() {
                    self.clear_pending(irq);
                    self.last_ack = Some(irq);
                    self.update_irq_signal();
                    irq
                } else {
                    1023
                }
            }
            _ => 0,
        }
    }

    fn handle_cpu_write(&mut self, offset: u64, value: u32) {
        match offset {
            GICC_CTLR => self.cpu_ctrl = value & 1,
            GICC_PMR => self.priority_mask = value & 0xFF,
            GICC_EOIR => {
                self.last_ack = None;
            }
            _ => {}
        }
        self.update_irq_signal();
    }
}

impl Device for Gic {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        let offset = txn.offset;
        if offset >= 0x10000 {
            let cpu_off = offset - 0x10000;
            if txn.is_write {
                self.handle_cpu_write(cpu_off, txn.data_u32());
            } else {
                txn.set_data_u32(self.handle_cpu_read(cpu_off));
            }
        } else if txn.is_write {
            self.handle_dist_write(offset, txn.data_u32());
        } else {
            txn.set_data_u32(self.handle_dist_read(offset));
        }
        txn.stall_cycles += 1;
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.dist_ctrl = 0;
        self.cpu_ctrl = 0;
        self.priority_mask = 0xFF;
        for e in &mut self.enabled {
            *e = 0;
        }
        for p in &mut self.pending {
            *p = 0;
        }
        for p in &mut self.priority {
            *p = 0;
        }
        self.last_ack = None;
        Ok(())
    }

    fn read_fast(&mut self, offset: Addr, _s: usize) -> HelmResult<u64> {
        if offset >= 0x10000 {
            Ok(self.handle_cpu_read(offset - 0x10000) as u64)
        } else {
            Ok(self.handle_dist_read(offset) as u64)
        }
    }

    fn write_fast(&mut self, offset: Addr, _s: usize, v: u64) -> HelmResult<()> {
        if offset >= 0x10000 {
            self.handle_cpu_write(offset - 0x10000, v as u32);
        } else {
            self.handle_dist_write(offset, v as u32);
        }
        Ok(())
    }

    fn name(&self) -> &str {
        &self.dev_name
    }

    fn tick(&mut self, _cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        let mut events = Vec::new();
        if let Some(irq) = self.highest_pending() {
            events.push(DeviceEvent::Irq {
                line: irq,
                assert: true,
            });
        }
        Ok(events)
    }
}

impl InterruptController for Gic {
    fn inject(&mut self, irq: u32, level: bool) {
        if level {
            self.set_pending(irq);
        } else {
            self.clear_pending(irq);
        }
        self.update_irq_signal();
    }

    fn pending_for_cpu(&self, _cpu_id: u32) -> bool {
        self.highest_pending().is_some()
    }

    fn ack(&mut self, _cpu_id: u32) -> Option<u32> {
        if let Some(irq) = self.highest_pending() {
            self.clear_pending(irq);
            self.last_ack = Some(irq);
            Some(irq)
        } else {
            None
        }
    }
}
