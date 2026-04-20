// src/format/json.rs -- JsonFormatter: structured JSON output.

use super::ReportFormatter;
use crate::snapshot::HelmSpySnapshot;
use serde_json::{json, to_vec_pretty};

/// Structured JSON formatter.
///
/// Output is a single JSON object. All integer values are JSON numbers.
/// Floating-point fields (ipc, hit_rate, miss_rate) are JSON numbers (f64).
#[derive(Default)]
pub struct JsonFormatter;

impl ReportFormatter for JsonFormatter {
    fn format_session(&self, s: &HelmSpySnapshot) -> Vec<u8> {
        let total = s.insn_mix_total().max(1);
        let mix: Vec<_> = s
            .insn_mix
            .iter()
            .map(|(class, count)| {
                json!({
                    "name":  format!("insn_mix.{class}"),
                    "value": count,
                    "unit":  "instructions",
                    "pct":   (100.0 * (*count as f64) / (total as f64))
                })
            })
            .collect();

        let hot_pcs: Vec<_> = s
            .hot_pcs
            .iter()
            .take(20)
            .map(|(pc, count)| json!({ "pc": format!("{pc:#x}"), "count": count }))
            .collect();

        let mut obj = json!({
            "helm_report_version": 1,
            "timestamp_ns": s.snapshot_ns,
            "sim_insns": s.insn_count,
            "sim_ticks": s.tick_count,
            "sim_ipc":   s.ipc(),
            "insn_mix":  mix,
            "branch_direction": {
                "taken": s.branch_direction.taken,
                "not_taken": s.branch_direction.not_taken,
            },
            "mmu_activity": {
                "tlb_hits": s.mmu_activity.tlb_hits,
                "tlb_misses": s.mmu_activity.tlb_misses,
                "stage1_walks": s.mmu_activity.stage1_walks,
                "stage2_walks": s.mmu_activity.stage2_walks,
            },
            "hot_pcs":   hot_pcs,
        });
        if let Some(ref filter) = s.scoreboard_filter {
            obj["scoreboard_filter"] = json!({
                "pc_start": format!("{:#x}", filter.start),
                "pc_end": format!("{:#x}", filter.end),
            });
        }
        if let Some(ref filter) = s.scoreboard_addr_filter {
            obj["scoreboard_addr_filter"] = json!({
                "addr_start": format!("{:#x}", filter.start),
                "addr_end": format!("{:#x}", filter.end),
            });
        }

        if let Some(ref stats) = s.user_stage2_insn_abort {
            obj["user_stage2_insn_abort_events"] = json!(stats.events);
            obj["user_stage2_insn_abort_repeats"] = json!(stats.repeats);
        }
        obj["jit_activity"] = json!({
            "block_compile_events": s.jit_activity.block_compile_events,
            "block_compile_guest_insns": s.jit_activity.block_compile_guest_insns,
            "block_execute_events": s.jit_activity.block_execute_events,
            "block_retired_insns": s.jit_activity.block_retired_insns,
            "trace_compile_events": s.jit_activity.trace_compile_events,
            "trace_compile_guest_insns": s.jit_activity.trace_compile_guest_insns,
            "trace_execute_events": s.jit_activity.trace_execute_events,
            "trace_execute_insns": s.jit_activity.trace_execute_insns,
            "fallback_events": s.jit_activity.fallback_events,
            "fallback_insns": s.jit_activity.fallback_insns,
            "cache_hit_events": s.jit_activity.cache_hit_events,
            "cache_miss_events": s.jit_activity.cache_miss_events,
            "cache_promote_events": s.jit_activity.cache_promote_events,
            "guard_exit_events": s.jit_activity.guard_exit_events,
            "guard_retire_events": s.jit_activity.guard_retire_events,
        });

        if let Some(ref c) = s.cache_l1d {
            obj["cache_l1d"] = json!({
                "name":     c.name,
                "hits":     c.hits,
                "misses":   c.misses,
                "hit_rate": c.hit_rate,
            });
        }

        if let Some(ref bp) = s.branch_pred {
            obj["branch_pred"] = json!({
                "name":           bp.name,
                "kind":           bp.kind,
                "predictions":    bp.predictions,
                "mispredictions": bp.mispredictions,
                "miss_rate":      bp.miss_rate,
            });
        }

        to_vec_pretty(&obj).unwrap_or_else(|_| b"{}".to_vec())
    }

