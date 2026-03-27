//! GICv3 interrupt controller — distributor + per-PE redistributors + ICC_* sysregs.
//!
//! GICv3 replaces the GICv2 MMIO CPU interface (GICC) with ICC_* system registers,
//! adds a per-PE redistributor (GICR), and extends SPI routing to use MPIDR affinity.
//!
//! # Quick start
//! ```ignore
//! let (gicd, gicrs, irq_lines, shared) = helm_hw_intc::gicv3::build_gicv3_mp(256, 4, &affinities);
//! // Map gicd at GICD_BASE, gicrs[i] at GICR_BASE + i * 0x20000
//! // irq_lines[i] is Arc<AtomicBool> — poll from vCPU step loop
//! ```

pub mod distributor;
pub mod redistributor;
pub mod sysregs;

pub use distributor::Gicv3Distributor;
pub use redistributor::Gicv3Redistributor;

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use helm_diag::sim_info;

pub(crate) const SPURIOUS_IRQ: u32 = 1023;
pub(crate) const MAX_SPI: usize = 1020;

// ── Structs ───────────────────────────────────────────────────────────────────

/// Distributor registers — manage SPIs (INTID 32..=1019).
pub struct Gicv3DistState {
    /// GICD_CTLR: EnableGrp1S[0], EnableGrp1NS[1], ARE_NS[4], ARE_S[5].
    pub ctlr: u32,
    /// GICD_TYPER: read-only, computed at build time.
    pub typer: u32,
    /// Maximum INTID count (rounded up to next multiple of 32, max 1020).
    pub num_irqs: u32,
    /// GICD_IGROUPR[n]: 1=Group1NS per SPI. Word index = (intid-32)/32.
    pub group: Vec<u32>,
    /// GICD_ISENABLER / GICD_ICENABLER.
    pub enabled: Vec<u32>,
    /// GICD_ISPENDR / GICD_ICPENDR.
    pub pending: Vec<u32>,
    /// GICD_ISACTIVER / GICD_ICACTIVER.
    pub active: Vec<u32>,
    /// GICD_IPRIORITYR: one byte per INTID (index 0 = INTID 0).
    pub priority: Vec<u8>,
    /// GICD_ICFGR: 2 bits per INTID (0=level, 2=edge).
    pub config: Vec<u32>,
    /// GICD_IROUTER: 64-bit per SPI. Index = intid - 32.
    /// bit[63]=IRM (any-PE), bits[39:32]=Aff3, [23:16]=Aff2, [15:8]=Aff1, [7:0]=Aff0.
    pub irouter: Vec<u64>,
    /// Physical asserted level — for level-sensitive re-pending after EOI.
    pub physical_level: Vec<u32>,
}

impl Gicv3DistState {
    fn new(num_irqs: u32) -> Self {
        let n = (num_irqs.min(MAX_SPI as u32) + 31) / 32;
        let it_lines = (n as u32).saturating_sub(1) & 0x1F;
        let typer = it_lines | (0b00111 << 19); // IDbits=7 → 1020 INTIDs
        Self {
            ctlr: 0x30, // ARE_NS=1, ARE_S=1 — GICv3 affinity routing default
            typer,
            num_irqs: n * 32,
            group: vec![0u32; n as usize],
            enabled: vec![0u32; n as usize],
            pending: vec![0u32; n as usize],
            active: vec![0u32; n as usize],
            priority: vec![0u8; n as usize * 32],
            config: vec![0u32; n as usize * 2],
            irouter: vec![0u64; n as usize * 32],
            physical_level: vec![0u32; n as usize],
        }
    }
}

/// Per-vCPU ICC_* system register state.
pub struct Gicv3CpuIfState {
    /// ICC_SRE_EL1: hardwired SRE=1, DFB=1, DIB=1 → reset value 0x7.
    pub icc_sre_el1: u32,
    /// ICC_PMR_EL1: priority mask (0xFF = all allowed).
    pub icc_pmr: u8,
    /// ICC_BPR1_EL1: binary point register.
    pub icc_bpr1: u8,
    /// ICC_CTLR_EL1: EOImode[1], CBPR[0]. IDbits/A3V are RO.
    pub icc_ctlr: u32,
    /// ICC_IGRPEN0_EL1.
    pub icc_igrpen0: u32,
    /// ICC_IGRPEN1_EL1: enable Group 1 Non-Secure.
    pub icc_igrpen1: u32,
    /// Running priority: priority of highest active IRQ, 0xFF if none.
    pub running_pri: u8,
    /// Active interrupt stack for deactivation (EOImode=1 path). LIFO.
    pub active_stack: Vec<(u32, u8)>,
    /// ICC_AP1R0..3_EL1: Active Priorities bitmaps.
    pub active_priorities: [u32; 4],
}

