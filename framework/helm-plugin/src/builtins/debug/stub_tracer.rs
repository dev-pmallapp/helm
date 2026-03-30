//! Stub instruction tracer — identifies unimplemented instructions that are
//! silently skipped during execution.
//!
//! This is the primary debugging tool for diagnosing "why is my binary stuck"
//! situations. It captures the first N unique stub instruction encodings and
//! reports them at atexit, sorted by frequency.

use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::{HelmPluginRegistry, PluginInsnInfo};
use std::collections::HashMap;
use std::sync::Mutex;

pub struct StubTracer {
    stubs: &'static Mutex<StubData>,
    max_unique: usize,
}

struct StubData {
    /// (opcode_name, raw_encoding) → count
    by_name: HashMap<&'static str, u64>,
    /// raw_encoding → (opcode_name, first_pc, count)
    by_encoding: HashMap<u32, (&'static str, u64, u64)>,
    total_stubs: u64,
    total_insns: u64,
}

impl StubTracer {
    pub fn new() -> Self {
        Self {
            stubs: Box::leak(Box::new(Mutex::new(StubData {
                by_name: HashMap::new(),
                by_encoding: HashMap::new(),
                total_stubs: 0,
                total_insns: 0,
            }))),
            max_unique: 50,
        }
    }
}

impl Default for StubTracer {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for StubTracer {
    fn name(&self) -> &str {
        "stub-tracer"
    }

    fn install(&mut self, reg: &mut HelmPluginRegistry, args: &HelmPluginArgs) {
        self.max_unique = args.get("max").and_then(|v| v.parse().ok()).unwrap_or(50);
        let data = self.stubs;
        let max = self.max_unique;

        reg.on_insn_exec(Box::new(move |_vcpu, insn: &PluginInsnInfo| {
            let mut d = data.lock().unwrap();
            d.total_insns += 1;

            if insn.is_stub {
                d.total_stubs += 1;
                *d.by_name.entry(insn.opcode_name).or_insert(0) += 1;

                if d.by_encoding.len() < max {
                    let entry =
                        d.by_encoding
                            .entry(insn.raw)
                            .or_insert((insn.opcode_name, insn.pc, 0));
                    entry.2 += 1;
                } else if let Some(entry) = d.by_encoding.get_mut(&insn.raw) {
                    entry.2 += 1;
                }
            }
        }));
    }

    fn atexit(&mut self) {
        let d = self.stubs.lock().unwrap();

        if d.total_stubs == 0 {
            log::info!(
                "[stub-tracer] No stub instructions encountered in {} insns",
                d.total_insns
            );
            return;
        }

        let pct = d.total_stubs as f64 / d.total_insns.max(1) as f64 * 100.0;
        eprintln!("\n╔══ STUB TRACER REPORT ══════════════════════════════════════");
        eprintln!("║ Total instructions: {:>12}", d.total_insns);
        eprintln!("║ Stub (no-op) insns: {:>12}  ({pct:.1}%)", d.total_stubs);
        eprintln!("╠══ By category ═════════════════════════════════════════════");

        let mut by_name: Vec<_> = d.by_name.iter().collect();
        by_name.sort_by(|a, b| b.1.cmp(a.1));
        for (name, count) in &by_name {
            let p = **count as f64 / d.total_stubs as f64 * 100.0;
            eprintln!("║  {name:<20} {count:>10}  ({p:5.1}%)");
        }

        eprintln!("╠══ Top unique encodings ════════════════════════════════════");
        let mut by_enc: Vec<_> = d.by_encoding.iter().collect();
        by_enc.sort_by(|a, b| b.1 .2.cmp(&a.1 .2));
        for (raw, (name, first_pc, count)) in by_enc.iter().take(20) {
            eprintln!("║  {raw:#010x}  {name:<16} first@{first_pc:#010x}  count={count}");
        }
        eprintln!("╚═══════════════════════════════════════════════════════════\n");
    }
}

#[cfg(test)]
#[path = "tests/stub_tracer.rs"]
mod tests;