    fn format_counter(&self, name: &str, value: u64, unit: &str) -> Vec<u8> {
        let obj = json!({ "name": name, "value": value, "unit": unit });
        to_vec_pretty(&obj).unwrap_or_default()
    }

    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8> {
        let bins: Vec<_> = bins
            .iter()
            .map(|(l, c)| json!({ "bin": l, "count": c }))
            .collect();
        let obj = json!({ "name": name, "bins": bins });
        to_vec_pretty(&obj).unwrap_or_default()
    }

    fn content_type(&self) -> &'static str {
        "application/json; charset=utf-8"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::ReportFormatter;

    fn parse_output(snap: &crate::snapshot::HelmSpySnapshot) -> serde_json::Value {
        let bytes = JsonFormatter::default().format_session(snap);
        serde_json::from_slice(&bytes).expect("output is not valid JSON")
    }

    #[test]
    fn json_formatter_is_valid_json() {
        let snap = crate::tests::test_snapshot();
        let _v = parse_output(&snap);
    }

    #[test]
    fn json_formatter_sim_insns_field() {
        let snap = crate::tests::test_snapshot();
        let v = parse_output(&snap);
        assert_eq!(v["sim_insns"].as_u64(), Some(10_000_000));
    }

    #[test]
    fn json_formatter_insn_mix_array() {
        let snap = crate::tests::test_snapshot();
        let v = parse_output(&snap);
        let mix = v["insn_mix"].as_array().expect("insn_mix should be array");
        assert!(!mix.is_empty());
        assert!(mix
            .iter()
            .any(|e| e["name"].as_str() == Some("insn_mix.IntAlu")));
    }

    #[test]
    fn json_formatter_cache_field() {
        let snap = crate::tests::test_snapshot();
        let v = parse_output(&snap);
        assert!(v["cache_l1d"].is_object(), "cache_l1d should be present");
        assert!(v["cache_l1d"]["hits"].is_number());
        assert!(v["cache_l1d"]["hit_rate"].is_number());
    }

    #[test]
    fn json_formatter_hot_pcs_array() {
        let snap = crate::tests::test_snapshot();
        let v = parse_output(&snap);
        let hot_pcs = v["hot_pcs"].as_array().expect("hot_pcs should be array");
        assert!(!hot_pcs.is_empty());
        assert!(hot_pcs[0]["pc"].is_string());
        assert!(hot_pcs[0]["count"].is_number());
    }

    #[test]
    fn json_formatter_user_stage2_stats_fields() {
        let snap = crate::tests::test_snapshot();
        let v = parse_output(&snap);
        assert_eq!(v["user_stage2_insn_abort_events"].as_u64(), Some(7));
        assert_eq!(v["user_stage2_insn_abort_repeats"].as_u64(), Some(2));
    }

    #[test]
    fn json_formatter_jit_activity_object() {
        let snap = crate::tests::test_snapshot();
        let v = parse_output(&snap);
        assert_eq!(v["jit_activity"]["block_compile_events"].as_u64(), Some(12));
        assert_eq!(
            v["jit_activity"]["trace_compile_guest_insns"].as_u64(),
            Some(144)
        );
        assert_eq!(v["jit_activity"]["fallback_events"].as_u64(), Some(5));
        assert_eq!(v["jit_activity"]["cache_hit_events"].as_u64(), Some(40));
        assert_eq!(v["jit_activity"]["guard_exit_events"].as_u64(), Some(6));
        assert_eq!(v["jit_activity"]["trace_execute_events"].as_u64(), Some(8));
    }

    #[test]
    fn json_formatter_branch_direction_and_filter_objects() {
        let snap = crate::tests::test_snapshot();
        let v = parse_output(&snap);
        assert_eq!(v["branch_direction"]["taken"].as_u64(), Some(900_000));
        assert_eq!(
            v["scoreboard_filter"]["pc_start"].as_str(),
            Some("0xffff800010000000")
        );
        assert_eq!(
            v["scoreboard_addr_filter"]["addr_start"].as_str(),
            Some("0x40000000")
        );
        assert_eq!(v["mmu_activity"]["tlb_hits"].as_u64(), Some(1_000_000));
    }

    #[test]
    fn json_formatter_content_type() {
        assert!(JsonFormatter::default()
            .content_type()
            .contains("application/json"));
    }
}