impl Default for Gicv3CpuIfState {
    fn default() -> Self {
        Self {
            icc_sre_el1: 0x7,
            icc_pmr: 0xFF,
            icc_bpr1: 0,
            icc_ctlr: 0,
            icc_igrpen0: 0,
            icc_igrpen1: 0,
            running_pri: 0xFF,
            active_stack: Vec::new(),
            active_priorities: [0u32; 4],
        }
    }
}

/// Per-vCPU redistributor + CPU interface state.
pub struct Gicv3RedistState {
    // ── GICR RD_base ─────────────────────────────────────────────────────────
    /// GICR_CTLR: EnableLPIs[0] (once set, sticky).
    pub ctlr: u32,
    /// GICR_WAKER: ProcessorSleep[1] (RW), ChildrenAsleep[2] (RO).
    pub waker: u32,
    /// GICR_PROPBASER: LPI config table PA + attrs.
    pub propbaser: u64,
    /// GICR_PENDBASER: LPI pending table PA + attrs.
    pub pendbaser: u64,
    // ── GICR SGI_base (banked per PE) ────────────────────────────────────────
    pub sgi_ppi_group:    u32,
    pub sgi_ppi_enabled:  u32,
    pub sgi_ppi_pending:  u32,
    pub sgi_ppi_active:   u32,
    pub sgi_ppi_priority: [u8; 32],
    pub sgi_ppi_config:   [u32; 2],
    // ── CPU interface ─────────────────────────────────────────────────────────
    pub cpu_if: Gicv3CpuIfState,
    // ── IRQ line to vCPU step loop ────────────────────────────────────────────
    pub irq_line: Arc<AtomicBool>,
    // ── Affinity ──────────────────────────────────────────────────────────────
    /// Packed Aff3[39:32].Aff2[23:16].Aff1[15:8].Aff0[7:0] matching MPIDR_EL1.
    pub affinity: u64,
    /// GICR_TYPER: precomputed read-only value.
    pub typer: u64,
}

impl Gicv3RedistState {
    fn new(cpu_idx: usize, affinity: u64, irq_line: Arc<AtomicBool>, is_last: bool) -> Self {
        // GICR_TYPER: affinity in [63:32], Last in [4], DirectLPI in [3], PLPIS in [0]
        let typer = (affinity << 32)
            | if is_last { 1u64 << 4 } else { 0 }
            | (1u64 << 3) // DirectLPI
            | 1u64;       // PLPIS
        let _ = cpu_idx; // used for logging if needed
        Self {
            ctlr: 0,
            waker: 0,
            propbaser: 0,
            pendbaser: 0,
            sgi_ppi_group: 0,
            sgi_ppi_enabled: 0,
            sgi_ppi_pending: 0,
            sgi_ppi_active: 0,
            sgi_ppi_priority: [0u8; 32],
            sgi_ppi_config: [0u32; 2],
            cpu_if: Gicv3CpuIfState::default(),
            irq_line,
            affinity,
            typer,
        }
    }
}

/// Combined GICv3 state: one distributor + N redistributors.
pub struct GicV3SharedState {
    pub dist: Gicv3DistState,
    /// One entry per vCPU, indexed by cpu_idx.
    pub redists: Vec<Gicv3RedistState>,
    sgi_log_budget: u32,
}

impl GicV3SharedState {
    fn new(num_irqs: u32, affinities: &[u64]) -> (Self, Vec<Arc<AtomicBool>>) {
        let num_cpus = affinities.len().max(1);
        let mut irq_lines = Vec::with_capacity(num_cpus);
        let mut redists = Vec::with_capacity(num_cpus);
        for (i, &aff) in affinities.iter().enumerate() {
            let line = Arc::new(AtomicBool::new(false));
            irq_lines.push(Arc::clone(&line));
            redists.push(Gicv3RedistState::new(i, aff, line, i + 1 == num_cpus));
        }
        let state = Self {
            dist: Gicv3DistState::new(num_irqs),
            redists,
            sgi_log_budget: 4,
        };
        (state, irq_lines)
    }

