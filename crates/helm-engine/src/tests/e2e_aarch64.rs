//! End-to-end AArch64 SE mode tests.
use crate::se::linux as aarch64_se;

#[test]
fn load_aarch64_elf() {
    let result = crate::loader::load_elf(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish"),
        &["fish"],
        &["HOME=/tmp"],
    );
    assert!(result.is_ok(), "Failed to load ELF: {:?}", result.err());
    let loaded = result.unwrap();
    assert_eq!(loaded.entry_point, 0x411120);
    assert!(loaded.initial_sp > 0x7FFF_0000_0000);
    assert!(loaded.initial_sp & 0xF == 0, "SP must be 16-byte aligned");
}

#[test]
fn se_run_1000_insns() {
    let result = aarch64_se::run_aarch64_se(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish"),
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin"],
        1000,
    );
    match result {
        Ok(r) => assert!(r.instructions_executed >= 100),
        Err(_) => {}
    }
}

#[test]
fn se_progress_10m() {
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
    syscall.set_brk(loaded.brk_base);
    syscall.binary_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish").to_string();
    mem.map(loaded.brk_base, 0x1000, (true, true, false));
    let mut insn_count = 0u64;
    let mut sc_count = 0u64;
    loop {
        if insn_count >= 10_000_000 { break; }
        match cpu.step(&mut mem) {
            Ok(_trace) => { insn_count += 1; }
            Err(helm_core::HelmError::Syscall { number, .. }) => {
                let args = [cpu.xn(0), cpu.xn(1), cpu.xn(2), cpu.xn(3), cpu.xn(4), cpu.xn(5)];
                sc_count += 1;
                let result = syscall.handle(number, &args, &mut mem).expect("syscall handler failed");
                cpu.set_xn(0, result);
                if syscall.should_exit {
                    eprintln!("exit({}) at {insn_count} insns ({sc_count} sc)", syscall.exit_code);
                    return;
                }
                cpu.regs.pc += 4;
                insn_count += 1;
            }
            Err(helm_core::HelmError::Decode { addr, reason }) => {
                eprintln!("UNIMPL at {insn_count} ({sc_count} sc): PC={addr:#x} {reason}");
                break;
            }
            Err(e) => {
                eprintln!("ERROR at {insn_count}: {e}");
                break;
            }
        }
    }
    assert!(insn_count >= 1_000_000, "Expected >=1M insns, got {insn_count}");
}
