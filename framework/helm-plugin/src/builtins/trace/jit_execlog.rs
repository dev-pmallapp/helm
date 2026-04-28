//! JIT execution trace logger -- records block-level dispatch events.
//!
//! # Arguments
//!
//! - `max`:       Maximum number of entries to record (default: unlimited).
//! - `tail`:      If true, keep only the last `max` entries (ring buffer).
//! - `pc_start`:  Only record blocks whose start PC >= this value.
//! - `pc_end`:    Only record blocks whose start PC < this value.
//! - `show_exit`: If true, include exit code in each entry.
//! - `regs`:      If true, include register dump in each entry.

use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::HelmPluginRegistry;
use crate::{ArchContext, JitBlockInfo};
use std::sync::{Arc, Mutex};

struct JitExecLogConfig {
    max: usize,
    tail: bool,
    show_exit: bool,
    show_regs: bool,
    pc_start: Option<u64>,
    pc_end: Option<u64>,
}

pub struct JitExecLog {
    lines: Arc<Mutex<Vec<String>>>,
    pc_start: Option<u64>,
    pc_end: Option<u64>,
    max: usize,
    tail: bool,
}

impl JitExecLog {
    pub fn new() -> Self {
        Self {
            lines: Arc::new(Mutex::new(Vec::new())),
            pc_start: None,
            pc_end: None,
            max: usize::MAX,
            tail: false,
        }
    }

    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

impl Default for JitExecLog {
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

fn format_regs(ctx: &ArchContext) -> String {
    match ctx {
        ArchContext::Aarch64 {
            x,
            sp,
            pc,
            nzcv,
            current_el,
            ..
        } => {
            let mut s = format!(" sp={sp:#018x} nzcv={nzcv:#010x} el={current_el} pc={pc:#018x}");
            for (i, val) in x.iter().enumerate() {
                if *val != 0 {
                    s.push_str(&format!(" x{i}={val:#x}"));
                }
            }
            s
        }
        _ => String::new(),
    }
}

impl HelmPlugin for JitExecLog {
    fn name(&self) -> &str {
        "jit_execlog"
    }

    fn install(&mut self, reg: &mut HelmPluginRegistry, args: &HelmPluginArgs) {
        let config = JitExecLogConfig {
            max: args.get_usize("max").unwrap_or(usize::MAX),
            tail: args.get_bool("tail").unwrap_or(false),
            show_exit: args.get_bool("show_exit").unwrap_or(false),
            show_regs: args.get_bool("regs").unwrap_or(false),
            pc_start: parse_u64_arg(args, "pc_start"),
            pc_end: parse_u64_arg(args, "pc_end"),
        };
        self.pc_start = config.pc_start;
        self.pc_end = config.pc_end;
        self.max = config.max;
        self.tail = config.tail;
        let lines = Arc::clone(&self.lines);

        reg.on_jit_block(Box::new(move |info: &JitBlockInfo| {
            if config.pc_start.is_some_and(|s| info.pc < s) {
                return;
            }
            if config.pc_end.is_some_and(|e| info.pc >= e) {
                return;
            }
            let mut guard = lines.lock().unwrap();
            if config.max == 0 {
                return;
            }
            let mut entry = format!(
                "pc={:#018x}->{:#018x} retired={}",
                info.pc, info.next_pc, info.insns_retired
            );
            if config.show_exit {
                entry.push_str(&format!(" exit={:#x}", info.exit_code));
            }
            if config.show_regs {
                entry.push_str(&format_regs(&info.context));
            }
            if config.tail {
                if guard.len() >= config.max {
                    guard.remove(0);
                }
            } else if guard.len() >= config.max {
                return;
            }
            guard.push(entry);
        }));
    }

    fn atexit(&mut self) {
        let guard = self.lines.lock().unwrap();
        if guard.is_empty() {
            return;
        }
        let mut config_parts = Vec::new();
        if let Some(s) = self.pc_start {
            config_parts.push(format!("pc_start={s:#x}"));
        }
        if let Some(e) = self.pc_end {
            config_parts.push(format!("pc_end={e:#x}"));
        }
        if self.max < usize::MAX {
            config_parts.push(format!("max={}", self.max));
        }
        if self.tail {
            config_parts.push("tail".to_string());
        }
        let config_str = if config_parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", config_parts.join(", "))
        };
        eprintln!("[jit-execlog] {} blocks recorded{config_str}", guard.len());
        for line in guard.iter() {
            eprintln!("[jit-execlog] {line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_info(pc: u64, next_pc: u64, retired: u32) -> JitBlockInfo {
        JitBlockInfo {
            pc,
            next_pc,
            insns_retired: retired,
            exit_code: 0,
            context: ArchContext::None,
        }
    }

    #[test]
    fn records_block_within_window() {
        let mut log = JitExecLog::new();
        let mut reg = HelmPluginRegistry::new();
        log.install(&mut reg, &HelmPluginArgs::parse("pc_start=0x1000,pc_end=0x2000"));
        reg.fire_jit_block(&make_info(0x1000, 0x1010, 4));
        assert_eq!(log.lines().len(), 1);
    }

    #[test]
    fn filters_block_outside_window() {
        let mut log = JitExecLog::new();
        let mut reg = HelmPluginRegistry::new();
        log.install(&mut reg, &HelmPluginArgs::parse("pc_start=0x1000"));
        reg.fire_jit_block(&make_info(0x500, 0x510, 4));
        assert_eq!(log.lines().len(), 0);
    }

    #[test]
    fn respects_max_limit() {
        let mut log = JitExecLog::new();
        let mut reg = HelmPluginRegistry::new();
        log.install(&mut reg, &HelmPluginArgs::parse("max=3"));
        for i in 0..5 {
            reg.fire_jit_block(&make_info(0x1000 + i * 4, 0x1004 + i * 4, 1));
        }
        assert_eq!(log.lines().len(), 3);
    }

    #[test]
    fn tail_mode_keeps_last_n() {
        let mut log = JitExecLog::new();
        let mut reg = HelmPluginRegistry::new();
        log.install(&mut reg, &HelmPluginArgs::parse("max=3,tail=true"));
        for i in 0..5u64 {
            reg.fire_jit_block(&make_info(0x1000 + i * 4, 0x1004 + i * 4, 1));
        }
        let lines = log.lines();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("0x0000000000001008"));
    }
}
