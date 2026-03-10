//! Sampling controller for fast-forward + detailed simulation phases.

/// Which phase of a sampled simulation run we are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplingPhase {
    /// Skip instructions as fast as possible (functional only).
    FastForward,
    /// Warm up caches and predictors without collecting stats.
    Warmup,
    /// Collect detailed statistics.
    Detailed,
    /// Drain the pipeline after the ROI.
    Cooldown,
    /// All phases complete.
    Done,
}

/// Controls multi-phase sampled simulation.
pub struct SamplingController {
    pub fast_forward_insns: u64,
    pub warmup_insns: u64,
    pub detailed_insns: u64,
    pub cooldown_insns: u64,
    executed: u64,
    phase: SamplingPhase,
}

impl SamplingController {
    pub fn new(fast_forward: u64, warmup: u64, detailed: u64, cooldown: u64) -> Self {
        Self {
            fast_forward_insns: fast_forward,
            warmup_insns: warmup,
            detailed_insns: detailed,
            cooldown_insns: cooldown,
            executed: 0,
            phase: SamplingPhase::FastForward,
        }
    }

    /// Notify the controller that `n` instructions were executed.
    /// Returns the (possibly new) current phase.
    pub fn advance(&mut self, n: u64) -> SamplingPhase {
        self.executed += n;
        self.phase = self.compute_phase();
        self.phase
    }

    pub fn phase(&self) -> SamplingPhase {
        self.phase
    }

    fn compute_phase(&self) -> SamplingPhase {
        let ff = self.fast_forward_insns;
        let wu = ff + self.warmup_insns;
        let det = wu + self.detailed_insns;
        let cd = det + self.cooldown_insns;

        if self.executed < ff {
            SamplingPhase::FastForward
        } else if self.executed < wu {
            SamplingPhase::Warmup
        } else if self.executed < det {
            SamplingPhase::Detailed
        } else if self.executed < cd {
            SamplingPhase::Cooldown
        } else {
            SamplingPhase::Done
        }
    }
}
