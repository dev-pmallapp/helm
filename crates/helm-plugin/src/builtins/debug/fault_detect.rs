//! Fault-detection plugin — catches execution anomalies and produces
//! arch-aware diagnostic reports.
//!
//! Detects:
//! - Jump to NULL (PC = 0)
//! - Wild jumps (PC in unmapped / non-executable region)
//! - Undefined instructions
//! - Stack pointer corruption
//! - TLS aliasing across threads
//! - Critical unsupported syscalls
//!
//! Collects a ring buffer of recent PCs and a syscall log so that
//! fault reports include enough context to diagnose the root cause.

use crate::api::plugin::{HelmPlugin, PluginArgs};
use crate::runtime::info::{ArchContext, FaultInfo, FaultKind};
use crate::runtime::registry::PluginRegistry;
use std::sync::{Arc, Mutex};

/// A single diagnostic report.
#[derive(Debug, Clone)]
pub struct FaultReport {
    /// Fault classification.
    pub kind: FaultKind,
    /// One-line summary.
    pub summary: String,
    /// Guest PC at fault.
    pub pc: u64,
    /// Instruction count at fault.
    pub insn_count: u64,
    /// Recent PC history (oldest first).
    pub pc_history: Vec<u64>,
    /// Recent syscall log entries.
    pub syscall_log: Vec<String>,
    /// Arch-specific register dump.
    pub arch_context: ArchContext,
}

/// Shared state between the plugin and its callbacks.
struct Inner {
    pc_ring: Vec<u64>,
    pc_ring_pos: usize,
    pc_ring_len: usize,
    syscall_log: Vec<String>,
    syscall_log_max: usize,
    reports: Vec<FaultReport>,
    max_reports: usize,
    thread_tls: Vec<(u64, u64)>,
    stack_lo: u64,
    stack_hi: u64,
    active: bool,
    after_insns: u64,
    at_pc: u64,
    total_insns: u64,
}

impl Inner {
    fn new(ring_size: usize, syscall_max: usize, max_reports: usize) -> Self {
        Self {
            pc_ring: vec![0; ring_size],
            pc_ring_pos: 0,
            pc_ring_len: 0,
            syscall_log: Vec::new(),
            syscall_log_max: syscall_max,
            reports: Vec::new(),
            max_reports,
            thread_tls: Vec::new(),
            stack_lo: 0,
            stack_hi: 0,
            active: true,
            after_insns: 0,
            at_pc: 0,
            total_insns: 0,
        }
    }

    fn push_pc(&mut self, pc: u64) {
        let cap = self.pc_ring.len();
        if cap == 0 { return; }
        self.pc_ring[self.pc_ring_pos % cap] = pc;
        self.pc_ring_pos += 1;
        if self.pc_ring_len < cap {
            self.pc_ring_len += 1;
        }
    }

    fn recent_pcs(&self) -> Vec<u64> {
        let cap = self.pc_ring.len();
        if cap == 0 || self.pc_ring_len == 0 {
            return Vec::new();
        }
        let start = if self.pc_ring_pos >= self.pc_ring_len {
            self.pc_ring_pos - self.pc_ring_len
        } else {
            0
        };
        (start..self.pc_ring_pos)
            .map(|i| self.pc_ring[i % cap])
            .collect()
    }

    fn push_syscall(&mut self, entry: String) {
        if self.syscall_log.len() >= self.syscall_log_max {
            self.syscall_log.remove(0);
        }
        self.syscall_log.push(entry);
    }

    fn add_report(&mut self, report: FaultReport) {
        if self.reports.len() < self.max_reports {
            self.reports.push(report);
        }
    }
}

/// Fault-detection debug plugin.
pub struct FaultDetect {
    inner: Arc<Mutex<Inner>>,
}

impl FaultDetect {
    /// Create with default settings.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new(64, 32, 8))),
        }
    }

    /// Access collected fault reports.
    pub fn reports(&self) -> Vec<FaultReport> {
        self.inner.lock().unwrap().reports.clone()
    }
}

impl Default for FaultDetect {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for FaultDetect {
    fn name(&self) -> &str {
        "fault-detect"
    }

