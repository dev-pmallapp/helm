//! GICv3 composite device — distributor + redistributors + ICC.
//!
//! `GicV3` implements the [`Device`] and [`InterruptController`] traits
//! and presents a single MMIO region containing the distributor,
//! redistributors, and (optionally) an ITS.
//!
//! ## MMIO layout (default, QEMU-virt-compatible)
//!
//! | Offset | Size | Component |
//! |--------|------|-----------|
//! | `0x0_0000` | 64 KB | Distributor (GICD) |
//! | `0x8_0000` | 128 KB | ITS (optional) |
//! | `0xA_0000` | N × 128 KB | Redistributors |

use crate::device::{Device, DeviceEvent};
use crate::irq::InterruptController;
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::{HelmResult, IrqSignal};

use super::common::*;
use super::distributor::GicDistributor;
use super::icc::{IccReg, IccState};
use super::its::GicIts;
use super::lpi::{LpiConfigTable, LpiPendingTable};
use super::redistributor::{GicRedistributor, GICR_RD_SIZE, GICR_STRIDE};
use super::GicVersion;

/// Default offset of redistributors within the GicV3 MMIO region.
const DEFAULT_REDIST_OFFSET: u64 = 0xA_0000;
/// Default offset of the ITS within the GicV3 MMIO region.
const DEFAULT_ITS_OFFSET: u64 = 0x8_0000;
/// Number of LPIs to support by default.
const DEFAULT_NUM_LPIS: u32 = 8192;

/// GICv3 device model.
pub struct GicV3 {
    dev_name: String,
    region: MemRegion,
    /// Distributor state.
    pub distributor: GicDistributor,
    /// Per-PE redistributors.
    pub redistributors: Vec<GicRedistributor>,
    /// Per-PE ICC (CPU-interface) state.
    pub icc_states: Vec<IccState>,
    /// Optional Interrupt Translation Service.
    pub its: Option<GicIts>,
    /// Number of processing elements.
    pub num_pes: u32,
    /// Byte offset of redistributors within the MMIO region.
    redist_offset: u64,
    /// Byte offset of the ITS within the MMIO region.
    its_offset: u64,
    /// Per-PE IRQ signal (raised when any IRQ is pending).
    irq_signals: Vec<Option<IrqSignal>>,
    /// LPI pending table (software cache).
    pub lpi_pending: LpiPendingTable,
    /// LPI configuration table (software cache).
    pub lpi_config: LpiConfigTable,
}

impl GicV3 {
    /// Create a GICv3 with `num_irqs` SPIs and `num_pes` processing
    /// elements.
    pub fn new(name: impl Into<String>, num_irqs: u32, num_pes: u32) -> Self {
        let num_pes = num_pes.max(1);
        let n = name.into();
        let redist_end = DEFAULT_REDIST_OFFSET + (num_pes as u64) * GICR_STRIDE;
        let its_end = DEFAULT_ITS_OFFSET + 0x2_0000;
        let total_size = redist_end.max(its_end);

        let mut redists = Vec::with_capacity(num_pes as usize);
        for pe in 0..num_pes {
            redists.push(GicRedistributor::new(pe, pe == num_pes - 1));
        }

        Self {
            region: MemRegion {
                name: n.clone(),
                base: 0,
                size: total_size,
                kind: crate::region::RegionKind::Io,
                priority: 0,
            },
            dev_name: n,
            distributor: GicDistributor::new(GicVersion::V3, num_irqs),
            redistributors: redists,
            icc_states: (0..num_pes).map(|_| IccState::new()).collect(),
            its: Some(GicIts::new()),
            num_pes,
            redist_offset: DEFAULT_REDIST_OFFSET,
            its_offset: DEFAULT_ITS_OFFSET,
            irq_signals: (0..num_pes).map(|_| None).collect(),
            lpi_pending: LpiPendingTable::new(DEFAULT_NUM_LPIS),
            lpi_config: LpiConfigTable::new(DEFAULT_NUM_LPIS),
        }
    }

    /// Attach per-PE IRQ signals.
    pub fn set_irq_signals(&mut self, signals: Vec<IrqSignal>) {
        for (idx, sig) in signals.into_iter().enumerate() {
            if idx < self.irq_signals.len() {
                self.irq_signals[idx] = Some(sig);
            }
        }
    }

    /// Attach a single IRQ signal for PE 0.
    pub fn set_irq_signal(&mut self, signal: IrqSignal) {
        if !self.irq_signals.is_empty() {
            self.irq_signals[0] = Some(signal);
        }
    }

