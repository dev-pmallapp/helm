// src/format/csv.rs -- CsvFormatter: timestamp_ns,metric,value lines.

use super::ReportFormatter;
use crate::snapshot::HelmSpySnapshot;

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

        out.push_str("timestamp_ns,metric,value\n");

        let mut row = |metric: &str, value: &str| {
            if metric.contains(',') {
                out.push('"');
                out.push_str(metric);
                out.push('"');
            } else {
                out.push_str(metric);
            }
            out.push(',');
            out.push_str(&ts.to_string());
            out.push(',');
            out.push_str(value);
            out.push('\n');
        };

        row("sim_insns", &s.insn_count.to_string());
        row("sim_ticks", &s.tick_count.to_string());
        row("sim_ipc", &format!("{:.6}", s.ipc()));

        let total = s.insn_mix_total().max(1);
        for (class, count) in &s.insn_mix {
            row(&format!("insn_mix.{class}"), &count.to_string());
            let pct = 100.0 * (*count as f64) / (total as f64);
            row(
                &format!("insn_mix.{class}.pct"),
                &format!("{pct:.2}"),
            );
        }

        if let Some(ref c) = s.cache_l1d {
            row(
                &format!("cache_{}.hits", c.name),
                &c.hits.to_string(),
            );
            row(
                &format!("cache_{}.misses", c.name),
                &c.misses.to_string(),
            );
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
        format!("{name},0,{value}\n").into_bytes()
    }

    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8> {
        let mut out = String::new();
        for (label, count) in bins {
            out.push_str(&format!("{name}.{label},0,{count}\n"));
        }
        out.into_bytes()
    }

    fn content_type(&self) -> &'static str {
        "text/csv; charset=utf-8"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::ReportFormatter;

    fn parse_csv(
        snap: &crate::snapshot::HelmSpySnapshot,
    ) -> Vec<Vec<String>> {
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
        let found = rows.iter().any(|r| r.len() >= 3 && r[0] == "sim_insns");
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
            .any(|r| r.len() >= 2 && r[1].parse::<u64>().is_ok());
        assert!(
            has_numeric_ts,
            "no numeric timestamp found in CSV rows"
        );
    }

    #[test]
    fn csv_formatter_content_type() {
        assert!(
            CsvFormatter::default()
                .content_type()
                .contains("text/csv")
        );
    }
}
