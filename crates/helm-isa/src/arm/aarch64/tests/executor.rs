//! Tests for Aarch64TraitExecutor and Aarch64TraitDecoder.

use crate::arm::aarch64::cpu_state::Aarch64CpuState;
use crate::arm::aarch64::executor::Aarch64TraitExecutor;
use crate::arm::aarch64::trait_decoder::Aarch64TraitDecoder;
use helm_core::cpu::CpuState;
use helm_core::decode::Decoder;
use helm_core::exec::Executor;
use helm_core::insn::InsnClass;
use helm_core::mem::{MemFault, MemFaultKind, MemoryAccess};
use helm_core::types::Addr;
use std::collections::HashMap;

/// Simple flat memory for testing.
struct TestMem {
    data: HashMap<Addr, u8>,
}

impl TestMem {
    fn new() -> Self {
        Self { data: HashMap::new() }
    }

    fn write_u32(&mut self, addr: Addr, val: u32) {
        for (i, b) in val.to_le_bytes().iter().enumerate() {
            self.data.insert(addr + i as u64, *b);
        }
    }

    fn map_region(&mut self, base: Addr, size: u64) {
        for i in 0..size {
            self.data.entry(base + i).or_insert(0);
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

fn setup(insns: &[u32]) -> (Aarch64CpuState, TestMem, Aarch64TraitDecoder, Aarch64TraitExecutor) {
    let mut mem = TestMem::new();
    // Map code region
    mem.map_region(0x1000, 0x1000);
    // Map data/stack region
    mem.map_region(0x10000, 0x10000);

    for (i, insn) in insns.iter().enumerate() {
        mem.write_u32(0x1000 + (i as u64) * 4, *insn);
    }

    let mut cpu = Aarch64CpuState::new();
    cpu.set_pc(0x1000);
    cpu.set_gpr(31, 0x18000); // SP

    let decoder = Aarch64TraitDecoder;
    let executor = Aarch64TraitExecutor::new();

    (cpu, mem, decoder, executor)
}

fn step_one(
    cpu: &mut Aarch64CpuState,
    mem: &mut TestMem,
    dec: &Aarch64TraitDecoder,
    exec: &mut Aarch64TraitExecutor,
) {
    let pc = cpu.pc();
    let mut buf = [0u8; 4];
    mem.fetch(pc, &mut buf).unwrap();
    let insn = dec.decode(pc, &buf).unwrap();
    let outcome = exec.execute(&insn, cpu, mem);
    if outcome.exception.is_none() {
        cpu.set_pc(outcome.next_pc);
    }
}

// ── Decoder Tests ──────────────────────────────────────────────────

#[test]
fn decode_add_x0_x1_x2() {
    // ADD X0, X1, X2 = 0x8B020020
    let dec = Aarch64TraitDecoder;
    let insn = dec.decode(0x1000, &0x8B020020u32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::IntAlu);
    assert_eq!(insn.len, 4);
    assert_eq!(insn.pc, 0x1000);
}

#[test]
fn decode_ldr_x0() {
    // LDR X0, [X1] = 0xF9400020
    let dec = Aarch64TraitDecoder;
    let insn = dec.decode(0, &0xF9400020u32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::Load);
    assert!(insn.flags.contains(helm_core::insn::InsnFlags::LOAD));
}

#[test]
fn decode_str_x0() {
    // STR X0, [X1] = 0xF9000020
    let dec = Aarch64TraitDecoder;
    let insn = dec.decode(0, &0xF9000020u32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::Store);
    assert!(insn.flags.contains(helm_core::insn::InsnFlags::STORE));
}

#[test]
fn decode_b() {
    // B #4 = 0x14000001
    let dec = Aarch64TraitDecoder;
    let insn = dec.decode(0, &0x14000001u32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::Branch);
    assert!(insn.flags.contains(helm_core::insn::InsnFlags::BRANCH));
}

#[test]
fn decode_bl() {
    // BL #4 = 0x94000001
    let dec = Aarch64TraitDecoder;
    let insn = dec.decode(0, &0x94000001u32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::Call);
    assert!(insn.flags.contains(helm_core::insn::InsnFlags::CALL));
}

#[test]
fn decode_ret() {
    // RET = 0xD65F03C0
    let dec = Aarch64TraitDecoder;
    let insn = dec.decode(0, &0xD65F03C0u32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::Return);
    assert!(insn.flags.contains(helm_core::insn::InsnFlags::RETURN));
}

#[test]
fn decode_stp() {
    // STP X29, X30, [SP, #-16]! = 0xA9BF7BFD
    let dec = Aarch64TraitDecoder;
    let insn = dec.decode(0, &0xA9BF7BFDu32.to_le_bytes()).unwrap();
    assert_eq!(insn.class, InsnClass::StorePair);
    assert!(insn.flags.contains(helm_core::insn::InsnFlags::STORE));
    assert!(insn.flags.contains(helm_core::insn::InsnFlags::PAIR));
}

// ── Executor Tests ─────────────────────────────────────────────────

#[test]
fn executor_add_x0_x1_x2() {
    // ADD X0, X1, X2 = 0x8B020020
    let (mut cpu, mut mem, dec, mut exec) = setup(&[0x8B020020]);
    cpu.set_gpr(1, 100);
    cpu.set_gpr(2, 200);

    step_one(&mut cpu, &mut mem, &dec, &mut exec);

    assert_eq!(cpu.gpr(0), 300);
    assert_eq!(cpu.pc(), 0x1004);
}

#[test]
fn executor_sub_x0_x1_x2() {
    // SUB X0, X1, X2 = 0xCB020020
    let (mut cpu, mut mem, dec, mut exec) = setup(&[0xCB020020]);
    cpu.set_gpr(1, 500);
    cpu.set_gpr(2, 200);

    step_one(&mut cpu, &mut mem, &dec, &mut exec);

    assert_eq!(cpu.gpr(0), 300);
}

#[test]
fn executor_movz() {
    // MOVZ X0, #42 = 0xD2800540
    let (mut cpu, mut mem, dec, mut exec) = setup(&[0xD2800540]);

    step_one(&mut cpu, &mut mem, &dec, &mut exec);

    assert_eq!(cpu.gpr(0), 42);
}

#[test]
fn executor_str_ldr_round_trip() {
    // STR X0, [X1]     = 0xF9000020
    // LDR X2, [X1]     = 0xF9400022
    let (mut cpu, mut mem, dec, mut exec) = setup(&[0xF9000020, 0xF9400022]);
    cpu.set_gpr(0, 0xDEAD_BEEF);
    cpu.set_gpr(1, 0x10000);

    step_one(&mut cpu, &mut mem, &dec, &mut exec);
    step_one(&mut cpu, &mut mem, &dec, &mut exec);

    assert_eq!(cpu.gpr(2), 0xDEAD_BEEF);
}

#[test]
fn executor_branch() {
    // B #8 (skip one instruction) = 0x14000002
    let (mut cpu, mut mem, dec, mut exec) = setup(&[0x14000002]);

    step_one(&mut cpu, &mut mem, &dec, &mut exec);

    assert_eq!(cpu.pc(), 0x1008); // skipped to PC+8
}

#[test]
fn executor_multi_instruction_sequence() {
    // MOVZ X0, #10     = 0xD2800140
    // MOVZ X1, #20     = 0xD2800281
    // ADD X2, X0, X1   = 0x8B010002
    let (mut cpu, mut mem, dec, mut exec) = setup(&[0xD2800140, 0xD2800281, 0x8B010002]);

    step_one(&mut cpu, &mut mem, &dec, &mut exec);
    step_one(&mut cpu, &mut mem, &dec, &mut exec);
    step_one(&mut cpu, &mut mem, &dec, &mut exec);

    assert_eq!(cpu.gpr(0), 10);
    assert_eq!(cpu.gpr(1), 20);
    assert_eq!(cpu.gpr(2), 30);
}
