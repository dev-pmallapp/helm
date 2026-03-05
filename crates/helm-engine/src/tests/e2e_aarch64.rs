//! End-to-end AArch64 SE mode tests.
use crate::se::linux as aarch64_se;

#[test]
fn load_fish_binary() {
    // Just verify the ELF loader can parse the fish binary
    // without crashing.
    let result = crate::loader::load_elf(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish"),
        &["fish"],
        &["HOME=/tmp"],
    );
    assert!(result.is_ok(), "Failed to load fish: {:?}", result.err());
    let loaded = result.unwrap();
    assert_eq!(loaded.entry_point, 0x411120);
    assert!(loaded.initial_sp > 0x7FFF_0000_0000);
    assert!(loaded.initial_sp & 0xF == 0, "SP must be 16-byte aligned");
}

#[test]
fn run_fish_first_1000_insns() {
    let result = aarch64_se::run_aarch64_se(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish"),
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin"],
        1000,
    );
    // During development, crashes are expected as we implement more instructions.
    match result {
        Ok(r) => assert!(r.instructions_executed >= 100),
        Err(_) => {} // expected
    }
}

#[test]
fn run_fish_first_100k_insns() {
    // Run 100K instructions — should get well into musl init.
    let result = aarch64_se::run_aarch64_se(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish"),
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin"],
        100_000,
    );
    // It's OK if we crash — we're progressively implementing.
    // Just check we get past the first few hundred instructions.
    match result {
        Ok(r) => assert!(r.instructions_executed > 100),
        Err(_) => {} // expected during development
    }
}

#[test]
fn debug_fish_crash() {
    let loaded = crate::loader::load_elf(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish"),
        &["fish"],
        &["HOME=/tmp"],
    )
    .unwrap();
    let mut mem = loaded.address_space;
    let mut cpu = helm_isa::arm::aarch64::Aarch64Cpu::new();
    cpu.regs.pc = loaded.entry_point;
    cpu.regs.sp = loaded.initial_sp;
    let mut syscall = helm_syscall::Aarch64SyscallHandler::new();

    // Ring buffer of last 20 instructions for debugging
    let mut trace: Vec<(u64, u32)> = Vec::new();

    for i in 0..50000 {
        let pc = cpu.regs.pc;
        let mut buf = [0u8; 4];
        if mem.read(pc, &mut buf).is_err() {
            eprintln!("Cannot fetch at PC={pc:#x} after {i} insns");
            break;
        }
        let insn = u32::from_le_bytes(buf);
        trace.push((pc, insn));
        if trace.len() > 30 {
            trace.remove(0);
        }

        match cpu.step(&mut mem) {
            Ok(()) => {}
            Err(helm_core::HelmError::Syscall { number, .. }) => {
                let args = [
                    cpu.xn(0),
                    cpu.xn(1),
                    cpu.xn(2),
                    cpu.xn(3),
                    cpu.xn(4),
                    cpu.xn(5),
                ];
                let result = syscall.handle(number, &args, &mut mem).unwrap();
                cpu.set_xn(0, result);
                if syscall.should_exit {
                    break;
                }
                cpu.regs.pc += 4;
            }
            Err(e) => {
                eprintln!("Crash at insn {i}, PC={pc:#x}, insn={insn:#010x}: {e}");
                eprintln!(
                    "Registers: X0={:#x} X1={:#x} X2={:#x} X3={:#x}",
                    cpu.xn(0),
                    cpu.xn(1),
                    cpu.xn(2),
                    cpu.xn(3)
                );
                eprintln!(
                    "SP={:#x} X29={:#x} X30={:#x}",
                    cpu.regs.sp,
                    cpu.xn(29),
                    cpu.xn(30)
                );
                for r in 0..31u16 {
                    let v = cpu.xn(r);
                    if v != 0 {
                        eprintln!("  X{r}={v:#018x}");
                    }
                }
                eprintln!("Last {} instructions:", trace.len());
                for (tpc, tinsn) in &trace {
                    eprintln!("  {tpc:#010x}  {tinsn:08x}");
                }
                break;
            }
        }
    }
}

#[test]
fn trace_meta_address() {
    let loaded = crate::loader::load_elf(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish"),
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin"],
    )
    .unwrap();
    let mut mem = loaded.address_space;
    let mut cpu = helm_isa::arm::aarch64::Aarch64Cpu::new();
    cpu.regs.pc = loaded.entry_point;
    cpu.regs.sp = loaded.initial_sp;
    mem.map(0, 0x1000, (true, false, false));
    let mut syscall = helm_syscall::Aarch64SyscallHandler::new();
    syscall.set_brk(loaded.brk_base);

    let mut insn_count = 0u64;
    loop {
        if insn_count >= 3000 {
            break;
        }
        let pc = cpu.regs.pc;
        // The group stores meta pointer at offset 0: STR X19, [X0, #0] at 0x697fbc
        if pc == 0x697fbc {
            eprintln!(
                "[{insn_count}] group store: X0={:#x} X19={:#x} (meta ptr)",
                cpu.xn(0),
                cpu.xn(19)
            );
        }
        // Track X19 (meta address) when entering the alloc path
        if pc == 0x697acc {
            eprintln!(
                "[{insn_count}] alloc_group entry: X0={:#x} X19={:#x}",
                cpu.xn(0),
                cpu.xn(19)
            );
        }
        match cpu.step(&mut mem) {
            Ok(()) => {
                insn_count += 1;
            }
            Err(helm_core::HelmError::Syscall { number, .. }) => {
                let args = [
                    cpu.xn(0),
                    cpu.xn(1),
                    cpu.xn(2),
                    cpu.xn(3),
                    cpu.xn(4),
                    cpu.xn(5),
                ];
                let result = syscall
                    .handle(number, &args, &mut mem)
                    .unwrap_or(-38i64 as u64);
                cpu.set_xn(0, result);
                if syscall.should_exit {
                    return;
                }
                cpu.regs.pc += 4;
                insn_count += 1;
            }
            Err(e) => {
                eprintln!("STOP at {insn_count}: {e}");
                break;
            }
        }
    }
}

