//! `CpuStats` -- aggregate of CPU-side runtime counters
//! (commit, branch, ...).
//!
//! Same shape as `JitPerfStats`: every field is a `PerfCounter` /
//! `LabelCounter`, so increments are interior-mutable
//! (`Clone`-cheap Arc bumps) and the struct collapses to
//! zero-sized fields when `helm-stats/stats` is off.
//!
//! Implements `StatsProducer` so the engine can hand it to a
//! `StatsScope` rooted at a per-vCPU canonical path
//! (`system.cpu<N>`) and the registry view shares the underlying
//! `Arc<AtomicU64>` storage with the hot path.

use crate::{LabelCounter, PerfCounter, StatsProducer, StatsScope};

/// Aggregate CPU-side runtime counters. One instance per engine
/// (today the engine is single-vCPU; per-vCPU slots become a
/// `Vec<CpuStats>` once the inner loop fans out).
#[derive(Clone, Default)]
pub struct CpuStats {
    /// Instructions committed (retired) by the CPU.
    pub committed_insns: PerfCounter,
    /// Cycles consumed by the CPU (timing model output snapshotted
    /// per step). Today this duplicates `system.cpu.cycles`; kept
    /// distinct so per-vCPU CPU stats can fan out independently.
    pub cycles: PerfCounter,
    /// Branches that resolved as taken.
    pub branch_taken: PerfCounter,
    /// Branches that resolved as not-taken.
    pub branch_not_taken: PerfCounter,
    /// Branches that were mispredicted by the timing model.
    pub branch_mispredict: PerfCounter,
    /// Per-class committed-op breakdown (gem5 `commit.committed_ops`
    /// per insn class). Sparse to handle ISA-specific class names.
    pub committed_ops: LabelCounter,
}

impl CpuStats {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StatsProducer for CpuStats {
    fn register_stats(&self, scope: &mut StatsScope<'_>) {
        // commit subtree.
        let mut commit = scope.child("commit");
        commit.adopt_counter(
            "committed_insns",
            "Instructions committed by the CPU",
            self.committed_insns.clone(),
        );
        commit.adopt_counter(
            "cycles",
            "CPU cycles consumed",
            self.cycles.clone(),
        );
        commit.adopt_label_counter(
            "committed_ops",
            "Per-class committed-op breakdown",
            self.committed_ops.clone(),
        );
        // branch subtree.
        let mut branch = scope.child("branch");
        branch.adopt_counter(
            "taken",
            "Branches that resolved as taken",
            self.branch_taken.clone(),
        );
        branch.adopt_counter(
            "not_taken",
            "Branches that resolved as not-taken",
            self.branch_not_taken.clone(),
        );
        branch.adopt_counter(
            "mispredict",
            "Branches mispredicted by the timing model",
            self.branch_mispredict.clone(),
        );
    }
}

#[cfg(all(test, feature = "stats"))]
mod tests {
    use super::CpuStats;
    use crate::{StatsProducer, StatsRegistry, StatsRegistryRead, StatsScope};

    #[test]
    fn register_under_per_vcpu_path() {
        let stats = CpuStats::new();
        stats.committed_insns.add(5);
        stats.branch_taken.inc();
        stats.branch_taken.inc();
        stats.branch_not_taken.inc();
        stats.committed_ops.bump_static("alu");

        let mut reg = StatsRegistry::new();
        {
            let mut scope = StatsScope::new(&mut reg, "system.cpu0");
            stats.register_stats(&mut scope);
        }

        assert_eq!(
            reg.counter_value("system.cpu0.commit.committed_insns"),
            Some(5)
        );
        assert_eq!(reg.counter_value("system.cpu0.branch.taken"), Some(2));
        assert_eq!(reg.counter_value("system.cpu0.branch.not_taken"), Some(1));
        let snap = reg
            .label_snapshot("system.cpu0.commit.committed_ops")
            .expect("committed_ops registered");
        let total: u64 = snap.iter().map(|(_, v)| *v).sum();
        assert_eq!(total, 1);
    }

    #[test]
    fn shared_storage_after_register() {
        let stats = CpuStats::new();
        let mut reg = StatsRegistry::new();
        {
            let mut scope = StatsScope::new(&mut reg, "system.cpu");
            stats.register_stats(&mut scope);
        }
        // Bumping after register must be visible via the registry.
        stats.committed_insns.add(10);
        stats.branch_mispredict.inc();
        assert_eq!(
            reg.counter_value("system.cpu.commit.committed_insns"),
            Some(10)
        );
        assert_eq!(reg.counter_value("system.cpu.branch.mispredict"), Some(1));
    }
}
