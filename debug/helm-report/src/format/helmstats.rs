// src/format/helmstats.rs -- HelmstatsFormatter: gem5-compatible stats.txt output.
//
// Dual-impl per `docs/design/helm-report/HLD.md` § 13.
//
// The `helmstats` crate feature additionally exposes the
// `emit_config_ini` and `emit_stats_txt` writer entry points -- those
// helpers consume `&helm_stats::StatsRegistry` directly today. Slice
// S5 will lift them to `&dyn StatsRegistry` once helm-stats grows the
// trait.

#[cfg(feature = "report")]
pub use live::HelmstatsFormatter;
#[cfg(not(feature = "report"))]
pub use noop::HelmstatsFormatter;

#[cfg(feature = "helmstats")]
pub use writer::{emit_config_ini, emit_stats_txt};

#[cfg(feature = "report")]
mod live {

use crate::format::ReportFormatter;
use crate::snapshot::HelmSpySnapshot;
use std::fmt::Write;

/// gem5-compatible `stats.txt` formatter.
///
/// Column alignment: name in column 0..39, value in column 40..59,
/// comment starting at column 60. Matches gem5 output for known metric
/// names (`system.cpu.committedInsts`, etc.).
#[derive(Default)]
pub struct HelmstatsFormatter;

impl HelmstatsFormatter {
    fn line(out: &mut String, name: &str, val: &str, comment: &str) {
        let _ = writeln!(out, "{name:<40}{val:<20}# {comment}");
    }
}

impl ReportFormatter for HelmstatsFormatter {
    fn format_session(&self, s: &HelmSpySnapshot) -> Vec<u8> {
        let mut out = String::with_capacity(2048);
        out.push_str("---------- Begin Simulation Statistics ----------\n");

        Self::line(
            &mut out,
            "sim_insns",
            &s.insn_count.to_string(),
            "Number of instructions simulated",
        );
        Self::line(
            &mut out,
            "sim_ticks",
            &s.tick_count.to_string(),
            "Number of ticks simulated",
        );
        Self::line(
            &mut out,
            "sim_freq",
            "1000000000",
            "Frequency of simulated ticks",
        );
        Self::line(
            &mut out,
            "system.cpu.committedInsts",
            &s.insn_count.to_string(),
            "Committed instructions",
        );
        Self::line(
            &mut out,
            "system.cpu.ipc",
            &format!("{:.6}", s.ipc()),
            "Instructions per tick",
        );
        if let Some(ref filter) = s.scoreboard_filter {
            Self::line(
                &mut out,
                "system.cpu.scoreboard.pc_start",
                &format!("{:#x}", filter.start),
                "PC start for scoreboard-filtered counters",
            );
            Self::line(
                &mut out,
                "system.cpu.scoreboard.pc_end",
                &format!("{:#x}", filter.end),
                "PC end for scoreboard-filtered counters",
            );
        }
        if let Some(ref filter) = s.scoreboard_addr_filter {
            Self::line(
                &mut out,
                "system.cpu.scoreboard.addr_start",
                &format!("{:#x}", filter.start),
                "Address start for scoreboard-filtered counters",
            );
            Self::line(
                &mut out,
                "system.cpu.scoreboard.addr_end",
                &format!("{:#x}", filter.end),
                "Address end for scoreboard-filtered counters",
            );
        }
        Self::line(
            &mut out,
            "system.cpu.branch_direction.taken",
            &s.branch_direction.taken.to_string(),
            "Taken branch events",
        );
        Self::line(
            &mut out,
            "system.cpu.branch_direction.not_taken",
            &s.branch_direction.not_taken.to_string(),
            "Not-taken branch events",
        );
        Self::line(
            &mut out,
            "system.cpu.mmu.tlb_hits",
            &s.mmu_activity.tlb_hits.to_string(),
            "MMU translations served from the software TLB",
        );
        Self::line(
            &mut out,
            "system.cpu.mmu.tlb_misses",
            &s.mmu_activity.tlb_misses.to_string(),
            "MMU translations that missed in the software TLB",
        );
        Self::line(
            &mut out,
            "system.cpu.mmu.stage1_walks",
            &s.mmu_activity.stage1_walks.to_string(),
            "Stage-1 MMU page table walks",
        );
        Self::line(
            &mut out,
            "system.cpu.mmu.stage2_walks",
            &s.mmu_activity.stage2_walks.to_string(),
            "Stage-2 MMU page table walks",
        );
        if let Some(ref stats) = s.user_stage2_insn_abort {
            Self::line(
                &mut out,
                "system.cpu.user_stage2_insn_abort_events",
                &stats.events.to_string(),
                "Observed low-VA EL1 stage-2 instruction aborts",
            );
            Self::line(
                &mut out,
                "system.cpu.user_stage2_insn_abort_repeats",
                &stats.repeats.to_string(),
                "Repeated low-VA EL1 stage-2 instruction aborts",
            );
        }
        Self::line(
            &mut out,
            "system.cpu.jit.block_compile_events",
            &s.jit_activity.block_compile_events.to_string(),
            "JIT block compile probe events",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.block_compile_guest_insns",
            &s.jit_activity.block_compile_guest_insns.to_string(),
            "Guest instructions compiled into JIT blocks",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.block_execute_events",
            &s.jit_activity.block_execute_events.to_string(),
            "JIT block execute probe events",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.block_retired_insns",
            &s.jit_activity.block_retired_insns.to_string(),
            "Guest instructions retired through JIT block probes",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.trace_compile_events",
            &s.jit_activity.trace_compile_events.to_string(),
            "JIT trace compile probe events",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.trace_compile_guest_insns",
            &s.jit_activity.trace_compile_guest_insns.to_string(),
            "Guest instructions compiled into JIT traces",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.trace_execute_events",
            &s.jit_activity.trace_execute_events.to_string(),
            "JIT trace execute probe events",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.trace_execute_insns",
            &s.jit_activity.trace_execute_insns.to_string(),
            "Guest instructions retired by JIT trace execution",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.fallback_events",
            &s.jit_activity.fallback_events.to_string(),
            "JIT fallback probe events",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.fallback_insns",
            &s.jit_activity.fallback_insns.to_string(),
            "Guest instructions retired by JIT fallback batches",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.cache_hit_events",
            &s.jit_activity.cache_hit_events.to_string(),
            "JIT cache hit probe events",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.cache_miss_events",
            &s.jit_activity.cache_miss_events.to_string(),
            "JIT cache miss probe events",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.cache_promote_events",
            &s.jit_activity.cache_promote_events.to_string(),
            "JIT cache promote probe events",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.guard_exit_events",
            &s.jit_activity.guard_exit_events.to_string(),
            "JIT trace guard-exit probe events",
        );
        Self::line(
            &mut out,
            "system.cpu.jit.guard_retire_events",
            &s.jit_activity.guard_retire_events.to_string(),
            "JIT trace retire-on-guard probe events",
        );

        let total = s.insn_mix_total().max(1);
        for (class, count) in &s.insn_mix {
            let name = format!("system.cpu.op_class_0::{class}");
            let pct = 100.0 * (*count as f64) / (total as f64);
            Self::line(&mut out, &name, &format!("{count}  {pct:.4}%"), "");
        }

        if let Some(ref c) = s.cache_l1d {
            Self::line(
                &mut out,
                "system.cpu.dcache.overall_hits::total",
                &c.hits.to_string(),
                "",
            );
            Self::line(
                &mut out,
                "system.cpu.dcache.overall_misses::total",
                &c.misses.to_string(),
                "",
            );
            Self::line(
                &mut out,
                "system.cpu.dcache.overall_miss_rate::total",
                &format!("{:.6}", 1.0 - c.hit_rate),
                "",
            );
        }

        if let Some(ref bp) = s.branch_pred {
            Self::line(
                &mut out,
                "system.cpu.branchPred.lookups",
                &bp.predictions.to_string(),
                "",
            );
            Self::line(
                &mut out,
                "system.cpu.branchPred.mispredicts",
                &bp.mispredictions.to_string(),
                "",
            );
        }

        out.push_str("----------  End Simulation Statistics  ----------\n");
        out.into_bytes()
    }