    /// True if the SPI routes to the given redistributor via GICD_IROUTER.
    fn affinity_matches(irouter: u64, redist_affinity: u64) -> bool {
        if irouter & (1u64 << 63) != 0 {
            return true; // IRM=1: any PE
        }
        // Compare Aff3[39:32], Aff2[23:16], Aff1[15:8], Aff0[7:0]
        let mask = 0x0000_00FF_00FF_FFFFu64;
        (irouter & mask) == (redist_affinity & mask)
    }

    /// Highest priority pending IRQ eligible for delivery to cpu_idx.
    /// Returns (INTID, priority) or None.
    pub fn highest_pending_for_cpu(&self, cpu_idx: usize) -> Option<(u32, u8)> {
        let redist = self.redists.get(cpu_idx)?;
        let cpu_if = &redist.cpu_if;

        // Must be globally enabled
        if self.dist.ctlr & 0x2 == 0 { // EnableGrp1NS
            return None;
        }
        // CPU interface must have Group 1 enabled
        if cpu_if.icc_igrpen1 & 1 == 0 {
            return None;
        }
        // WAKER: ProcessorSleep suppresses delivery
        if redist.waker & 0x2 != 0 {
            return None;
        }

        let pmr = cpu_if.icc_pmr;
        let running = cpu_if.running_pri;
        let mut best: Option<(u32, u8)> = None;

        // SGI/PPI — iterate only set candidate bits instead of scanning all 32 lines.
        let mut local = redist.sgi_ppi_pending & redist.sgi_ppi_enabled & !redist.sgi_ppi_active;
        while local != 0 {
            let bit = local.trailing_zeros() as usize;
            local &= local - 1;
            let prio = redist.sgi_ppi_priority[bit];
            if prio < pmr && prio < running && best.map_or(true, |(_, bp)| prio < bp) {
                best = Some((bit as u32, prio));
            }
        }

        // SPI — iterate word-by-word and visit only set candidate bits.
        let spi_count = self.dist.num_irqs.saturating_sub(32) as usize;
        let word_count = spi_count.div_ceil(32);
        for word_idx in 0..word_count {
            let pending = self.dist.pending.get(word_idx).copied().unwrap_or(0);
            let enabled = self.dist.enabled.get(word_idx).copied().unwrap_or(0);
            let active = self.dist.active.get(word_idx).copied().unwrap_or(0);
            let mut candidates = pending & enabled & !active;

            // Mask off padding bits in the final word.
            if word_idx + 1 == word_count {
                let rem = spi_count & 31;
                if rem != 0 {
                    candidates &= (1u32 << rem) - 1;
                }
            }

            while candidates != 0 {
                let bit = candidates.trailing_zeros() as usize;
                candidates &= candidates - 1;
                let spi_idx = word_idx * 32 + bit;
                let irouter = self.dist.irouter.get(spi_idx).copied().unwrap_or(0);
                if !Self::affinity_matches(irouter, redist.affinity) {
                    continue;
                }
                let intid = 32 + spi_idx;
                let prio = self.dist.priority.get(intid).copied().unwrap_or(0);
                if prio < pmr && prio < running && best.map_or(true, |(_, bp)| prio < bp) {
                    best = Some((intid as u32, prio));
                }
            }
        }
        best
    }

    /// Update the IRQ line for one PE.
    pub fn update_irq_line(&self, cpu_idx: usize) {
        if let Some(redist) = self.redists.get(cpu_idx) {
            let pending = self.highest_pending_for_cpu(cpu_idx).is_some();
            redist.irq_line.store(pending, Ordering::Relaxed);
        }
    }

    /// Update all PE IRQ lines.
    pub fn update_all_irq_lines(&self) {
        for i in 0..self.redists.len() {
            self.update_irq_line(i);
        }
    }

