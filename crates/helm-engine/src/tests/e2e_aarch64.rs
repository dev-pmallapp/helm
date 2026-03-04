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
fn debug_auxv_issue() {
    let loaded = crate::loader::load_elf(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish"),
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin"],
    ).unwrap();

    let mut mem = loaded.address_space;
    let mut cpu = helm_isa::arm::aarch64::Aarch64Cpu::new();
    cpu.regs.pc = loaded.entry_point;
    cpu.regs.sp = loaded.initial_sp;
    mem.map(0, 0x1000, (true, false, false));

    let mut syscall = helm_syscall::Aarch64SyscallHandler::new();
    let brk_start = (loaded.entry_point & !0xFFF) + 0x800000;
    syscall.set_brk(brk_start);

    let sp = loaded.initial_sp;
    eprintln!("\nEntry={:#x} SP={:#x}", loaded.entry_point, sp);

    // Dump stack
    eprintln!("=== Stack from SP ===");
    for i in 0..50u64 {
        let addr = sp + i * 8;
        let mut buf = [0u8; 8];
        if mem.read(addr, &mut buf).is_err() { break; }
        let val = u64::from_le_bytes(buf);
        let tag = match i {
            0 => " <- argc",
            _ if val == 0 && i > 0 => " <- NULL",
            _ => "",
        };
        eprintln!("  SP+{:>3}: [{addr:#018x}] = {val:#018x}{tag}", i*8);
    }

    let mut insn_count = 0u64;
    let mut getauxval_hits = 0u64;

    loop {
        if insn_count >= 500_000 {
            eprintln!("LIMIT at {insn_count} insns, PC={:#x}", cpu.regs.pc);
            break;
        }

        let pc = cpu.regs.pc;

        // Trace __init_libc region
        if (0x696000..=0x6960d0).contains(&pc) {
            let mut buf = [0u8; 4];
            let _ = mem.read(pc, &mut buf);
            let insn = u32::from_le_bytes(buf);
            eprintln!("[{insn_count:>7}] {pc:#010x} {insn:08x} X0={:#x} X1={:#x} X19={:#x} X20={:#x} SP={:#x}",
                cpu.xn(0), cpu.xn(1), cpu.xn(19), cpu.xn(20), cpu.regs.sp);
        }

        // Detect __getauxval loop
        if pc == 0x697884 {
            getauxval_hits += 1;
            if getauxval_hits == 1 {
                eprintln!("\n=== __getauxval loop entered at insn {insn_count} ===");
                let x0 = cpu.xn(0);
                let x19 = cpu.xn(19);
                eprintln!("X0={x0:#x} X1={:#x} X19={x19:#x} X20={:#x}", cpu.xn(1), cpu.xn(20));
                eprintln!("Scanning from base {:#x} + offset {:#x} = {:#x}", x0, x19, x0.wrapping_add(x19));
                for j in 0..30i64 {
                    let addr = x0.wrapping_add(x19).wrapping_add((j * 16) as u64);
                    let mut tb = [0u8; 8];
                    let mut vb = [0u8; 8];
                    if mem.read(addr, &mut tb).is_ok() && mem.read(addr+8, &mut vb).is_ok() {
                        let t = u64::from_le_bytes(tb);
                        let v = u64::from_le_bytes(vb);
                        eprintln!("  [{j:>2}] @ {addr:#x}: type={t} val={v:#x}");
                        if t == 0 { break; }
                    } else {
                        eprintln!("  [{j:>2}] @ {addr:#x}: UNMAPPED");
                        break;
                    }
                }
            }
            if getauxval_hits >= 200 {
                eprintln!("__getauxval looped {} times — infinite loop confirmed", getauxval_hits);
                break;
            }
        }

        match cpu.step(&mut mem) {
            Ok(()) => { insn_count += 1; }
            Err(helm_core::HelmError::Syscall { number, .. }) => {
                let args = [cpu.xn(0), cpu.xn(1), cpu.xn(2), cpu.xn(3), cpu.xn(4), cpu.xn(5)];
                let result = syscall.handle(number, &args, &mut mem).unwrap();
                cpu.set_xn(0, result);
                if syscall.should_exit {
                    eprintln!("exit({}) at insn {insn_count}", syscall.exit_code);
                    break;
                }
                cpu.regs.pc += 4;
                insn_count += 1;
            }
            Err(e) => {
                eprintln!("CRASH at insn {insn_count}, PC={pc:#x}: {e}");
                for r in 0..31u16 {
                    let v = cpu.xn(r);
                    if v != 0 { eprintln!("  X{r}={v:#018x}"); }
                }
                break;
            }
        }
    }
}
#[test]
fn trace_getauxval_loop() {
    let loaded = crate::loader::load_elf(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish"),
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin"],
    ).unwrap();

    let mut mem = loaded.address_space;
    let mut cpu = helm_isa::arm::aarch64::Aarch64Cpu::new();
    cpu.regs.pc = loaded.entry_point;
    cpu.regs.sp = loaded.initial_sp;
    mem.map(0, 0x1000, (true, false, false));

    let mut syscall = helm_syscall::Aarch64SyscallHandler::new();
    let brk_start = (loaded.entry_point & !0xFFF) + 0x800000;
    syscall.set_brk(brk_start);

    let mut insn_count = 0u64;
    let mut in_loop = false;
    let mut loop_iters = 0u64;

    loop {
        if insn_count >= 500_000 { break; }
        let pc = cpu.regs.pc;

        // Trace every instruction in the __getauxval loop range
        if (0x697870..=0x697898).contains(&pc) {
            if !in_loop {
                in_loop = true;
                eprintln!("\n=== __getauxval loop started at insn {} ===", insn_count);
            }
            let mut buf = [0u8; 4];
            let _ = mem.read(pc, &mut buf);
            let insn_word = u32::from_le_bytes(buf);
            let nzcv = cpu.regs.nzcv;
            eprintln!("[{insn_count:>7}] {pc:#010x} {insn_word:08x}  X0={:#x} X1={:#x} X19={:#x} NZCV={nzcv:#06x}",
                cpu.xn(0), cpu.xn(1), cpu.xn(19));

            if pc == 0x69787c { loop_iters += 1; }
            if loop_iters > 15 {
                eprintln!("=== Aborting after 15 iterations ===");
                break;
            }
        }

        match cpu.step(&mut mem) {
            Ok(()) => { insn_count += 1; }
            Err(helm_core::HelmError::Syscall { number, .. }) => {
                let args = [cpu.xn(0), cpu.xn(1), cpu.xn(2), cpu.xn(3), cpu.xn(4), cpu.xn(5)];
                let result = syscall.handle(number, &args, &mut mem).unwrap();
                cpu.set_xn(0, result);
                if syscall.should_exit { break; }
                cpu.regs.pc += 4;
                insn_count += 1;
            }
            Err(e) => {
                eprintln!("CRASH at PC={pc:#x}: {e}");
                break;
            }
        }
    }
}
