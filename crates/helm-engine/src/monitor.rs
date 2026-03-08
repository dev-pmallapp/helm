//! QEMU-like interactive monitor for debugging simulations.
//!
//! Provides a `(helm)` prompt for inspecting and controlling a running
//! simulation. Enabled via `--monitor` on the CLI.
//!
//! ```text
//! (helm) info registers
//! (helm) x/16x 0xffffffc080010800
//! (helm) break start_kernel
//! (helm) continue 100000000
//! (helm) step 100
//! (helm) info irq
//! (helm) quit
//! ```

use crate::se::session::StopReason;
use crate::symbols::SymbolTable;
use std::io::{self, BufRead, Write};

/// Trait for simulation sessions that can be controlled by the monitor.
pub trait MonitorTarget {
    fn run(&mut self, max_insns: u64) -> StopReason;
    fn run_until_pc(&mut self, pc: u64, max_insns: u64) -> StopReason;
    fn pc(&self) -> u64;
    fn xn(&self, n: u32) -> u64;
    fn sp(&self) -> u64;
    fn read_memory(&self, addr: u64, size: usize) -> Option<Vec<u8>>;
    fn insn_count(&self) -> u64;
    fn virtual_cycles(&self) -> u64;
    fn current_el(&self) -> u8;
    fn daif(&self) -> u32;
    fn sysreg(&self, name: &str) -> Option<u64>;
    fn irq_count(&self) -> u64;
    fn has_exited(&self) -> bool;
    fn symbols(&self) -> &SymbolTable;
}

/// Interactive monitor.
pub struct Monitor {
    breakpoints: Vec<u64>,
    default_continue: u64,
}

impl Monitor {
    pub fn new() -> Self {
        Self {
            breakpoints: Vec::new(),
            default_continue: 100_000_000,
        }
    }

    /// Run the interactive monitor loop.
    pub fn run_interactive(&mut self, target: &mut dyn MonitorTarget) {
        let stdin = io::stdin();
        let mut stdout = io::stdout();

        // Print initial state
        self.print_status(target);

        loop {
            print!("(helm) ");
            let _ = stdout.flush();

            let mut line = String::new();
            match stdin.lock().read_line(&mut line) {
                Ok(0) | Err(_) => break, // EOF
                _ => {}
            }

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if self.dispatch(target, line) {
                break; // quit
            }
        }
    }

    /// Dispatch a single command. Returns true if the monitor should exit.
    fn dispatch(&mut self, target: &mut dyn MonitorTarget, line: &str) -> bool {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            return false;
        }

        match parts[0] {
            "quit" | "q" => return true,

            "continue" | "c" => {
                let n = parts.get(1)
                    .and_then(|s| parse_number(s))
                    .unwrap_or(self.default_continue);
                self.do_continue(target, n);
            }

            "step" | "s" => {
                let n = parts.get(1)
                    .and_then(|s| parse_number(s))
                    .unwrap_or(1);
                self.do_continue(target, n);
            }

            "break" | "b" => {
                if let Some(addr_str) = parts.get(1) {
                    self.do_break(target, addr_str);
                } else {
                    println!("Usage: break <address|symbol>");
                }
            }

            "delete" => {
                if let Some(idx) = parts.get(1).and_then(|s| s.parse::<usize>().ok()) {
                    if idx < self.breakpoints.len() {
                        let addr = self.breakpoints.remove(idx);
                        let sym = target.symbols().resolve(addr)
                            .map(|(n, o)| if o == 0 { n.to_string() } else { format!("{n}+{o:#x}") })
                            .unwrap_or_default();
                        println!("Deleted breakpoint {idx}: {addr:#x} {sym}");
                    } else {
                        println!("No breakpoint #{idx}");
                    }
                } else {
                    println!("Usage: delete <breakpoint_number>");
                }
            }

            "info" | "i" => {
                match parts.get(1).copied() {
                    Some("registers" | "regs" | "reg" | "r") => self.info_registers(target),
                    Some("irq" | "irqs" | "interrupts") => self.info_irq(target),
                    Some("break" | "breakpoints" | "b") => self.info_breakpoints(target),
                    Some("mem" | "memory") => self.info_memory(target),
                    Some("timer" | "timers") => self.info_timer(target),
                    Some("el" | "exception") => self.info_exception(target),
                    _ => {
                        println!("info subcommands: registers, irq, break, mem, timer, exception");
                    }
                }
            }

            "print" | "p" => {
                if let Some(expr) = parts.get(1) {
                    self.do_print(target, expr);
                } else {
                    println!("Usage: print <$reg|address|symbol>");
                }
            }

            cmd if cmd.starts_with("x/") => {
                self.do_examine(target, cmd, parts.get(1));
            }

            "help" | "h" | "?" => {
                self.print_help();
            }

            _ => {
                println!("Unknown command: {line}. Type 'help' for available commands.");
            }
        }

