//! `helm-timing` — timing model trait + Virtual / Interval / Accurate implementations.
//!
//! `HelmEngine<T: TimingModel>` is monomorphized over the timing model.
//! Each model is compiled as a separate specialization — no vtable, no overhead.
//!
//! # Models
//! - [`Virtual`]  — event-driven ideal IPC (Phase 0/1)
//! - [`Interval`] — Sniper-style interval simulation (<10% MAPE, Phase 1)
//! - [`Accurate`] — cycle-accurate in-order/OoO pipeline (Phase 3)

use helm_event::{EventQueue, Tick};

// ── InsnInfo ──────────────────────────────────────────────────────────────────

/// Per-instruction metadata passed to the timing model's hot path.
pub struct InsnInfo {
    pub pc: u64,
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
    fn on_insn(&mut self, info: &InsnInfo) -> u64;

    /// Notify the model of a completed memory access (L1/L2 outcome).
    fn on_mem_access(&mut self, access: &MemAccess);

    /// Notify the model of a branch outcome (taken, predicted correctly?).
    fn on_branch(&mut self, taken: bool, predicted: bool);

    /// Current simulated cycle count.
    fn current_cycles(&self) -> Tick;

    /// Called at every interval boundary (Interval model) or every instruction
    /// (Virtual/Accurate). May post events into `eq`.
    fn on_boundary(&mut self, eq: &mut EventQueue);
}

// ── Virtual ───────────────────────────────────────────────────────────────────

/// Ideal-IPC timing: every instruction costs exactly `1 / ipc` cycles.
///
/// Used in Phase 0 (no timing) and as the fastest Phase 1 mode.
/// The event queue is advanced every quantum.
pub struct Virtual {
    cycles_per_insn: u64, // fixed for now; fractional IPC handled by rounding
    current_cycles: Tick,
}

impl Virtual {
    /// `ipc` = instructions per cycle (e.g. 1.0, 2.0, 0.5).
    pub fn new(ipc: f64) -> Self {
        let cpi = (1.0 / ipc).ceil() as u64;
        Self { cycles_per_insn: cpi.max(1), current_cycles: 0 }
    }
}

impl Default for Virtual {
    fn default() -> Self { Self::new(1.0) }
}

impl TimingModel for Virtual {
    #[inline(always)]
    fn on_insn(&mut self, _info: &InsnInfo) -> u64 {
        self.current_cycles += self.cycles_per_insn;
        self.cycles_per_insn
    }

    #[inline(always)]
    fn on_mem_access(&mut self, _access: &MemAccess) {}

    #[inline(always)]
    fn on_branch(&mut self, _taken: bool, _predicted: bool) {}

    fn current_cycles(&self) -> Tick { self.current_cycles }

    fn on_boundary(&mut self, _eq: &mut EventQueue) {}
}

// ── Interval ──────────────────────────────────────────────────────────────────

/// Sniper-style interval simulation.
///
/// Tracks an out-of-order window of `window_size` instructions.
/// At each interval boundary, IPC is estimated from dependency chains
/// and cache miss penalties. Target: <10% MAPE vs. real hardware.
///
/// Full implementation in Phase 1. Currently a stub that delegates to Virtual.
pub struct Interval {
    inner: Virtual,
    interval_len: u64, // instructions per interval
    insns_in_interval: u64,
}

impl Interval {
    pub fn new(ipc: f64, interval_len: u64) -> Self {
        Self { inner: Virtual::new(ipc), interval_len, insns_in_interval: 0 }
    }
}

impl Default for Interval {
    fn default() -> Self { Self::new(2.0, 10_000) }
}

impl TimingModel for Interval {
    fn on_insn(&mut self, info: &InsnInfo) -> u64 {
        self.insns_in_interval += 1;
        self.inner.on_insn(info)
    }
    fn on_mem_access(&mut self, access: &MemAccess) { self.inner.on_mem_access(access); }
    fn on_branch(&mut self, taken: bool, predicted: bool) { self.inner.on_branch(taken, predicted); }
    fn current_cycles(&self) -> Tick { self.inner.current_cycles() }
    fn on_boundary(&mut self, eq: &mut EventQueue) {
        if self.insns_in_interval >= self.interval_len {
            self.insns_in_interval = 0;
            // TODO(phase-1): compute actual IPC from OoO window model
            self.inner.on_boundary(eq);
        }
    }
}

// ── Accurate ──────────────────────────────────────────────────────────────────

/// Cycle-accurate in-order pipeline model (Phase 3 placeholder).
pub struct Accurate {
    inner: Virtual,
}

impl Default for Accurate {
    fn default() -> Self { Self { inner: Virtual::new(1.0) } }
}

impl TimingModel for Accurate {
    fn on_insn(&mut self, info: &InsnInfo) -> u64 { self.inner.on_insn(info) }
    fn on_mem_access(&mut self, access: &MemAccess) { self.inner.on_mem_access(access); }
    fn on_branch(&mut self, taken: bool, predicted: bool) { self.inner.on_branch(taken, predicted); }
    fn current_cycles(&self) -> Tick { self.inner.current_cycles() }
    fn on_boundary(&mut self, eq: &mut EventQueue) { self.inner.on_boundary(eq); }
}
