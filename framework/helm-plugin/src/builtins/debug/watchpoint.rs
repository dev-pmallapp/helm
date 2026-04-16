use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::{HelmPluginRegistry, InsnClass, MemFilter};
use helm_diag::{is_monitor_discarding, sim_info};
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
    match_paddr: bool,
    writes_only: bool,
    value: Option<u64>,
    hit_count: u64,
    log_limit: u64,
    window: usize,
    hits: Vec<WatchHit>,
    recent_insns: Vec<RecentInsn>,
    captured_insns: Vec<RecentInsn>,
    dump_on_fault: bool,
    dump_on_exit: bool,
    dumped: bool,
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
                match_paddr: false,
                writes_only: true,
                value: None,
                hit_count: 0,
                log_limit: 8,
                window: 32,
                hits: Vec::new(),
                recent_insns: Vec::new(),
                captured_insns: Vec::new(),
                dump_on_fault: true,
                dump_on_exit: true,
                dumped: false,
            })),
        }
    }

    pub fn with_addr(addr: u64, size: u64, writes_only: bool, value: Option<u64>) -> Self {
        Self {
            config: Arc::new(Mutex::new(WatchConfig {
                addr,
                size,
                match_paddr: false,
                writes_only,
                value,
                hit_count: 0,
                log_limit: 8,
                window: 32,
                hits: Vec::new(),
                recent_insns: Vec::new(),
                captured_insns: Vec::new(),
                dump_on_fault: true,
                dump_on_exit: true,
                dumped: false,
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
        // Parse args: addr=0x1000,size=8,type=write,value=0xDEAD,space=pa
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
        if let Some(space) = args.get("space") {
            self.config.lock().unwrap().match_paddr = space == "pa";
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
        if let Some(dump) = args.get("dump") {
            let mut guard = self.config.lock().unwrap();
            match dump {
                "fault" => {
                    guard.dump_on_fault = true;
                    guard.dump_on_exit = false;
                }
                "atexit" => {
                    guard.dump_on_fault = false;
                    guard.dump_on_exit = true;
                }
                _ => {
                    guard.dump_on_fault = true;
                    guard.dump_on_exit = true;
                }
            }
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
                let access_addr = if guard.match_paddr {
                    info.paddr
                } else {
                    info.vaddr
                };
                let access_end = access_addr + info.size as u64;
                let watch_end = guard.addr + guard.size;

                // Check overlap
                if access_addr < watch_end && access_end > guard.addr {
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
            let snapshot = {
                let mut guard = config.lock().unwrap();
                if !guard.dump_on_fault || guard.hit_count == 0 || guard.dumped {
                    None
                } else {
                    guard.dumped = true;
                    Some(WatchDump::from_config(&guard, "fault", Some(fault.pc), Some(fault.kind.to_string())))
                }
            };
            if let Some(snapshot) = snapshot {
                emit_watch_dump(&snapshot);
            }
        }));
    }

    fn atexit(&mut self) {
        let snapshot = {
            let mut guard = self.config.lock().unwrap();
            if !guard.dump_on_exit || guard.hit_count == 0 || guard.dumped {
                None
            } else {
                guard.dumped = true;
                Some(WatchDump::from_config(&guard, "atexit", None, None))
            }
        };
        if let Some(snapshot) = snapshot {
            emit_watch_dump(&snapshot);
        }
    }
}

#[derive(Clone, Debug)]
struct WatchDump {
    addr: u64,
    size: u64,
    hit_count: u64,
    reason: &'static str,
    fault_pc: Option<u64>,
    fault_kind: Option<String>,
    hits: Vec<WatchHit>,
    captured_insns: Vec<RecentInsn>,
}

impl WatchDump {
    fn from_config(
        guard: &WatchConfig,
        reason: &'static str,
        fault_pc: Option<u64>,
        fault_kind: Option<String>,
    ) -> Self {
        Self {
            addr: guard.addr,
            size: guard.size,
            hit_count: guard.hit_count,
            reason,
            fault_pc,
            fault_kind,
            hits: guard.hits.clone(),
            captured_insns: guard.captured_insns.clone(),
        }
    }
}

fn emit_watch_dump(dump: &WatchDump) {
    let mirror_stderr = is_monitor_discarding();
    sim_info!(
        component = "watchpoint",
        "reason={} addr={:#018x} size={} hits={}{}{}",
        dump.reason,
        dump.addr,
        dump.size,
        dump.hit_count,
        dump.fault_pc
            .map(|pc| format!(" fault_pc={pc:#018x}"))
            .unwrap_or_default(),
        dump.fault_kind
            .as_deref()
            .map(|kind| format!(" fault_kind={kind}"))
            .unwrap_or_default()
    );
    if mirror_stderr {
        eprintln!(
            "[watchpoint] reason={} addr={:#018x} size={} hits={}{}{}",
            dump.reason,
            dump.addr,
            dump.size,
            dump.hit_count,
            dump.fault_pc
                .map(|pc| format!(" fault_pc={pc:#018x}"))
                .unwrap_or_default(),
            dump.fault_kind
                .as_deref()
                .map(|kind| format!(" fault_kind={kind}"))
                .unwrap_or_default()
        );
    }
    for hit in &dump.hits {
        let kind = if hit.is_store { 'W' } else { 'R' };
        sim_info!(
            component = "watchpoint",
            pc = hit.pc,
            "hit={} raw={:#010x} opcode={} class={:?} [{}] va={:#018x} pa={:#018x} size={} old={:?} new={:?}",
            hit.hit_index,
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
        if mirror_stderr {
            eprintln!(
                "[watchpoint] pc={:#018x} hit={} raw={:#010x} opcode={} class={:?} [{}] va={:#018x} pa={:#018x} size={} old={:?} new={:?}",
                hit.pc,
                hit.hit_index,
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
    }
    if !dump.captured_insns.is_empty() {
        sim_info!(
            component = "watchpoint",
            "recent instructions before first hit ({})",
            dump.captured_insns.len()
        );
        if mirror_stderr {
            eprintln!(
                "[watchpoint] recent instructions before first hit ({})",
                dump.captured_insns.len()
            );
        }
        for (idx, insn) in dump.captured_insns.iter().enumerate() {
            sim_info!(
                component = "watchpoint",
                pc = insn.pc,
                "insn[{idx:02}] raw={:#010x} opcode={} class={:?}",
                insn.raw,
                insn.opcode_name,
                insn.class,
            );
            if mirror_stderr {
                eprintln!(
                    "[watchpoint] pc={:#018x} insn[{idx:02}] raw={:#010x} opcode={} class={:?}",
                    insn.pc,
                    insn.raw,
                    insn.opcode_name,
                    insn.class,
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/watchpoint.rs"]
mod tests;
