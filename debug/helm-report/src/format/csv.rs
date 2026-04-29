// src/format/csv.rs -- CsvFormatter: timestamp_ns,metric,value lines.
//
// Dual-impl per `docs/design/helm-report/HLD.md` § 13.

#[cfg(feature = "report")]
pub use live::CsvFormatter;
#[cfg(not(feature = "report"))]
pub use noop::CsvFormatter;

#[cfg(feature = "report")]
mod live {

use crate::format::ReportFormatter;
use crate::snapshot::HelmSpySnapshot;

const CSV_COLUMNS: [&str; 3] = ["timestamp_ns", "metric", "value"];

fn push_csv_header(out: &mut String) {
    out.push_str(CSV_COLUMNS[0]);
    out.push(',');
    out.push_str(CSV_COLUMNS[1]);
    out.push(',');
    out.push_str(CSV_COLUMNS[2]);
    out.push('\n');
}

fn push_csv_row(out: &mut String, timestamp_ns: u64, metric: &str, value: &str) {
    out.push_str(&timestamp_ns.to_string());
    out.push(',');
    if metric.contains(',') {
        out.push('"');
        out.push_str(metric);
        out.push('"');
    } else {
        out.push_str(metric);
    }
    out.push(',');
    out.push_str(value);
    out.push('\n');
}

/// CSV formatter: `timestamp_ns,metric,value` lines.
///
/// The first line is always the header row. Subsequent lines are one metric per row.
/// Floating-point values are formatted with 6 decimal places.
#[derive(Default)]
pub struct CsvFormatter;

impl ReportFormatter for CsvFormatter {
    fn format_session(&self, s: &HelmSpySnapshot) -> Vec<u8> {
        let mut out = String::with_capacity(1024);
        let ts = s.snapshot_ns;

        push_csv_header(&mut out);

        let mut row = |metric: &str, value: &str| push_csv_row(&mut out, ts, metric, value);

        row("sim_insns", &s.insn_count.to_string());
        row("sim_ticks", &s.tick_count.to_string());
        row("sim_ipc", &format!("{:.6}", s.ipc()));
        if let Some(ref filter) = s.scoreboard_filter {
            row("scoreboard_pc_start", &format!("{:#x}", filter.start));
            row("scoreboard_pc_end", &format!("{:#x}", filter.end));
        }
        if let Some(ref filter) = s.scoreboard_addr_filter {
            row("scoreboard_addr_start", &format!("{:#x}", filter.start));
            row("scoreboard_addr_end", &format!("{:#x}", filter.end));
        }
        row(
            "branch_direction_taken",
            &s.branch_direction.taken.to_string(),
        );
        row(
            "branch_direction_not_taken",
            &s.branch_direction.not_taken.to_string(),
        );
        row("mmu_tlb_hits", &s.mmu_activity.tlb_hits.to_string());
        row("mmu_tlb_misses", &s.mmu_activity.tlb_misses.to_string());
        row("mmu_stage1_walks", &s.mmu_activity.stage1_walks.to_string());
        row("mmu_stage2_walks", &s.mmu_activity.stage2_walks.to_string());

        if let Some(ref stats) = s.user_stage2_insn_abort {
            row("user_stage2_insn_abort_events", &stats.events.to_string());
            row("user_stage2_insn_abort_repeats", &stats.repeats.to_string());
        }
        row(
            "jit_block_compile_events",
            &s.jit_activity.block_compile_events.to_string(),
        );
        row(
            "jit_block_compile_guest_insns",
            &s.jit_activity.block_compile_guest_insns.to_string(),
        );
        row(
            "jit_block_execute_events",
            &s.jit_activity.block_execute_events.to_string(),
        );
        row(
            "jit_block_retired_insns",
            &s.jit_activity.block_retired_insns.to_string(),
        );
        row(
            "jit_trace_compile_events",
            &s.jit_activity.trace_compile_events.to_string(),
        );
        row(
            "jit_trace_compile_guest_insns",
            &s.jit_activity.trace_compile_guest_insns.to_string(),
        );
        row(
            "jit_trace_execute_events",
            &s.jit_activity.trace_execute_events.to_string(),
        );
        row(
            "jit_trace_execute_insns",
            &s.jit_activity.trace_execute_insns.to_string(),
        );
        row(
            "jit_fallback_events",
            &s.jit_activity.fallback_events.to_string(),
        );
        row(
            "jit_fallback_insns",
            &s.jit_activity.fallback_insns.to_string(),
        );
        row(
            "jit_cache_hit_events",
            &s.jit_activity.cache_hit_events.to_string(),
        );
        row(
            "jit_cache_miss_events",
            &s.jit_activity.cache_miss_events.to_string(),
        );
        row(
            "jit_cache_promote_events",
            &s.jit_activity.cache_promote_events.to_string(),
        );
        row(
            "jit_guard_exit_events",
            &s.jit_activity.guard_exit_events.to_string(),
        );
        row(
            "jit_guard_retire_events",
            &s.jit_activity.guard_retire_events.to_string(),
        );

        let total = s.insn_mix_total().max(1);
        for (class, count) in &s.insn_mix {
            row(&format!("insn_mix.{class}"), &count.to_string());
            let pct = 100.0 * (*count as f64) / (total as f64);
            row(&format!("insn_mix.{class}.pct"), &format!("{pct:.2}"));
        }

        if let Some(ref c) = s.cache_l1d {
            row(&format!("cache_{}.hits", c.name), &c.hits.to_string());
            row(&format!("cache_{}.misses", c.name), &c.misses.to_string());
            row(
                &format!("cache_{}.hit_rate", c.name),
                &format!("{:.6}", c.hit_rate),
            );
        }

        if let Some(ref bp) = s.branch_pred {
            row(
                &format!("branch_pred_{}.predictions", bp.name),
                &bp.predictions.to_string(),
            );
            row(
                &format!("branch_pred_{}.mispredictions", bp.name),
                &bp.mispredictions.to_string(),
            );
            row(
                &format!("branch_pred_{}.miss_rate", bp.name),
                &format!("{:.6}", bp.miss_rate),
            );
        }

        out.into_bytes()
    }

