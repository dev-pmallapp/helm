use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::{HelmPluginRegistry, InsnClass, MemFilter};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct RecentInsn {
    vcpu_idx: usize,
    pc: u64,
    raw: u32,
    opcode_name: &'static str,
    class: InsnClass,
}

#[derive(Clone, Debug)]
struct WatchHit {
    hit_index: u64,
    pc: u64,
    raw: u32,
    opcode_name: &'static str,
    class: InsnClass,
    vaddr: u64,
    paddr: u64,
    size: u8,
    is_store: bool,
    value_before: Option<u64>,
    value_after: Option<u64>,
}

struct WatchConfig {
    addr: u64,
    size: u64,
    writes_only: bool,
    value: Option<u64>,
    hit_count: u64,
    log_limit: u64,
    window: usize,
    hits: Vec<WatchHit>,
    recent_insns: Vec<RecentInsn>,
    captured_insns: Vec<RecentInsn>,
}

/// Address watchpoint — fires a fault callback when a watched address is accessed.
pub struct Watchpoint {
    config: Arc<Mutex<WatchConfig>>,
}

impl Watchpoint {
    pub fn new() -> Self {
        Self {
            config: Arc::new(Mutex::new(WatchConfig {
                addr: 0,
                size: 8,
                writes_only: true,
                value: None,
                hit_count: 0,
                log_limit: 8,
                window: 32,
                hits: Vec::new(),
                recent_insns: Vec::new(),
                captured_insns: Vec::new(),
            })),
        }
    }

    pub fn with_addr(addr: u64, size: u64, writes_only: bool, value: Option<u64>) -> Self {
        Self {
            config: Arc::new(Mutex::new(WatchConfig {
                addr,
                size,
                writes_only,
                value,
                hit_count: 0,
                log_limit: 8,
                window: 32,
                hits: Vec::new(),
                recent_insns: Vec::new(),
                captured_insns: Vec::new(),
            })),
        }
    }
}

impl Default for Watchpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for Watchpoint {
    fn name(&self) -> &str {
        "watchpoint"
    }