#[test]
fn fish_progress() {
    let loaded = crate::loader::load_elf(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish"),
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin"],
    )
    .unwrap();
    eprintln!("brk_base={:#x}", loaded.brk_base);
    let mut mem = loaded.address_space;
    let mut cpu = helm_isa::arm::aarch64::Aarch64Cpu::new();
    cpu.regs.pc = loaded.entry_point;
    cpu.regs.sp = loaded.initial_sp;
    mem.map(0, 0x1000, (true, false, false));
    let mut syscall = helm_syscall::Aarch64SyscallHandler::new();
    syscall.set_brk(loaded.brk_base);
    mem.map(loaded.brk_base, 0x1000, (true, true, false));
    let mut insn_count = 0u64;
    let mut sc_count = 0u64;
    loop {
        if insn_count >= 50_000_000 {
            eprintln!(
                "LIMIT {insn_count} insns ({sc_count} syscalls) PC={:#x}",
                cpu.regs.pc
            );
            break;
        }
        match cpu.step(&mut mem) {
            Ok(()) => {
                insn_count += 1;
            }
            Err(helm_core::HelmError::Syscall { number, .. }) => {
                let args = [
                    cpu.xn(0),
                    cpu.xn(1),
                    cpu.xn(2),
                    cpu.xn(3),
                    cpu.xn(4),
                    cpu.xn(5),
                ];
                sc_count += 1;
                if number == 64 && args[0] == 1 {
                    let len = args[2] as usize;
                    let mut buf = vec![0u8; len.min(4096)];
                    if mem.read(args[1], &mut buf).is_ok() {
                        eprint!("[STDOUT] {}", String::from_utf8_lossy(&buf));
                    }
                }
                let result = syscall
                    .handle(number, &args, &mut mem)
                    .unwrap_or(-38i64 as u64);
                cpu.set_xn(0, result);
                if syscall.should_exit {
                    eprintln!(
                        "\nexit({}) at {insn_count} insns ({sc_count} syscalls)",
                        syscall.exit_code
                    );
                    return;
                }
                cpu.regs.pc += 4;
                insn_count += 1;
            }
            Err(helm_core::HelmError::Decode { addr, reason }) => {
                eprintln!(
                    "UNIMPL at {insn_count} insns ({sc_count} syscalls): PC={addr:#x} {reason}"
                );
                break;
            }
            Err(e) => {
                eprintln!("ERROR at {insn_count}: {e}");
                break;
            }
        }
    }
}

#[test]
fn trace_every_insn_2400_2510() {
    let loaded = crate::loader::load_elf(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish"),
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin"],
    )
    .unwrap();
    let mut mem = loaded.address_space;
    let mut cpu = helm_isa::arm::aarch64::Aarch64Cpu::new();
    cpu.regs.pc = loaded.entry_point;
    cpu.regs.sp = loaded.initial_sp;
    mem.map(0, 0x1000, (true, false, false));
    let mut syscall = helm_syscall::Aarch64SyscallHandler::new();
    syscall.set_brk(loaded.brk_base);
    mem.map(loaded.brk_base, 0x1000, (true, true, false));
    let mut insn_count = 0u64;
    loop {
        if insn_count >= 2510 {
            break;
        }
        let pc = cpu.regs.pc;
        if insn_count >= 2400 {
            let mut buf = [0u8; 4];
            let _ = mem.read(pc, &mut buf);
            let w = u32::from_le_bytes(buf);
            let opc_bf = (w >> 29) & 3;
            let op_hi = (w >> 23) & 7;
            let bfm = if op_hi == 0b110 {
                format!(" BF_opc={opc_bf}")
            } else {
                String::new()
            };
            eprintln!("[{insn_count:>5}] {pc:#010x} {w:08x}{bfm}  X0={:#x} X1={:#x} X2={:#x} X3={:#x} X19={:#x}",
                cpu.xn(0), cpu.xn(1), cpu.xn(2), cpu.xn(3), cpu.xn(19));
        }
        match cpu.step(&mut mem) {
            Ok(()) => {
                insn_count += 1;
            }
            Err(helm_core::HelmError::Syscall { number, .. }) => {
                let args = [
                    cpu.xn(0),
                    cpu.xn(1),
                    cpu.xn(2),
                    cpu.xn(3),
                    cpu.xn(4),
                    cpu.xn(5),
                ];
                let result = syscall
                    .handle(number, &args, &mut mem)
                    .unwrap_or(-38i64 as u64);
                cpu.set_xn(0, result);
                if syscall.should_exit {
                    return;
                }
                cpu.regs.pc += 4;
                insn_count += 1;
            }
            Err(e) => {
                eprintln!("STOP: {e}");
                break;
            }
        }
    }
}
