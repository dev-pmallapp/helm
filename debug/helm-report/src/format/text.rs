// src/format/text.rs -- TextFormatter: human-readable gem5-style text output.
//
// Dual-impl per `docs/design/helm-report/HLD.md` § 13. Live formatter
// emits the gem5-style table; noop formatter returns empty buffers.

#[cfg(feature = "report")]
pub use live::TextFormatter;
#[cfg(not(feature = "report"))]
pub use noop::TextFormatter;

#[cfg(feature = "report")]
mod live {

use crate::format::ReportFormatter;
use crate::snapshot::HelmSpySnapshot;
use std::fmt::Write;

/// Human-readable gem5-style text output.
///
/// Column width: metric name left-padded to 40 chars, value right-justified in 20 chars.
/// Percentages appended for instruction mix lines.
#[derive(Default)]
pub struct TextFormatter;

const SEP: &str = "---------- Begin Simulation Statistics ----------";
const SEP_END: &str = "----------  End Simulation Statistics  ----------";

impl TextFormatter {
    fn format_metric(out: &mut String, name: &str, value: impl std::fmt::Display, comment: &str) {
        let comment_part = if comment.is_empty() {
            String::new()
        } else {
            format!("  # {comment}")
        };
        let _ = writeln!(out, "{name:<40}{value:>20}{comment_part}");
    }
}

impl ReportFormatter for TextFormatter {
    fn format_session(&self, s: &HelmSpySnapshot) -> Vec<u8> {
        let mut out = String::with_capacity(2048);
        out.push_str(SEP);
        out.push('\n');

        Self::format_metric(&mut out, "sim_insns", s.insn_count, "Instructions retired");
        Self::format_metric(&mut out, "sim_ticks", s.tick_count, "Ticks simulated");
        if s.tick_count > 0 {
            Self::format_metric(
                &mut out,
                "sim_ipc",
                format!("{:.6}", s.ipc()),
                "Instructions per cycle",
            );
        }
        if let Some(ref filter) = s.scoreboard_filter {
            Self::format_metric(
                &mut out,
                "scoreboard_pc_start",
                format!("{:#x}", filter.start),
                "PC start for scoreboard-filtered counters",
            );
            Self::format_metric(
                &mut out,
                "scoreboard_pc_end",
                format!("{:#x}", filter.end),
                "PC end for scoreboard-filtered counters",
            );
        }
        if let Some(ref filter) = s.scoreboard_addr_filter {
            Self::format_metric(
                &mut out,
                "scoreboard_addr_start",
                format!("{:#x}", filter.start),
                "Address start for scoreboard-filtered counters",
            );
            Self::format_metric(
                &mut out,
                "scoreboard_addr_end",
                format!("{:#x}", filter.end),
                "Address end for scoreboard-filtered counters",
            );
        }
        Self::format_metric(
            &mut out,
            "branch_direction_taken",
            s.branch_direction.taken,
            "Taken branch events counted by the branch-direction scoreboard",
        );
        Self::format_metric(
            &mut out,
            "branch_direction_not_taken",
            s.branch_direction.not_taken,
            "Not-taken branch events counted by the branch-direction scoreboard",
        );
        Self::format_metric(
            &mut out,
            "mmu_tlb_hits",
            s.mmu_activity.tlb_hits,
            "MMU translations served from the software TLB",
        );
        Self::format_metric(
            &mut out,
            "mmu_tlb_misses",
            s.mmu_activity.tlb_misses,
            "MMU translations that missed in the software TLB",
        );
        Self::format_metric(
            &mut out,
            "mmu_stage1_walks",
            s.mmu_activity.stage1_walks,
            "Stage-1 MMU page table walks",
        );
        Self::format_metric(
            &mut out,
            "mmu_stage2_walks",
            s.mmu_activity.stage2_walks,
            "Stage-2 MMU page table walks",
        );

        if let Some(ref stats) = s.user_stage2_insn_abort {
            Self::format_metric(
                &mut out,
                "user_stage2_insn_abort_events",
                stats.events,
                "Observed low-VA EL1 stage-2 instruction aborts",
            );
            Self::format_metric(
                &mut out,
                "user_stage2_insn_abort_repeats",
                stats.repeats,
                "Repeated low-VA EL1 stage-2 instruction aborts",
            );
        }
        Self::format_metric(
            &mut out,
            "jit_block_compile_events",
            s.jit_activity.block_compile_events,
            "JIT block compilations observed via probes",
        );
        Self::format_metric(
            &mut out,
            "jit_block_compile_guest_insns",
            s.jit_activity.block_compile_guest_insns,
            "Guest instructions compiled into JIT blocks",
        );
        Self::format_metric(
            &mut out,
            "jit_block_execute_events",
            s.jit_activity.block_execute_events,
            "JIT block dispatches observed via probes",
        );
        Self::format_metric(
            &mut out,
            "jit_block_retired_insns",
            s.jit_activity.block_retired_insns,
            "Guest instructions retired through JIT block execution probes",
        );
        Self::format_metric(
            &mut out,
            "jit_trace_compile_events",
            s.jit_activity.trace_compile_events,
            "JIT trace compilations observed via probes",
        );
        Self::format_metric(
            &mut out,
            "jit_trace_compile_guest_insns",
            s.jit_activity.trace_compile_guest_insns,
            "Guest instructions compiled into JIT traces",
        );
        Self::format_metric(
            &mut out,
            "jit_trace_execute_events",
            s.jit_activity.trace_execute_events,
            "JIT trace execute probe events",
        );
        Self::format_metric(
            &mut out,
            "jit_trace_execute_insns",
            s.jit_activity.trace_execute_insns,
            "Guest instructions retired through JIT trace execution",
        );
        Self::format_metric(
            &mut out,
            "jit_fallback_events",
            s.jit_activity.fallback_events,
            "JIT fallback batches observed via probes",
        );
        Self::format_metric(
            &mut out,
            "jit_fallback_insns",
            s.jit_activity.fallback_insns,
            "Guest instructions retired by JIT fallback batches",
        );
        Self::format_metric(
            &mut out,
            "jit_cache_hit_events",
            s.jit_activity.cache_hit_events,
            "JIT cache hit probe events",
        );
        Self::format_metric(
            &mut out,
            "jit_cache_miss_events",
            s.jit_activity.cache_miss_events,
            "JIT cache miss probe events",
        );
        Self::format_metric(
            &mut out,
            "jit_cache_promote_events",
            s.jit_activity.cache_promote_events,
            "JIT cache promote probe events",
        );
        Self::format_metric(
            &mut out,
            "jit_guard_exit_events",
            s.jit_activity.guard_exit_events,
            "JIT trace guard-exit probe events",
        );
        Self::format_metric(
            &mut out,
            "jit_guard_retire_events",
            s.jit_activity.guard_retire_events,
            "JIT trace retire-on-guard probe events",
        );

        // Instruction mix
        let total = s.insn_mix_total().max(1);
        for (class, count) in &s.insn_mix {
            let pct = 100.0 * (*count as f64) / (total as f64);
            let name = format!("insn_mix.{class}");
            let val = format!("{count:>12}  {pct:>6.2}%");
            let _ = writeln!(out, "{name:<40}{val}");
        }

        // Cache
        if let Some(ref c) = s.cache_l1d {
            let prefix = format!("cache_{}", c.name);
            Self::format_metric(&mut out, &format!("{prefix}.hits"), c.hits, "");
            Self::format_metric(&mut out, &format!("{prefix}.misses"), c.misses, "");
            Self::format_metric(
                &mut out,
                &format!("{prefix}.hit_rate"),
                format!("{:.6}", c.hit_rate),
                "",
            );
        }

        // Branch predictor
        if let Some(ref bp) = s.branch_pred {
            let prefix = format!("branch_pred_{}", bp.name);
            Self::format_metric(
                &mut out,
                &format!("{prefix}.predictions"),
                bp.predictions,
                "",
            );
            Self::format_metric(
                &mut out,
                &format!("{prefix}.mispredictions"),
                bp.mispredictions,
                "",
            );
            Self::format_metric(
                &mut out,
                &format!("{prefix}.miss_rate"),
                format!("{:.6}", bp.miss_rate),
                "",
            );
        }

        // Hot PCs (top 10)
        for (i, (pc, count)) in s.hot_pcs.iter().take(10).enumerate() {
            let name = format!("hot_pcs[{i}]");
            let val = format!("{pc:#018x}  count={count}");
            let _ = writeln!(out, "{name:<40}{val}");
        }

        out.push_str(SEP_END);
        out.push('\n');
        out.into_bytes()
    }

