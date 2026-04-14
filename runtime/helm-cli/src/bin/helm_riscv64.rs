//! `helm-riscv64` — RISC-V 64 SE mode launcher.
//!
//! Runs a statically-linked RISC-V 64 Linux ELF binary in syscall-emulation mode.
//!
//! # Usage
//!
//! ```text
//! helm-riscv64 ./hello
//! helm-riscv64 ./busybox sh -c 'echo hello'
//! helm-riscv64 --mem-size 256 ./hello arg1 arg2
//! ```

use std::process;

use helm_engine::{build_simulator, ExecMode, Isa, StopReason, TimingChoice};

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Warn)
        .parse_default_env()
        .init();

    let raw: Vec<String> = std::env::args().collect();
    let args = &raw[1..];

    // Parse --mem-size N (in MB) and the binary + argv
    let mut mem_mb: usize = 128;
    let mut binary_idx: Option<usize> = None;

    let mut i = 0;
    while i < args.len() {
        if args[i] == "--mem-size" {
            i += 1;
            mem_mb = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(128);
        } else if !args[i].starts_with("--") {
            binary_idx = Some(i);
            break;
        }
        i += 1;
    }

    let binary_idx = binary_idx.unwrap_or_else(|| {
        eprintln!("usage: helm-riscv64 [--mem-size MB] <binary> [args...]");
        process::exit(1);
    });

    let binary = &args[binary_idx];
    let argv: Vec<&str> = args[binary_idx..].iter().map(String::as_str).collect();
    // Pass through host environment variables to the guest
    let envp_owned: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
    let envp: Vec<&str> = envp_owned.iter().map(String::as_str).collect();

    let mem_size = mem_mb * 1024 * 1024;
    let mut sim = build_simulator(
        Isa::RiscV,
        ExecMode::Syscall,
        TimingChoice::VirtualTiming { ipc: 1.0 },
        0x1000,
        mem_size,
    );

    if let Err(e) = sim.load_riscv64_elf(binary, &argv, &envp) {
        eprintln!("helm-riscv64: failed to load {binary}: {e}");
        process::exit(1);
    }

    const QUANTUM: u64 = 1_000_000;
    loop {
        match sim.run(QUANTUM) {
            StopReason::Exit { code } => {
                process::exit(code);
            }
            StopReason::Quantum => {}
            StopReason::Breakpoint => {
                eprintln!("helm-riscv64: breakpoint at pc={:#x}", sim.pc());
                process::exit(0);
            }
            StopReason::Unsupported => {
                eprintln!(
                    "helm-riscv64: unsupported instruction at pc={:#x}",
                    sim.pc()
                );
                process::exit(1);
            }
            StopReason::Exception(e) => {
                eprintln!(
                    "helm-riscv64: unhandled exception at pc={:#x}: {e:?}",
                    sim.pc()
                );
                process::exit(1);
            }
        }
    }
}
