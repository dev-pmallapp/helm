//! `helm-timing` — timing model trait + VirtualTiming / IntervalTiming / AccurateTiming implementations.
//!
//! `HelmEngine<T: TimingModel>` is monomorphized over the timing model.
//! Each model is compiled as a separate specialization — no vtable, no overhead.
//!
//! # Models
//! - [`VirtualTiming`]  — event-driven ideal IPC (Phase 0/1)
//! - [`IntervalTiming`] — analytical interval timing (Phase 3 foundation)
//! - [`AccurateTiming`] — cycle-accurate in-order/OoO pipeline (Phase 3)

#![allow(missing_docs)]

use helm_event::{EventQueue, Tick};

pub const TIMING_MAX_SRC_REGS: usize = 4;
pub const TIMING_MAX_DST_REGS: usize = 2;
pub const TIMING_REG_SLOTS: usize = 64;
pub const TIMING_AARCH64_SP_REG: u8 = 63;
/// FP and vector registers share slots 32–63 (same physical reg file on AArch64).
pub const TIMING_FP_REG_BASE: u8 = 32;
pub const TIMING_VEC_REG_BASE: u8 = 32;
const INTERVAL_MAX_PENDING_ACCESSES: usize = 4;
const INTERVAL_LOAD_MLP_SLOTS: usize = 2;
const INTERVAL_STORE_BUFFER_SLOTS: usize = 2;

// ── TimingInsnClass ────────────────────────────────────────────────────────────────

/// Coarse instruction class used by timing models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TimingInsnClass {
    IntAlu = 0,
    IntMul = 1,
    Branch = 2,
    Load = 3,
    Store = 4,
    FpAlu = 5,
    SimdAlu = 6,
    System = 7,
    Nop = 8,
    Atomic = 9,
    Unknown = 10,
}

impl TimingInsnClass {
    /// Legacy fallback for older callers that only populate boolean flags.
    #[inline(always)]
    pub fn from_legacy_flags(is_branch: bool, is_load: bool, is_store: bool, is_fp: bool) -> Self {
        if is_branch {
            Self::Branch
        } else if is_load {
            Self::Load
        } else if is_store {
            Self::Store
        } else if is_fp {
            Self::FpAlu
        } else {
            Self::IntAlu
        }
    }
}

// ── TimingInsnInfo ──────────────────────────────────────────────────────────────────

/// Per-instruction metadata passed to the timing model's hot path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingInsnInfo {
    pub pc: u64,
    pub class: TimingInsnClass,
    pub is_branch: bool,
    pub is_load: bool,
    pub is_store: bool,
    pub is_fp: bool,
    pub src_regs: [u8; TIMING_MAX_SRC_REGS],
    pub src_reg_count: u8,
    pub dst_regs: [u8; TIMING_MAX_DST_REGS],
    pub dst_reg_count: u8,
}

impl TimingInsnInfo {
    #[inline(always)]
    pub fn new_basic(
        pc: u64,
        class: TimingInsnClass,
        is_branch: bool,
        is_load: bool,
        is_store: bool,
        is_fp: bool,
    ) -> Self {
        Self {
            pc,
            class,
            is_branch,
            is_load,
            is_store,
            is_fp,
            src_regs: [0; TIMING_MAX_SRC_REGS],
            src_reg_count: 0,
            dst_regs: [0; TIMING_MAX_DST_REGS],
            dst_reg_count: 0,
        }
    }

    #[inline(always)]
    pub fn src_regs(&self) -> &[u8] {
        &self.src_regs[..usize::from(self.src_reg_count).min(TIMING_MAX_SRC_REGS)]
    }

    #[inline(always)]
    pub fn dst_regs(&self) -> &[u8] {
        &self.dst_regs[..usize::from(self.dst_reg_count).min(TIMING_MAX_DST_REGS)]
    }
}

// ── MemAccess ─────────────────────────────────────────────────────────────────