    fn format_counter(&self, name: &str, value: u64, _unit: &str) -> Vec<u8> {
        let mut out = String::new();
        push_csv_row(&mut out, 0, name, &value.to_string());
        out.into_bytes()
    }

    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8> {
        let mut out = String::new();
        for (label, count) in bins {
            push_csv_row(&mut out, 0, &format!("{name}.{label}"), &count.to_string());
        }
        out.into_bytes()
    }

    fn content_type(&self) -> &'static str {
        "text/csv; charset=utf-8"
    }
}

}

#[cfg(not(feature = "report"))]
mod noop {
    use crate::format::ReportFormatter;
    use crate::snapshot::HelmSpySnapshot;

    /// ZST shell -- empty buffers in every formatter method.
    #[derive(Default)]
    pub struct CsvFormatter;

    impl ReportFormatter for CsvFormatter {
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
            "text/csv; charset=utf-8"
        }
    }
}

#[cfg(all(test, feature = "report"))]
mod tests {
    use super::*;
    use crate::format::ReportFormatter;

    fn parse_csv(snap: &crate::snapshot::HelmSpySnapshot) -> Vec<Vec<String>> {
        let bytes = CsvFormatter::default().format_session(snap);
        let s = String::from_utf8(bytes).unwrap();
        s.lines()
            .map(|l| l.split(',').map(str::to_owned).collect())
            .collect()
    }

    #[test]
    fn csv_formatter_header_row() {
        let snap = crate::tests::test_snapshot();
        let rows = parse_csv(&snap);
        assert!(!rows.is_empty());
        assert_eq!(rows[0], vec!["timestamp_ns", "metric", "value"]);
    }

    #[test]
    fn csv_formatter_sim_insns_row_present() {
        let snap = crate::tests::test_snapshot();
        let rows = parse_csv(&snap);
        let found = rows.iter().any(|r| r.len() >= 3 && r[1] == "sim_insns");
        assert!(found, "sim_insns row not found in CSV");
    }

    #[test]
    fn csv_formatter_three_columns() {
        let snap = crate::tests::test_snapshot();
        let rows = parse_csv(&snap);
        for row in rows.iter().skip(1) {
            assert_eq!(
                row.len(),
                3,
                "CSV row does not have exactly 3 columns: {row:?}"
            );
        }
    }

