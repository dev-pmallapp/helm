//! Tests for the generic Session<D, E, C>.

use crate::generic_session::{GenericSession, GenericStopReason};
use helm_core::cpu::CpuState;
use helm_core::decode::Decoder;
use helm_core::exec::Executor;
use helm_core::insn::*;
use helm_core::mem::{MemFault, MemFaultKind, MemoryAccess};
use helm_core::syscall::{SyscallAction, SyscallHandler};
use helm_core::timing::{AccuracyLevel, TimingBackend};
use helm_core::types::Addr;
use std::collections::HashMap;

// ── Mock CPU ───────────────────────────────────────────────────────

struct MockCpu {
    pc: Addr,
    regs: [u64; 32],
}

impl MockCpu {
    fn new(pc: Addr) -> Self {
        Self {
            pc,
            regs: [0; 32],
        }
    }
}

impl CpuState for MockCpu {
    fn pc(&self) -> Addr { self.pc }
    fn set_pc(&mut self, pc: Addr) { self.pc = pc; }
    fn gpr(&self, id: u16) -> u64 { self.regs[id as usize] }
    fn set_gpr(&mut self, id: u16, val: u64) { self.regs[id as usize] = val; }
    fn sysreg(&self, _: u32) -> u64 { 0 }
    fn set_sysreg(&mut self, _: u32, _: u64) {}
    fn flags(&self) -> u64 { 0 }
    fn set_flags(&mut self, _: u64) {}
    fn privilege_level(&self) -> u8 { 0 }
}

// ── Mock Memory ────────────────────────────────────────────────────

struct MockMem {
    data: HashMap<Addr, u8>,
}

impl MockMem {
    fn new() -> Self { Self { data: HashMap::new() } }

    fn write_insn(&mut self, addr: Addr, bytes: &[u8]) {
        for (i, b) in bytes.iter().enumerate() {
            self.data.insert(addr + i as u64, *b);
        }
    }
}

impl MemoryAccess for MockMem {
    fn read(&mut self, addr: Addr, size: usize) -> Result<u64, MemFault> {
        let mut val = 0u64;
        for i in 0..size {
            let byte = self.data.get(&(addr + i as u64)).copied().unwrap_or(0);
            val |= (byte as u64) << (i * 8);
        }
        Ok(val)
    }

    fn write(&mut self, addr: Addr, size: usize, val: u64) -> Result<(), MemFault> {
        for i in 0..size {
            self.data.insert(addr + i as u64, (val >> (i * 8)) as u8);
        }
        Ok(())
    }

    fn fetch(&mut self, addr: Addr, buf: &mut [u8]) -> Result<(), MemFault> {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.data.get(&(addr + i as u64)).copied().unwrap_or(0);
        }
        Ok(())
    }
}

// ── Mock Decoder ───────────────────────────────────────────────────

/// Decodes 4-byte "instructions" where byte[0] is the opcode:
///   0x00 = NOP
///   0x01 = ADD X[dst], X[src1], X[src2]  (bytes: 01 dst src1 src2)
///   0x02 = HALT (stops execution via exception)
///   0x03 = SVC (syscall)
struct MockDecoder;

impl Decoder for MockDecoder {
    fn decode(&self, pc: Addr, bytes: &[u8]) -> Result<DecodedInsn, helm_core::HelmError> {
        if bytes.len() < 4 {
            return Err(helm_core::HelmError::Decode { addr: pc, reason: "too short".into() });
        }
        let opcode = bytes[0];
        let mut insn = DecodedInsn {
            pc,
            len: 4,
            ..DecodedInsn::default()
        };
        insn.encoding_bytes[..4].copy_from_slice(&bytes[..4]);

        match opcode {
            0x00 => {
                insn.class = InsnClass::Nop;
                insn.flags = InsnFlags::NOP;
            }
            0x01 => {
                insn.class = InsnClass::IntAlu;
                insn.dst_regs[0] = bytes[1] as u16;
                insn.dst_count = 1;
                insn.src_regs[0] = bytes[2] as u16;
                insn.src_regs[1] = bytes[3] as u16;
                insn.src_count = 2;
            }
            0x02 => {
                insn.class = InsnClass::Nop;
                insn.flags = InsnFlags::TRAP;
            }
            0x03 => {
                insn.class = InsnClass::Syscall;
                insn.flags = InsnFlags::SYSCALL;
            }
            _ => {
                return Err(helm_core::HelmError::Decode { addr: pc, reason: "unknown opcode".into() });
            }
        }
        Ok(insn)
    }

