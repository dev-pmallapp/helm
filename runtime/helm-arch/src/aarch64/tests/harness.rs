//! Test harness for AArch64 decode + execute tests.

use crate::aarch64::{decode, execute, Aarch64ArchState};
use helm_core::{AccessType, HartException, MemFault, MemInterface};
use std::collections::HashMap;

pub const CODE_BASE: u64 = 0x40_0000;
pub const STACK_BASE: u64 = 0x7FFF_8000;
pub const DATA_BASE: u64 = 0x10_0000;

pub struct TestMem {
    pub data: HashMap<u64, u8>,
}

impl TestMem {
    pub fn new() -> Self { Self { data: HashMap::new() } }

    pub fn map_zeroed(&mut self, addr: u64, size: u64) {
        for i in 0..size { self.data.entry(addr + i).or_insert(0); }
    }

    pub fn load(&mut self, addr: u64, bytes: &[u8]) {
        for (i, &b) in bytes.iter().enumerate() { self.data.insert(addr + i as u64, b); }
    }
    pub fn load_u64(&mut self, addr: u64, val: u64) { self.load(addr, &val.to_le_bytes()); }
    pub fn load_u32(&mut self, addr: u64, val: u32) { self.load(addr, &val.to_le_bytes()); }
    pub fn load_u16(&mut self, addr: u64, val: u16) { self.load(addr, &val.to_le_bytes()); }
    pub fn load_u8(&mut self, addr: u64, val: u8) { self.data.insert(addr, val); }

    pub fn read_u64(&mut self, addr: u64) -> u64 {
        let mut b = [0u8; 8];
        for i in 0..8usize { b[i] = *self.data.get(&(addr + i as u64)).unwrap_or(&0); }
        u64::from_le_bytes(b)
    }
    pub fn read_u32(&mut self, addr: u64) -> u32 {
        let mut b = [0u8; 4];
        for i in 0..4usize { b[i] = *self.data.get(&(addr + i as u64)).unwrap_or(&0); }
        u32::from_le_bytes(b)
    }
    pub fn read_u16(&mut self, addr: u64) -> u16 {
        let mut b = [0u8; 2];
        for i in 0..2usize { b[i] = *self.data.get(&(addr + i as u64)).unwrap_or(&0); }
        u16::from_le_bytes(b)
    }
    pub fn read_u8(&mut self, addr: u64) -> u8 { *self.data.get(&addr).unwrap_or(&0) }
}

impl MemInterface for TestMem {
    fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        let mut val = 0u64;
        for i in 0..size {
            val |= (*self.data.get(&(addr + i as u64)).unwrap_or(&0) as u64) << (i * 8);
        }
        Ok(val)
    }
    fn write(&mut self, addr: u64, size: usize, val: u64, _ty: AccessType) -> Result<(), MemFault> {
        for i in 0..size { self.data.insert(addr + i as u64, (val >> (i * 8)) as u8); }
        Ok(())
    }
}

pub fn cpu_with_code(insns: &[u32]) -> (Aarch64ArchState, TestMem) {
    let mut mem = TestMem::new();
    let code_size = (insns.len() * 4 + 0x1000) as u64;
    mem.map_zeroed(CODE_BASE, code_size);
    for (i, &insn) in insns.iter().enumerate() {
        mem.load_u32(CODE_BASE + (i as u64 * 4), insn);
    }
    mem.map_zeroed(0x7FFF_0000, 0x10000);
    mem.map_zeroed(DATA_BASE, 0x4000);

    let mut a = Aarch64ArchState::new();
    a.pc = CODE_BASE;
    a.sp = STACK_BASE;
    a.sp_el1 = STACK_BASE;
    a.current_el = 1;
    a.spsel = true;
    (a, mem)
}

pub fn step(a: &mut Aarch64ArchState, mem: &mut TestMem) -> Result<(), HartException> {
    let raw = mem.read(a.pc, 4, AccessType::Fetch)
        .map_err(|_| HartException::InstructionAccessFault { addr: a.pc })? as u32;
    let insn = decode(raw, a.pc)
        .map_err(|_| HartException::IllegalInstruction { pc: a.pc, raw })?;
    let pc_written = execute(&insn, a, mem)?;
    if !pc_written { a.pc = a.pc.wrapping_add(4); }
    Ok(())
}

pub fn set_nzcv(a: &mut Aarch64ArchState, n: bool, z: bool, c: bool, v: bool) {
    a.nzcv = ((n as u32) << 31) | ((z as u32) << 30) | ((c as u32) << 29) | ((v as u32) << 28);
}

#[inline(always)] pub fn flag_n(a: &Aarch64ArchState) -> bool { a.nzcv >> 31 != 0 }
#[inline(always)] pub fn flag_z(a: &Aarch64ArchState) -> bool { a.nzcv & (1 << 30) != 0 }
#[inline(always)] pub fn flag_c(a: &Aarch64ArchState) -> bool { a.nzcv & (1 << 29) != 0 }
#[inline(always)] pub fn flag_v(a: &Aarch64ArchState) -> bool { a.nzcv & (1 << 28) != 0 }