    /// Acknowledge the highest pending IRQ for cpu_idx. Returns INTID (1023 = spurious).
    pub fn cpu_acknowledge(&mut self, cpu_idx: usize) -> u32 {
        let Some((intid, prio)) = self.highest_pending_for_cpu(cpu_idx) else {
            return SPURIOUS_IRQ;
        };
        // Priority drop
        {
            let redist = &mut self.redists[cpu_idx];
            redist.cpu_if.running_pri = prio;
            redist.cpu_if.active_stack.push((intid, prio));
            // Set active priority bit
            let grp = (prio >> 3) as usize;
            if grp < 4 {
                redist.cpu_if.active_priorities[grp] |= 1 << (prio & 7);
            }
        }
        // Clear pending, set active
        if intid < 32 {
            let redist = &mut self.redists[cpu_idx];
            let bit = 1u32 << intid;
            redist.sgi_ppi_pending &= !bit;
            redist.sgi_ppi_active  |= bit;
        } else {
            let word = (intid as usize - 32) / 32;
            let bit = 1u32 << ((intid - 32) & 31);
            if let Some(p) = self.dist.pending.get_mut(word) { *p &= !bit; }
            if let Some(a) = self.dist.active.get_mut(word)  { *a |= bit; }
        }
        self.update_irq_line(cpu_idx);
        intid
    }

    /// End of Interrupt. EOImode=0: combined priority-drop+deactivation.
    pub fn cpu_eoi(&mut self, cpu_idx: usize, intid: u32) {
        let eoimode = {
            let redist = &self.redists[cpu_idx];
            (redist.cpu_if.icc_ctlr >> 1) & 1
        };
        // Restore running priority
        {
            let redist = &mut self.redists[cpu_idx];
            // Pop matching entry from active stack
            if let Some(pos) = redist.cpu_if.active_stack.iter().rposition(|(id, _)| *id == intid) {
                let (_, prio) = redist.cpu_if.active_stack.remove(pos);
                let grp = (prio >> 3) as usize;
                if grp < 4 {
                    redist.cpu_if.active_priorities[grp] &= !(1 << (prio & 7));
                }
            }
            redist.cpu_if.running_pri = redist.cpu_if.active_stack
                .last().map(|&(_, p)| p).unwrap_or(0xFF);
        }
        // Deactivate (EOImode=0 only)
        if eoimode == 0 {
            self.cpu_deactivate(cpu_idx, intid);
        } else {
            self.update_irq_line(cpu_idx);
        }
    }

    /// Deactivate interrupt — clears active bit (ICC_DIR_EL1 path, EOImode=1).
    pub fn cpu_deactivate(&mut self, cpu_idx: usize, intid: u32) {
        if intid < 32 {
            let redist = &mut self.redists[cpu_idx];
            redist.sgi_ppi_active &= !(1u32 << intid);
        } else {
            let word = (intid as usize - 32) / 32;
            let bit = 1u32 << ((intid - 32) & 31);
            if let Some(a) = self.dist.active.get_mut(word) { *a &= !bit; }
            // Re-pend if still physically asserted (level-sensitive)
            if let Some(lvl) = self.dist.physical_level.get(word) {
                if *lvl & bit != 0 {
                    if let Some(p) = self.dist.pending.get_mut(word) { *p |= bit; }
                }
            }
        }
        self.update_irq_line(cpu_idx);
    }

    /// Generate a Group 1 SGI from source_cpu_idx.
    pub fn generate_sgi(
        &mut self,
        source_cpu_idx: usize,
        intid: u32,
        aff3: u8, aff2: u8, aff1: u8,
        rs: u8,
        tlist: u16,
        irm: bool,
    ) {
        if intid >= 16 { return; }
        let bit = 1u32 << intid;
        let mut targets = Vec::new();

        if irm {
            // Broadcast to all except source
            for (i, redist) in self.redists.iter_mut().enumerate() {
                if i != source_cpu_idx {
                    redist.sgi_ppi_pending |= bit;
                    targets.push(i);
                }
            }
        } else {
            // Targeted: match affinity
            for (i, redist) in self.redists.iter_mut().enumerate() {
                let aff = redist.affinity;
                let raff3 = ((aff >> 32) & 0xFF) as u8;
                let raff2 = ((aff >> 16) & 0xFF) as u8;
                let raff1 = ((aff >>  8) & 0xFF) as u8;
                let raff0 = ( aff        & 0xFF) as u8;
                if raff3 != aff3 || raff2 != aff2 || raff1 != aff1 { continue; }
                let base_aff0 = rs.saturating_mul(16);
                let offset = raff0.wrapping_sub(base_aff0);
                if offset < 16 && tlist & (1u16 << offset) != 0 {
                    redist.sgi_ppi_pending |= bit;
                    targets.push(i);
                }
            }
        }

        if self.sgi_log_budget > 0 {
            self.sgi_log_budget -= 1;
            sim_info!(
                component = "gicv3-sgi",
                "cpu{} SGI{} targets={:?} irm={} aff={}.{}.{} rs={} tlist={:#06x}",
                source_cpu_idx, intid, targets, irm, aff3, aff2, aff1, rs, tlist
            );
        }
        self.update_all_irq_lines();
    }

