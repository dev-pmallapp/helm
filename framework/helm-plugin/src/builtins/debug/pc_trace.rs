use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::{ArchContext, FaultInfo, HelmPluginRegistry, InsnClass, MemInfo};
use helm_diag::sim_info;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegMode {
    Delta,
    Full,
    None,
}

impl RegMode {
    fn parse(args: &HelmPluginArgs) -> Self {
        match args.get_or("regs", "delta") {
            "full" => Self::Full,
            "none" => Self::None,
            _ => Self::Delta,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TraceMemFilter {
    None,
    All,
    Reads,
    Writes,
}

impl TraceMemFilter {
    fn parse(args: &HelmPluginArgs) -> Self {
        match args.get_or("mem", "all") {
            "none" => Self::None,
            "reads" => Self::Reads,
            "writes" => Self::Writes,
            _ => Self::All,
        }
    }

    fn matches(self, is_store: bool) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Reads => !is_store,
            Self::Writes => is_store,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DumpMode {
    on_fault: bool,
    on_exit: bool,
}

impl DumpMode {
    fn parse(args: &HelmPluginArgs) -> Self {
        match args.get_or("dump", "both") {
            "fault" => Self {
                on_fault: true,
                on_exit: false,
            },
            "atexit" => Self {
                on_fault: false,
                on_exit: true,
            },
            _ => Self {
                on_fault: true,
                on_exit: true,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FaultScope {
    Any,
    Match,
}

impl FaultScope {
    fn parse(args: &HelmPluginArgs) -> Self {
        match args.get_or("fault", "any") {
            "match" => Self::Match,
            _ => Self::Any,
        }
    }
}

#[derive(Clone, Debug)]
struct Aarch64Snapshot {
    x: [u64; 31],
    sp: u64,
    nzcv: u32,
    current_el: u8,
    tpidrro_el0: u64,
}

#[derive(Clone, Debug)]
struct TracedMem {
    pc: u64,
    raw: u32,
    opcode_name: &'static str,
    class: InsnClass,
    vaddr: u64,
    paddr: u64,
    size: u8,
    is_store: bool,
    is_atomic: bool,
    value_before: Option<u64>,
    value_after: Option<u64>,
}

#[derive(Clone, Debug)]
struct TracedHit {
    hit_index: usize,
    vcpu_idx: usize,
    pc: u64,
    raw: u32,
    opcode_name: &'static str,
    class: InsnClass,
    context_summary: String,
    mem: Vec<TracedMem>,
}

#[derive(Clone, Debug, Default)]
struct PendingMemBatch {
    pc: u64,
    raw: u32,
    mem: Vec<TracedMem>,
}

#[derive(Clone, Debug)]
struct DumpSnapshot {
    filter_desc: String,
    reason: &'static str,
    fault_pc: Option<u64>,
    fault_kind: Option<String>,
    fault_message: Option<String>,
    hits: Vec<TracedHit>,
}

struct Inner {
    pc: Option<u64>,
    pc_start: Option<u64>,
    pc_end: Option<u64>,
    max_hits: usize,
    max_mem_per_hit: usize,
    reg_mode: RegMode,
    mem_filter: TraceMemFilter,
    dump_mode: DumpMode,
    fault_scope: FaultScope,
    hits: Vec<TracedHit>,
    last_aarch64_by_vcpu: HashMap<usize, Aarch64Snapshot>,
    pending_mem_by_vcpu: HashMap<usize, PendingMemBatch>,
    dumped: bool,
}

impl Inner {
    fn new() -> Self {
        Self {
            pc: None,
            pc_start: None,
            pc_end: None,
            max_hits: 32,
            max_mem_per_hit: 4,
            reg_mode: RegMode::Delta,
            mem_filter: TraceMemFilter::All,
            dump_mode: DumpMode {
                on_fault: true,
                on_exit: true,
            },
            fault_scope: FaultScope::Any,
            hits: Vec::new(),
            last_aarch64_by_vcpu: HashMap::new(),
            pending_mem_by_vcpu: HashMap::new(),
            dumped: false,
        }
    }

    fn install_args(&mut self, args: &HelmPluginArgs) {
        self.pc = parse_u64_arg(args, "pc");
        self.pc_start = parse_u64_arg(args, "pc_start");
        self.pc_end = parse_u64_arg(args, "pc_end");
        self.max_hits = args.get_usize("max").unwrap_or(32).max(1);
        self.max_mem_per_hit = args.get_usize("mem-max").unwrap_or(4);
        self.reg_mode = RegMode::parse(args);
        self.mem_filter = TraceMemFilter::parse(args);
        self.dump_mode = DumpMode::parse(args);
        self.fault_scope = FaultScope::parse(args);
        self.hits.clear();
        self.last_aarch64_by_vcpu.clear();
        self.pending_mem_by_vcpu.clear();
        self.dumped = false;
    }

    fn enabled(&self) -> bool {
        self.pc.is_some() || self.pc_start.is_some() || self.pc_end.is_some()
    }

    fn matches_pc(&self, pc: u64) -> bool {
        if !self.enabled() {
            return false;
        }
        if self.pc.is_some_and(|expected| pc != expected) {
            return false;
        }
        if self.pc_start.is_some_and(|start| pc < start) {
            return false;
        }
        if self.pc_end.is_some_and(|end| pc >= end) {
            return false;
        }
        true
    }

    fn filter_desc(&self) -> String {
        if let Some(pc) = self.pc {
            return format!("pc={pc:#018x}");
        }
        match (self.pc_start, self.pc_end) {
            (Some(start), Some(end)) => format!("pc in [{start:#018x}, {end:#018x})"),
            (Some(start), None) => format!("pc>={start:#018x}"),
            (None, Some(end)) => format!("pc<{end:#018x}"),
            (None, None) => "disabled".to_string(),
        }
    }

    fn record_hit(
        &mut self,
        vcpu_idx: usize,
        pc: u64,
        raw: u32,
        opcode_name: &'static str,
        class: InsnClass,
        context: &ArchContext,
    ) {
        if self.hits.len() >= self.max_hits {
            return;
        }
        let context_summary = match context {
            ArchContext::Aarch64 {
                x,
                sp,
                pc: _,
                nzcv,
                current_el,
                tpidrro_el0,
            } => {
                let previous = self.last_aarch64_by_vcpu.get(&vcpu_idx);
                let summary = summarize_aarch64(
                    x,
                    *sp,
                    *nzcv,
                    *current_el,
                    *tpidrro_el0,
                    self.reg_mode,
                    previous,
                );
                self.last_aarch64_by_vcpu.insert(
                    vcpu_idx,
                    Aarch64Snapshot {
                        x: *x,
                        sp: *sp,
                        nzcv: *nzcv,
                        current_el: *current_el,
                        tpidrro_el0: *tpidrro_el0,
                    },
                );
                summary
            }
            ArchContext::RiscV { x, pc: _ } => summarize_riscv(x, self.reg_mode),
            ArchContext::None => String::new(),
        };

        let hit_index = self.hits.len() + 1;
        let mem = self.take_pending_mem(vcpu_idx, pc, raw);
        self.hits.push(TracedHit {
            hit_index,
            vcpu_idx,
            pc,
            raw,
            opcode_name,
            class,
            context_summary,
            mem,
        });
    }

    fn record_mem(&mut self, vcpu_idx: usize, info: &MemInfo) {
        if !self.mem_filter.matches(info.is_store) {
            return;
        }
        self.push_pending_mem(vcpu_idx, info);
    }

    fn push_pending_mem(&mut self, vcpu_idx: usize, info: &MemInfo) {
        let batch = self.pending_mem_by_vcpu.entry(vcpu_idx).or_default();
        if batch.pc != info.pc || batch.raw != info.raw {
            batch.pc = info.pc;
            batch.raw = info.raw;
            batch.mem.clear();
        }
        if batch.mem.len() >= self.max_mem_per_hit {
            return;
        }
        batch.mem.push(TracedMem {
            pc: info.pc,
            raw: info.raw,
            opcode_name: info.opcode_name,
            class: info.class,
            vaddr: info.vaddr,
            paddr: info.paddr,
            size: info.size,
            is_store: info.is_store,
            is_atomic: info.is_atomic,
            value_before: info.value_before,
            value_after: info.value_after,
        });
    }

    fn take_pending_mem(&mut self, vcpu_idx: usize, pc: u64, raw: u32) -> Vec<TracedMem> {
        let Some(batch) = self.pending_mem_by_vcpu.get_mut(&vcpu_idx) else {
            return Vec::new();
        };
        if batch.pc != pc || batch.raw != raw {
            return Vec::new();
        }
        std::mem::take(&mut batch.mem)
    }

    fn prepare_dump(
        &mut self,
        reason: &'static str,
        fault_pc: Option<u64>,
        fault_kind: Option<String>,
        fault_message: Option<String>,
    ) -> Option<DumpSnapshot> {
        if self.dumped || self.hits.is_empty() {
            return None;
        }
        self.dumped = true;
        Some(DumpSnapshot {
            filter_desc: self.filter_desc(),
            reason,
            fault_pc,
            fault_kind,
            fault_message,
            hits: self.hits.clone(),
        })
    }
}

/// Debug plugin: capture repeated executions at a specific PC and correlate
/// them with the memory accesses produced by that instruction.
pub struct PcTrace {
    inner: Arc<Mutex<Inner>>,
}

impl PcTrace {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new())),
        }
    }
}

impl Default for PcTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for PcTrace {
    fn name(&self) -> &str {
        "pc_trace"
    }

    fn install(&mut self, reg: &mut HelmPluginRegistry, args: &HelmPluginArgs) {
        self.inner = Arc::new(Mutex::new(Inner::new()));
        self.inner.lock().unwrap().install_args(args);

        let inner_insn = Arc::clone(&self.inner);
        reg.on_insn_exec(Box::new(move |vcpu_idx, insn| {
            let mut guard = inner_insn.lock().unwrap();
            if !guard.matches_pc(insn.pc) {
                return;
            }
            guard.record_hit(
                vcpu_idx,
                insn.pc,
                insn.raw,
                insn.opcode_name,
                insn.class,
                &insn.context,
            );
        }));

        let inner_mem = Arc::clone(&self.inner);
        reg.on_mem_access(
            crate::runtime::MemFilter::All,
            Box::new(move |vcpu_idx, info| {
                let mut guard = inner_mem.lock().unwrap();
                if !guard.matches_pc(info.pc) {
                    return;
                }
                guard.record_mem(vcpu_idx, info);
            }),
        );

        let inner_fault = Arc::clone(&self.inner);
        reg.on_fault(Box::new(move |fault: &FaultInfo| {
            let snapshot = {
                let mut guard = inner_fault.lock().unwrap();
                if !guard.dump_mode.on_fault {
                    None
                } else if guard.fault_scope == FaultScope::Match && !guard.matches_pc(fault.pc) {
                    None
                } else {
                    guard.prepare_dump(
                        "fault",
                        Some(fault.pc),
                        Some(fault.kind.to_string()),
                        Some(fault.message.clone()),
                    )
                }
            };
            if let Some(snapshot) = snapshot {
                emit_dump(&snapshot);
            }
        }));
    }

    fn atexit(&mut self) {
        let snapshot = {
            let mut guard = self.inner.lock().unwrap();
            if !guard.dump_mode.on_exit {
                None
            } else {
                guard.prepare_dump("atexit", None, None, None)
            }
        };
        if let Some(snapshot) = snapshot {
            emit_dump(&snapshot);
        }
    }
}

fn parse_u64_arg(args: &HelmPluginArgs, key: &str) -> Option<u64> {
    args.get(key).and_then(|raw| {
        raw.strip_prefix("0x")
            .map(|hex| u64::from_str_radix(hex, 16).ok())
            .unwrap_or_else(|| raw.parse::<u64>().ok())
    })
}

fn summarize_aarch64(
    x: &[u64; 31],
    sp: u64,
    nzcv: u32,
    current_el: u8,
    tpidrro_el0: u64,
    reg_mode: RegMode,
    previous: Option<&Aarch64Snapshot>,
) -> String {
    let mut parts = vec![
        format!("sp={sp:#018x}"),
        format!("nzcv={nzcv:#010x}"),
        format!("el={current_el}"),
    ];
    if tpidrro_el0 != 0 || previous.is_some_and(|prev| prev.tpidrro_el0 != tpidrro_el0) {
        parts.push(format!("tpidrro_el0={tpidrro_el0:#018x}"));
    }
    if previous.is_some_and(|prev| prev.sp != sp) {
        parts.push(format!("sp-delta={sp:#018x}"));
    }
    if previous.is_some_and(|prev| prev.nzcv != nzcv) {
        parts.push(format!("nzcv-delta={nzcv:#010x}"));
    }
    if previous.is_some_and(|prev| prev.current_el != current_el) {
        parts.push(format!("el-delta={current_el}"));
    }

    let regs = match reg_mode {
        RegMode::None => Vec::new(),
        RegMode::Full => x
            .iter()
            .enumerate()
            .filter(|(_, val)| **val != 0)
            .map(|(idx, val)| format!("x{idx}={val:#x}"))
            .collect(),
        RegMode::Delta => x
            .iter()
            .enumerate()
            .filter(|(idx, val)| previous.map_or(**val != 0, |prev| prev.x[*idx] != **val))
            .map(|(idx, val)| format!("x{idx}={val:#x}"))
            .collect(),
    };
    if !regs.is_empty() {
        parts.push(format!("regs=[{}]", regs.join(" ")));
    }
    parts.join(" ")
}

fn summarize_riscv(x: &[u64; 32], reg_mode: RegMode) -> String {
    if reg_mode == RegMode::None {
        return String::new();
    }
    let regs: Vec<_> = x
        .iter()
        .enumerate()
        .filter(|(_, val)| **val != 0)
        .map(|(idx, val)| format!("x{idx}={val:#x}"))
        .collect();
    if regs.is_empty() {
        String::new()
    } else {
        format!("regs=[{}]", regs.join(" "))
    }
}

fn emit_dump(snapshot: &DumpSnapshot) {
    sim_info!(
        component = "pc-trace",
        "reason={} filter={} hits={}{}{}{}",
        snapshot.reason,
        snapshot.filter_desc,
        snapshot.hits.len(),
        snapshot
            .fault_pc
            .map(|pc| format!(" fault_pc={pc:#018x}"))
            .unwrap_or_default(),
        snapshot
            .fault_kind
            .as_deref()
            .map(|kind| format!(" fault_kind={kind}"))
            .unwrap_or_default(),
        snapshot
            .fault_message
            .as_deref()
            .map(|msg| format!(" fault_msg={msg}"))
            .unwrap_or_default()
    );
    for hit in &snapshot.hits {
        sim_info!(
            component = "pc-trace",
            pc = hit.pc,
            "hit={} vcpu={} raw={:#010x} opcode={} class={:?} {}",
            hit.hit_index,
            hit.vcpu_idx,
            hit.raw,
            hit.opcode_name,
            hit.class,
            hit.context_summary
        );
        for (idx, access) in hit.mem.iter().enumerate() {
            let kind = if access.is_store { 'W' } else { 'R' };
            let atomic = if access.is_atomic { " atomic" } else { "" };
            sim_info!(
                component = "pc-trace",
                pc = access.pc,
                "  mem[{idx}] [{kind}] raw={:#010x} opcode={} class={:?} va={:#018x} pa={:#018x} size={}{} old={:?} new={:?}",
                access.raw,
                access.opcode_name,
                access.class,
                access.vaddr,
                access.paddr,
                access.size,
                atomic,
                access.value_before,
                access.value_after
            );
        }
    }
}

#[cfg(test)]
#[path = "tests/pc_trace.rs"]
mod tests;
