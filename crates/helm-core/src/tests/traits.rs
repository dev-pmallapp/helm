use crate::cpu::CpuState;
use crate::decode::Decoder;
use crate::exec::Executor;
use crate::insn::*;
use crate::mem::{MemFault, MemFaultKind, MemoryAccess};
use crate::syscall::{SyscallAction, SyscallHandler};
use crate::timing::{AccuracyLevel, TimingBackend};
use crate::types::Addr;

// --- Mock Decoder ---

struct NoopDecoder;

impl Decoder for NoopDecoder {
    fn decode(&self, pc: Addr, _bytes: &[u8]) -> Result<DecodedInsn, crate::HelmError> {
        Ok(DecodedInsn {
            pc,
            len: 4,
            class: InsnClass::Nop,
            flags: InsnFlags::NOP,
            ..DecodedInsn::default()
        })
    }

    fn min_insn_size(&self) -> usize {
        4
    }
}

// --- Mock Executor ---

struct NoopExecutor;

impl Executor for NoopExecutor {
    fn execute(
        &mut self,
        insn: &DecodedInsn,
        _cpu: &mut dyn CpuState,
        _mem: &mut dyn MemoryAccess,
    ) -> ExecOutcome {
        ExecOutcome {
            next_pc: insn.pc + insn.len as u64,
            ..ExecOutcome::default()
        }
    }
}

// --- Mock TimingBackend ---

struct ZeroTiming;

impl TimingBackend for ZeroTiming {
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::FE
    }
    fn account(&mut self, _insn: &DecodedInsn, _outcome: &ExecOutcome) -> u64 {
        0
    }
}

// --- Mock SyscallHandler ---

struct ExitHandler;

impl SyscallHandler for ExitHandler {
    fn handle(
        &mut self,
        nr: u64,
        _cpu: &mut dyn CpuState,
        _mem: &mut dyn MemoryAccess,
    ) -> SyscallAction {
        SyscallAction::Exit { code: nr }
    }
}

// --- Mock CPU and Memory (reuse patterns from cpu.rs/mem.rs tests) ---

struct TinyCpu {
    pc: Addr,
    regs: [u64; 4],
}

impl CpuState for TinyCpu {
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

struct NullMem;

impl MemoryAccess for NullMem {
    fn read(&mut self, addr: Addr, _size: usize) -> Result<u64, MemFault> {
        Err(MemFault { addr, is_write: false, kind: MemFaultKind::Unmapped })
    }
    fn write(&mut self, addr: Addr, _size: usize, _val: u64) -> Result<(), MemFault> {
        Err(MemFault { addr, is_write: true, kind: MemFaultKind::Unmapped })
    }
    fn fetch(&mut self, _addr: Addr, buf: &mut [u8]) -> Result<(), MemFault> {
        buf.fill(0);
        Ok(())
    }
}

#[test]
fn decoder_produces_valid_insn() {
    let dec = NoopDecoder;
    let insn = dec.decode(0x1000, &[0; 4]).unwrap();
    assert_eq!(insn.pc, 0x1000);
    assert_eq!(insn.len, 4);
    assert_eq!(insn.class, InsnClass::Nop);
    assert_eq!(dec.min_insn_size(), 4);
}

#[test]
fn executor_advances_pc() {
    let mut exec = NoopExecutor;
    let insn = DecodedInsn {
        pc: 0x1000,
        len: 4,
        ..DecodedInsn::default()
    };
    let mut cpu = TinyCpu { pc: 0x1000, regs: [0; 4] };
    let mut mem = NullMem;
    let outcome = exec.execute(&insn, &mut cpu, &mut mem);
    assert_eq!(outcome.next_pc, 0x1004);
}

#[test]
fn timing_backend_fe_returns_zero() {
    let mut timing = ZeroTiming;
    assert_eq!(timing.accuracy(), AccuracyLevel::FE);
    let insn = DecodedInsn::default();
    let outcome = ExecOutcome::default();
    assert_eq!(timing.account(&insn, &outcome), 0);
}

#[test]
fn syscall_handler_returns_exit() {
    let mut handler = ExitHandler;
    let mut cpu = TinyCpu { pc: 0, regs: [0; 4] };
    let mut mem = NullMem;
    match handler.handle(93, &mut cpu, &mut mem) {
        SyscallAction::Exit { code } => assert_eq!(code, 93),
        _ => panic!("expected Exit"),
    }
}

#[test]
fn accuracy_level_serialization() {
    let json = serde_json::to_string(&AccuracyLevel::ITE).unwrap();
    assert_eq!(json, "\"ITE\"");
    let parsed: AccuracyLevel = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, AccuracyLevel::ITE);
}
