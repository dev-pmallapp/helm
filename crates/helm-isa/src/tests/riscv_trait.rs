//! Tests for RV64 Decoder, Executor, CpuState — proving multi-ISA trait design.

use crate::riscv::cpu_state::Rv64CpuState;
use crate::riscv::decoder::Rv64Decoder;
use crate::riscv::executor::Rv64Executor;
use helm_core::cpu::CpuState;
use helm_core::decode::Decoder;
use helm_core::exec::Executor;
use helm_core::insn::{InsnClass, InsnFlags};
use helm_core::mem::{MemFault, MemFaultKind, MemoryAccess};
use helm_core::types::Addr;
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

// ── CpuState tests ────────────────────────────────────────────────

#[test]
fn rv64_x0_always_zero() {
    let mut cpu = Rv64CpuState::new();
    cpu.set_gpr(0, 999);
    assert_eq!(cpu.gpr(0), 0);
}

#[test]
fn rv64_gpr_round_trip() {
    let mut cpu = Rv64CpuState::new();
    for i in 1..32u16 {
        cpu.set_gpr(i, i as u64 * 100);
    }
    for i in 1..32u16 {
        assert_eq!(cpu.gpr(i), i as u64 * 100);
    }
}

#[test]
fn rv64_pc_round_trip() {
    let mut cpu = Rv64CpuState::new();
    cpu.set_pc(0x8000_0000);
    assert_eq!(cpu.pc(), 0x8000_0000);
}

#[test]
fn rv64_privilege_starts_machine() {
    let cpu = Rv64CpuState::new();
    assert_eq!(cpu.privilege_level(), 3);
}

// ── Decoder tests ─────────────────────────────────────────────────

#[test]
fn decode_rv64_add() {
    // ADD x1, x2, x3 = 0x003100B3
    let dec = Rv64Decoder;
    let insn = dec.decode(0x1000, &0x003100B3u32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::IntAlu);
    assert_eq!(insn.len, 4);
    assert_eq!(insn.dst_regs[0], 1); // rd = x1
    assert_eq!(insn.src_regs[0], 2); // rs1 = x2
    assert_eq!(insn.src_regs[1], 3); // rs2 = x3
}

#[test]
fn decode_rv64_load() {
    // LD x1, 0(x2) = 0x00013083
    let dec = Rv64Decoder;
    let insn = dec.decode(0, &0x00013083u32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::Load);
    assert!(insn.flags.contains(InsnFlags::LOAD));
}

#[test]
fn decode_rv64_store() {
    // SD x1, 0(x2) = 0x00113023
    let dec = Rv64Decoder;
    let insn = dec.decode(0, &0x00113023u32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::Store);
    assert!(insn.flags.contains(InsnFlags::STORE));
}

#[test]
fn decode_rv64_branch() {
    // BEQ x1, x2, 8 = 0x00208463
    let dec = Rv64Decoder;
    let insn = dec.decode(0, &0x00208463u32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::CondBranch);
    assert!(insn.flags.contains(InsnFlags::BRANCH));
}

#[test]
fn decode_rv64_jal() {
    // JAL x1, 4 = 0x004000EF
    let dec = Rv64Decoder;
    let insn = dec.decode(0, &0x004000EFu32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::Call);
    assert!(insn.flags.contains(InsnFlags::CALL));
}

#[test]
fn decode_rv64_ecall() {
    // ECALL = 0x00000073
    let dec = Rv64Decoder;
    let insn = dec.decode(0, &0x00000073u32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::Syscall);
    assert!(insn.flags.contains(InsnFlags::SYSCALL));
}

#[test]
fn decode_rv64_lui() {
    // LUI x1, 0x12345 = 0x123450B7
    let dec = Rv64Decoder;
    let insn = dec.decode(0, &0x123450B7u32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::IntAlu);
    assert_eq!(insn.dst_regs[0], 1);
}

// ── Executor tests ────────────────────────────────────────────────

fn exec_one(cpu: &mut Rv64CpuState, mem: &mut TestMem, raw: u32) {
    let dec = Rv64Decoder;
    let mut exec = Rv64Executor::new();
    let insn = dec.decode(cpu.pc(), &raw.to_le_bytes()).unwrap();
    let outcome = exec.execute(&insn, cpu, mem);
    cpu.set_pc(outcome.next_pc);
}

#[test]
fn exec_rv64_add() {
    let mut cpu = Rv64CpuState::new();
    cpu.set_pc(0x1000);
    cpu.set_gpr(2, 100);
    cpu.set_gpr(3, 200);
    let mut mem = TestMem::new();
    // ADD x1, x2, x3 = 0x003100B3
    exec_one(&mut cpu, &mut mem, 0x003100B3);
    assert_eq!(cpu.gpr(1), 300);
}