        false
    }

    fn do_continue(&mut self, target: &mut dyn MonitorTarget, n: u64) {
        if target.has_exited() {
            println!("Simulation has exited.");
            return;
        }

        let reason = if self.breakpoints.is_empty() {
            target.run(n)
        } else {
            // Run with breakpoint checking
            let mut remaining = n;
            let mut reason = StopReason::InsnLimit;
            while remaining > 0 {
                // Run in chunks, checking breakpoints
                let chunk = remaining.min(1024);
                reason = target.run(chunk);
                remaining = remaining.saturating_sub(chunk);

                // Check if we hit a breakpoint
                let pc = target.pc();
                if self.breakpoints.contains(&pc) {
                    reason = StopReason::Breakpoint { pc };
                    break;
                }

                match &reason {
                    StopReason::Exited { .. } | StopReason::Error(_) => break,
                    _ => {}
                }
            }
            reason
        };

        self.print_stop_reason(target, &reason);
    }

    fn do_break(&mut self, target: &dyn MonitorTarget, addr_str: &str) {
        let addr = if addr_str.starts_with("0x") || addr_str.starts_with("0X") {
            parse_number(addr_str)
        } else if let Some(a) = target.symbols().lookup(addr_str) {
            Some(a)
        } else {
            parse_number(addr_str)
        };

        match addr {
            Some(a) => {
                let idx = self.breakpoints.len();
                self.breakpoints.push(a);
                let sym = target.symbols().resolve(a)
                    .map(|(n, o)| if o == 0 { format!(" <{n}>") } else { format!(" <{n}+{o:#x}>") })
                    .unwrap_or_default();
                println!("Breakpoint {idx} at {a:#x}{sym}");
            }
            None => println!("Cannot resolve: {addr_str}"),
        }
    }

    fn info_registers(&self, target: &dyn MonitorTarget) {
        println!("PC  = {:#018x}  EL{} SP_sel={} DAIF={:#x} NZCV={:#x}",
                 target.pc(), target.current_el(), target.daif() >> 6,
                 target.daif(),
                 target.sysreg("nzcv").unwrap_or(0));

        // Show PC symbol
        if let Some((name, off)) = target.symbols().resolve(target.pc()) {
            if off < 0x10000 {
                println!("     = {name}+{off:#x}");
            }
        }

        println!("SP  = {:#018x}", target.sp());
        for row in 0..8 {
            let i = row * 4;
            println!("X{:<2} = {:#018x}  X{:<2} = {:#018x}  X{:<2} = {:#018x}  X{:<2} = {:#018x}",
                     i, target.xn(i),
                     i + 1, target.xn(i + 1),
                     i + 2, target.xn(i + 2),
                     i + 3, target.xn(i + 3));
        }
        // X28, X29 (FP), X30 (LR)
        println!("X28 = {:#018x}  FP  = {:#018x}  LR  = {:#018x}",
                 target.xn(28), target.xn(29), target.xn(30));

        // Show LR symbol
        if let Some((name, off)) = target.symbols().resolve(target.xn(30)) {
            if off < 0x10000 {
                println!("     LR = {name}+{off:#x}");
            }
        }
    }

    fn info_irq(&self, target: &dyn MonitorTarget) {
        println!("IRQs delivered: {}", target.irq_count());
        println!("DAIF: {:#x} (D={} A={} I={} F={})",
                 target.daif(),
                 (target.daif() >> 9) & 1,
                 (target.daif() >> 8) & 1,
                 (target.daif() >> 7) & 1,
                 (target.daif() >> 6) & 1);
        if let Some(vbar) = target.sysreg("vbar_el1") {
            println!("VBAR_EL1: {vbar:#x}");
        }
    }

    fn info_breakpoints(&self, target: &dyn MonitorTarget) {
        if self.breakpoints.is_empty() {
            println!("No breakpoints.");
            return;
        }
        for (i, &addr) in self.breakpoints.iter().enumerate() {
            let sym = target.symbols().resolve(addr)
                .map(|(n, o)| if o == 0 { format!(" <{n}>") } else { format!(" <{n}+{o:#x}>") })
                .unwrap_or_default();
            println!("  #{i}: {addr:#x}{sym}");
        }
    }

    fn info_memory(&self, target: &dyn MonitorTarget) {
        if let Some(sctlr) = target.sysreg("sctlr_el1") {
            println!("SCTLR_EL1: {sctlr:#x} (MMU={})", sctlr & 1);
        }
        if let Some(tcr) = target.sysreg("tcr_el1") {
            println!("TCR_EL1:   {tcr:#x}");
        }
        if let Some(ttbr0) = target.sysreg("ttbr0_el1") {
            println!("TTBR0_EL1: {ttbr0:#x}");
        }
        if let Some(ttbr1) = target.sysreg("ttbr1_el1") {
            println!("TTBR1_EL1: {ttbr1:#x}");
        }
    }

    fn info_timer(&self, target: &dyn MonitorTarget) {
        if let Some(ctl) = target.sysreg("cntv_ctl_el0") {
            let enable = ctl & 1;
            let imask = (ctl >> 1) & 1;
            let istatus = (ctl >> 2) & 1;
            println!("CNTV_CTL_EL0: {ctl:#x} (enable={enable}, imask={imask}, istatus={istatus})");
        }
        if let Some(cval) = target.sysreg("cntv_cval_el0") {
            println!("CNTV_CVAL_EL0: {cval} ({cval:#x})");
        }
        println!("insn_count (≈CNTVCT): {}", target.insn_count());
    }

    fn info_exception(&self, target: &dyn MonitorTarget) {
        println!("Current EL: {}", target.current_el());
        for (name, reg) in &[
            ("VBAR_EL1", "vbar_el1"), ("ELR_EL1", "elr_el1"),
            ("SPSR_EL1", "spsr_el1"), ("ESR_EL1", "esr_el1"),
            ("FAR_EL1", "far_el1"),
        ] {
            if let Some(val) = target.sysreg(reg) {
                println!("{name}: {val:#x}");
            }
        }
    }

    fn do_print(&self, target: &dyn MonitorTarget, expr: &str) {
        if let Some(reg) = expr.strip_prefix('$') {
            let val = match reg {
                "pc" => Some(target.pc()),
                "sp" => Some(target.sp()),
                "lr" => Some(target.xn(30)),
                "fp" => Some(target.xn(29)),
                _ if reg.starts_with('x') => {
                    reg[1..].parse::<u32>().ok().map(|n| target.xn(n))
                }
                _ => target.sysreg(reg),
            };
            match val {
                Some(v) => {
                    print!("{v:#x} ({v})");
                    if let Some((name, off)) = target.symbols().resolve(v) {
                        if off < 0x100000 {
                            print!(" = {name}+{off:#x}");
                        }
                    }
                    println!();
                }
                None => println!("Unknown register: {reg}"),
            }
        } else if let Some(addr) = parse_number(expr) {
            if let Some((name, off)) = target.symbols().resolve(addr) {
                println!("{addr:#x} = {name}+{off:#x}");
            } else {
                println!("{addr:#x}");
            }
        } else if let Some(addr) = target.symbols().lookup(expr) {
            println!("{expr} = {addr:#x}");
        } else {
            println!("Cannot evaluate: {expr}");
        }
    }

    fn do_examine(&self, target: &dyn MonitorTarget, cmd: &str, addr_arg: Option<&&str>) {
        // Parse x/NNf addr — e.g. x/16x 0x40200000
        let spec = &cmd[2..]; // skip "x/"
        let mut count = 1u64;
        let mut fmt = 'x';

        if !spec.is_empty() {
            let num_end = spec.find(|c: char| !c.is_ascii_digit()).unwrap_or(spec.len());
            if num_end > 0 {
                count = spec[..num_end].parse().unwrap_or(1);
            }
            if num_end < spec.len() {
                fmt = spec[num_end..].chars().next().unwrap_or('x');
            }
        }

        let addr = addr_arg
            .and_then(|s| {
                if s.starts_with("0x") || s.starts_with("0X") {
                    parse_number(s)
                } else if let Some(a) = target.symbols().lookup(s) {
                    Some(a)
                } else {
                    parse_number(s)
                }
            })
            .unwrap_or_else(|| target.pc());

        let word_size: usize = match fmt {
            'b' => 1,
            'h' => 2,
            'w' | 'x' => 4,
            'g' => 8,
            _ => 4,
        };

        let total_bytes = count as usize * word_size;
        let data = match target.read_memory(addr, total_bytes) {
            Some(d) => d,
            None => {
                println!("Cannot read memory at {addr:#x}");
                return;
            }
        };

        // Print in rows of 4 words
        let words_per_row = 4;
        for row_start in (0..count as usize).step_by(words_per_row) {
            let row_addr = addr + (row_start * word_size) as u64;
            print!("{row_addr:#018x}: ");
            for i in 0..words_per_row {
                let idx = row_start + i;
                if idx >= count as usize {
                    break;
                }
                let byte_off = idx * word_size;
                if byte_off + word_size > data.len() {
                    break;
                }
                let val = match word_size {
                    1 => data[byte_off] as u64,
                    2 => u16::from_le_bytes(data[byte_off..byte_off + 2].try_into().unwrap()) as u64,
                    4 => u32::from_le_bytes(data[byte_off..byte_off + 4].try_into().unwrap()) as u64,
                    8 => u64::from_le_bytes(data[byte_off..byte_off + 8].try_into().unwrap()),
                    _ => 0,
                };
                match word_size {
                    1 => print!("{val:02x} "),
                    2 => print!("{val:04x} "),
                    4 => print!("{val:08x} "),
                    8 => print!("{val:016x} "),
                    _ => print!("{val:x} "),
                }
            }
            println!();
        }
    }

    fn print_status(&self, target: &dyn MonitorTarget) {
        println!("HELM monitor — type 'help' for commands");
        println!("PC={:#x} EL{} insns={} cycles={} IRQs={}",
                 target.pc(), target.current_el(),
                 target.insn_count(), target.virtual_cycles(),
                 target.irq_count());
        if let Some((name, off)) = target.symbols().resolve(target.pc()) {
            if off < 0x10000 {
                println!("  at {name}+{off:#x}");
            }
        }
        println!("Symbols: {} loaded", target.symbols().len());
    }

    fn print_stop_reason(&self, target: &dyn MonitorTarget, reason: &StopReason) {
        match reason {
            StopReason::InsnLimit => {
                print!("Stopped: instruction limit at PC={:#x}", target.pc());
            }
            StopReason::Breakpoint { pc } => {
                print!("Breakpoint hit at PC={pc:#x}");
            }
            StopReason::Exited { code } => {
                println!("Program exited with code {code}");
                return;
            }
            StopReason::Error(msg) => {
                println!("Error: {msg}");
                return;
            }
        }
        if let Some((name, off)) = target.symbols().resolve(target.pc()) {
            if off < 0x10000 {
                print!(" <{name}+{off:#x}>");
            }
        }
        println!(" (insns={}, cycles={})", target.insn_count(), target.virtual_cycles());
    }

    fn print_help(&self) {
        println!("Commands:");
        println!("  continue [N]         Run N instructions (default {})", self.default_continue);
        println!("  step [N]             Step N instructions (default 1)");
        println!("  break <addr|symbol>  Set breakpoint");
        println!("  delete <N>           Delete breakpoint #N");
        println!("  info registers       Show CPU registers");
        println!("  info irq             Show interrupt state");
        println!("  info break           List breakpoints");
        println!("  info mem             Show MMU/page table state");
        println!("  info timer           Show timer registers");
        println!("  info exception       Show exception registers");
        println!("  print <$reg|sym>     Print register or symbol value");
        println!("  x/Nf <addr|sym>      Examine memory (f: x=32bit, g=64bit, b=byte, h=16bit)");
        println!("  help                 This message");
        println!("  quit                 Exit monitor");
    }
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_number(s: &str) -> Option<u64> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        s.parse().ok()
    }
}