/// Information about a completed memory access.
pub struct MemAccess {
    pub addr: u64,
    pub size: usize,
    pub is_store: bool,
    pub hit_l1: bool,
    pub hit_l2: bool,
}

// ── TimingModel ───────────────────────────────────────────────────────────────

/// Timing model interface — the `T` in `HelmEngine<T>`.
///
/// Called from the inner loop after every instruction (and every memory access
/// for models that track latency). Must be `Send` so the engine can be moved
/// across threads between quanta.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimingModelCaps {
    pub idealized_fast_run: bool,
    pub needs_operand_timing: bool,
    pub needs_mem_access_timing: bool,
}

pub trait TimingModel: Send + 'static {
    fn model_caps() -> TimingModelCaps
    where
        Self: Sized,
    {
        TimingModelCaps::default()
    }

    /// Advance time by the cost of one instruction. Returns cycles consumed.
    fn on_insn(&mut self, info: &TimingInsnInfo) -> u64;

    /// Notify the model of a completed memory access (L1/L2 outcome).
    fn on_mem_access(&mut self, access: &MemAccess);

    /// Notify the model of a branch outcome (taken, predicted correctly?).
    fn on_branch(&mut self, taken: bool, predicted: bool);

    /// Current simulated cycle count.
    fn current_cycles(&self) -> Tick;

    /// Fast-forward simulated time to `tick` without retiring instructions.
    ///
    /// Used when the engine can prove the guest is idle (for example, WFI)
    /// and needs device/event time to continue progressing.
    fn advance_to(&mut self, tick: Tick);

    /// Called at every interval boundary (IntervalTiming model) or every instruction
    /// (VirtualTiming/AccurateTiming). May post events into `eq`.
    fn on_boundary(&mut self, eq: &mut EventQueue);
}

// ── VirtualTiming ───────────────────────────────────────────────────────────────────

/// Ideal-IPC timing: every instruction costs exactly `1 / ipc` cycles.
///
/// Used in Phase 0 (no timing) and as the fastest Phase 1 mode.
/// The event queue is advanced every quantum.
pub struct VirtualTiming {
    cycles_per_insn: f64,
    exact_cycles: f64,
    current_cycles: Tick,
}

#[inline(always)]
fn sanitize_ipc(ipc: f64) -> f64 {
    if ipc.is_finite() && ipc > 0.0 {
        ipc
    } else {
        1.0
    }
}

impl VirtualTiming {
    /// `ipc` = instructions per cycle (e.g. 1.0, 2.0, 0.5).
    pub fn new(ipc: f64) -> Self {
        Self {
            cycles_per_insn: 1.0 / sanitize_ipc(ipc),
            exact_cycles: 0.0,
            current_cycles: 0,
        }
    }
}

impl Default for VirtualTiming {
    fn default() -> Self {
        Self::new(1.0)
    }
}

impl TimingModel for VirtualTiming {
    fn model_caps() -> TimingModelCaps {
        TimingModelCaps {
            idealized_fast_run: true,
            needs_operand_timing: false,
            needs_mem_access_timing: false,
        }
    }

    #[inline(always)]
    fn on_insn(&mut self, _info: &TimingInsnInfo) -> u64 {
        let before = self.current_cycles;
        self.exact_cycles += self.cycles_per_insn;
        self.current_cycles = self.exact_cycles.floor() as Tick;
        self.current_cycles.saturating_sub(before)
    }

    #[inline(always)]
    fn on_mem_access(&mut self, _access: &MemAccess) {}

    #[inline(always)]
    fn on_branch(&mut self, _taken: bool, _predicted: bool) {}

    fn current_cycles(&self) -> Tick {
        self.current_cycles
    }

    fn advance_to(&mut self, tick: Tick) {
        if tick > self.current_cycles {
            self.exact_cycles = tick as f64;
            self.current_cycles = tick;
        }
    }

    fn on_boundary(&mut self, _eq: &mut EventQueue) {}
}

