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
pub use writer::{emit_config_ini, emit_config_ini_with_params, emit_config_json, emit_stats_txt};

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

    use helm_stats::StatsRegistryRead;
    use std::fs::File;
    use std::io::{self, Write};
    use std::path::Path;

    /// Emit a gem5-style `config.ini` file describing every registered
    /// metric. The current registry is a flat dot-path namespace, so
    /// every metric -- counter, histogram, label counter, formula --
    /// is listed in a single `[stats]` section keyed by its dot-path
    /// with `type=` and `desc=` annotations. Future slices that grow
    /// the SimObject tree will replace this with a per-section emit
    /// rooted on each object's canonical path (gem5 `[system.cpu0]`
    /// shape).
    pub fn emit_config_ini(
        registry: &dyn StatsRegistryRead,
        path: &Path,
    ) -> io::Result<()> {
        emit_config_ini_with_params(registry, &[], path)
    }

    /// Same as `emit_config_ini`, but also injects a list of
    /// per-SimObject parameter sections into the output. Each
    /// parameter section gets a leading `type = <kind>` line so a
    /// gem5-style consumer can distinguish object class parameters
    /// from registered metrics under the same `[system.<obj>]`
    /// header. Stats for the same section are merged in afterwards.
    ///
    /// `params` is an ordered slice of `(section_path, type_name,
    /// [(leaf, value)])` tuples; the writer emits them in the order
    /// provided so callers can preserve traversal order. Sections
    /// that overlap with metric paths share the same INI header.
    pub fn emit_config_ini_with_params(
        registry: &dyn StatsRegistryRead,
        params: &[(String, String, Vec<(String, String)>)],
        path: &Path,
    ) -> io::Result<()> {
        use std::collections::BTreeMap;
        // Bucket metrics into INI sections by their longest dot-prefix.
        // `system.cpu.mmu.tlb_hits` lands under `[system.cpu.mmu]`
        // with leaf key `tlb_hits`; root-scope metrics fall back to
        // `[stats]` so the file remains valid INI.
        let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        // Pre-seed sections with parameter blocks so `type = <kind>`
        // and class parameters appear before registered metrics on
        // the same object. Parameter values are emitted as bare INI
        // strings -- callers must pre-render them to text.
        for (section_path, type_name, entries) in params {
            let bucket = sections.entry(section_path.clone()).or_default();
            bucket.insert(
                "type".to_string(),
                type_name.clone(),
            );
            for (leaf, value) in entries {
                bucket.insert(leaf.clone(), value.clone());
            }
        }
        let mut push = |dot_path: &str, body: String| {
            let (section, leaf) = split_section(dot_path);
            sections
                .entry(section.to_string())
                .or_default()
                .insert(leaf.to_string(), body);
        };
        registry.for_each_counter(&mut |dot_path, _v, desc| {
            push(
                dot_path,
                format!("{{ type = counter, desc = \"{}\" }}", escape_ini(desc)),
            );
        });
        registry.for_each_histogram(&mut |dot_path, buckets, desc| {
            push(
                dot_path,
                format!(
                    "{{ type = histogram, buckets = {}, desc = \"{}\" }}",
                    buckets.len(),
                    escape_ini(desc)
                ),
            );
        });
        registry.for_each_label(&mut |dot_path, _snap, desc| {
            push(
                dot_path,
                format!("{{ type = label_counter, desc = \"{}\" }}", escape_ini(desc)),
            );
        });
        registry.for_each_formula(&mut |dot_path, _v, desc| {
            push(
                dot_path,
                format!("{{ type = formula, desc = \"{}\" }}", escape_ini(desc)),
            );
        });
        let mut file = File::create(path)?;
        writeln!(file, "; Generated by helm-report::emit_config_ini")?;
        for (section, entries) in &sections {
            writeln!(file, "[{section}]")?;
            for (leaf, body) in entries {
                writeln!(file, "{leaf} = {body}")?;
            }
            writeln!(file)?;
        }
        Ok(())
    }

    /// Split a dot-path into `(section, leaf)`. Section is everything
    /// up to the last `.`; if the path has no dot, section falls back
    /// to the literal `"stats"` so root-scope metrics land in a
    /// well-known section.
    fn split_section(path: &str) -> (&str, &str) {
        match path.rfind('.') {
            Some(idx) => (&path[..idx], &path[idx + 1..]),
            None => ("stats", path),
        }
    }

    /// Escape `"` and `\` in INI string values.
    fn escape_ini(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }

    /// Emit a gem5-style `config.json` file alongside `config.ini`.
    /// JSON keys mirror the INI section keys; downstream tooling can
    /// pick whichever it prefers.
    pub fn emit_config_json(
        registry: &dyn StatsRegistryRead,
        path: &Path,
    ) -> io::Result<()> {
        let mut entries: Vec<String> = Vec::new();
        let mut push = |path: &str, ty: &str, desc: &str, extra: &str| {
            // JSON-escape just enough for the description (quotes,
            // backslashes). This stays vendored to avoid pulling
            // serde_json transitively for the writer path.
            let desc = desc.replace('\\', "\\\\").replace('"', "\\\"");
            entries.push(format!(
                "  \"{path}\": {{ \"type\": \"{ty}\", \"desc\": \"{desc}\"{extra} }}"
            ));
        };
        registry.for_each_counter(&mut |p, _v, d| push(p, "counter", d, ""));
        registry.for_each_histogram(&mut |p, buckets, d| {
            let extra = format!(", \"buckets\": {}", buckets.len());
            push(p, "histogram", d, &extra);
        });
        registry.for_each_label(&mut |p, _s, d| push(p, "label_counter", d, ""));
        registry.for_each_formula(&mut |p, _v, d| push(p, "formula", d, ""));
        let mut file = File::create(path)?;
        writeln!(file, "{{")?;
        for (i, entry) in entries.iter().enumerate() {
            if i + 1 == entries.len() {
                writeln!(file, "{entry}")?;
            } else {
                writeln!(file, "{entry},")?;
            }
        }
        writeln!(file, "}}")?;
        Ok(())
    }

    /// Emit a gem5-style `stats.txt` block. Counters / histograms /
    /// label counters / formulas are rendered through
    /// `StatsRegistry::dump_text()` so the line shape stays in sync
    /// with the rest of the formatter pipeline.
    pub fn emit_stats_txt(
        registry: &dyn StatsRegistryRead,
        path: &Path,
    ) -> io::Result<()> {
        let mut file = File::create(path)?;
        writeln!(file, "---------- Begin Simulation Statistics ----------")?;
        registry.for_each_counter(&mut |p, v, d| {
            let _ = writeln!(file, "{p:<40}{v:<20}# {d}");
        });
        registry.for_each_histogram(&mut |p, buckets, d| {
            let total: u64 = buckets.iter().sum();
            for (i, count) in buckets.iter().enumerate() {
                let key = format!("{p}::bucket_{i}");
                let _ = writeln!(file, "{key:<40}{count:<20}# {d}");
            }
            let key = format!("{p}::total");
            let _ = writeln!(file, "{key:<40}{total:<20}# {d}");
        });
        registry.for_each_label(&mut |p, snap, d| {
            let total: u64 = snap.iter().map(|(_, v)| *v).sum();
            for (label, count) in snap {
                let key = format!("{p}::{label}");
                let _ = writeln!(file, "{key:<40}{count:<20}# {d}");
            }
            let key = format!("{p}::total");
            let _ = writeln!(file, "{key:<40}{total:<20}# {d}");
        });
        registry.for_each_formula(&mut |p, v, d| {
            let val = format!("{v:.6}");
            let _ = writeln!(file, "{p:<40}{val:<20}# {d}");
        });
        writeln!(file, "----------  End Simulation Statistics  ----------")?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tempfile::tempdir;

        #[test]
        fn emit_config_ini_creates_file_with_header() {
            let mut registry = helm_stats::StatsRegistry::new();
            registry.counter("system.cpu0.cycles", "cpu cycles").add(0);
            let dir = tempdir().unwrap();
            let path = dir.path().join("config.ini");
            emit_config_ini(&registry as &dyn StatsRegistryRead, &path).unwrap();
            let contents = std::fs::read_to_string(&path).unwrap();
            assert!(contents.contains("emit_config_ini"));
            // Section bucketed on the longest dot-prefix.
            assert!(contents.contains("[system.cpu0]"));
            // Leaf key inside the section.
            assert!(contents.contains("cycles = { type = counter"));
            // Description preserved.
            assert!(contents.contains("desc = \"cpu cycles\""));
        }

        #[test]
        fn emit_config_ini_groups_metrics_by_object_section() {
            let mut registry = helm_stats::StatsRegistry::new();
            registry.counter("system.cpu0.cycles", "cpu cycles").add(1);
            registry
                .counter("system.cpu0.insns_retired", "insns retired")
                .add(1);
            registry
                .counter("system.cpu0.mmu.tlb_hits", "tlb hits")
                .add(1);
            registry
                .counter("system.cpu0.mmu.tlb_misses", "tlb misses")
                .add(1);
            registry.counter("system.uart.tx_bytes", "tx").add(1);
            // Root-scope metric falls back to [stats].
            registry.counter("orphan", "orphan").add(0);
            let dir = tempdir().unwrap();
            let path = dir.path().join("config.ini");
            emit_config_ini(&registry as &dyn StatsRegistryRead, &path).unwrap();
            let contents = std::fs::read_to_string(&path).unwrap();

            // Required sections.
            for section in ["[stats]", "[system.cpu0]", "[system.cpu0.mmu]", "[system.uart]"] {
                assert!(
                    contents.contains(section),
                    "missing {section} in config.ini:\n{contents}"
                );
            }
            // Section ordering is lexical.
            let stats_pos = contents.find("[stats]").unwrap();
            let cpu0_pos = contents.find("[system.cpu0]").unwrap();
            let mmu_pos = contents.find("[system.cpu0.mmu]").unwrap();
            let uart_pos = contents.find("[system.uart]").unwrap();
            assert!(stats_pos < cpu0_pos);
            assert!(cpu0_pos < mmu_pos);
            assert!(mmu_pos < uart_pos);
            // No fully-qualified `system.cpu0.cycles =` lines outside
            // their section -- leaves are local now.
            assert!(!contents.contains("system.cpu0.cycles ="));
            // Leaf survives.
            assert!(contents.contains("cycles = { type = counter"));
        }

        #[test]
        fn emit_config_ini_with_params_includes_object_parameters() {
            // Two children: a Cpu-style block and a PL011-style stub.
            // The system root block carries timing/mode knobs.
            let mut registry = helm_stats::StatsRegistry::new();
            registry
                .counter("system.cpu0.commit.cycles", "cycles")
                .add(0);
            let params: Vec<(String, String, Vec<(String, String)>)> = vec![
                (
                    "system".to_string(),
                    "System".to_string(),
                    vec![
                        ("timing".to_string(), "virtual".to_string()),
                        ("num_cpus".to_string(), "2".to_string()),
                    ],
                ),
                (
                    "system.cpu0".to_string(),
                    "Cpu".to_string(),
                    vec![
                        ("isa".to_string(), "aarch64".to_string()),
                        ("model".to_string(), "perf".to_string()),
                    ],
                ),
                (
                    "system.uart".to_string(),
                    "Pl011".to_string(),
                    vec![],
                ),
            ];
            let dir = tempdir().unwrap();
            let path = dir.path().join("config.ini");
            emit_config_ini_with_params(
                &registry as &dyn StatsRegistryRead,
                &params,
                &path,
            )
            .unwrap();
            let contents = std::fs::read_to_string(&path).unwrap();
            // Per-object headers exist.
            for section in ["[system]", "[system.cpu0]", "[system.uart]"] {
                assert!(
                    contents.contains(section),
                    "missing {section} in config.ini:\n{contents}"
                );
            }
            // Type rows present.
            assert!(contents.contains("type = System"));
            assert!(contents.contains("type = Cpu"));
            assert!(contents.contains("type = Pl011"));
            // Parameter values folded in.
            assert!(contents.contains("timing = virtual"));
            assert!(contents.contains("num_cpus = 2"));
            assert!(contents.contains("isa = aarch64"));
            assert!(contents.contains("model = perf"));
            // Stats leaf still emitted under its section.
            assert!(
                contents.contains("[system.cpu0.commit]"),
                "missing commit section:\n{contents}"
            );
        }

        #[test]
        fn emit_stats_txt_creates_file_with_markers() {
            let mut registry = helm_stats::StatsRegistry::new();
            registry.counter("system.cpu0.cycles", "cpu cycles").add(42);
            let dir = tempdir().unwrap();
            let path = dir.path().join("stats.txt");
            emit_stats_txt(&registry as &dyn StatsRegistryRead, &path).unwrap();
            let contents = std::fs::read_to_string(&path).unwrap();
            assert!(contents.contains("Begin Simulation Statistics"));
            assert!(contents.contains("End Simulation Statistics"));
            // Counter emitted with its value and description.
            assert!(
                contents.contains("system.cpu0.cycles") && contents.contains("42"),
                "missing system.cpu0.cycles=42 line:\n{contents}"
            );
            assert!(contents.contains("# cpu cycles"));
        }

        #[test]
        fn emit_config_json_lists_every_metric_kind() {
            let mut registry = helm_stats::StatsRegistry::new();
            registry.counter("c", "counter").add(1);
            let h = registry.histogram("h", "histogram", &[2, 4]);
            h.record(1);
            let l = registry.label_counter("l", "labels");
            l.bump_static("a");
            let dir = tempdir().unwrap();
            let path = dir.path().join("config.json");
            emit_config_json(&registry as &dyn StatsRegistryRead, &path).unwrap();
            let contents = std::fs::read_to_string(&path).unwrap();
            assert!(contents.contains("\"c\": { \"type\": \"counter\""));
            assert!(contents.contains("\"h\": { \"type\": \"histogram\""));
            assert!(contents.contains("\"l\": { \"type\": \"label_counter\""));
            // Closes with a balanced `}`.
            assert!(contents.trim_end().ends_with("}"));
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
