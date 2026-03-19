/// Implemented by any analysis primitive or aggregate that needs to
/// finalize per-vCPU local state after a vCPU quantum ends.
pub trait QuantumObserver: Send + Sync {
    /// Called by the engine at every `run()` return and before checkpoint save.
    ///
    /// `vcpu`       -- the vCPU index whose quantum just completed.
    /// `insn_count` -- total retired instruction count for this vCPU so far.
    fn quantum_end(&mut self, vcpu: usize, insn_count: u64);
}