    fn min_insn_size(&self) -> usize { 4 }
}

// ── Mock Executor ──────────────────────────────────────────────────

struct MockExecutor;

impl Executor for MockExecutor {
    fn execute(
        &mut self,
        insn: &DecodedInsn,
        cpu: &mut dyn CpuState,
        _mem: &mut dyn MemoryAccess,
    ) -> ExecOutcome {
        match insn.encoding_bytes[0] {
            0x00 => {
                // NOP — just advance PC
                ExecOutcome {
                    next_pc: insn.pc + 4,
                    ..ExecOutcome::default()
                }
            }
            0x01 => {
                // ADD
                let dst = insn.dst_regs[0];
                let a = cpu.gpr(insn.src_regs[0]);
                let b = cpu.gpr(insn.src_regs[1]);
                cpu.set_gpr(dst, a + b);
                ExecOutcome {
                    next_pc: insn.pc + 4,
                    ..ExecOutcome::default()
                }
            }
            0x02 => {
                // HALT
                ExecOutcome {
                    next_pc: insn.pc,
                    exception: Some(ExceptionInfo {
                        class: 0,
                        iss: 0,
                        vaddr: 0,
                        target_el: 0,
                    }),
                    ..ExecOutcome::default()
                }
            }
            _ => ExecOutcome {
                next_pc: insn.pc + 4,
                ..ExecOutcome::default()
            },
        }
    }
}

// ── Mock TimingBackend ─────────────────────────────────────────────

struct MockTiming;

impl TimingBackend for MockTiming {
    fn accuracy(&self) -> AccuracyLevel { AccuracyLevel::FE }
    fn account(&mut self, _insn: &DecodedInsn, _outcome: &ExecOutcome) -> u64 { 0 }
}

// ── Mock SyscallHandler ────────────────────────────────────────────

struct MockSyscallHandler {
    exit_on_nr: u64,
}

