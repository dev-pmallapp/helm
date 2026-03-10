//! End-to-end test: inflate binary through GenericSession.
//!
//! Proves the full trait-based pipeline works: Aarch64TraitDecoder →
//! Aarch64TraitExecutor → Aarch64CpuState → OwnedFlatMemory →
//! TraitSyscallHandler → NullBackend.

use crate::generic_session::{GenericSession, GenericStopReason};
use crate::loader;
use helm_core::cpu::CpuState;
use helm_isa::arm::aarch64::cpu_state::Aarch64CpuState;
use helm_isa::arm::aarch64::executor::Aarch64TraitExecutor;
use helm_isa::arm::aarch64::trait_decoder::Aarch64TraitDecoder;
use helm_memory::flat::OwnedFlatMemory;
use helm_syscall::adapter::TraitSyscallHandler;
use helm_timing::NullBackend;

const INFLATE_BIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/binaries/inflate_test"
);

const MAX_INSNS: u64 = 50_000_000;

#[test]
fn generic_session_inflate_interpreted() {
    let loaded = loader::load_elf(INFLATE_BIN, &["inflate_test"], &[]).unwrap();

    let mut cpu = Aarch64CpuState::new();
    cpu.set_pc(loaded.entry_point);
    cpu.set_gpr(31, loaded.initial_sp);

    let mut address_space = loaded.address_space;
    address_space.map(0, 0x1000, (true, false, false));
    address_space.map(loaded.brk_base, 0x1000, (true, true, false));

    let mut syscall = TraitSyscallHandler::new();
    syscall.set_brk(loaded.brk_base);

    let decoder = Aarch64TraitDecoder;
    let executor = Aarch64TraitExecutor::new();
    let mem = OwnedFlatMemory::new(address_space);

    let mut session = GenericSession::new(
        decoder,
        executor,
        cpu,
        Box::new(mem),
        Box::new(NullBackend),
    );
    session.set_syscall_handler(Box::new(syscall));

    let reason = session.run_interpreted(MAX_INSNS);

    assert_eq!(
        reason,
        GenericStopReason::Exit(0),
        "inflate should exit 0, got {:?} at PC={:#x} after {} insns",
        reason,
        session.cpu.pc(),
        session.insn_count
    );
}