    fn format_counter(&self, name: &str, value: u64, comment: &str) -> Vec<u8> {
        let mut out = String::new();
        Self::line(&mut out, name, &value.to_string(), comment);
        out.into_bytes()
    }

    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8> {
        let mut out = String::new();
        for (label, count) in bins {
            Self::line(
                &mut out,
                &format!("{name}::{label}"),
                &count.to_string(),
                "",
            );
        }
        out.into_bytes()
    }

    fn content_type(&self) -> &'static str {
        "text/plain; charset=utf-8"
    }
}

}

#[cfg(not(feature = "report"))]
mod noop {
    use crate::format::ReportFormatter;
    use crate::snapshot::HelmSpySnapshot;

    /// ZST shell.
    #[derive(Default)]
    pub struct HelmstatsFormatter;

    impl ReportFormatter for HelmstatsFormatter {
        #[inline(always)]
        fn format_session(&self, _s: &HelmSpySnapshot) -> Vec<u8> {
            Vec::new()
        }

        #[inline(always)]
        fn format_counter(&self, _name: &str, _value: u64, _comment: &str) -> Vec<u8> {
            Vec::new()
        }

        #[inline(always)]
        fn format_histogram(&self, _name: &str, _bins: &[(&str, u64)]) -> Vec<u8> {
            Vec::new()
        }