    fn format_counter(&self, name: &str, value: u64, unit: &str) -> Vec<u8> {
        format!("{name:<40}{value:>20}  # {unit}\n").into_bytes()
    }

    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8> {
        let mut out = String::new();
        for (label, count) in bins {
            let key = format!("{name}.{label}");
            let _ = writeln!(out, "{key:<40}{count:>20}");
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

    /// ZST shell -- every formatter method returns an empty buffer.
    #[derive(Default)]
    pub struct TextFormatter;

    impl ReportFormatter for TextFormatter {
        #[inline(always)]
        fn format_session(&self, _s: &HelmSpySnapshot) -> Vec<u8> {
            Vec::new()
        }

        #[inline(always)]
        fn format_counter(&self, _name: &str, _value: u64, _unit: &str) -> Vec<u8> {
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

#[cfg(all(test, feature = "report"))]
mod tests {
    use super::*;
    use crate::format::ReportFormatter;

    #[test]
    fn text_formatter_contains_sim_insns() {
        let snap = crate::tests::test_snapshot();
        let fmt = TextFormatter::default();
        let out = String::from_utf8(fmt.format_session(&snap)).unwrap();
        assert!(out.contains("sim_insns"), "missing sim_insns");
        assert!(out.contains("10000000"), "wrong insn count");
    }

    #[test]
    fn text_formatter_contains_begin_end_markers() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("Begin Simulation Statistics"));
        assert!(out.contains("End Simulation Statistics"));
    }

    #[test]
    fn text_formatter_insn_mix_percentages() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        // IntAlu = 5_000_000 / 10_000_000 = 50.00%
        assert!(out.contains("insn_mix.IntAlu"), "missing IntAlu");
        assert!(out.contains("50.00%"), "wrong percentage for IntAlu");
    }

    #[test]
    fn text_formatter_cache_present() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("cache_l1d.hits"), "missing cache hits");
        assert!(out.contains("cache_l1d.misses"), "missing cache misses");
        assert!(out.contains("cache_l1d.hit_rate"), "missing hit rate");
    }