    /// Assert a peripheral SPI (called from GicV3Sink::on_assert).
    pub fn assert_spi(&mut self, intid: u32) {
        if intid < 32 || intid as usize >= self.dist.num_irqs as usize { return; }
        let word = (intid as usize - 32) / 32;
        let bit = 1u32 << ((intid - 32) & 31);
        if let Some(lvl) = self.dist.physical_level.get_mut(word) { *lvl |= bit; }
        if let Some(p)   = self.dist.pending.get_mut(word)        { *p   |= bit; }
        self.update_all_irq_lines();
    }

    /// Deassert a peripheral SPI.
    pub fn deassert_spi(&mut self, intid: u32) {
        if intid < 32 || intid as usize >= self.dist.num_irqs as usize { return; }
        let word = (intid as usize - 32) / 32;
        let bit = 1u32 << ((intid - 32) & 31);
        if let Some(lvl) = self.dist.physical_level.get_mut(word) { *lvl &= !bit; }
        if let Some(p)   = self.dist.pending.get_mut(word)        { *p   &= !bit; }
        self.update_all_irq_lines();
    }
}

// ── Builders ──────────────────────────────────────────────────────────────────

/// Build a single-CPU GICv3 instance (affinity = 0x0).
pub fn build_gicv3(
    num_irqs: u32,
) -> (Gicv3Distributor, Gicv3Redistributor, Arc<AtomicBool>, Arc<Mutex<GicV3SharedState>>) {
    let (gicd, mut gicrs, mut lines, shared) = build_gicv3_mp(num_irqs, 1, &[0x0]);
    (gicd, gicrs.remove(0), lines.remove(0), shared)
}

/// Build an N-CPU GICv3 instance.
pub fn build_gicv3_mp(
    num_irqs: u32,
    _num_cpus: usize,
    affinities: &[u64],
) -> (Gicv3Distributor, Vec<Gicv3Redistributor>, Vec<Arc<AtomicBool>>, Arc<Mutex<GicV3SharedState>>) {
    let (state, irq_lines) = GicV3SharedState::new(num_irqs, affinities);
    let shared = Arc::new(Mutex::new(state));
    let cpu_count = irq_lines.len();
    let mut gicrs = Vec::with_capacity(cpu_count);
    for cpu_idx in 0..cpu_count {
        gicrs.push(Gicv3Redistributor::new(Arc::clone(&shared), cpu_idx));
    }
    (Gicv3Distributor::new(Arc::clone(&shared)), gicrs, irq_lines, shared)
}

// ── GicV3Sink ─────────────────────────────────────────────────────────────────

/// Routes a device `InterruptPin` assertion into a GICv3 SPI.
pub struct GicV3Sink {
    pub shared: Arc<Mutex<GicV3SharedState>>,
    pub intid: u32,
}

impl GicV3Sink {
    pub fn new(shared: Arc<Mutex<GicV3SharedState>>, intid: u32) -> Self {
        Self { shared, intid }
    }
}