        #[inline(always)]
        fn content_type(&self) -> &'static str {
            "text/plain; charset=utf-8"
        }
    }
}

#[cfg(feature = "helmstats")]
mod writer {
    //! gem5-shaped writer entry points exposed under the `helmstats`
    //! feature. These produce `config.ini` and `stats.txt` files
    //! given a populated `helm_stats::StatsRegistry`.
    //!
    //! Slice S5 (`docs/research/gem5-stats-helm-adaptation.md` § 4)
    //! will replace the concrete `&helm_stats::StatsRegistry` with
    //! `&dyn StatsRegistry` once `helm-stats` grows the trait. Until
    //! then, the signatures live behind `helmstats` so they can move
    //! freely with that work.

    use std::fs::File;
    use std::io::{self, Write};
    use std::path::Path;

    /// Emit a gem5-style `config.ini` file describing the registered
    /// counters and histograms. Today the registry is a flat
    /// dot-path namespace, so the generated INI is a single
    /// `[stats]` section listing every metric path with a
    /// type-annotation comment.
    pub fn emit_config_ini(
        registry: &helm_stats::StatsRegistry,
        path: &Path,
    ) -> io::Result<()> {
        let dump = registry.dump_json();
        let mut file = File::create(path)?;
        writeln!(file, "[stats]")?;
        writeln!(file, "; Generated by helm-report::emit_config_ini")?;
        writeln!(file, "; Source: helm_stats::StatsRegistry::dump_json()")?;
        writeln!(file, "json_payload = {dump}")?;
        Ok(())
    }

    /// Emit a gem5-style `stats.txt` file. Each registered metric
    /// becomes one `name<padding>value<padding># desc` line, wrapped
    /// by the `Begin/End Simulation Statistics` markers.
    pub fn emit_stats_txt(
        registry: &helm_stats::StatsRegistry,
        path: &Path,
    ) -> io::Result<()> {
        let dump = registry.dump_json();
        let mut file = File::create(path)?;
        writeln!(file, "---------- Begin Simulation Statistics ----------")?;

        // Walk the JSON payload as `(key, value)` pairs so we get
        // gem5's `name<40>value<20># desc` line shape. We use the
        // already-rendered registry JSON to avoid coupling to
        // `PerfCounter`/`PerfHistogram` internals before Slice S5.
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&dump) {
            if let Some(obj) = value.as_object() {
                let mut keys: Vec<&String> = obj.keys().collect();
                keys.sort();
                for key in keys {
                    let val = &obj[key];
                    writeln!(file, "{key:<40}{val:<20}# (helm-stats)")?;
                }
            }
        }

