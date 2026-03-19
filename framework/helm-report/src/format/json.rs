// src/format/json.rs -- JsonFormatter: structured JSON output.

use super::ReportFormatter;
use crate::snapshot::SpySpySnapshot;
use serde_json::{json, to_vec_pretty};

/// Structured JSON formatter.
///
/// Output is a single JSON object. All integer values are JSON numbers.
/// Floating-point fields (ipc, hit_rate, miss_rate) are JSON numbers (f64).
#[derive(Default)]
pub struct JsonFormatter;

impl ReportFormatter for JsonFormatter {
    fn format_session(&self, s: &SpySpySnapshot) -> Vec<u8> {
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
            .map(|(pc, count)| {
                json!({ "pc": format!("{pc:#x}"), "count": count })
            })
            .collect();

        let mut obj = json!({
            "helm_report_version": 1,
            "timestamp_ns": s.snapshot_ns,
            "sim_insns": s.insn_count,
            "sim_ticks": s.tick_count,
            "sim_ipc":   s.ipc(),
            "insn_mix":  mix,
            "hot_pcs":   hot_pcs,
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

    fn parse_output(snap: &crate::snapshot::SpySpySnapshot) -> serde_json::Value {
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
    fn json_formatter_content_type() {
        assert!(
            JsonFormatter::default()
                .content_type()
                .contains("application/json")
        );
    }
}