    #[test]
    fn text_formatter_hot_pcs() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("hot_pcs[0]"), "missing hot PC entry");
        assert!(out.contains("0xffff800010012a4c"), "wrong PC address");
    }

    #[test]
    fn text_formatter_user_stage2_stats() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("user_stage2_insn_abort_events"));
        assert!(out.contains("user_stage2_insn_abort_repeats"));
    }

    #[test]
    fn text_formatter_jit_activity_stats() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("jit_block_compile_events"));
        assert!(out.contains("jit_block_execute_events"));
        assert!(out.contains("jit_trace_compile_events"));
        assert!(out.contains("jit_fallback_events"));
        assert!(out.contains("jit_cache_hit_events"));
        assert!(out.contains("jit_guard_exit_events"));
        assert!(out.contains("jit_trace_execute_events"));
    }

    #[test]
    fn text_formatter_branch_direction_and_filter_stats() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("branch_direction_taken"));
        assert!(out.contains("branch_direction_not_taken"));
        assert!(out.contains("scoreboard_pc_start"));
        assert!(out.contains("scoreboard_pc_end"));
        assert!(out.contains("scoreboard_addr_start"));
        assert!(out.contains("scoreboard_addr_end"));
        assert!(out.contains("mmu_tlb_hits"));
        assert!(out.contains("mmu_stage2_walks"));
    }

    #[test]
    fn text_formatter_format_counter() {
        let fmt = TextFormatter::default();
        let out = String::from_utf8(fmt.format_counter("my_counter", 42, "things")).unwrap();
        assert!(out.contains("my_counter"), "missing counter name");
        assert!(out.contains("42"), "missing counter value");
        assert!(out.contains("things"), "missing unit");
    }

    #[test]
    fn text_formatter_content_type() {
        assert!(TextFormatter::default()
            .content_type()
            .contains("text/plain"));
    }
}
