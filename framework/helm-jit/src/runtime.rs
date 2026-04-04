//! Runtime-facing JIT boundary types.
//!
//! This module is the start of the crate boundary move from `helm-engine`
//! into `helm-jit`: shared runtime policy/config should live here, while the
//! engine implements the host side.

/// JIT runtime policy knobs shared between host and executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitRuntimeConfig {
    /// Maximum interpreter instructions to run after a JIT fallback before
    /// re-entering the JIT loop.
    pub interp_fallback_batch_insns: u64,
}

impl Default for JitRuntimeConfig {
    fn default() -> Self {
        Self {
            interp_fallback_batch_insns: 256,
        }
    }
}

/// Default runtime policy for the current JIT loop.
pub const DEFAULT_RUNTIME_CONFIG: JitRuntimeConfig = JitRuntimeConfig {
    interp_fallback_batch_insns: 256,
};

/// Minimal host-side hook for JIT/interpreter cooperation.
///
/// The full `run_jit()` loop still lives in `helm-engine` today, but this
/// trait establishes the host boundary that later slices can build on.
pub trait JitRuntimeHost {
    /// Host-specific stop-reason type returned by interpreter batches.
    type StopReason;

    /// Run a bounded interpreter batch from the current architectural state.
    fn run_interpreter_batch(&mut self, max_insns: u64) -> Self::StopReason;
}