// ── IntervalTiming ──────────────────────────────────────────────────────────────────

/// Sniper-style interval simulation.
///
/// Tracks fixed-size instruction windows and accumulates interval-local
/// dependency-critical execution time, branch misprediction penalties, and
/// memory stall cycles.
pub struct IntervalTiming {
    ipc: f64,
    interval_len: u64,
    committed_cycles: Tick,
    open_interval: OpenInterval,
    latencies: IntervalClassLatencies,
    branch_mispredict_penalty: Tick,
    l2_hit_penalty: Tick,
    dram_penalty: Tick,
}

#[derive(Debug, Clone, Copy)]
struct OpenInterval {
    insns: u64,
    completion_tail: Tick,
    branch_penalty_cycles: Tick,
    pending_load_stalls: [Tick; INTERVAL_MAX_PENDING_ACCESSES],
    pending_load_count: u8,
    pending_store_stalls: [Tick; INTERVAL_MAX_PENDING_ACCESSES],
    pending_store_count: u8,
    load_slots_ready: [Tick; INTERVAL_LOAD_MLP_SLOTS],
    store_slots_ready: [Tick; INTERVAL_STORE_BUFFER_SLOTS],
    store_drain_tail: Tick,
    reg_ready: [Tick; TIMING_REG_SLOTS],
}