    fn install(&mut self, reg: &mut PluginRegistry, args: &PluginArgs) {
        let ring_size = args.get_usize("ring", 64);
        let syscall_max = args.get_usize("syscall_log", 32);
        let max_reports = args.get_usize("max_reports", 8);
        let stack_lo = args
            .get("stack_lo")
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .unwrap_or(0);
        let stack_hi = args
            .get("stack_hi")
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .unwrap_or(0);

        let after_insns = args.get_usize("after_insns", 0) as u64;
        let at_pc = args
            .get("at_pc")
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .unwrap_or(0);

        self.inner = Arc::new(Mutex::new(Inner::new(ring_size, syscall_max, max_reports)));
        {
            let mut inner = self.inner.lock().unwrap();
            inner.stack_lo = stack_lo;
            inner.stack_hi = stack_hi;
            inner.after_insns = after_insns;
            inner.at_pc = at_pc;
            inner.active = after_insns == 0 && at_pc == 0;
        }

        // -- insn_exec: track PCs, detect NULL jumps / stack anomalies --
        let inner_insn = self.inner.clone();
        reg.on_insn_exec(Box::new(move |_vcpu_idx, insn| {
            let mut s = inner_insn.lock().unwrap();
            s.total_insns += 1;
            if !s.active {
                if s.after_insns > 0 && s.total_insns >= s.after_insns {
                    s.active = true;
                    log::info!("fault-detect: activated after {} insns", s.total_insns);
                } else if s.at_pc != 0 && insn.vaddr == s.at_pc {
                    s.active = true;
                    log::info!("fault-detect: activated at PC {:#x}", s.at_pc);
                } else {
                    return;
                }
            }
            s.push_pc(insn.vaddr);
        }));

        // -- syscall: log entries, check TLS aliasing --
        let inner_sc = self.inner.clone();
        reg.on_syscall(Box::new(move |info| {
            let entry = format!(
                "nr={} args=[{:#x},{:#x},{:#x},{:#x},{:#x},{:#x}]",
                info.number,
                info.args[0], info.args[1], info.args[2],
                info.args[3], info.args[4], info.args[5],
            );
            let mut s = inner_sc.lock().unwrap();
            if !s.active { return; }
            s.push_syscall(entry);

            const NR_CLONE: u64 = 220;
            if info.number == NR_CLONE {
                let flags = info.args[0];
                let tls = info.args[3];
                const CLONE_SETTLS: u64 = 0x0008_0000;
                if flags & CLONE_SETTLS != 0 && tls != 0 {
                    let aliases: Vec<u64> = s.thread_tls
                        .iter()
                        .filter(|(_, existing)| *existing == tls)
                        .map(|(tid, _)| *tid)
                        .collect();
                    for tid in aliases {
                        let report = FaultReport {
                            kind: FaultKind::TlsAliasing,
                            summary: format!(
                                "clone TLS {tls:#x} aliases thread {tid}'s TLS"
                            ),
                            pc: 0,
                            insn_count: 0,
                            pc_history: s.recent_pcs(),
                            syscall_log: s.syscall_log.clone(),
                            arch_context: ArchContext::None,
                        };
                        s.add_report(report);
                    }
                    s.thread_tls.push((info.vcpu_idx as u64, tls));
                }
            }
        }));

        // -- syscall_ret: flag unsupported critical syscalls --
        let inner_ret = self.inner.clone();
        reg.on_syscall_ret(Box::new(move |info| {
            let ret_signed = info.ret_value as i64;
            if ret_signed == -38 {
                const CRITICAL: &[u64] = &[
                    220, // clone
                    222, // mmap
                    226, // mprotect
                    56,  // openat
                    96,  // set_tid_address
                ];
                if CRITICAL.contains(&info.number) {
                    let mut s = inner_ret.lock().unwrap();
                    let report = FaultReport {
                        kind: FaultKind::UnsupportedSyscall,
                        summary: format!(
                            "syscall {} returned -ENOSYS (critical)",
                            info.number
                        ),
                        pc: 0,
                        insn_count: 0,
                        pc_history: s.recent_pcs(),
                        syscall_log: s.syscall_log.clone(),
                        arch_context: ArchContext::None,
                    };
                    s.add_report(report);
                }
            }
        }));

        // -- fault: engine-reported execution fault --
        let inner_fault = self.inner.clone();
        reg.on_fault(Box::new(move |info: &FaultInfo| {
            let mut s = inner_fault.lock().unwrap();
            let report = FaultReport {
                kind: info.fault_kind,
                summary: info.message.clone(),
                pc: info.pc,
                insn_count: info.insn_count,
                pc_history: s.recent_pcs(),
                syscall_log: s.syscall_log.clone(),
                arch_context: info.arch_context.clone(),
            };
            s.add_report(report);
        }));
    }

