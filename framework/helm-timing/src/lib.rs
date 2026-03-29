//! `helm-timing` — timing model trait + VirtualTiming / IntervalTiming / AccurateTiming implementations.
//!
//! `HelmEngine<T: TimingModel>` is monomorphized over the timing model.
//! Each model is compiled as a separate specialization — no vtable, no overhead.
//!
//! # Models
//! - [`VirtualTiming`]  — event-driven ideal IPC (Phase 0/1)
//! - [`IntervalTiming`] — class-weighted interval timing (Phase 1 starting point)
//! - [`AccurateTiming`] — cycle-accurate in-order/OoO pipeline (Phase 3)

#![allow(missing_docs)]

use helm_event::{EventQueue, Tick};

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
pub struct TimingInsnInfo {
    pub pc: u64,
    pub class: TimingInsnClass,
    pub is_branch: bool,
    pub is_load: bool,
    pub is_store: bool,
    pub is_fp: bool,
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
pub trait TimingModel: Send + 'static {
    /// Advance time by the cost of one instruction. Returns cycles consumed.
    fn on_insn(&mut self, info: &TimingInsnInfo) -> u64;

    /// Notify the model of a completed memory access (L1/L2 outcome).
    fn on_mem_access(&mut self, access: &MemAccess);

    /// Notify the model of a branch outcome (taken, predicted correctly?).
    fn on_branch(&mut self, taken: bool, predicted: bool);

    /// Current simulated cycle count.
    fn current_cycles(&self) -> Tick;

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
    cycles_per_insn: u64, // fixed for now; fractional IPC handled by rounding
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
        let cpi = (1.0 / sanitize_ipc(ipc)).ceil() as u64;
        Self {
            cycles_per_insn: cpi.max(1),
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
    #[inline(always)]
    fn on_insn(&mut self, _info: &TimingInsnInfo) -> u64 {
        self.current_cycles += self.cycles_per_insn;
        self.cycles_per_insn
    }

    #[inline(always)]
    fn on_mem_access(&mut self, _access: &MemAccess) {}

    #[inline(always)]
    fn on_branch(&mut self, _taken: bool, _predicted: bool) {}

    fn current_cycles(&self) -> Tick {
        self.current_cycles
    }

    fn on_boundary(&mut self, _eq: &mut EventQueue) {}
}

// ── IntervalTiming ──────────────────────────────────────────────────────────────────

/// Sniper-style interval simulation.
///
/// Tracks an out-of-order window of `window_size` instructions.
/// At each interval boundary, IPC is estimated from dependency chains
/// and cache miss penalties. Target: <10% MAPE vs. real hardware.
pub struct IntervalTiming {
    ipc: f64,
    interval_len: u64, // instructions per interval
    committed_cycles: Tick,
    insns_in_interval: u64,
    interval_work: f64,
    branch_penalty: Tick,
    mem_stall_cycles: Tick,
}

impl IntervalTiming {
    pub fn new(ipc: f64, interval_len: u64) -> Self {
        Self {
            ipc: sanitize_ipc(ipc),
            interval_len: interval_len.max(1),
            committed_cycles: 0,
            insns_in_interval: 0,
            interval_work: 0.0,
            branch_penalty: 0,
            mem_stall_cycles: 0,
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
    fn class_work_units(class: TimingInsnClass) -> f64 {
        match class {
            TimingInsnClass::IntAlu => 1.0,
            TimingInsnClass::IntMul => 3.0,
            TimingInsnClass::Branch => 1.0,
            TimingInsnClass::Load | TimingInsnClass::Store => 2.0,
            TimingInsnClass::FpAlu => 3.0,
            TimingInsnClass::SimdAlu => 2.5,
            TimingInsnClass::System => 2.0,
            TimingInsnClass::Nop => 0.5,
            TimingInsnClass::Atomic => 4.0,
            TimingInsnClass::Unknown => 1.5,
        }
    }

    #[inline(always)]
    fn estimate_open_interval_cycles(&self) -> Tick {
        if self.insns_in_interval == 0 {
            0
        } else {
            (self.interval_work / self.ipc).ceil() as Tick
                + self.branch_penalty
                + self.mem_stall_cycles
        }
    }

    #[inline(always)]
    fn commit_interval_if_ready(&mut self) {
        if self.insns_in_interval < self.interval_len {
            return;
        }

        self.committed_cycles += self.estimate_open_interval_cycles();
        self.reset_open_interval();
    }

    #[inline(always)]
    fn reset_open_interval(&mut self) {
        self.insns_in_interval = 0;
        self.interval_work = 0.0;
        self.branch_penalty = 0;
        self.mem_stall_cycles = 0;
    }

    #[inline(always)]
    fn mem_penalty(access: &MemAccess) -> Tick {
        if access.hit_l1 {
            0
        } else if access.hit_l2 {
            4
        } else {
            12
        }
    }
}

impl Default for IntervalTiming {
    fn default() -> Self {
        Self::new(2.0, 10_000)
    }
}

impl TimingModel for IntervalTiming {
    fn on_insn(&mut self, info: &TimingInsnInfo) -> u64 {
        let before = self.current_cycles();
        self.commit_interval_if_ready();
        self.insns_in_interval += 1;
        self.interval_work += Self::class_work_units(Self::effective_class(info));
        self.current_cycles().saturating_sub(before)
    }
    fn on_mem_access(&mut self, access: &MemAccess) {
        self.mem_stall_cycles += Self::mem_penalty(access);
    }
    fn on_branch(&mut self, taken: bool, predicted: bool) {
        if taken != predicted {
            self.branch_penalty += 5;
        }
    }
    fn current_cycles(&self) -> Tick {
        self.committed_cycles + self.estimate_open_interval_cycles()
    }
    fn on_boundary(&mut self, eq: &mut EventQueue) {
        let _ = eq;
        self.commit_interval_if_ready();
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
        }
    }

    #[test]
    fn virtual_current_cycles_is_monotonic() {
        let mut timing = VirtualTiming::new(2.0);
        assert_eq!(timing.current_cycles(), 0);
        assert_eq!(timing.on_insn(&info(TimingInsnClass::IntAlu)), 1);
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
        assert_eq!(int_mul.current_cycles(), 6);
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
    fn interval_unknown_class_falls_back_to_legacy_flags() {
        let mut timing = IntervalTiming::new(2.0, 4);
        let load = TimingInsnInfo {
            pc: 0x1000,
            class: TimingInsnClass::Unknown,
            is_branch: false,
            is_load: true,
            is_store: false,
            is_fp: false,
        };

        timing.on_insn(&load);
        assert_eq!(timing.current_cycles(), 1);
    }

    #[test]
    fn accurate_currently_delegates_to_virtual() {
        let mut timing = AccurateTiming::default();
        assert_eq!(timing.on_insn(&info(TimingInsnClass::IntAlu)), 1);
        assert_eq!(timing.current_cycles(), 1);
    }
}
