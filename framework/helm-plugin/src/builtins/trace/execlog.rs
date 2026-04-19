use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::HelmPluginRegistry;
use std::sync::{Arc, Mutex};

/// Execution trace logger — records "vcpu PC raw" lines up to `max`.
pub struct ExecLog {
    lines: Arc<Mutex<Vec<String>>>,
}

impl ExecLog {
    pub fn new() -> Self {
        Self {
            lines: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Return all recorded trace lines.
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

impl Default for ExecLog {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_u64_arg(args: &HelmPluginArgs, key: &str) -> Option<u64> {
    args.get(key).and_then(|raw| {
        raw.strip_prefix("0x")
            .map(|hex| u64::from_str_radix(hex, 16).ok())
            .unwrap_or_else(|| raw.parse::<u64>().ok())
    })
}

impl HelmPlugin for ExecLog {
    fn name(&self) -> &str {
        "execlog"
    }

    fn install(&mut self, reg: &mut HelmPluginRegistry, args: &HelmPluginArgs) {
        let max = args.get_usize("max").unwrap_or(usize::MAX);
        let tail = args.get_bool("tail").unwrap_or(false);
        let show_regs = args.get_bool("regs").unwrap_or(false);
        let pc_filter = parse_u64_arg(args, "pc");
        let pc_start = parse_u64_arg(args, "pc_start");
        let el_filter: Option<u8> = args.get("el").and_then(|v| v.parse().ok());
        let pc_end = parse_u64_arg(args, "pc_end");
        let tpidrro_filter = parse_u64_arg(args, "tpidrro");
        let lines = Arc::clone(&self.lines);

        reg.on_insn_exec(Box::new(move |vcpu_idx, insn| {
            if pc_filter.is_some_and(|pc| insn.pc != pc) {
                return;
            }
            if pc_start.is_some_and(|pc| insn.pc < pc) {
                return;
            }
            if let Some(el) = el_filter {
                if let crate::runtime::ArchContext::Aarch64 { current_el, .. } = &insn.context {
                    if *current_el != el {
                        return;
                    }
                }
            }
            if let Some(expected_tpidrro) = tpidrro_filter {
                if let crate::runtime::ArchContext::Aarch64 { tpidrro_el0, .. } = &insn.context {
                    if *tpidrro_el0 != expected_tpidrro {
                        return;
                    }
                }
            }
            if pc_end.is_some_and(|pc| insn.pc >= pc) {
                return;
            }
            let mut guard = lines.lock().unwrap();
            if max == 0 {
                return;
            }
            let mut entry = format!(
                "vcpu={} pc={:#018x} raw={:#010x}",
                vcpu_idx, insn.pc, insn.raw
            );
            if show_regs {
                match &insn.context {
                    crate::runtime::ArchContext::Aarch64 {
                        x,
                        sp,
                        pc: _,
                        nzcv,
                        current_el,
                        tpidrro_el0,
                    } => {
                        entry.push_str(&format!(
                            "  sp={:#018x} nzcv={:#010x} el={} tpidrro_el0={:#018x}",
                            sp, nzcv, current_el, tpidrro_el0
                        ));
                        for (i, r) in x.iter().enumerate() {
                            if *r != 0 {
                                entry.push_str(&format!(" x{}={:#x}", i, r));
                            }
                        }
                    }
                    crate::runtime::ArchContext::RiscV { x, pc: _ } => {
                        for (i, r) in x.iter().enumerate() {
                            if *r != 0 {
                                entry.push_str(&format!(" x{}={:#x}", i, r));
                            }
                        }
                    }
                    crate::runtime::ArchContext::None => {}
                }
            }
            if tail {
                if guard.len() >= max {
                    guard.remove(0);
                }
            } else if guard.len() >= max {
                return;
            }
            guard.push(entry);
        }));
    }

    fn atexit(&mut self) {
        let guard = self.lines.lock().unwrap();
        for line in guard.iter() {
            eprintln!("[execlog] {}", line);
        }
    }
}

#[cfg(test)]
#[path = "tests/execlog.rs"]
mod tests;