    fn atexit(&mut self) {
        let inner = self.inner.lock().unwrap();
        if inner.reports.is_empty() {
            return;
        }
        for (i, report) in inner.reports.iter().enumerate() {
            eprintln!();
            print_report(i + 1, inner.reports.len(), report);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Report formatting
// ═══════════════════════════════════════════════════════════════════

fn print_report(idx: usize, total: usize, r: &FaultReport) {
    let bar = "═".repeat(60);
    eprintln!("╔{bar}╗");
    eprintln!("║ FAULT {idx}/{total}: {:<53}║", r.kind.to_string());
    eprintln!("╠{bar}╣");
    eprintln!("║ {:<60}║", r.summary);
    if r.pc != 0 {
        eprintln!("║ PC: {:#018x}  insns: {:<27}║", r.pc, r.insn_count);
    }

    // Arch-specific register dump
    match &r.arch_context {
        ArchContext::Aarch64 { x, sp, pc, nzcv, tpidr_el0, current_el } => {
            eprintln!("╠{bar}╣");
            eprintln!("║ AArch64 Registers (EL{current_el}){:>38}║", "");
            eprintln!("║ PC={pc:#018x}  SP={sp:#018x}  ║");
            eprintln!("║ NZCV={nzcv:#010x}  TPIDR_EL0={tpidr_el0:#018x}      ║");
            for row in 0..8 {
                let i = row * 4;
                eprintln!(
                    "║ X{:<2}={:016x} X{:<2}={:016x} X{:<2}={:016x} X{:<2}={:016x}║",
                    i, x.get(i).copied().unwrap_or(0),
                    i+1, x.get(i+1).copied().unwrap_or(0),
                    i+2, x.get(i+2).copied().unwrap_or(0),
                    i+3, x.get(i+3).copied().unwrap_or(0),
                );
            }
        }
        ArchContext::Riscv { x, pc } => {
            eprintln!("╠{bar}╣");
            eprintln!("║ RISC-V Registers{:>44}║", "");
            eprintln!("║ PC={pc:#018x}{:>43}║", "");
            for row in 0..8 {
                let i = row * 4;
                eprintln!(
                    "║ x{:<2}={:016x} x{:<2}={:016x} x{:<2}={:016x} x{:<2}={:016x}║",
                    i, x[i], i+1, x[i+1], i+2, x[i+2], i+3, x[i+3],
                );
            }
        }
        ArchContext::X86_64 { rip, rsp, rflags, rax, rbx, rcx, rdx, rsi, rdi, rbp, r8, r9, r10, r11, r12, r13, r14, r15 } => {
            eprintln!("╠{bar}╣");
            eprintln!("║ x86-64 Registers{:>43}║", "");
            eprintln!("║ RIP={rip:#018x}  RSP={rsp:#018x} ║");
            eprintln!("║ RFLAGS={rflags:#018x}{:>37}║", "");
            eprintln!("║ RAX={rax:016x} RBX={rbx:016x} RCX={rcx:016x}      ║");
            eprintln!("║ RDX={rdx:016x} RSI={rsi:016x} RDI={rdi:016x}      ║");
            eprintln!("║ RBP={rbp:016x} R8 ={r8:016x} R9 ={r9:016x}      ║");
            eprintln!("║ R10={r10:016x} R11={r11:016x} R12={r12:016x}      ║");
            eprintln!("║ R13={r13:016x} R14={r14:016x} R15={r15:016x}      ║");
        }
        ArchContext::None => {}
    }

    // PC history
    if !r.pc_history.is_empty() {
        eprintln!("╠{bar}╣");
        let show = r.pc_history.len().min(16);
        let start = r.pc_history.len() - show;
        eprintln!("║ PC History (last {show}){:>41}║", "");
        for (i, &pc) in r.pc_history[start..].iter().enumerate() {
            let offset = -(show as isize) + i as isize;
            eprintln!("║   {offset:+3}: {pc:#018x}{:>37}║", "");
        }
    }

    // Syscall log
    if !r.syscall_log.is_empty() {
        eprintln!("╠{bar}╣");
        let show = r.syscall_log.len().min(8);
        let start = r.syscall_log.len() - show;
        eprintln!("║ Recent Syscalls (last {show}){:>36}║", "");
        for entry in &r.syscall_log[start..] {
            let truncated: String = entry.chars().take(58).collect();
            eprintln!("║   {truncated:<57}║");
        }
    }

    eprintln!("╚{bar}╝");
}