        writeln!(file, "----------  End Simulation Statistics  ----------")?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tempfile::tempdir;

        #[test]
        fn emit_config_ini_creates_file_with_header() {
            let registry = helm_stats::StatsRegistry::new();
            let dir = tempdir().unwrap();
            let path = dir.path().join("config.ini");
            emit_config_ini(&registry, &path).unwrap();
            let contents = std::fs::read_to_string(&path).unwrap();
            assert!(contents.contains("[stats]"));
            assert!(contents.contains("emit_config_ini"));
        }

        #[test]
        fn emit_stats_txt_creates_file_with_markers() {
            let registry = helm_stats::StatsRegistry::new();
            let dir = tempdir().unwrap();
            let path = dir.path().join("stats.txt");
            emit_stats_txt(&registry, &path).unwrap();
            let contents = std::fs::read_to_string(&path).unwrap();
            assert!(contents.contains("Begin Simulation Statistics"));
            assert!(contents.contains("End Simulation Statistics"));
        }
    }
}

#[cfg(all(test, feature = "report"))]
mod tests {
    use super::*;
    use crate::format::ReportFormatter;

    #[test]
    fn helmstats_formatter_begin_end_markers() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(HelmstatsFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("Begin Simulation Statistics"));
        assert!(out.contains("End Simulation Statistics"));
    }

    #[test]
    fn helmstats_formatter_committed_insns_key() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(HelmstatsFormatter::default().format_session(&snap)).unwrap();
        assert!(
            out.contains("system.cpu.committedInsts"),
            "missing system.cpu.committedInsts key"
        );
    }

    #[test]
    fn helmstats_formatter_ipc_key() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(HelmstatsFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("system.cpu.ipc"), "missing system.cpu.ipc key");
    }

    #[test]
    fn helmstats_formatter_cache_keys_present() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(HelmstatsFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("dcache.overall_hits"), "missing dcache hits");
        assert!(
            out.contains("dcache.overall_misses"),
            "missing dcache misses"
        );
    }

    #[test]
    fn helmstats_formatter_user_stage2_stats_keys_present() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(HelmstatsFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("system.cpu.user_stage2_insn_abort_events"));
        assert!(out.contains("system.cpu.user_stage2_insn_abort_repeats"));
    }

    #[test]
    fn helmstats_formatter_jit_activity_keys_present() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(HelmstatsFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("system.cpu.jit.block_compile_events"));
        assert!(out.contains("system.cpu.jit.block_execute_events"));
        assert!(out.contains("system.cpu.jit.trace_compile_events"));
        assert!(out.contains("system.cpu.jit.fallback_events"));
        assert!(out.contains("system.cpu.jit.cache_hit_events"));
        assert!(out.contains("system.cpu.jit.guard_exit_events"));
        assert!(out.contains("system.cpu.jit.trace_execute_events"));
    }

    #[test]
    fn helmstats_formatter_branch_direction_and_filter_keys_present() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(HelmstatsFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("system.cpu.branch_direction.taken"));
        assert!(out.contains("system.cpu.branch_direction.not_taken"));
        assert!(out.contains("system.cpu.scoreboard.pc_start"));
        assert!(out.contains("system.cpu.scoreboard.addr_start"));
        assert!(out.contains("system.cpu.mmu.tlb_hits"));
    }

    #[test]
    fn helmstats_formatter_content_type() {
        assert!(HelmstatsFormatter::default()
            .content_type()
            .contains("text/plain"));
    }
}