    /// Find the highest-priority pending interrupt for a PE, considering
    /// both its redistributor (SGI/PPI) and the distributor (SPI).
    pub fn highest_pending_for_pe(&self, pe: u32, priority_mask: u8) -> Option<u32> {
        let pe_idx = pe as usize;
        if pe_idx >= self.redistributors.len() {
            return None;
        }
        let redist = &self.redistributors[pe_idx];

        let mut best: Option<(u32, u8)> = None;

        if let Some(irq) = redist.highest_pending_sgi_ppi(priority_mask) {
            let prio = redist
                .sgi_ppi_priority
                .get(irq as usize)
                .copied()
                .unwrap_or(0xFF);
            best = Some((irq, prio));
        }

        for irq in SPI_START..self.distributor.num_irqs {
            if !self.distributor.is_irq_pending(irq) || !self.distributor.is_irq_enabled(irq) {
                continue;
            }
            let prio = self
                .distributor
                .priority
                .get(irq as usize)
                .copied()
                .unwrap_or(0xFF);
            if prio >= priority_mask {
                continue;
            }
            if self.distributor.are_enabled() && self.distributor.spi_target_pe(irq) != pe {
                continue;
            }
            if best.is_none_or(|(_, bp)| prio < bp) {
                best = Some((irq, prio));
            }
        }

        best.map(|(irq, _)| irq)
    }

    /// Raise or lower per-PE IRQ signals based on current pending state.
    fn update_irq_signals(&self) {
        let dist_enabled = self.distributor.is_enabled();
        for pe in 0..self.num_pes {
            let pe_idx = pe as usize;
            if let Some(sig) = self.irq_signals.get(pe_idx).and_then(|s| s.as_ref()) {
                let icc = &self.icc_states[pe_idx];
                if dist_enabled && icc.group1_enabled() {
                    if self.highest_pending_for_pe(pe, icc.pmr).is_some() {
                        sig.raise();
                    } else {
                        sig.lower();
                    }
                } else {
                    sig.lower();
                }
            }
        }
    }

    /// Handle an ICC system register read for a specific PE.
    pub fn sysreg_read(&mut self, pe: u32, reg: IccReg) -> u64 {
        let pe_idx = pe as usize;
        if pe_idx >= self.icc_states.len() {
            return 0;
        }
        match reg {
            IccReg::Iar1 | IccReg::Iar0 => {
                let pmr = self.icc_states[pe_idx].pmr;
                if let Some(irq) = self.highest_pending_for_pe(pe, pmr) {
                    let prio = if irq < 32 {
                        self.redistributors[pe_idx]
                            .sgi_ppi_priority
                            .get(irq as usize)
                            .copied()
                            .unwrap_or(0)
                    } else {
                        self.distributor
                            .priority
                            .get(irq as usize)
                            .copied()
                            .unwrap_or(0)
                    };
                    if irq < 32 {
                        self.redistributors[pe_idx].clear_pending(irq);
                        self.redistributors[pe_idx].set_active(irq);
                    } else {
                        self.distributor.clear_pending(irq);
                        self.distributor.set_active(irq);
                    }
                    self.icc_states[pe_idx].priority_drop(prio);
                    self.update_irq_signals();
                    irq as u64
                } else {
                    SPURIOUS_IRQ as u64
                }
            }
            other => self.icc_states[pe_idx].read_simple(other),
        }
    }

    /// Handle an ICC system register write for a specific PE.
    pub fn sysreg_write(&mut self, pe: u32, reg: IccReg, val: u64) {
        let pe_idx = pe as usize;
        if pe_idx >= self.icc_states.len() {
            return;
        }
        match reg {
            IccReg::Eoir1 | IccReg::Eoir0 => {
                let irq = val as u32;
                if irq < 32 {
                    self.redistributors[pe_idx].clear_active(irq);
                } else if irq < self.distributor.num_irqs {
                    self.distributor.clear_active(irq);
                }
                self.icc_states[pe_idx].deactivate();
                self.update_irq_signals();
            }
            IccReg::Sgi1r => {
                self.handle_sgi_write(pe, val);
            }
            other => {
                self.icc_states[pe_idx].write_simple(other, val);
                self.update_irq_signals();
            }
        }
    }

    /// Process an `ICC_SGI1R_EL1` write — generate an SGI to target PEs.
    fn handle_sgi_write(&mut self, source_pe: u32, val: u64) {
        let intid = ((val >> 24) & 0xF) as u32;
        let irm = (val >> 40) & 1;
        let target_list = (val & 0xFFFF) as u16;
        let aff1 = ((val >> 16) & 0xFF) as u32;
        let aff2 = ((val >> 32) & 0xFF) as u32;
        let aff3 = ((val >> 48) & 0xFF) as u32;

        for pe in 0..self.num_pes {
            if irm == 1 {
                if pe == source_pe {
                    continue;
                }
            } else {
                let rd = &self.redistributors[pe as usize];
                let rd_aff1 = ((rd.affinity >> 8) & 0xFF) as u32;
                let rd_aff2 = ((rd.affinity >> 16) & 0xFF) as u32;
                let rd_aff3 = ((rd.affinity >> 24) & 0xFF) as u32;
                if rd_aff1 != aff1 || rd_aff2 != aff2 || rd_aff3 != aff3 {
                    continue;
                }
                let rd_aff0 = (rd.affinity & 0xFF) as u32;
                if rd_aff0 >= 16 || target_list & (1 << rd_aff0) == 0 {
                    continue;
                }
            }
            self.redistributors[pe as usize].set_pending(intid);
        }
        self.update_irq_signals();
    }
}

