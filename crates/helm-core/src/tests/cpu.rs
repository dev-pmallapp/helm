use crate::cpu::CpuState;
use crate::types::Addr;
use std::collections::HashMap;

struct MockCpu {
    pc: Addr,
    gprs: [u64; 32],
    vregs: [[u8; 32]; 32],
    flags_val: u64,
    sysregs: HashMap<u32, u64>,
}

impl MockCpu {
    fn new() -> Self {
        Self {
            pc: 0,
            gprs: [0; 32],
            vregs: [[0; 32]; 32],
            flags_val: 0,
            sysregs: HashMap::new(),
        }
    }
}

impl CpuState for MockCpu {
    fn pc(&self) -> Addr {
        self.pc
    }
    fn set_pc(&mut self, pc: Addr) {
        self.pc = pc;
    }
    fn gpr(&self, id: u16) -> u64 {
        self.gprs[id as usize]
    }
    fn set_gpr(&mut self, id: u16, val: u64) {
        self.gprs[id as usize] = val;
    }
    fn sysreg(&self, enc: u32) -> u64 {
        self.sysregs.get(&enc).copied().unwrap_or(0)
    }
    fn set_sysreg(&mut self, enc: u32, val: u64) {
        self.sysregs.insert(enc, val);
    }
    fn flags(&self) -> u64 {
        self.flags_val
    }
    fn set_flags(&mut self, f: u64) {
        self.flags_val = f;
    }
    fn privilege_level(&self) -> u8 {
        0
    }
    fn gpr_wide(&self, id: u16, dst: &mut [u8]) -> usize {
        let src = &self.vregs[id as usize];
        let n = dst.len().min(32);
        dst[..n].copy_from_slice(&src[..n]);
        n
    }
    fn set_gpr_wide(&mut self, id: u16, src: &[u8]) {
        let n = src.len().min(32);
        self.vregs[id as usize][..n].copy_from_slice(&src[..n]);
    }
}

#[test]
fn gpr_round_trip() {
    let mut cpu = MockCpu::new();
    for i in 0..32u16 {
        cpu.set_gpr(i, (i as u64) * 1000 + 42);
        assert_eq!(cpu.gpr(i), (i as u64) * 1000 + 42);
    }
}

#[test]
fn pc_round_trip() {
    let mut cpu = MockCpu::new();
    cpu.set_pc(0xDEAD_BEEF_CAFE);
    assert_eq!(cpu.pc(), 0xDEAD_BEEF_CAFE);
}

#[test]
fn sysreg_round_trip() {
    let mut cpu = MockCpu::new();
    cpu.set_sysreg(0xC200, 0x1234);
    assert_eq!(cpu.sysreg(0xC200), 0x1234);
    assert_eq!(cpu.sysreg(0x9999), 0); // unset returns 0
}

#[test]
fn flags_round_trip() {
    let mut cpu = MockCpu::new();
    cpu.set_flags(0xF000_0000);
    assert_eq!(cpu.flags(), 0xF000_0000);
}

#[test]
fn wide_reg_round_trip_16() {
    let mut cpu = MockCpu::new();
    let data = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    cpu.set_gpr_wide(0, &data);
    let mut out = [0u8; 16];
    let n = cpu.gpr_wide(0, &mut out);
    assert_eq!(n, 16);
    assert_eq!(out, data);
}

#[test]
fn wide_reg_round_trip_32() {
    let mut cpu = MockCpu::new();
    let data: [u8; 32] = std::array::from_fn(|i| i as u8);
    cpu.set_gpr_wide(5, &data);
    let mut out = [0u8; 32];
    let n = cpu.gpr_wide(5, &mut out);
    assert_eq!(n, 32);
    assert_eq!(out, data);
}

#[test]
fn wide_reg_default_returns_zero() {
    // Default implementation returns 0 (no wide regs)
    struct MinimalCpu;
    impl CpuState for MinimalCpu {
        fn pc(&self) -> Addr { 0 }
        fn set_pc(&mut self, _: Addr) {}
        fn gpr(&self, _: u16) -> u64 { 0 }
        fn set_gpr(&mut self, _: u16, _: u64) {}
        fn sysreg(&self, _: u32) -> u64 { 0 }
        fn set_sysreg(&mut self, _: u32, _: u64) {}
        fn flags(&self) -> u64 { 0 }
        fn set_flags(&mut self, _: u64) {}
        fn privilege_level(&self) -> u8 { 0 }
    }

    let cpu = MinimalCpu;
    let mut out = [0u8; 16];
    assert_eq!(cpu.gpr_wide(0, &mut out), 0);
}
