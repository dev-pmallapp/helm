//! `helm-probe` -- zero-cost typed probe points for helm-ng.
// Probe<T> uses unsafe Send+Sync impls (safe by construction: closure bounds enforce it).
// Public event structs don't need individual docs; module-level doc covers them.
#![allow(missing_docs, unsafe_code)]
//!
//! # Feature behaviour
//! - default build: `Probe<T>` is zero-sized; all probe sites collapse to no-op guards.
//! - `--features instrumentation`: `Probe<T>` holds `Vec<Listener>`; `has_listeners()` is
//!   O(1) empty check; predicted-not-taken when no subscribers.
//! - `--features probe-full`: enables `instrumentation` plus richer event fields.

mod events;
mod macros; // probe!() re-exported by #[macro_export]
mod probe;

pub use events::{
    BranchEvent, BranchKind, CpuFaultEvent, CpuStepEvent, InsnClass, IrqEvent, MemAccessEvent,
    MmioEvent,
    JitBackendId, JitBlockCompileEvent, JitBlockExecuteEvent,
    JitCacheEvent, JitCacheOp, JitFallbackEvent,
    JitGuardExitEvent, JitTraceCompileEvent, JitTraceExecuteEvent,
};
pub use probe::Probe;

// Thread-local instruction count -- updated by the engine before each step.
// Used by helm-spy triggers and windows to gate observations without
// passing insn_count through every probe event.
thread_local! {
    static PROBE_INSN_COUNT: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
}

/// Update the per-thread probe instruction count.
/// Called by `HelmEngine` at the start of each step.
#[inline]
pub fn update_probe_insn_count(n: u64) {
    PROBE_INSN_COUNT.with(|c| c.set(n));
}

/// Read the per-thread probe instruction count.
#[inline]
pub fn probe_insn_count() -> u64 {
    PROBE_INSN_COUNT.with(std::cell::Cell::get)
}

/// CPU probe bundle. Add as `pub probes: CpuProbes` on `HelmEngine<T>`.
#[derive(Default)]
pub struct CpuProbes {
    pub pre_step: Probe<CpuStepEvent>,
    pub post_step: Probe<CpuStepEvent>,
    pub fault: Probe<CpuFaultEvent>,
    pub mem: Probe<MemAccessEvent>,
    pub branch: Probe<BranchEvent>,
}

impl CpuProbes {
    /// Returns `true` if any probe has at least one subscriber.
    /// Always `false` unless the `instrumentation` feature is enabled.
    pub fn any_active(&self) -> bool {
        self.pre_step.has_listeners()
            || self.post_step.has_listeners()
            || self.fault.has_listeners()
            || self.mem.has_listeners()
            || self.branch.has_listeners()
    }
}

/// JIT probe bundle. Add as `pub jit_probes: JitProbes` on `HelmEngine<T>`.
///
/// Events are emitted at block/trace dispatch granularity, not per guest
/// instruction. Callers needing per-instruction detail should use `CpuProbes`
/// with interpreter fallback (see `JitDebugController::force_interpreter`).
#[derive(Default)]
pub struct JitProbes {
    pub block_compile: Probe<JitBlockCompileEvent>,
    pub block_execute: Probe<JitBlockExecuteEvent>,
    pub trace_compile: Probe<JitTraceCompileEvent>,
    pub trace_execute: Probe<JitTraceExecuteEvent>,
    pub cache: Probe<JitCacheEvent>,
    pub guard_exit: Probe<JitGuardExitEvent>,
    pub fallback: Probe<JitFallbackEvent>,
}

impl JitProbes {
    /// Returns `true` if any JIT probe has at least one subscriber.
    pub fn any_active(&self) -> bool {
        self.block_compile.has_listeners()
            || self.block_execute.has_listeners()
            || self.trace_compile.has_listeners()
            || self.trace_execute.has_listeners()
            || self.cache.has_listeners()
            || self.guard_exit.has_listeners()
            || self.fallback.has_listeners()
    }
}

/// GIC probe bundle. Add as `pub probes: GicProbes` on `GicState` (feature-gated).
#[derive(Default)]
pub struct GicProbes {
    pub irq_asserted: Probe<IrqEvent>,
    pub irq_deasserted: Probe<IrqEvent>,
    pub eoi: Probe<IrqEvent>,
}
