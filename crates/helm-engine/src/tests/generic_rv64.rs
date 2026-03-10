//! End-to-end test: RV64 program through GenericSession.
//!
//! Proves the same GenericSession works with a completely different ISA.

use crate::generic_session::{GenericSession, GenericStopReason};
use helm_core::cpu::CpuState;
use helm_core::mem::{MemFault, MemoryAccess};
use helm_core::syscall::{SyscallAction, SyscallHandler};
use helm_core::types::Addr;
use helm_isa::riscv::cpu_state::Rv64CpuState;
use helm_isa::riscv::decoder::Rv64Decoder;
use helm_isa::riscv::executor::Rv64Executor;
use helm_timing::NullBackend;
use std::collections::HashMap;

struct TestMem {
    data: HashMap<Addr, u8>,
}

impl TestMem {
    fn new() -> Self { Self { data: HashMap::new() } }
    fn write_u32(&mut self, addr: Addr, val: u32) {
        for (i, b) in val.to_le_bytes().iter().enumerate() {
            self.data.insert(addr + i as u64, *b);
        }
    }
}

impl MemoryAccess for TestMem {
    fn read(&mut self, addr: Addr, size: usize) -> Result<u64, MemFault> {
        let mut val = 0u64;
        for i in 0..size {
            val |= (*self.data.get(&(addr + i as u64)).unwrap_or(&0) as u64) << (i * 8);
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
            *b = *self.data.get(&(addr + i as u64)).unwrap_or(&0);
        }
        Ok(())
    }
}

/// RV64 syscall handler: ECALL with a7=93 → exit(a0).
struct Rv64SyscallHandler;

impl SyscallHandler for Rv64SyscallHandler {
    fn handle(
        &mut self,
        _nr: u64,
        cpu: &mut dyn CpuState,
        _mem: &mut dyn MemoryAccess,
    ) -> SyscallAction {
        let a7 = cpu.gpr(17); // syscall number in a7 (x17)
        let a0 = cpu.gpr(10); // first arg in a0 (x10)
        match a7 {
            93 => SyscallAction::Exit { code: a0 },
            _ => SyscallAction::Handled(0),
        }
    }
}

#[test]
fn rv64_compute_and_exit() {
    // Program: compute 10 + 20 + 30 = 60, then exit(60)
    //   ADDI x1, x0, 10     # x1 = 10
    //   ADDI x2, x0, 20     # x2 = 20
    //   ADDI x3, x0, 30     # x3 = 30
    //   ADD  x4, x1, x2     # x4 = 30
    //   ADD  x10, x4, x3    # x10 = 60 (a0 = exit code)
    //   ADDI x17, x0, 93    # x17 = 93 (a7 = exit syscall)
    //   ECALL                # exit(60)

    let mut mem = TestMem::new();
    mem.write_u32(0x1000, 0x00A00093); // ADDI x1, x0, 10
    mem.write_u32(0x1004, 0x01400113); // ADDI x2, x0, 20
    mem.write_u32(0x1008, 0x01E00193); // ADDI x3, x0, 30
    mem.write_u32(0x100C, 0x00208233); // ADD  x4, x1, x2
    mem.write_u32(0x1010, 0x00320533); // ADD  x10, x4, x3
    mem.write_u32(0x1014, 0x05D00893); // ADDI x17, x0, 93
    mem.write_u32(0x1018, 0x00000073); // ECALL

    let mut cpu = Rv64CpuState::new();
    cpu.set_pc(0x1000);

    let mut session = GenericSession::new(
        Rv64Decoder,
        Rv64Executor::new(),
        cpu,
        Box::new(mem),
        Box::new(NullBackend),
    );
    session.set_syscall_handler(Box::new(Rv64SyscallHandler));

    let reason = session.run_interpreted(100);

    assert_eq!(reason, GenericStopReason::Exit(60));
    assert_eq!(session.insn_count, 7);
}

#[test]
fn rv64_loop_and_exit() {
    // Program: sum 1..5 in a loop, exit with result
    //   ADDI x1, x0, 0      # sum = 0
    //   ADDI x2, x0, 1      # i = 1
    //   ADDI x3, x0, 6      # limit = 6
    // loop:
    //   ADD  x1, x1, x2     # sum += i
    //   ADDI x2, x2, 1      # i++
    //   BNE  x2, x3, loop   # if i != 6, loop (-8 bytes)
    //   ADD  x10, x1, x0    # a0 = sum
    //   ADDI x17, x0, 93    # a7 = exit
    //   ECALL

    let mut mem = TestMem::new();
    mem.write_u32(0x1000, 0x00000093); // ADDI x1, x0, 0
    mem.write_u32(0x1004, 0x00100113); // ADDI x2, x0, 1
    mem.write_u32(0x1008, 0x00600193); // ADDI x3, x0, 6
    // loop @ 0x100C:
    mem.write_u32(0x100C, 0x002080B3); // ADD  x1, x1, x2
    mem.write_u32(0x1010, 0x00110113); // ADDI x2, x2, 1
    mem.write_u32(0x1014, 0xFE311CE3); // BNE  x2, x3, -8
    // after loop:
    mem.write_u32(0x1018, 0x00008533); // ADD  x10, x1, x0
    mem.write_u32(0x101C, 0x05D00893); // ADDI x17, x0, 93
    mem.write_u32(0x1020, 0x00000073); // ECALL

    let mut cpu = Rv64CpuState::new();
    cpu.set_pc(0x1000);

    let mut session = GenericSession::new(
        Rv64Decoder,
        Rv64Executor::new(),
        cpu,
        Box::new(mem),
        Box::new(NullBackend),
    );
    session.set_syscall_handler(Box::new(Rv64SyscallHandler));

    let reason = session.run_interpreted(1000);

    // sum 1..5 = 1+2+3+4+5 = 15
    assert_eq!(reason, GenericStopReason::Exit(15));
}