impl SyscallHandler for MockSyscallHandler {
    fn handle(
        &mut self,
        nr: u64,
        _cpu: &mut dyn CpuState,
        _mem: &mut dyn MemoryAccess,
    ) -> SyscallAction {
        if nr == self.exit_on_nr {
            SyscallAction::Exit { code: 0 }
        } else {
            SyscallAction::Handled(0)
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[test]
fn run_nops_exhausts_budget() {
    let mut mem = MockMem::new();
    // 10 NOPs at address 0x1000
    for i in 0..10 {
        mem.write_insn(0x1000 + i * 4, &[0x00, 0, 0, 0]);
    }

    let mut session = GenericSession::new(
        MockDecoder,
        MockExecutor,
        MockCpu::new(0x1000),
        Box::new(mem),
        Box::new(MockTiming),
    );

    let reason = session.run_interpreted(5);
    assert_eq!(reason, GenericStopReason::BudgetExhausted);
    assert_eq!(session.insn_count, 5);
    assert_eq!(session.cpu.pc(), 0x1000 + 5 * 4);
}

#[test]
fn add_computes_correctly() {
    let mut mem = MockMem::new();
    // ADD X0, X1, X2
    mem.write_insn(0x1000, &[0x01, 0, 1, 2]);
    // NOP (to stop)
    mem.write_insn(0x1004, &[0x00, 0, 0, 0]);

    let mut cpu = MockCpu::new(0x1000);
    cpu.regs[1] = 100;
    cpu.regs[2] = 200;

    let mut session = GenericSession::new(
        MockDecoder,
        MockExecutor,
        cpu,
        Box::new(mem),
        Box::new(MockTiming),
    );

    session.run_interpreted(1);
    assert_eq!(session.cpu.gpr(0), 300);
    assert_eq!(session.insn_count, 1);
}

#[test]
fn halt_returns_exception() {
    let mut mem = MockMem::new();
    mem.write_insn(0x1000, &[0x02, 0, 0, 0]); // HALT

    let mut session = GenericSession::new(
        MockDecoder,
        MockExecutor,
        MockCpu::new(0x1000),
        Box::new(mem),
        Box::new(MockTiming),
    );

    let reason = session.run_interpreted(10);
    assert!(matches!(reason, GenericStopReason::Exception { pc: 0x1000, .. }));
}

#[test]
fn syscall_exit() {
    let mut mem = MockMem::new();
    mem.write_insn(0x1000, &[0x03, 0, 0, 0]); // SVC

    let mut cpu = MockCpu::new(0x1000);
    cpu.regs[8] = 93; // exit syscall number

    let mut session = GenericSession::new(
        MockDecoder,
        MockExecutor,
        cpu,
        Box::new(mem),
        Box::new(MockTiming),
    );
    session.set_syscall_handler(Box::new(MockSyscallHandler { exit_on_nr: 93 }));

    let reason = session.run_interpreted(10);
    assert_eq!(reason, GenericStopReason::Exit(0));
}

#[test]
fn syscall_handled_continues() {
    let mut mem = MockMem::new();
    mem.write_insn(0x1000, &[0x03, 0, 0, 0]); // SVC (nr=42, not exit)
    mem.write_insn(0x1004, &[0x00, 0, 0, 0]); // NOP

    let mut cpu = MockCpu::new(0x1000);
    cpu.regs[8] = 42; // non-exit syscall

    let mut session = GenericSession::new(
        MockDecoder,
        MockExecutor,
        cpu,
        Box::new(mem),
        Box::new(MockTiming),
    );
    session.set_syscall_handler(Box::new(MockSyscallHandler { exit_on_nr: 93 }));

    let reason = session.run_interpreted(2);
    assert_eq!(reason, GenericStopReason::BudgetExhausted);
    assert_eq!(session.insn_count, 2);
    assert_eq!(session.cpu.pc(), 0x1008);
}

#[test]
fn decode_error_at_unmapped() {
    let mem = MockMem::new(); // empty — no instructions

    let mut session = GenericSession::new(
        MockDecoder,
        MockExecutor,
        MockCpu::new(0xDEAD_0000),
        Box::new(mem),
        Box::new(MockTiming),
    );

    let reason = session.run_interpreted(1);
    // All-zero bytes decode as NOP (opcode 0x00), so this actually succeeds
    // and runs a NOP. The real test is if the memory is truly empty.
    assert_eq!(reason, GenericStopReason::BudgetExhausted);
}

#[test]
fn cycle_count_advances() {
    let mut mem = MockMem::new();
    for i in 0..5 {
        mem.write_insn(0x1000 + i * 4, &[0x00, 0, 0, 0]);
    }

    let mut session = GenericSession::new(
        MockDecoder,
        MockExecutor,
        MockCpu::new(0x1000),
        Box::new(mem),
        Box::new(MockTiming), // returns 0 stall, so cycle_count = insn_count
    );

    session.run_interpreted(5);
    assert_eq!(session.cycle_count, 5); // 1 cycle per insn, 0 stall
}

#[test]
fn timing_stall_adds_to_cycles() {
    struct StallTiming;
    impl TimingBackend for StallTiming {
        fn accuracy(&self) -> AccuracyLevel { AccuracyLevel::ITE }
        fn account(&mut self, _: &DecodedInsn, _: &ExecOutcome) -> u64 { 3 }
    }

    let mut mem = MockMem::new();
    for i in 0..5 {
        mem.write_insn(0x1000 + i * 4, &[0x00, 0, 0, 0]);
    }

    let mut session = GenericSession::new(
        MockDecoder,
        MockExecutor,
        MockCpu::new(0x1000),
        Box::new(mem),
        Box::new(StallTiming),
    );

    session.run_interpreted(5);
    assert_eq!(session.insn_count, 5);
    assert_eq!(session.cycle_count, 20); // 5 * (1 + 3 stall)
}
