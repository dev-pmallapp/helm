// src/format/gemstats.rs -- GemstatsFormatter: gem5-compatible stats.txt output.

use std::fmt::Write;
use super::ReportFormatter;
use crate::snapshot::SpySpySnapshot;

/// gem5-compatible `stats.txt` formatter.
///
/// Column alignment: name in column 0..39, value in column 40..59,
/// comment starting at column 60. Matches gem5 output for known metric
/// names (`system.cpu.committedInsts`, etc.).
#[derive(Default)]
pub struct GemstatsFormatter;

impl GemstatsFormatter {
    fn line(out: &mut String, name: &str, val: &str, comment: &str) {
        let _ = writeln!(out, "{name:<40}{val:<20}# {comment}");
    }
}

impl ReportFormatter for GemstatsFormatter {
    fn format_session(&self, s: &SpySpySnapshot) -> Vec<u8> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::ReportFormatter;

    #[test]
    fn gemstats_formatter_begin_end_markers() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(
            GemstatsFormatter::default().format_session(&snap),
        )
        .unwrap();
        assert!(out.contains("Begin Simulation Statistics"));
        assert!(out.contains("End Simulation Statistics"));
    }

    #[test]
    fn gemstats_formatter_committed_insns_key() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(
            GemstatsFormatter::default().format_session(&snap),
        )
        .unwrap();
        assert!(
            out.contains("system.cpu.committedInsts"),
            "missing system.cpu.committedInsts key"
        );
    }

    #[test]
    fn gemstats_formatter_ipc_key() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(
            GemstatsFormatter::default().format_session(&snap),
        )
        .unwrap();
        assert!(
            out.contains("system.cpu.ipc"),
            "missing system.cpu.ipc key"
        );
    }

    #[test]
    fn gemstats_formatter_cache_keys_present() {
        let snap = crate::tests::test_snapshot();
        let out = String::from_utf8(
            GemstatsFormatter::default().format_session(&snap),
        )
        .unwrap();
        assert!(
            out.contains("dcache.overall_hits"),
            "missing dcache hits"
        );
        assert!(
            out.contains("dcache.overall_misses"),
            "missing dcache misses"
        );
    }

    #[test]
    fn gemstats_formatter_content_type() {
        assert!(
            GemstatsFormatter::default()
                .content_type()
                .contains("text/plain")
        );
    }
}