impl helm_devices::InterruptSink for GicV3Sink {
    fn on_assert(&self, _wire_id: helm_devices::WireId) {
        self.shared.lock().unwrap().assert_spi(self.intid);
    }
    fn on_deassert(&self, _wire_id: helm_devices::WireId) {
        self.shared.lock().unwrap().deassert_spi(self.intid);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gicv3(num_cpus: usize) -> Arc<Mutex<GicV3SharedState>> {
        let affinities: Vec<u64> = (0..num_cpus).map(|i| i as u64).collect();
        let (_gicd, _gicrs, _lines, shared) = build_gicv3_mp(256, num_cpus, &affinities);
        shared
    }

    fn enable_gicv3(s: &mut GicV3SharedState, cpu_idx: usize) {
        s.dist.ctlr |= 0x2; // EnableGrp1NS
        s.redists[cpu_idx].cpu_if.icc_igrpen1 = 1;
        s.redists[cpu_idx].cpu_if.icc_pmr = 0xFF;
        s.redists[cpu_idx].waker = 0; // ProcessorSleep = 0
        s.redists[cpu_idx].sgi_ppi_enabled = 0xFFFF_FFFF; // enable all SGI/PPI
    }

    #[test]
    fn spurious_when_disabled() {
        let shared = make_gicv3(1);
        let mut s = shared.lock().unwrap();
        assert_eq!(s.cpu_acknowledge(0), SPURIOUS_IRQ);
    }

    #[test]
    fn spi_assert_acknowledge_eoi() {
        let shared = make_gicv3(1);
        let mut s = shared.lock().unwrap();
        enable_gicv3(&mut s, 0);
        // Route SPI 32 to cpu 0
        s.dist.irouter[0] = 0; // Aff0=0 matches cpu 0
        s.dist.enabled[0] = 1; // enable INTID 32
        s.dist.priority[32] = 0x40;
        s.assert_spi(32);
        assert!(s.highest_pending_for_cpu(0).is_some());
        let intid = s.cpu_acknowledge(0);
        assert_eq!(intid, 32);
        // After ACK: pending cleared, active set
        assert_eq!(s.dist.pending[0] & 1, 0);
        assert_eq!(s.dist.active[0] & 1, 1);
        // EOI: active cleared
        s.cpu_eoi(0, 32);
        assert_eq!(s.dist.active[0] & 1, 0);
    }

    #[test]
    fn priority_masking() {
        let shared = make_gicv3(1);
        let mut s = shared.lock().unwrap();
        enable_gicv3(&mut s, 0);
        s.redists[0].cpu_if.icc_pmr = 0x20; // mask all >= 0x20
        s.dist.irouter[0] = 0;
        s.dist.enabled[0] = 1;
        s.dist.priority[32] = 0x40; // lower priority than mask
        s.assert_spi(32);
        assert!(s.highest_pending_for_cpu(0).is_none());
        // Raise mask
        s.redists[0].cpu_if.icc_pmr = 0xFF;
        assert!(s.highest_pending_for_cpu(0).is_some());
    }

    #[test]
    fn spi_affinity_routing() {
        let shared = make_gicv3(2);
        let mut s = shared.lock().unwrap();
        enable_gicv3(&mut s, 0);
        enable_gicv3(&mut s, 1);
        s.dist.enabled[0] = 1;
        s.dist.priority[32] = 0x40;
        // Route to cpu 1 (Aff0=1)
        s.dist.irouter[0] = 1;
        s.assert_spi(32);
        // CPU 0 should not see it
        assert!(s.highest_pending_for_cpu(0).is_none());
        // CPU 1 should see it
        assert!(s.highest_pending_for_cpu(1).is_some());
    }

    #[test]
    fn spi_irm_routing() {
        let shared = make_gicv3(2);
        let mut s = shared.lock().unwrap();
        enable_gicv3(&mut s, 0);
        enable_gicv3(&mut s, 1);
        s.dist.enabled[0] = 1;
        s.dist.priority[32] = 0x40;
        // IRM=1: any PE
        s.dist.irouter[0] = 1u64 << 63;
        s.assert_spi(32);
        assert!(s.highest_pending_for_cpu(0).is_some());
        assert!(s.highest_pending_for_cpu(1).is_some());
    }

    #[test]
    fn sgi_broadcast() {
        let shared = make_gicv3(3);
        let mut s = shared.lock().unwrap();
        for i in 0..3 { enable_gicv3(&mut s, i); }
        // Broadcast SGI 5 from cpu 0 (IRM=true → all except self)
        s.generate_sgi(0, 5, 0, 0, 0, 0, 0, true);
        // CPU 0 should NOT see it (self excluded)
        assert!(s.highest_pending_for_cpu(0).is_none());
        // CPU 1 and 2 should see it
        assert_eq!(s.highest_pending_for_cpu(1).unwrap().0, 5);
        assert_eq!(s.highest_pending_for_cpu(2).unwrap().0, 5);
    }

    #[test]
    fn sgi_targeted() {
        let shared = make_gicv3(3);
        let mut s = shared.lock().unwrap();
        for i in 0..3 { enable_gicv3(&mut s, i); }
        // Target SGI 3 to cpu 2 only (Aff0=2, tlist bit 2)
        s.generate_sgi(0, 3, 0, 0, 0, 0, 0b100, false);
        assert!(s.highest_pending_for_cpu(0).is_none());
        assert!(s.highest_pending_for_cpu(1).is_none());
        assert_eq!(s.highest_pending_for_cpu(2).unwrap().0, 3);
    }

    #[test]
    fn eoimode1_split_deactivation() {
        let shared = make_gicv3(1);
        let mut s = shared.lock().unwrap();
        enable_gicv3(&mut s, 0);
        s.redists[0].cpu_if.icc_ctlr = 0x2; // EOImode=1
        s.dist.irouter[0] = 0;
        s.dist.enabled[0] = 1;
        s.dist.priority[32] = 0x40;
        s.assert_spi(32);
        let intid = s.cpu_acknowledge(0);
        assert_eq!(intid, 32);
        // EOIR: priority drop only, active NOT cleared
        s.cpu_eoi(0, 32);
        assert_eq!(s.dist.active[0] & 1, 1); // still active
        assert_eq!(s.redists[0].cpu_if.running_pri, 0xFF); // priority restored
        // DIR: deactivate
        s.cpu_deactivate(0, 32);
        assert_eq!(s.dist.active[0] & 1, 0); // now cleared
    }

    #[test]
    fn waker_suppresses_delivery() {
        let shared = make_gicv3(1);
        let mut s = shared.lock().unwrap();
        enable_gicv3(&mut s, 0);
        s.redists[0].waker = 0x2; // ProcessorSleep = 1
        s.dist.irouter[0] = 0;
        s.dist.enabled[0] = 1;
        s.dist.priority[32] = 0x40;
        s.assert_spi(32);
        assert!(s.highest_pending_for_cpu(0).is_none());
        // Wake up
        s.redists[0].waker = 0;
        assert!(s.highest_pending_for_cpu(0).is_some());
    }

    #[test]
    fn preemption_higher_priority_wins() {
        let shared = make_gicv3(1);
        let mut s = shared.lock().unwrap();
        enable_gicv3(&mut s, 0);
        s.dist.irouter[0] = 0;
        s.dist.irouter[1] = 0;
        s.dist.enabled[0] = 0x3; // INTID 32 + 33
        s.dist.priority[32] = 0x80;
        s.dist.priority[33] = 0x20; // higher priority (lower number)
        s.assert_spi(32);
        s.assert_spi(33);
        // Should pick INTID 33 (priority 0x20) first
        let first = s.cpu_acknowledge(0);
        assert_eq!(first, 33);
        // Now running_pri = 0x20; SPI 32 (prio 0x80) is NOT preemptible
        assert!(s.highest_pending_for_cpu(0).is_none());
        // Deassert SPI 33 so it doesn't re-pend after EOI (level-sensitive)
        s.deassert_spi(33);
        // EOI 33; now SPI 32 is visible
        s.cpu_eoi(0, 33);
        assert_eq!(s.highest_pending_for_cpu(0).unwrap().0, 32);
    }

    #[test]
    fn level_sensitive_repend_after_deactivate() {
        let shared = make_gicv3(1);
        let mut s = shared.lock().unwrap();
        enable_gicv3(&mut s, 0);
        s.dist.irouter[0] = 0;
        s.dist.enabled[0] = 1;
        s.dist.priority[32] = 0x40;
        s.assert_spi(32); // sets physical_level + pending
        let intid = s.cpu_acknowledge(0);
        assert_eq!(intid, 32);
        s.cpu_eoi(0, 32); // deactivate → re-pend because physical_level still set
        // Should be pending again
        assert!(s.highest_pending_for_cpu(0).is_some());
        // Deassert the physical line
        s.deassert_spi(32);
        let _ = s.cpu_acknowledge(0); // consume the re-pended interrupt
        s.cpu_eoi(0, 32);
        // Now truly gone
        assert!(s.highest_pending_for_cpu(0).is_none());
    }
}