impl Device for GicV3 {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        let offset = txn.offset;
        let redist_end = self.redist_offset + (self.num_pes as u64) * GICR_STRIDE;

        if offset < 0x10000 {
            if txn.is_write {
                self.distributor.write(offset, txn.data_u32());
            } else {
                txn.set_data_u32(self.distributor.read(offset));
            }
        } else if offset >= self.redist_offset && offset < redist_end {
            let rel = offset - self.redist_offset;
            let pe = (rel / GICR_STRIDE) as usize;
            let frame_off = rel % GICR_STRIDE;
            if pe < self.redistributors.len() {
                if frame_off < GICR_RD_SIZE {
                    if txn.is_write {
                        self.redistributors[pe].write_rd(frame_off, txn.data_u32());
                    } else {
                        txn.set_data_u32(self.redistributors[pe].read_rd(frame_off));
                    }
                } else {
                    let sgi_off = frame_off - GICR_RD_SIZE;
                    if txn.is_write {
                        self.redistributors[pe].write_sgi(sgi_off, txn.data_u32());
                    } else {
                        txn.set_data_u32(self.redistributors[pe].read_sgi(sgi_off));
                    }
                }
            }
        } else if let Some(ref mut its) = self.its {
            if offset >= self.its_offset && offset < self.its_offset + 0x2_0000 {
                let its_off = offset - self.its_offset;
                if txn.is_write {
                    its.write(its_off, txn.data_u32());
                } else {
                    txn.set_data_u32(its.read(its_off));
                }
            }
        }

        txn.stall_cycles += 1;
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.distributor.reset();
        for rd in &mut self.redistributors {
            rd.reset();
        }
        for icc in &mut self.icc_states {
            icc.reset();
        }
        if let Some(ref mut its) = self.its {
            its.reset();
        }
        self.lpi_pending.clear_all();
        Ok(())
    }

    fn read_fast(&mut self, offset: Addr, size: usize) -> HelmResult<u64> {
        let mut txn = Transaction::read(0, size);
        txn.offset = offset;
        self.transact(&mut txn)?;
        Ok(txn.data_u32() as u64)
    }

    fn write_fast(&mut self, offset: Addr, size: usize, val: u64) -> HelmResult<()> {
        let mut txn = Transaction::write(0, size, val);
        txn.offset = offset;
        self.transact(&mut txn)?;
        Ok(())
    }

    fn name(&self) -> &str {
        &self.dev_name
    }

    fn tick(&mut self, _cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        if let Some(ref mut its) = self.its {
            for (target_pe, intid) in its.drain_pending() {
                if intid >= LPI_START {
                    self.lpi_pending.set_pending(intid);
                } else if intid < 32 {
                    if let Some(rd) = self.redistributors.get_mut(target_pe as usize) {
                        rd.set_pending(intid);
                    }
                } else {
                    self.distributor.set_pending(intid);
                }
            }
        }

        let mut events = Vec::new();
        for pe in 0..self.num_pes {
            let pmr = self.icc_states[pe as usize].pmr;
            if self.highest_pending_for_pe(pe, pmr).is_some() {
                events.push(DeviceEvent::Irq {
                    line: pe,
                    assert: true,
                });
            }
        }
        self.update_irq_signals();
        Ok(events)
    }
}

impl InterruptController for GicV3 {
    fn inject(&mut self, irq: u32, level: bool) {
        if irq < 32 {
            if level {
                self.redistributors[0].set_pending(irq);
            } else {
                self.redistributors[0].clear_pending(irq);
            }
        } else if irq >= LPI_START {
            if level {
                self.lpi_pending.set_pending(irq);
            }
        } else if level {
            self.distributor.set_pending(irq);
        } else {
            self.distributor.clear_pending(irq);
        }
        self.update_irq_signals();
    }

    fn pending_for_cpu(&self, cpu_id: u32) -> bool {
        let pe = cpu_id as usize;
        if pe >= self.icc_states.len() {
            return false;
        }
        let pmr = self.icc_states[pe].pmr;
        self.highest_pending_for_pe(cpu_id, pmr).is_some()
    }

    fn ack(&mut self, cpu_id: u32) -> Option<u32> {
        let pe = cpu_id as usize;
        if pe >= self.icc_states.len() {
            return None;
        }
        let pmr = self.icc_states[pe].pmr;
        if let Some(irq) = self.highest_pending_for_pe(cpu_id, pmr) {
            if irq < 32 {
                self.redistributors[pe].clear_pending(irq);
            } else {
                self.distributor.clear_pending(irq);
            }
            self.update_irq_signals();
            Some(irq)
        } else {
            None
        }
    }
}
