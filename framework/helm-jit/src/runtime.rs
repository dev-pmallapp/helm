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

/// Result of executing a bounded interpreter fallback batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterpreterFallback<S> {
    /// The interpreter consumed a bounded batch and the JIT loop may resume.
    Resume {
        /// Number of instructions retired by the interpreter batch.
        consumed: u64,
        /// Remaining JIT budget after the interpreter batch completed.
        budget_remaining: u64,
    },
    /// The host produced a terminal or non-resumable stop reason.
    Stop {
        /// Host-specific stop reason returned by the interpreter batch.
        stop: S,
        /// Number of instructions retired by the interpreter batch.
        consumed: u64,
        /// Remaining JIT budget after the interpreter batch completed.
        budget_remaining: u64,
    },
}

/// Minimal host-side hook for JIT/interpreter cooperation.
///
/// The full `run_jit()` loop still lives in `helm-engine` today, but this
/// trait establishes the host boundary that later slices can build on.
pub trait JitRuntimeHost {
    /// Host-specific stop-reason type returned by interpreter batches.
    type StopReason;

    /// Total retired instructions visible to the host.
    fn insns_retired(&self) -> u64;

    /// Run a bounded interpreter batch from the current architectural state.
    fn run_interpreter_batch(&mut self, max_insns: u64) -> Self::StopReason;

    /// Return `true` when the stop reason allows the JIT loop to resume.
    fn is_resumable_stop(stop: &Self::StopReason) -> bool;
}

/// Execute the shared bounded interpreter-fallback policy.
pub fn run_bounded_interpreter_fallback<H: JitRuntimeHost>(
    host: &mut H,
    budget_remaining: u64,
    config: JitRuntimeConfig,
) -> InterpreterFallback<H::StopReason> {
    let batch = config.interp_fallback_batch_insns.min(budget_remaining);
    let before = host.insns_retired();
    let stop = host.run_interpreter_batch(batch);
    let consumed = host.insns_retired().saturating_sub(before);
    let budget_remaining = budget_remaining.saturating_sub(consumed);

    if H::is_resumable_stop(&stop) && budget_remaining > 0 {
        InterpreterFallback::Resume {
            consumed,
            budget_remaining,
        }
    } else {
        InterpreterFallback::Stop {
            stop,
            consumed,
            budget_remaining,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Stop {
        Quantum,
        Exit,
    }

    struct MockHost {
        retired: u64,
        batch_result: Stop,
        batch_consumed: u64,
    }

    impl JitRuntimeHost for MockHost {
        type StopReason = Stop;

        fn insns_retired(&self) -> u64 {
            self.retired
        }

        fn run_interpreter_batch(&mut self, max_insns: u64) -> Self::StopReason {
            self.retired = self.retired.saturating_add(self.batch_consumed.min(max_insns));
            self.batch_result
        }

        fn is_resumable_stop(stop: &Self::StopReason) -> bool {
            matches!(stop, Stop::Quantum)
        }
    }

    #[test]
    fn bounded_fallback_resumes_when_quantum_and_budget_remains() {
        let mut host = MockHost {
            retired: 10,
            batch_result: Stop::Quantum,
            batch_consumed: 8,
        };

        let result = run_bounded_interpreter_fallback(
            &mut host,
            32,
            JitRuntimeConfig {
                interp_fallback_batch_insns: 16,
            },
        );

        assert_eq!(
            result,
            InterpreterFallback::Resume {
                consumed: 8,
                budget_remaining: 24,
            }
        );
    }

    #[test]
    fn bounded_fallback_stops_on_non_resumable_reason() {
        let mut host = MockHost {
            retired: 4,
            batch_result: Stop::Exit,
            batch_consumed: 6,
        };

        let result = run_bounded_interpreter_fallback(
            &mut host,
            32,
            JitRuntimeConfig {
                interp_fallback_batch_insns: 16,
            },
        );

        assert_eq!(
            result,
            InterpreterFallback::Stop {
                stop: Stop::Exit,
                consumed: 6,
                budget_remaining: 26,
            }
        );
    }
}