impl Default for OpenInterval {
    fn default() -> Self {
        Self {
            insns: 0,
            completion_tail: 0,
            branch_penalty_cycles: 0,
            pending_load_stalls: [0; INTERVAL_MAX_PENDING_ACCESSES],
            pending_load_count: 0,
            pending_store_stalls: [0; INTERVAL_MAX_PENDING_ACCESSES],
            pending_store_count: 0,
            load_slots_ready: [0; INTERVAL_LOAD_MLP_SLOTS],
            store_slots_ready: [0; INTERVAL_STORE_BUFFER_SLOTS],
            store_drain_tail: 0,
            reg_ready: [0; TIMING_REG_SLOTS],
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct IntervalClassLatencies {
    int_alu: Tick,
    int_mul: Tick,
    branch: Tick,
    load: Tick,
    store: Tick,
    fp_alu: Tick,
    simd_alu: Tick,
    system: Tick,
    nop: Tick,
    atomic: Tick,
    unknown: Tick,
}

impl Default for IntervalClassLatencies {
    fn default() -> Self {
        Self {
            int_alu: 1,
            int_mul: 3,
            branch: 1,
            load: 4,
            store: 1,
            fp_alu: 4,
            simd_alu: 3,
            system: 2,
            nop: 1,
            atomic: 8,
            unknown: 1,
        }
    }
}

impl IntervalTiming {
    pub fn new(ipc: f64, interval_len: u64) -> Self {
        Self {
            ipc: sanitize_ipc(ipc),
            interval_len: interval_len.max(1),
            committed_cycles: 0,
            open_interval: OpenInterval::default(),
            latencies: IntervalClassLatencies::default(),
            branch_mispredict_penalty: 5,
            l2_hit_penalty: 4,
            dram_penalty: 12,
        }
    }

    #[inline(always)]
    fn effective_class(info: &TimingInsnInfo) -> TimingInsnClass {
        if info.class == TimingInsnClass::Unknown
            && (info.is_branch || info.is_load || info.is_store || info.is_fp)
        {
            TimingInsnClass::from_legacy_flags(
                info.is_branch,
                info.is_load,
                info.is_store,
                info.is_fp,
            )
        } else {
            info.class
        }
    }

    #[inline(always)]
    fn class_latency(&self, class: TimingInsnClass) -> Tick {
        match class {
            TimingInsnClass::IntAlu => self.latencies.int_alu,
            TimingInsnClass::IntMul => self.latencies.int_mul,
            TimingInsnClass::Branch => self.latencies.branch,
            TimingInsnClass::Load => self.latencies.load,
            TimingInsnClass::Store => self.latencies.store,
            TimingInsnClass::FpAlu => self.latencies.fp_alu,
            TimingInsnClass::SimdAlu => self.latencies.simd_alu,
            TimingInsnClass::System => self.latencies.system,
            TimingInsnClass::Nop => self.latencies.nop,
            TimingInsnClass::Atomic => self.latencies.atomic,
            TimingInsnClass::Unknown => self.latencies.unknown,
        }
    }

    #[inline(always)]
    fn estimate_open_interval_cycles(&self) -> Tick {
        if self.open_interval.insns == 0 {
            0
        } else {
            let core_tail = self.open_interval.completion_tail + self.pending_load_stall_estimate();
            (core_tail + self.open_interval.branch_penalty_cycles)
                .max(self.open_interval.store_drain_tail + self.pending_store_stall_estimate())
        }
    }

    #[inline(always)]
    fn pending_load_stall_estimate(&self) -> Tick {
        self.open_interval.pending_load_stalls[..usize::from(self.open_interval.pending_load_count)]
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
    }

    #[inline(always)]
    fn pending_store_stall_estimate(&self) -> Tick {
        self.open_interval.pending_store_stalls
            [..usize::from(self.open_interval.pending_store_count)]
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
    }

    #[inline(always)]
    fn next_issue_cycle(&self) -> Tick {
        ((self.open_interval.insns as f64) / self.ipc).floor() as Tick
    }

    #[inline(always)]
    fn reg_slot(reg: u8) -> Option<usize> {
        let slot = usize::from(reg);
        (slot < TIMING_REG_SLOTS).then_some(slot)
    }

    #[inline(always)]
    fn src_ready_cycle(&self, info: &TimingInsnInfo) -> Tick {
        debug_assert!(
            info.src_regs()
                .iter()
                .all(|&reg| Self::reg_slot(reg).is_some()),
            "timing source register index out of range"
        );
        info.src_regs()
            .iter()
            .filter_map(|&reg| Self::reg_slot(reg).map(|slot| self.open_interval.reg_ready[slot]))
            .max()
            .unwrap_or(0)
    }

    #[inline(always)]
    fn commit_closed_interval(&mut self) {
        if self.open_interval.insns < self.interval_len {
            return;
        }

        self.committed_cycles += self.estimate_open_interval_cycles();
        self.reset_open_interval();
    }

    #[inline(always)]
    fn reset_open_interval(&mut self) {
        self.open_interval = OpenInterval::default();
    }

    #[inline(always)]
    fn consume_pending_loads(&mut self, issue_at: Tick) -> Tick {
        let mut max_complete_at = 0;
        let count = usize::from(self.open_interval.pending_load_count);
        for idx in 0..count {
            let stall = self.open_interval.pending_load_stalls[idx];
            let slot_idx = self
                .open_interval
                .load_slots_ready
                .iter()
                .enumerate()
                .min_by_key(|(_, ready)| *ready)
                .map(|(slot_idx, _)| slot_idx)
                .unwrap_or(0);
            let slot_ready = self.open_interval.load_slots_ready[slot_idx];
            let mem_complete_at = issue_at.max(slot_ready) + stall;
            self.open_interval.load_slots_ready[slot_idx] = mem_complete_at;
            max_complete_at = max_complete_at.max(mem_complete_at);
        }
        self.open_interval.pending_load_count = 0;
        self.open_interval.pending_load_stalls.fill(0);
        max_complete_at
    }

    #[inline(always)]
    fn consume_pending_stores(&mut self, issue_at: Tick) {
        let mut max_complete_at = self.open_interval.store_drain_tail;
        let count = usize::from(self.open_interval.pending_store_count);
        for idx in 0..count {
            let stall = self.open_interval.pending_store_stalls[idx];
            let slot_idx = self
                .open_interval
                .store_slots_ready
                .iter()
                .enumerate()
                .min_by_key(|(_, ready)| *ready)
                .map(|(slot_idx, _)| slot_idx)
                .unwrap_or(0);
            let slot_ready = self.open_interval.store_slots_ready[slot_idx];
            let store_complete_at = issue_at.max(slot_ready) + stall;
            self.open_interval.store_slots_ready[slot_idx] = store_complete_at;
            max_complete_at = max_complete_at.max(store_complete_at);
        }
        self.open_interval.store_drain_tail = max_complete_at;
        self.open_interval.pending_store_count = 0;
        self.open_interval.pending_store_stalls.fill(0);
    }
}

impl Default for IntervalTiming {
    fn default() -> Self {
        Self::new(2.0, 10_000)
    }
}

impl TimingModel for IntervalTiming {
    fn model_caps() -> TimingModelCaps {
        TimingModelCaps {
            idealized_fast_run: false,
            needs_operand_timing: true,
            needs_mem_access_timing: true,
        }
    }

    fn on_insn(&mut self, info: &TimingInsnInfo) -> u64 {
        let before = self.current_cycles();
        self.commit_closed_interval();
        let latency = self.class_latency(Self::effective_class(info));
        let issue_at = self.next_issue_cycle().max(self.src_ready_cycle(info));
        let mem_complete_at = self.consume_pending_loads(issue_at);
        if info.is_store {
            self.consume_pending_stores(issue_at);
        }
        let complete_at = (issue_at + latency).max(mem_complete_at);
        debug_assert!(
            info.dst_regs()
                .iter()
                .all(|&reg| Self::reg_slot(reg).is_some()),
            "timing destination register index out of range"
        );
        for &dst_reg in info.dst_regs() {
            if let Some(slot) = Self::reg_slot(dst_reg) {
                self.open_interval.reg_ready[slot] = complete_at;
            }
        }
        self.open_interval.completion_tail = self.open_interval.completion_tail.max(complete_at);
        self.open_interval.insns += 1;
        self.current_cycles().saturating_sub(before)
    }
    fn on_mem_access(&mut self, access: &MemAccess) {
        let stall = if access.hit_l1 {
            0
        } else if access.hit_l2 {
            self.l2_hit_penalty
        } else {
            self.dram_penalty
        };
        if stall == 0 {
            return;
        }

        let (stalls, count) = if access.is_store {
            (
                &mut self.open_interval.pending_store_stalls,
                &mut self.open_interval.pending_store_count,
            )
        } else {
            (
                &mut self.open_interval.pending_load_stalls,
                &mut self.open_interval.pending_load_count,
            )
        };

        let idx = usize::from(*count).min(INTERVAL_MAX_PENDING_ACCESSES - 1);
        stalls[idx] += stall;
        if usize::from(*count) < INTERVAL_MAX_PENDING_ACCESSES {
            *count += 1;
        }
    }
    fn on_branch(&mut self, taken: bool, predicted: bool) {
        if taken != predicted {
            self.open_interval.branch_penalty_cycles += self.branch_mispredict_penalty;
        }
    }
    fn current_cycles(&self) -> Tick {
        self.committed_cycles + self.estimate_open_interval_cycles()
    }
    fn advance_to(&mut self, tick: Tick) {
        let current = self.current_cycles();
        if tick > current {
            self.committed_cycles = tick;
            self.reset_open_interval();
        }
    }
    fn on_boundary(&mut self, eq: &mut EventQueue) {
        let _ = eq;
        self.commit_closed_interval();
    }
}

// ── AccurateTiming ──────────────────────────────────────────────────────────────────

/// Cycle-accurate in-order pipeline model (Phase 3 placeholder).
pub struct AccurateTiming {
    inner: VirtualTiming,
}

impl Default for AccurateTiming {
    fn default() -> Self {
        Self {
            inner: VirtualTiming::new(1.0),
        }
    }
}

impl TimingModel for AccurateTiming {
    fn model_caps() -> TimingModelCaps {
        TimingModelCaps {
            idealized_fast_run: false,
            needs_operand_timing: false,
            needs_mem_access_timing: false,
        }
    }

    fn on_insn(&mut self, info: &TimingInsnInfo) -> u64 {
        self.inner.on_insn(info)
    }
    fn on_mem_access(&mut self, access: &MemAccess) {
        self.inner.on_mem_access(access);
    }
    fn on_branch(&mut self, taken: bool, predicted: bool) {
        self.inner.on_branch(taken, predicted);
    }
    fn current_cycles(&self) -> Tick {
        self.inner.current_cycles()
    }
    fn advance_to(&mut self, tick: Tick) {
        self.inner.advance_to(tick);
    }
    fn on_boundary(&mut self, eq: &mut EventQueue) {
        self.inner.on_boundary(eq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(class: TimingInsnClass) -> TimingInsnInfo {
        TimingInsnInfo {
            pc: 0x1000,
            class,
            is_branch: matches!(class, TimingInsnClass::Branch),
            is_load: matches!(class, TimingInsnClass::Load),
            is_store: matches!(class, TimingInsnClass::Store),
            is_fp: matches!(class, TimingInsnClass::FpAlu | TimingInsnClass::SimdAlu),
            src_regs: [0; TIMING_MAX_SRC_REGS],
            src_reg_count: 0,
            dst_regs: [0; TIMING_MAX_DST_REGS],
            dst_reg_count: 0,
        }
    }

    fn info_with_regs(class: TimingInsnClass, src_regs: &[u8], dst_regs: &[u8]) -> TimingInsnInfo {
        let mut info = info(class);
        for (idx, reg) in src_regs
            .iter()
            .copied()
            .take(TIMING_MAX_SRC_REGS)
            .enumerate()
        {
            info.src_regs[idx] = reg;
            info.src_reg_count += 1;
        }
        for (idx, reg) in dst_regs
            .iter()
            .copied()
            .take(TIMING_MAX_DST_REGS)
            .enumerate()
        {
            info.dst_regs[idx] = reg;
            info.dst_reg_count += 1;
        }
        info
    }

    #[test]
    fn virtual_fractional_ipc_accumulates_cycles_across_instructions() {
        let mut timing = VirtualTiming::new(2.0);
        assert_eq!(timing.current_cycles(), 0);
        assert_eq!(timing.on_insn(&info(TimingInsnClass::IntAlu)), 0);
        assert_eq!(timing.current_cycles(), 0);
        assert_eq!(timing.on_insn(&info(TimingInsnClass::IntAlu)), 1);
        assert_eq!(timing.current_cycles(), 1);
        assert_eq!(timing.on_insn(&info(TimingInsnClass::IntAlu)), 0);
        assert_eq!(timing.current_cycles(), 1);
        assert_eq!(timing.on_insn(&info(TimingInsnClass::IntAlu)), 1);
        assert_eq!(timing.current_cycles(), 2);
    }

    #[test]
    fn interval_uses_instruction_class_weighting() {
        let mut int_alu = IntervalTiming::new(2.0, 4);
        let mut int_mul = IntervalTiming::new(2.0, 4);

        for _ in 0..4 {
            int_alu.on_insn(&info(TimingInsnClass::IntAlu));
            int_mul.on_insn(&info(TimingInsnClass::IntMul));
        }

        assert_eq!(int_alu.current_cycles(), 2);
        assert_eq!(int_mul.current_cycles(), 4);
    }

    #[test]
    fn interval_uses_distinct_load_and_store_latencies() {
        let mut store = IntervalTiming::new(2.0, 4);
        let mut load = IntervalTiming::new(2.0, 4);

        store.on_insn(&info(TimingInsnClass::Store));
        load.on_insn(&info(TimingInsnClass::Load));

        assert_eq!(store.current_cycles(), 1);
        assert_eq!(load.current_cycles(), 4);
    }

    #[test]
    fn interval_dependency_chain_extends_critical_path() {
        let mut independent = IntervalTiming::new(2.0, 8);
        let mut dependent = IntervalTiming::new(2.0, 8);

        independent.on_insn(&info_with_regs(TimingInsnClass::IntAlu, &[], &[1]));
        independent.on_insn(&info_with_regs(TimingInsnClass::IntAlu, &[], &[2]));
        independent.on_insn(&info_with_regs(TimingInsnClass::IntAlu, &[], &[3]));

        dependent.on_insn(&info_with_regs(TimingInsnClass::IntAlu, &[], &[1]));
        dependent.on_insn(&info_with_regs(TimingInsnClass::IntAlu, &[1], &[2]));
        dependent.on_insn(&info_with_regs(TimingInsnClass::IntAlu, &[2], &[3]));

        assert_eq!(independent.current_cycles(), 2);
        assert_eq!(dependent.current_cycles(), 3);
    }

    #[test]
    fn interval_second_destination_extends_dependency_chain() {
        let mut timing = IntervalTiming::new(2.0, 8);

        timing.on_insn(&info_with_regs(TimingInsnClass::Load, &[], &[1, 2]));
        timing.on_insn(&info_with_regs(TimingInsnClass::IntAlu, &[2], &[3]));

        assert_eq!(timing.current_cycles(), 5);
    }

    #[test]
    fn interval_penalties_raise_live_cycle_estimate() {
        let mut timing = IntervalTiming::new(2.0, 8);
        timing.on_insn(&info(TimingInsnClass::IntAlu));
        assert_eq!(timing.current_cycles(), 1);

        timing.on_branch(true, false);
        assert_eq!(timing.current_cycles(), 6);

        timing.on_mem_access(&MemAccess {
            addr: 0x2000,
            size: 8,
            is_store: false,
            hit_l1: false,
            hit_l2: true,
        });
        assert_eq!(timing.current_cycles(), 10);
    }

    #[test]
    fn interval_independent_load_misses_overlap_in_critical_path() {
        let mut timing = IntervalTiming::new(2.0, 8);

        timing.on_mem_access(&MemAccess {
            addr: 0x1000,
            size: 8,
            is_store: false,
            hit_l1: false,
            hit_l2: false,
        });
        timing.on_insn(&info_with_regs(TimingInsnClass::Load, &[], &[1]));

        timing.on_mem_access(&MemAccess {
            addr: 0x2000,
            size: 8,
            is_store: false,
            hit_l1: false,
            hit_l2: false,
        });
        timing.on_insn(&info_with_regs(TimingInsnClass::Load, &[], &[2]));

        assert_eq!(timing.current_cycles(), 12);
    }

    #[test]
    fn interval_third_independent_load_miss_waits_for_mlp_slot() {
        let mut timing = IntervalTiming::new(2.0, 8);

        for (dst, addr) in [(1, 0x1000), (2, 0x2000), (3, 0x3000)] {
            timing.on_mem_access(&MemAccess {
                addr,
                size: 8,
                is_store: false,
                hit_l1: false,
                hit_l2: false,
            });
            timing.on_insn(&info_with_regs(TimingInsnClass::Load, &[], &[dst]));
        }

        assert_eq!(timing.current_cycles(), 24);
    }

    #[test]
    fn interval_store_misses_do_not_consume_load_mlp_slots() {
        let mut timing = IntervalTiming::new(2.0, 8);

        for dst in [
            TimingInsnClass::Store,
            TimingInsnClass::Store,
            TimingInsnClass::Load,
        ] {
            timing.on_mem_access(&MemAccess {
                addr: 0x1000,
                size: 8,
                is_store: matches!(dst, TimingInsnClass::Store),
                hit_l1: false,
                hit_l2: false,
            });
            let info = match dst {
                TimingInsnClass::Store => info(TimingInsnClass::Store),
                _ => info_with_regs(TimingInsnClass::Load, &[], &[1]),
            };
            timing.on_insn(&info);
        }

        assert_eq!(timing.current_cycles(), 13);
    }

    #[test]
    fn interval_unknown_class_falls_back_to_legacy_flags() {
        let mut timing = IntervalTiming::new(2.0, 4);
        let load = TimingInsnInfo {
            pc: 0x1000,
            class: TimingInsnClass::Unknown,
            is_branch: false,
            is_load: true,
            is_store: false,
            is_fp: false,
            src_regs: [0; TIMING_MAX_SRC_REGS],
            src_reg_count: 0,
            dst_regs: [0; TIMING_MAX_DST_REGS],
            dst_reg_count: 0,
        };

        timing.on_insn(&load);
        assert_eq!(timing.current_cycles(), 4);
    }

    #[test]
    fn interval_boundary_commits_closed_windows() {
        let mut timing = IntervalTiming::new(2.0, 2);

        timing.on_insn(&info(TimingInsnClass::Load));
        timing.on_insn(&info(TimingInsnClass::Store));
        assert_eq!(timing.current_cycles(), 4);

        timing.on_boundary(&mut EventQueue::default());
        assert_eq!(timing.current_cycles(), 4);

        timing.on_insn(&info(TimingInsnClass::IntAlu));
        assert_eq!(timing.current_cycles(), 5);
    }

    #[test]
    fn accurate_currently_delegates_to_virtual() {
        let mut timing = AccurateTiming::default();
        assert_eq!(timing.on_insn(&info(TimingInsnClass::IntAlu)), 1);
        assert_eq!(timing.current_cycles(), 1);
    }

    #[test]
    fn virtual_advance_to_fast_forwards_monotonically() {
        let mut timing = VirtualTiming::new(1.0);
        timing.on_insn(&info(TimingInsnClass::IntAlu));

        timing.advance_to(10);
        assert_eq!(timing.current_cycles(), 10);

        timing.advance_to(4);
        assert_eq!(timing.current_cycles(), 10);
    }

    #[test]
    fn interval_advance_to_commits_idle_fast_forward() {
        let mut timing = IntervalTiming::new(2.0, 8);
        timing.on_insn(&info(TimingInsnClass::IntAlu));
        assert_eq!(timing.current_cycles(), 1);

        timing.advance_to(12);
        assert_eq!(timing.current_cycles(), 12);

        timing.on_insn(&info(TimingInsnClass::IntAlu));
        assert_eq!(timing.current_cycles(), 13);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "timing source register index out of range")]
    fn interval_panics_on_out_of_range_source_register_metadata_in_debug() {
        let mut timing = IntervalTiming::new(2.0, 8);

        timing.on_insn(&info_with_regs(TimingInsnClass::IntAlu, &[], &[1]));
        let malformed = info_with_regs(TimingInsnClass::IntAlu, &[255], &[2]);
        timing.on_insn(&malformed);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "timing destination register index out of range")]
    fn interval_panics_on_out_of_range_destination_register_metadata_in_debug() {
        let mut timing = IntervalTiming::new(2.0, 8);

        let malformed = info_with_regs(TimingInsnClass::Load, &[], &[255]);
        timing.on_insn(&malformed);
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn interval_ignores_out_of_range_source_register_metadata() {
        let mut timing = IntervalTiming::new(2.0, 8);

        timing.on_insn(&info_with_regs(TimingInsnClass::IntAlu, &[], &[1]));
        let malformed = info_with_regs(TimingInsnClass::IntAlu, &[255], &[2]);
        timing.on_insn(&malformed);

        assert_eq!(timing.current_cycles(), 2);
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn interval_ignores_out_of_range_destination_register_metadata() {
        let mut timing = IntervalTiming::new(2.0, 8);

        let malformed = info_with_regs(TimingInsnClass::Load, &[], &[255]);
        timing.on_insn(&malformed);
        timing.on_insn(&info_with_regs(TimingInsnClass::IntAlu, &[255], &[1]));

        assert_eq!(timing.current_cycles(), 5);
    }
}