    #[test]
    fn csv_formatter_timestamp_is_numeric() {
        let snap = crate::tests::test_snapshot();
        let rows = parse_csv(&snap);
        let has_numeric_ts = rows
            .iter()
            .skip(1)
            .any(|r| r.len() >= 1 && r[0].parse::<u64>().is_ok());
        assert!(has_numeric_ts, "no numeric timestamp found in CSV rows");
    }

    #[test]
    fn csv_formatter_user_stage2_stats_rows_present() {
        let snap = crate::tests::test_snapshot();
        let rows = parse_csv(&snap);
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "user_stage2_insn_abort_events"));
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "user_stage2_insn_abort_repeats"));
    }

    #[test]
    fn csv_formatter_jit_activity_rows_present() {
        let snap = crate::tests::test_snapshot();
        let rows = parse_csv(&snap);
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "jit_block_compile_events"));
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "jit_block_execute_events"));
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "jit_trace_compile_events"));
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "jit_fallback_events"));
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "jit_cache_hit_events"));
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "jit_guard_exit_events"));
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "jit_trace_execute_events"));
    }

    #[test]
    fn csv_formatter_branch_direction_and_filter_rows_present() {
        let snap = crate::tests::test_snapshot();
        let rows = parse_csv(&snap);
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "branch_direction_taken"));
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "branch_direction_not_taken"));
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "scoreboard_pc_start"));
        assert!(rows
            .iter()
            .any(|r| r.len() >= 3 && r[1] == "scoreboard_addr_start"));
        assert!(rows.iter().any(|r| r.len() >= 3 && r[1] == "mmu_tlb_hits"));
    }

    #[test]
    fn csv_formatter_content_type() {
        assert!(CsvFormatter::default().content_type().contains("text/csv"));
    }

    #[test]
    fn csv_formatter_rows_follow_header_order() {
        let snap = crate::tests::test_snapshot();
        let rows = parse_csv(&snap);
        let sim_insns = rows
            .iter()
            .skip(1)
            .find(|row| row.len() == 3 && row[1] == "sim_insns")
            .expect("sim_insns row missing");

        assert_eq!(sim_insns[0], snap.snapshot_ns.to_string());
        assert_eq!(sim_insns[1], "sim_insns");
        assert_eq!(sim_insns[2], snap.insn_count.to_string());
    }

    #[test]
    fn csv_round_trip_all_core_metrics_present() {
        let snap = crate::tests::test_snapshot();
        let rows = parse_csv(&snap);
        let metrics: Vec<&str> = rows.iter().skip(1).map(|r| r[1].as_str()).collect();

        let required = ["sim_insns", "sim_ticks", "sim_ipc"];
        for m in &required {
            assert!(metrics.contains(m), "missing required metric: {m}");
        }

        for (class, _) in &snap.insn_mix {
            let count_key = format!("insn_mix.{class}");
            let pct_key = format!("insn_mix.{class}.pct");
            assert!(
                metrics.iter().any(|m| *m == count_key),
                "missing insn_mix count for {class}"
            );
            assert!(
                metrics.iter().any(|m| *m == pct_key),
                "missing insn_mix pct for {class}"
            );
        }

        if let Some(ref c) = snap.cache_l1d {
            for suffix in &["hits", "misses", "hit_rate"] {
                let key = format!("cache_{}.{suffix}", c.name);
                assert!(
                    metrics.iter().any(|m| *m == key),
                    "missing cache metric: {key}"
                );
            }
        }

        if let Some(ref bp) = snap.branch_pred {
            for suffix in &["predictions", "mispredictions", "miss_rate"] {
                let key = format!("branch_pred_{}.{suffix}", bp.name);
                assert!(
                    metrics.iter().any(|m| *m == key),
                    "missing branch_pred metric: {key}"
                );
            }
        }
    }

    #[test]
    fn csv_round_trip_values_parseable() {
        let snap = crate::tests::test_snapshot();
        let rows = parse_csv(&snap);
        for row in rows.iter().skip(1) {
            assert_eq!(row.len(), 3, "row has wrong column count: {row:?}");
            row[0].parse::<u64>().expect("timestamp not u64");
            assert!(!row[1].is_empty(), "metric name is empty");
            assert!(!row[2].is_empty(), "value is empty");
        }
    }
}
