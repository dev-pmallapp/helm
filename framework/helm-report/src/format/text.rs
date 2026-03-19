// src/format/text.rs -- TextFormatter: human-readable gem5-style text output.

use std::fmt::Write;
use super::ReportFormatter;
use crate::snapshot::SpySpySnapshot;

/// Human-readable gem5-style text output.
///
/// Column width: metric name left-padded to 40 chars, value right-justified in 20 chars.
/// Percentages appended for instruction mix lines.
#[derive(Default)]
pub struct TextFormatter;

const SEP: &str = "---------- Begin Simulation Statistics ----------";
const SEP_END: &str = "----------  End Simulation Statistics  ----------";

impl TextFormatter {
    fn format_metric(
        out: &mut String,
        name: &str,
        value: impl std::fmt::Display,
        comment: &str,
    ) {
        let comment_part = if comment.is_empty() {
            String::new()
        } else {
            format!("  # {comment}")
        };
        let _ = writeln!(out, "{name:<40}{value:>20}{comment_part}");
    }
}

impl ReportFormatter for TextFormatter {
    fn format_session(&self, s: &SpySpySnapshot) -> Vec<u8> {
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

#[cfg(test)]
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
        let out =
            String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("Begin Simulation Statistics"));
        assert!(out.contains("End Simulation Statistics"));
    }

    #[test]
    fn text_formatter_insn_mix_percentages() {
        let snap = crate::tests::test_snapshot();
        let out =
            String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        // IntAlu = 5_000_000 / 10_000_000 = 50.00%
        assert!(out.contains("insn_mix.IntAlu"), "missing IntAlu");
        assert!(out.contains("50.00%"), "wrong percentage for IntAlu");
    }

    #[test]
    fn text_formatter_cache_present() {
        let snap = crate::tests::test_snapshot();
        let out =
            String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("cache_l1d.hits"), "missing cache hits");
        assert!(out.contains("cache_l1d.misses"), "missing cache misses");
        assert!(
            out.contains("cache_l1d.hit_rate"),
            "missing hit rate"
        );
    }

    #[test]
    fn text_formatter_hot_pcs() {
        let snap = crate::tests::test_snapshot();
        let out =
            String::from_utf8(TextFormatter::default().format_session(&snap)).unwrap();
        assert!(out.contains("hot_pcs[0]"), "missing hot PC entry");
        assert!(
            out.contains("0xffff800010012a4c"),
            "wrong PC address"
        );
    }

    #[test]
    fn text_formatter_format_counter() {
        let fmt = TextFormatter::default();
        let out =
            String::from_utf8(fmt.format_counter("my_counter", 42, "things")).unwrap();
        assert!(out.contains("my_counter"), "missing counter name");
        assert!(out.contains("42"), "missing counter value");
        assert!(out.contains("things"), "missing unit");
    }

    #[test]
    fn text_formatter_content_type() {
        assert!(
            TextFormatter::default()
                .content_type()
                .contains("text/plain")
        );
    }
}