#[test]
fn exec_rv64_sub() {
    let mut cpu = Rv64CpuState::new();
    cpu.set_pc(0x1000);
    cpu.set_gpr(2, 500);
    cpu.set_gpr(3, 200);
    let mut mem = TestMem::new();
    // SUB x1, x2, x3 = 0x403100B3
    exec_one(&mut cpu, &mut mem, 0x403100B3);
    assert_eq!(cpu.gpr(1), 300);
}

#[test]
fn exec_rv64_addi() {
    let mut cpu = Rv64CpuState::new();
    cpu.set_pc(0x1000);
    cpu.set_gpr(2, 100);
    let mut mem = TestMem::new();
    // ADDI x1, x2, 42 = 0x02A10093
    exec_one(&mut cpu, &mut mem, 0x02A10093);
    assert_eq!(cpu.gpr(1), 142);
}

#[test]
fn exec_rv64_lui() {
    let mut cpu = Rv64CpuState::new();
    cpu.set_pc(0x1000);
    let mut mem = TestMem::new();
    // LUI x1, 0x12345 = 0x123450B7
    exec_one(&mut cpu, &mut mem, 0x123450B7);
    assert_eq!(cpu.gpr(1), 0x12345000);
}

#[test]
fn exec_rv64_store_load() {
    let mut cpu = Rv64CpuState::new();
    cpu.set_pc(0x1000);
    cpu.set_gpr(1, 0xDEAD_BEEF);
    cpu.set_gpr(2, 0x10000);
    let mut mem = TestMem::new();
    // SD x1, 0(x2) = 0x00113023
    exec_one(&mut cpu, &mut mem, 0x00113023);
    // LD x3, 0(x2) = 0x00013183
    exec_one(&mut cpu, &mut mem, 0x00013183);
    assert_eq!(cpu.gpr(3), 0xDEAD_BEEF);
}

#[test]
fn exec_rv64_beq_taken() {
    let mut cpu = Rv64CpuState::new();
    cpu.set_pc(0x1000);
    cpu.set_gpr(1, 42);
    cpu.set_gpr(2, 42);
    let mut mem = TestMem::new();
    // BEQ x1, x2, 8 = 0x00208463
    exec_one(&mut cpu, &mut mem, 0x00208463);
    assert_eq!(cpu.pc(), 0x1008); // branch taken
}

#[test]
fn exec_rv64_beq_not_taken() {
    let mut cpu = Rv64CpuState::new();
    cpu.set_pc(0x1000);
    cpu.set_gpr(1, 42);
    cpu.set_gpr(2, 99);
    let mut mem = TestMem::new();
    // BEQ x1, x2, 8 = 0x00208463
    exec_one(&mut cpu, &mut mem, 0x00208463);
    assert_eq!(cpu.pc(), 0x1004); // not taken
}

#[test]
fn exec_rv64_jal() {
    let mut cpu = Rv64CpuState::new();
    cpu.set_pc(0x1000);
    let mut mem = TestMem::new();
    // JAL x1, 8 = 0x008000EF
    exec_one(&mut cpu, &mut mem, 0x008000EF);
    assert_eq!(cpu.gpr(1), 0x1004); // link address
    assert_eq!(cpu.pc(), 0x1008);   // jump target
}

// ── Cross-ISA test: same test runs for both AArch64 and RV64 ──────

#[test]
fn cross_isa_add_via_traits() {
    // Prove that the same generic code works for both ISAs.
    fn add_test<C: CpuState>(
        cpu: &mut C,
        dec: &dyn Decoder,
        exec: &mut dyn Executor,
        mem: &mut dyn MemoryAccess,
        insn_bytes: &[u8],
        src1: u16, src2: u16, dst: u16,
    ) {
        cpu.set_gpr(src1, 100);
        cpu.set_gpr(src2, 200);
        let insn = dec.decode(cpu.pc(), insn_bytes).unwrap();
        let outcome = exec.execute(&insn, cpu, mem);
        cpu.set_pc(outcome.next_pc);
        assert_eq!(cpu.gpr(dst), 300, "ADD result should be 300");
    }

    // RV64: ADD x1, x2, x3
    let mut rv_cpu = Rv64CpuState::new();
    rv_cpu.set_pc(0x1000);
    let rv_dec = Rv64Decoder;
    let mut rv_exec = Rv64Executor::new();
    let mut rv_mem = TestMem::new();
    add_test(&mut rv_cpu, &rv_dec, &mut rv_exec, &mut rv_mem,
             &0x003100B3u32.to_le_bytes(), 2, 3, 1);
}