    fn install(&mut self, reg: &mut HelmPluginRegistry, args: &HelmPluginArgs) {
        // Parse args: addr=0x1000,size=8,type=write,value=0xDEAD
        if let Some(addr_str) = args.get("addr") {
            let addr = if let Some(hex) = addr_str.strip_prefix("0x") {
                u64::from_str_radix(hex, 16).unwrap_or(0)
            } else {
                addr_str.parse::<u64>().unwrap_or(0)
            };
            let mut guard = self.config.lock().unwrap();
            guard.addr = addr;
        }
        if let Some(s) = args.get("size") {
            self.config.lock().unwrap().size = s.parse::<u64>().unwrap_or(8);
        }
        if let Some(ty) = args.get("type") {
            self.config.lock().unwrap().writes_only = ty != "all";
        }
        if let Some(val_str) = args.get("value") {
            let val = if let Some(hex) = val_str.strip_prefix("0x") {
                u64::from_str_radix(hex, 16).ok()
            } else {
                val_str.parse::<u64>().ok()
            };
            self.config.lock().unwrap().value = val;
        }
        if let Some(limit) = args.get("log-limit") {
            self.config.lock().unwrap().log_limit = limit.parse::<u64>().unwrap_or(8);
        }
        if let Some(window) = args.get("window") {
            self.config.lock().unwrap().window = window.parse::<usize>().unwrap_or(32);
        }

        let config = Arc::clone(&self.config);
        reg.on_insn_exec(Box::new(move |_vcpu_idx, insn| {
            let mut guard = config.lock().unwrap();
            guard.recent_insns.push(RecentInsn {
                vcpu_idx: _vcpu_idx,
                pc: insn.pc,
                raw: insn.raw,
                opcode_name: insn.opcode_name,
                class: insn.class,
            });
            let excess = guard.recent_insns.len().saturating_sub(guard.window);
            if excess > 0 {
                guard.recent_insns.drain(0..excess);
            }
        }));

        let config = Arc::clone(&self.config);
        let filter = if self.config.lock().unwrap().writes_only {
            MemFilter::WritesOnly
        } else {
            MemFilter::All
        };

        reg.on_mem_access(
            filter,
            Box::new(move |_vcpu_idx, info| {
                let mut guard = config.lock().unwrap();
                let access_end = info.vaddr + info.size as u64;
                let watch_end = guard.addr + guard.size;

                // Check overlap
                if info.vaddr < watch_end && access_end > guard.addr {
                    if let Some(expected) = guard.value {
                        let observed = if info.is_store {
                            info.value_after
                        } else {
                            info.value_before
                        };
                        if observed != Some(expected) {
                            return;
                        }
                    }
                    guard.hit_count += 1;
                    if guard.hits.len() < guard.log_limit as usize {
                        let hit_index = guard.hit_count;
                        guard.hits.push(WatchHit {
                            hit_index,
                            pc: info.pc,
                            raw: info.raw,
                            opcode_name: info.opcode_name,
                            class: info.class,
                            vaddr: info.vaddr,
                            paddr: info.paddr,
                            size: info.size,
                            is_store: info.is_store,
                            value_before: info.value_before,
                            value_after: info.value_after,
                        });
                    }
                    if guard.captured_insns.is_empty() {
                        guard.captured_insns = guard
                            .recent_insns
                            .iter()
                            .filter(|insn| insn.vcpu_idx == _vcpu_idx)
                            .cloned()
                            .collect();
                        guard.captured_insns.push(RecentInsn {
                            vcpu_idx: _vcpu_idx,
                            pc: info.pc,
                            raw: info.raw,
                            opcode_name: info.opcode_name,
                            class: info.class,
                        });
                    }
                }
            }),
        );

        let config = Arc::clone(&self.config);
        reg.on_fault(Box::new(move |fault| {
            let guard = config.lock().unwrap();
            if guard.hit_count == 0 {
                return;
            }
            eprintln!(
                "[watchpoint] fault pc={:#018x} kind={} watched_addr={:#018x} hits={}",
                fault.pc,
                fault.kind,
                guard.addr,
                guard.hit_count,
            );
            for hit in &guard.hits {
                let kind = if hit.is_store { 'W' } else { 'R' };
                eprintln!(
                    "[watchpoint]   hit={} pc={:#018x} raw={:#010x} opcode={} class={:?} [{}] va={:#018x} pa={:#018x} size={} old={:?} new={:?}",
                    hit.hit_index,
                    hit.pc,
                    hit.raw,
                    hit.opcode_name,
                    hit.class,
                    kind,
                    hit.vaddr,
                    hit.paddr,
                    hit.size,
                    hit.value_before,
                    hit.value_after,
                );
            }
            if !guard.captured_insns.is_empty() {
                eprintln!(
                    "[watchpoint] recent instructions before first hit ({}):",
                    guard.captured_insns.len()
                );
                for (idx, insn) in guard.captured_insns.iter().enumerate() {
                    eprintln!(
                        "[watchpoint]   insn[{idx:02}] pc={:#018x} raw={:#010x} opcode={} class={:?}",
                        insn.pc,
                        insn.raw,
                        insn.opcode_name,
                        insn.class,
                    );
                }
            }
        }));
    }

    fn atexit(&mut self) {
        let guard = self.config.lock().unwrap();
        eprintln!(
            "[watchpoint] addr={:#018x} size={} hits={}",
            guard.addr, guard.size, guard.hit_count
        );
        for hit in &guard.hits {
            let kind = if hit.is_store { 'W' } else { 'R' };
            eprintln!(
                "[watchpoint]   hit={} pc={:#018x} raw={:#010x} opcode={} class={:?} [{}] va={:#018x} pa={:#018x} size={} old={:?} new={:?}",
                hit.hit_index,
                hit.pc,
                hit.raw,
                hit.opcode_name,
                hit.class,
                kind,
                hit.vaddr,
                hit.paddr,
                hit.size,
                hit.value_before,
                hit.value_after,
            );
        }
        if !guard.captured_insns.is_empty() {
            eprintln!(
                "[watchpoint] recent instructions before first hit ({}):",
                guard.captured_insns.len()
            );
            for (idx, insn) in guard.captured_insns.iter().enumerate() {
                eprintln!(
                    "[watchpoint]   insn[{idx:02}] pc={:#018x} raw={:#010x} opcode={} class={:?}",
                    insn.pc, insn.raw, insn.opcode_name, insn.class,
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/watchpoint.rs"]
mod tests;
