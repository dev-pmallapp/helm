//! Bulk parametric AArch64 instruction tests.
//!
//! Uses macros to systematically generate tests for every instruction
//! variant × operand width × interesting boundary value.

use crate::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

fn cpu_exec(insns: &[u32]) -> (Aarch64Cpu, AddressSpace) {
    let mut cpu = Aarch64Cpu::new();
    let mut mem = AddressSpace::new();
    let base = 0x40_0000u64;
    let size = (insns.len() * 4 + 0x1000) as u64;
    mem.map(base, size, (true, true, true));
    for (i, insn) in insns.iter().enumerate() {
        let addr = base + (i as u64 * 4);
        mem.write(addr, &insn.to_le_bytes()).unwrap();
    }
    cpu.regs.pc = base;
    mem.map(0x7FFF_0000, 0x10000, (true, true, false));
    cpu.regs.sp = 0x7FFF_8000;
    mem.map(0x10_0000, 0x2000, (true, true, false));
    (cpu, mem)
}

fn set_flags(cpu: &mut Aarch64Cpu, n: bool, z: bool, c: bool, v: bool) {
    cpu.regs.nzcv = ((n as u32) << 31) | ((z as u32) << 30) | ((c as u32) << 29) | ((v as u32) << 28);
}

// ── Encoding helpers ──

fn dp2(sf: u32, op: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b0011010110 << 21) | (rm << 16) | (op << 10) | (rn << 5) | rd
}
fn dp1(sf: u32, op: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b1011010110 << 21) | (op << 10) | (rn << 5) | rd
}
fn csel_fam(sf: u32, inv: u32, inc: u32, rm: u32, cond: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (inv << 30) | (0b011010100 << 21) | (rm << 16) | (cond << 12) | (inc << 10) | (rn << 5) | rd
}
fn add_sub_imm(sf: u32, op: u32, s: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b10001 << 24) | (sh << 22) | (imm12 << 10) | (rn << 5) | rd
}
fn log_imm(sf: u32, opc: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100100 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn adc_fam(sf: u32, op: u32, s: u32, rm: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b11010000 << 21) | (rm << 16) | (rn << 5) | rd
}
fn add_sub_reg(sf: u32, op: u32, s: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (op << 30) | (s << 29) | (0b01011 << 24) | (shift << 22) | (rm << 16) | (imm6 << 10) | (rn << 5) | rd
}
fn log_reg(sf: u32, opc: u32, n: u32, shift: u32, rm: u32, imm6: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b01010 << 24) | (shift << 22) | (n << 21) | (rm << 16) | (imm6 << 10) | (rn << 5) | rd
}
fn bitfield(sf: u32, opc: u32, n: u32, immr: u32, imms: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100110 << 23) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
fn mov_wide(sf: u32, opc: u32, hw: u32, imm16: u32, rd: u32) -> u32 {
    (sf << 31) | (opc << 29) | (0b100101 << 23) | (hw << 21) | (imm16 << 5) | rd
}
fn str_x(imm12: u32, rn: u32, rt: u32) -> u32 { (0b11111001_00 << 22) | (imm12 << 10) | (rn << 5) | rt }
fn ldr_x(imm12: u32, rn: u32, rt: u32) -> u32 { (0b11111001_01 << 22) | (imm12 << 10) | (rn << 5) | rt }

// ── Macro for bulk test generation ──

macro_rules! test_dp2 {
    ($name:ident, $sf:expr, $op:expr, $a:expr, $b:expr, $expected:expr) => {
        #[test] fn $name() {
            let (mut c, mut m) = cpu_exec(&[dp2($sf, $op, 2, 1, 0)]);
            c.set_xn(1, $a); c.set_xn(2, $b);
            c.step(&mut m).unwrap();
            assert_eq!(c.xn(0), $expected);
        }
    };
}

macro_rules! test_dp1 {
    ($name:ident, $sf:expr, $op:expr, $a:expr, $expected:expr) => {
        #[test] fn $name() {
            let (mut c, mut m) = cpu_exec(&[dp1($sf, $op, 1, 0)]);
            c.set_xn(1, $a);
            c.step(&mut m).unwrap();
            assert_eq!(c.xn(0), $expected);
        }
    };
}

macro_rules! test_csel {
    ($name:ident, $sf:expr, $inv:expr, $inc:expr, $cond:expr, $rn_val:expr, $rm_val:expr,
     $n:expr, $z:expr, $c:expr, $v:expr, $expected:expr) => {
        #[test] fn $name() {
            let (mut c, mut m) = cpu_exec(&[csel_fam($sf, $inv, $inc, 2, $cond, 1, 0)]);
            c.set_xn(1, $rn_val); c.set_xn(2, $rm_val);
            set_flags(&mut c, $n, $z, $c, $v);
            c.step(&mut m).unwrap();
            assert_eq!(c.xn(0), $expected);
        }
    };
}

macro_rules! test_addsub_imm {
    ($name:ident, $sf:expr, $op:expr, $s:expr, $imm:expr, $a:expr, $expected:expr) => {
        #[test] fn $name() {
            let (mut c, mut m) = cpu_exec(&[add_sub_imm($sf, $op, $s, 0, $imm, 1, 0)]);
            c.set_xn(1, $a);
            c.step(&mut m).unwrap();
            assert_eq!(c.xn(0), $expected);
        }
    };
}

macro_rules! test_addsub_imm_flags {
    ($name:ident, $sf:expr, $op:expr, $imm:expr, $a:expr, $n:expr, $z:expr, $c:expr, $v:expr) => {
        #[test] fn $name() {
            let (mut c, mut m) = cpu_exec(&[add_sub_imm($sf, $op, 1, 0, $imm, 1, 31)]);
            c.set_xn(1, $a);
            c.step(&mut m).unwrap();
            assert_eq!(c.regs.n(), $n, "N flag");
            assert_eq!(c.regs.z(), $z, "Z flag");
            assert_eq!(c.regs.c(), $c, "C flag");
            assert_eq!(c.regs.v(), $v, "V flag");
        }
    };
}

macro_rules! test_adc {
    ($name:ident, $sf:expr, $op:expr, $s:expr, $a:expr, $b:expr, $carry_in:expr, $expected:expr) => {
        #[test] fn $name() {
            let (mut c, mut m) = cpu_exec(&[adc_fam($sf, $op, $s, 2, 1, 0)]);
            c.set_xn(1, $a); c.set_xn(2, $b);
            set_flags(&mut c, false, false, $carry_in, false);
            c.step(&mut m).unwrap();
            assert_eq!(c.xn(0), $expected);
        }
    };
}

macro_rules! test_log_reg {
    ($name:ident, $sf:expr, $opc:expr, $n:expr, $a:expr, $b:expr, $expected:expr) => {
        #[test] fn $name() {
            let (mut c, mut m) = cpu_exec(&[log_reg($sf, $opc, $n, 0, 2, 0, 1, 0)]);
            c.set_xn(1, $a); c.set_xn(2, $b);
            c.step(&mut m).unwrap();
            assert_eq!(c.xn(0), $expected);
        }
    };
}

macro_rules! test_addsub_reg {
    ($name:ident, $sf:expr, $op:expr, $s:expr, $a:expr, $b:expr, $expected:expr) => {
        #[test] fn $name() {
            let (mut c, mut m) = cpu_exec(&[add_sub_reg($sf, $op, $s, 0, 2, 0, 1, 0)]);
            c.set_xn(1, $a); c.set_xn(2, $b);
            c.step(&mut m).unwrap();
            assert_eq!(c.xn(0), $expected);
        }
    };
}

// ===================================================================
//  UDIV — boundary values
// ===================================================================
test_dp2!(udiv_64_1,       1, 0b000010, 100, 10, 10);
test_dp2!(udiv_64_max,     1, 0b000010, u64::MAX, 1, u64::MAX);
test_dp2!(udiv_64_by1,     1, 0b000010, 42, 1, 42);
test_dp2!(udiv_64_by0,     1, 0b000010, 42, 0, 0);
test_dp2!(udiv_64_eq,      1, 0b000010, 7, 7, 1);
test_dp2!(udiv_64_lt,      1, 0b000010, 3, 7, 0);
test_dp2!(udiv_32_1,       0, 0b000010, 100, 10, 10);
test_dp2!(udiv_32_by0,     0, 0b000010, 42, 0, 0);
test_dp2!(udiv_32_max,     0, 0b000010, 0xFFFF_FFFF, 1, 0xFFFF_FFFF);
test_dp2!(udiv_32_trunc,   0, 0b000010, 0x1_0000_000A, 2, 5);

// ===================================================================
//  SDIV — boundary values
// ===================================================================
test_dp2!(sdiv_64_pos,     1, 0b000011, 100, 7, 14);
test_dp2!(sdiv_64_neg,     1, 0b000011, (-100i64) as u64, 7, (-14i64) as u64);
test_dp2!(sdiv_64_neg_neg, 1, 0b000011, (-100i64) as u64, (-7i64) as u64, 14);
test_dp2!(sdiv_64_by0,     1, 0b000011, 42, 0, 0);
test_dp2!(sdiv_64_min_m1,  1, 0b000011, 0x8000_0000_0000_0000, (-1i64) as u64, 0x8000_0000_0000_0000);
test_dp2!(sdiv_32_pos,     0, 0b000011, 100, 7, 14);
test_dp2!(sdiv_32_neg,     0, 0b000011, (-100i32) as u32 as u64, 7, (-14i32) as u32 as u64);
test_dp2!(sdiv_32_by0,     0, 0b000011, 42, 0, 0);

// ===================================================================
//  LSLV / LSRV / ASRV / RORV — shift amounts and boundaries
// ===================================================================
test_dp2!(lslv_64_by0,    1, 0b001000, 0xFF, 0, 0xFF);
test_dp2!(lslv_64_by1,    1, 0b001000, 1, 1, 2);
test_dp2!(lslv_64_by63,   1, 0b001000, 1, 63, 0x8000_0000_0000_0000);
test_dp2!(lslv_64_by64,   1, 0b001000, 1, 64, 1);  // mod 64
test_dp2!(lslv_32_by0,    0, 0b001000, 0xFF, 0, 0xFF);
test_dp2!(lslv_32_by31,   0, 0b001000, 1, 31, 0x8000_0000);
test_dp2!(lslv_32_by32,   0, 0b001000, 1, 32, 1);  // mod 32
test_dp2!(lsrv_64_by0,    1, 0b001001, 0xFF, 0, 0xFF);
test_dp2!(lsrv_64_by1,    1, 0b001001, 2, 1, 1);
test_dp2!(lsrv_64_msb,    1, 0b001001, 0x8000_0000_0000_0000, 63, 1);
test_dp2!(lsrv_32_by0,    0, 0b001001, 0xFF, 0, 0xFF);
test_dp2!(lsrv_32_msb,    0, 0b001001, 0x8000_0000, 31, 1);
test_dp2!(asrv_64_pos,    1, 0b001010, 0x100, 4, 0x10);
test_dp2!(asrv_64_neg,    1, 0b001010, 0x8000_0000_0000_0000, 4, 0xF800_0000_0000_0000);
test_dp2!(asrv_64_neg63,  1, 0b001010, 0x8000_0000_0000_0000, 63, u64::MAX);
test_dp2!(asrv_32_pos,    0, 0b001010, 0x100, 4, 0x10);
test_dp2!(asrv_32_neg,    0, 0b001010, 0x8000_0000, 4, 0xF800_0000);
test_dp2!(rorv_64_by0,    1, 0b001011, 0xFF, 0, 0xFF);
test_dp2!(rorv_64_by1,    1, 0b001011, 1, 1, 0x8000_0000_0000_0000);
test_dp2!(rorv_64_by32,   1, 0b001011, 0xDEAD_BEEF_0000_0000, 32, 0x0000_0000_DEAD_BEEF);

// ===================================================================
//  CLZ — boundary values
// ===================================================================
test_dp1!(clz_64_zero,    1, 0b000100, 0, 64);
test_dp1!(clz_64_one,     1, 0b000100, 1, 63);
test_dp1!(clz_64_msb,     1, 0b000100, 0x8000_0000_0000_0000, 0);
test_dp1!(clz_64_mid,     1, 0b000100, 0x00FF_0000, 40);
test_dp1!(clz_32_zero,    0, 0b000100, 0, 32);
test_dp1!(clz_32_one,     0, 0b000100, 1, 31);
test_dp1!(clz_32_msb,     0, 0b000100, 0x8000_0000, 0);
test_dp1!(clz_32_byte,    0, 0b000100, 0xFF, 24);

// ===================================================================
//  REV / REV16 / RBIT
// ===================================================================
test_dp1!(rev_64_swap,    1, 0b000011, 0x0102030405060708, 0x0807060504030201);
test_dp1!(rev_32_swap,    0, 0b000011, 0x01020304, 0x04030201);
test_dp1!(rev16_64_swap,  1, 0b000001, 0x0102030405060708, 0x0201040306050807);
test_dp1!(rev16_32_swap,  0, 0b000001, 0x01020304, 0x02010403);
test_dp1!(rbit_64_one,    1, 0b000000, 1, 0x8000_0000_0000_0000);
test_dp1!(rbit_32_one,    0, 0b000000, 1, 0x8000_0000);
test_dp1!(rbit_64_zero,   1, 0b000000, 0, 0);
test_dp1!(rbit_64_max,    1, 0b000000, u64::MAX, u64::MAX);

// ===================================================================
//  CSEL / CSINC — all 15 conditions × taken/not-taken
// ===================================================================
// EQ(0) NE(1) CS(2) CC(3) MI(4) PL(5) VS(6) VC(7) HI(8) LS(9) GE(10) LT(11) GT(12) LE(13) AL(14)
test_csel!(csel64_eq_t,  1,0,0, 0,  10,20, false,true,false,false, 10);
test_csel!(csel64_eq_f,  1,0,0, 0,  10,20, false,false,false,false, 20);
test_csel!(csel64_ne_t,  1,0,0, 1,  10,20, false,false,false,false, 10);
test_csel!(csel64_ne_f,  1,0,0, 1,  10,20, false,true,false,false, 20);
test_csel!(csel64_cs_t,  1,0,0, 2,  10,20, false,false,true,false, 10);
test_csel!(csel64_cs_f,  1,0,0, 2,  10,20, false,false,false,false, 20);
test_csel!(csel64_cc_t,  1,0,0, 3,  10,20, false,false,false,false, 10);
test_csel!(csel64_cc_f,  1,0,0, 3,  10,20, false,false,true,false, 20);
test_csel!(csel64_mi_t,  1,0,0, 4,  10,20, true,false,false,false, 10);
test_csel!(csel64_mi_f,  1,0,0, 4,  10,20, false,false,false,false, 20);
test_csel!(csel64_pl_t,  1,0,0, 5,  10,20, false,false,false,false, 10);
test_csel!(csel64_pl_f,  1,0,0, 5,  10,20, true,false,false,false, 20);
test_csel!(csel64_vs_t,  1,0,0, 6,  10,20, false,false,false,true, 10);
test_csel!(csel64_vs_f,  1,0,0, 6,  10,20, false,false,false,false, 20);
test_csel!(csel64_vc_t,  1,0,0, 7,  10,20, false,false,false,false, 10);
test_csel!(csel64_vc_f,  1,0,0, 7,  10,20, false,false,false,true, 20);
test_csel!(csel64_hi_t,  1,0,0, 8,  10,20, false,false,true,false, 10);
test_csel!(csel64_hi_f,  1,0,0, 8,  10,20, false,true,true,false, 20);
test_csel!(csel64_ls_t,  1,0,0, 9,  10,20, false,true,false,false, 10);
test_csel!(csel64_ls_f,  1,0,0, 9,  10,20, false,false,true,false, 20);
test_csel!(csel64_ge_t,  1,0,0, 10, 10,20, false,false,false,false, 10);
test_csel!(csel64_ge_f,  1,0,0, 10, 10,20, true,false,false,false, 20);
test_csel!(csel64_lt_t,  1,0,0, 11, 10,20, true,false,false,false, 10);
test_csel!(csel64_lt_f,  1,0,0, 11, 10,20, false,false,false,false, 20);
test_csel!(csel64_gt_t,  1,0,0, 12, 10,20, false,false,false,false, 10);
test_csel!(csel64_gt_f,  1,0,0, 12, 10,20, false,true,false,false, 20);
test_csel!(csel64_le_t,  1,0,0, 13, 10,20, false,true,false,false, 10);
test_csel!(csel64_le_f,  1,0,0, 13, 10,20, false,false,false,false, 20);
test_csel!(csel64_al,    1,0,0, 14, 10,20, false,false,false,false, 10);
// 32-bit variants
test_csel!(csel32_eq_t,  0,0,0, 0,  0x1_0000_000A,20, false,true,false,false, 0xA);
test_csel!(csel32_ne_t,  0,0,0, 1,  0x1_0000_000A,20, false,false,false,false, 0xA);
// CSINC
test_csel!(csinc64_eq_t, 1,0,1, 0,  10,20, false,true,false,false, 10);
test_csel!(csinc64_eq_f, 1,0,1, 0,  10,20, false,false,false,false, 21);
test_csel!(csinc64_wrap, 1,0,1, 0,  10,u64::MAX, false,false,false,false, 0);
test_csel!(csinc32_eq_f, 0,0,1, 0,  10,0xFFFF_FFFF, false,false,false,false, 0);
// CSINV
test_csel!(csinv64_eq_f, 1,1,0, 0,  10,0, false,false,false,false, u64::MAX);
test_csel!(csinv64_eq_t, 1,1,0, 0,  10,0, false,true,false,false, 10);
// CSNEG
test_csel!(csneg64_eq_f, 1,1,1, 0,  10,5, false,false,false,false, (-5i64) as u64);
test_csel!(csneg64_eq_t, 1,1,1, 0,  10,5, false,true,false,false, 10);

// ===================================================================
//  ADD/SUB immediate — boundaries and flag checks
// ===================================================================
test_addsub_imm!(add64_0,     1,0,0, 0,    100, 100);
test_addsub_imm!(add64_max,   1,0,0, 0xFFF,0,   0xFFF);
test_addsub_imm!(add64_wrap,  1,0,0, 1,    u64::MAX, 0);
test_addsub_imm!(add32_wrap,  0,0,0, 1,    0xFFFF_FFFF, 0);
test_addsub_imm!(sub64_0,     1,1,0, 0,    100, 100);
test_addsub_imm!(sub64_basic, 1,1,0, 10,   50,  40);
test_addsub_imm!(sub64_under, 1,1,0, 1,    0,   u64::MAX);
test_addsub_imm!(sub32_under, 0,1,0, 1,    0,   0xFFFF_FFFF);
// Flag tests
test_addsub_imm_flags!(adds64_zero,  1,0, 0, 0,       false,true,false,false);
test_addsub_imm_flags!(adds64_neg,   1,0, 0, 0x8000_0000_0000_0000, true,false,false,false);
test_addsub_imm_flags!(adds64_carry, 1,0, 1, u64::MAX, false,true,true,false);
test_addsub_imm_flags!(adds64_ovf,   1,0, 1, 0x7FFF_FFFF_FFFF_FFFF, true,false,false,true);
test_addsub_imm_flags!(adds32_carry, 0,0, 1, 0xFFFF_FFFF, false,true,true,false);
test_addsub_imm_flags!(subs64_eq,    1,1, 42, 42,     false,true,true,false);
test_addsub_imm_flags!(subs64_gt,    1,1, 10, 42,     false,false,true,false);
test_addsub_imm_flags!(subs64_lt,    1,1, 100,42,     true,false,false,false);
test_addsub_imm_flags!(subs32_eq,    0,1, 1,  1,      false,true,true,false);

// ===================================================================
//  ADC / SBC — carry in/out
// ===================================================================
test_adc!(adc64_nc, 1,0,0, 10, 20, false, 30);
test_adc!(adc64_c,  1,0,0, 10, 20, true,  31);
test_adc!(adc64_max,1,0,0, u64::MAX, 0, true, 0);
test_adc!(adc32_nc, 0,0,0, 10, 20, false, 30);
test_adc!(adc32_c,  0,0,0, 10, 20, true,  31);
test_adc!(sbc64_nc, 1,1,0, 100, 30, true, 70);
test_adc!(sbc64_borrow, 1,1,0, 100, 30, false, 69);
test_adc!(sbc32_nc, 0,1,0, 100, 30, true, 70);

// ===================================================================
//  Logical register — AND/ORR/EOR/BIC/ORN/EON/ANDS/BICS
// ===================================================================
test_log_reg!(and_reg_64,   1, 0b00, 0, 0xFF00, 0x0FF0, 0x0F00);
test_log_reg!(orr_reg_64,   1, 0b01, 0, 0xFF00, 0x0FF0, 0xFFF0);
test_log_reg!(eor_reg_64,   1, 0b10, 0, 0xFF00, 0x0FF0, 0xF0F0);
test_log_reg!(bic_reg_64,   1, 0b00, 1, 0xFF00, 0x0FF0, 0xF000);
test_log_reg!(orn_reg_64,   1, 0b01, 1, 0, 0x0FF0, !0x0FF0u64);
test_log_reg!(eon_reg_64,   1, 0b10, 1, 0xFF00, 0x0FF0, !(0xFF00u64 ^ 0x0FF0));
test_log_reg!(and_reg_32,   0, 0b00, 0, 0xFF00, 0x0FF0, 0x0F00);
test_log_reg!(orr_reg_32,   0, 0b01, 0, 0xFF00, 0x0FF0, 0xFFF0);
test_log_reg!(eor_reg_32,   0, 0b10, 0, 0xFF00, 0x0FF0, 0xF0F0);
test_log_reg!(bic_reg_32,   0, 0b00, 1, 0xFF00, 0x0FF0, 0xF000);

// ===================================================================
//  ADD/SUB register — basic and with shift
// ===================================================================
test_addsub_reg!(add_reg_64,  1,0,0, 10, 20, 30);
test_addsub_reg!(add_reg_32,  0,0,0, 10, 20, 30);
test_addsub_reg!(sub_reg_64,  1,1,0, 50, 20, 30);
test_addsub_reg!(sub_reg_32,  0,1,0, 50, 20, 30);
test_addsub_reg!(adds_reg_64, 1,0,1, 10, 20, 30);
test_addsub_reg!(subs_reg_64, 1,1,1, 50, 20, 30);

#[test] fn add_reg_64_lsl2() {
    let (mut c, mut m) = cpu_exec(&[add_sub_reg(1, 0, 0, 0b00, 2, 2, 1, 0)]);
    c.set_xn(1, 10); c.set_xn(2, 3);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 10 + (3 << 2), "ADD X0, X1, X2, LSL #2");
}
#[test] fn sub_reg_64_lsr4() {
    let (mut c, mut m) = cpu_exec(&[add_sub_reg(1, 1, 0, 0b01, 2, 4, 1, 0)]);
    c.set_xn(1, 100); c.set_xn(2, 0x100);
    c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 100 - (0x100 >> 4), "SUB X0, X1, X2, LSR #4");
}

// ===================================================================
//  MOV wide — MOVZ/MOVN/MOVK all hw positions
// ===================================================================

#[test] fn movz_64_hw0() { let (mut c, mut m) = cpu_exec(&[mov_wide(1, 0b10, 0, 0xABCD, 0)]); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xABCD); }
#[test] fn movz_64_hw1() { let (mut c, mut m) = cpu_exec(&[mov_wide(1, 0b10, 1, 0xABCD, 0)]); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xABCD_0000); }
#[test] fn movz_64_hw2() { let (mut c, mut m) = cpu_exec(&[mov_wide(1, 0b10, 2, 0xABCD, 0)]); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xABCD_0000_0000); }
#[test] fn movz_64_hw3() { let (mut c, mut m) = cpu_exec(&[mov_wide(1, 0b10, 3, 0xABCD, 0)]); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xABCD_0000_0000_0000); }
#[test] fn movz_32_hw0() { let (mut c, mut m) = cpu_exec(&[mov_wide(0, 0b10, 0, 0xFFFF, 0)]); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xFFFF); }
#[test] fn movz_32_hw1() { let (mut c, mut m) = cpu_exec(&[mov_wide(0, 0b10, 1, 0xFFFF, 0)]); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xFFFF_0000); }
#[test] fn movn_64_hw0() { let (mut c, mut m) = cpu_exec(&[mov_wide(1, 0b00, 0, 0, 0)]); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), u64::MAX); }
#[test] fn movn_64_hw1() { let (mut c, mut m) = cpu_exec(&[mov_wide(1, 0b00, 1, 0xFFFF, 0)]); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xFFFF_FFFF_0000_FFFF); }
#[test] fn movk_64_hw0() {
    let (mut c, mut m) = cpu_exec(&[mov_wide(1, 0b11, 0, 0x1234, 0)]);
    c.set_xn(0, 0xFFFF_FFFF_FFFF_FFFF); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0xFFFF_FFFF_FFFF_1234);
}
#[test] fn movk_64_hw3() {
    let (mut c, mut m) = cpu_exec(&[mov_wide(1, 0b11, 3, 0x5678, 0)]);
    c.set_xn(0, 0); c.step(&mut m).unwrap();
    assert_eq!(c.xn(0), 0x5678_0000_0000_0000);
}

// ===================================================================
//  Bitfield — SBFM / UBFM / BFM edge cases
// ===================================================================

#[test] fn ubfm_lsr_64_by1()  { let (mut c, mut m) = cpu_exec(&[bitfield(1, 0b10, 1, 1, 63, 1, 0)]); c.set_xn(1, 0xF0); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0x78); }
#[test] fn ubfm_lsr_64_by32() { let (mut c, mut m) = cpu_exec(&[bitfield(1, 0b10, 1, 32, 63, 1, 0)]); c.set_xn(1, 0x1_0000_0000); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 1); }
#[test] fn ubfm_lsl_64_by8()  { let (mut c, mut m) = cpu_exec(&[bitfield(1, 0b10, 1, 56, 55, 1, 0)]); c.set_xn(1, 0xFF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xFF00); }
#[test] fn ubfm_uxtb_32()     { let (mut c, mut m) = cpu_exec(&[bitfield(0, 0b10, 0, 0, 7, 1, 0)]); c.set_xn(1, 0x1234_56FF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xFF); }
#[test] fn sbfm_sxtb_neg()    { let (mut c, mut m) = cpu_exec(&[bitfield(1, 0b00, 1, 0, 7, 1, 0)]); c.set_xn(1, 0x80); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xFFFF_FFFF_FFFF_FF80); }
#[test] fn sbfm_sxtb_pos()    { let (mut c, mut m) = cpu_exec(&[bitfield(1, 0b00, 1, 0, 7, 1, 0)]); c.set_xn(1, 0x7F); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0x7F); }
#[test] fn sbfm_sxth_neg()    { let (mut c, mut m) = cpu_exec(&[bitfield(1, 0b00, 1, 0, 15, 1, 0)]); c.set_xn(1, 0x8000); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xFFFF_FFFF_FFFF_8000); }
#[test] fn sbfm_sxtw_neg()    { let (mut c, mut m) = cpu_exec(&[bitfield(1, 0b00, 1, 0, 31, 1, 0)]); c.set_xn(1, 0x8000_0000); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xFFFF_FFFF_8000_0000); }
#[test] fn bfm_bfi_low8()     { let (mut c, mut m) = cpu_exec(&[bitfield(0, 0b01, 0, 0, 7, 1, 0)]); c.set_xn(0, 0xFF00); c.set_xn(1, 0xAB); c.step(&mut m).unwrap(); assert_eq!(c.xn(0), 0xFFAB); }

// ===================================================================
//  ADD/SUB shifted register — all shift types × multiple amounts
// ===================================================================

#[test] fn add_reg_64_lsl0()  { let (mut c, mut m) = cpu_exec(&[add_sub_reg(1,0,0,0b00,2,0,1,0)]); c.set_xn(1,10); c.set_xn(2,3); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),13); }
#[test] fn add_reg_64_lsl4()  { let (mut c, mut m) = cpu_exec(&[add_sub_reg(1,0,0,0b00,2,4,1,0)]); c.set_xn(1,10); c.set_xn(2,1); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),26); }
#[test] fn add_reg_64_lsl8()  { let (mut c, mut m) = cpu_exec(&[add_sub_reg(1,0,0,0b00,2,8,1,0)]); c.set_xn(1,0); c.set_xn(2,1); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),256); }
#[test] fn add_reg_64_lsr4()  { let (mut c, mut m) = cpu_exec(&[add_sub_reg(1,0,0,0b01,2,4,1,0)]); c.set_xn(1,10); c.set_xn(2,0x100); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),26); }
#[test] fn add_reg_64_lsr8()  { let (mut c, mut m) = cpu_exec(&[add_sub_reg(1,0,0,0b01,2,8,1,0)]); c.set_xn(1,0); c.set_xn(2,0x100); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),1); }
#[test] fn add_reg_64_asr4()  { let (mut c, mut m) = cpu_exec(&[add_sub_reg(1,0,0,0b10,2,4,1,0)]); c.set_xn(1,100); c.set_xn(2,(-32i64) as u64); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),98); }
#[test] fn sub_reg_64_lsl4()  { let (mut c, mut m) = cpu_exec(&[add_sub_reg(1,1,0,0b00,2,4,1,0)]); c.set_xn(1,100); c.set_xn(2,2); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),68); }
#[test] fn sub_reg_64_lsr4_b()  { let (mut c, mut m) = cpu_exec(&[add_sub_reg(1,1,0,0b01,2,4,1,0)]); c.set_xn(1,100); c.set_xn(2,0x100); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),84); }
#[test] fn add_reg_32_lsl4()  { let (mut c, mut m) = cpu_exec(&[add_sub_reg(0,0,0,0b00,2,4,1,0)]); c.set_xn(1,10); c.set_xn(2,1); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),26); }
#[test] fn sub_reg_32_lsl4()  { let (mut c, mut m) = cpu_exec(&[add_sub_reg(0,1,0,0b00,2,4,1,0)]); c.set_xn(1,100); c.set_xn(2,2); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),68); }
#[test] fn adds_reg_64_lsl4() { let (mut c, mut m) = cpu_exec(&[add_sub_reg(1,0,1,0b00,2,4,1,31)]); c.set_xn(1,u64::MAX-31); c.set_xn(2,2); c.step(&mut m).unwrap(); assert!(c.regs.z()); }
#[test] fn subs_reg_64_lsr4() { let (mut c, mut m) = cpu_exec(&[add_sub_reg(1,1,1,0b01,2,4,1,31)]); c.set_xn(1,16); c.set_xn(2,0x100); c.step(&mut m).unwrap(); assert!(c.regs.z()); }

// ===================================================================
//  Logical shifted register — all variants × shift amounts
// ===================================================================

#[test] fn and_reg_64_lsl4()  { let (mut c, mut m) = cpu_exec(&[log_reg(1,0b00,0,0b00,2,4,1,0)]); c.set_xn(1,0xFFF0); c.set_xn(2,0xFF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFF0); }
#[test] fn orr_reg_64_lsl8()  { let (mut c, mut m) = cpu_exec(&[log_reg(1,0b01,0,0b00,2,8,1,0)]); c.set_xn(1,0xFF); c.set_xn(2,0xFF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFFFF); }
#[test] fn eor_reg_64_lsr4()  { let (mut c, mut m) = cpu_exec(&[log_reg(1,0b10,0,0b01,2,4,1,0)]); c.set_xn(1,0x0F); c.set_xn(2,0xF0); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0); }
#[test] fn bic_reg_64_lsl0()  { let (mut c, mut m) = cpu_exec(&[log_reg(1,0b00,1,0b00,2,0,1,0)]); c.set_xn(1,0xFF); c.set_xn(2,0x0F); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xF0); }
#[test] fn orn_reg_64_lsl0()  { let (mut c, mut m) = cpu_exec(&[log_reg(1,0b01,1,0b00,2,0,1,0)]); c.set_xn(1,0); c.set_xn(2,0xFF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),!0xFFu64); }
#[test] fn eon_reg_64_lsl0()  { let (mut c, mut m) = cpu_exec(&[log_reg(1,0b10,1,0b00,2,0,1,0)]); c.set_xn(1,0xAA); c.set_xn(2,0xFF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),!(0xAA^0xFF)); }
#[test] fn ands_reg_64_lsl0() { let (mut c, mut m) = cpu_exec(&[log_reg(1,0b11,0,0b00,2,0,1,31)]); c.set_xn(1,0xFF); c.set_xn(2,0x100); c.step(&mut m).unwrap(); assert!(c.regs.z()); }
#[test] fn bics_reg_64_lsl0() { let (mut c, mut m) = cpu_exec(&[log_reg(1,0b11,1,0b00,2,0,1,31)]); c.set_xn(1,0xFF); c.set_xn(2,0xFF); c.step(&mut m).unwrap(); assert!(c.regs.z()); }
#[test] fn and_reg_32_lsl4()  { let (mut c, mut m) = cpu_exec(&[log_reg(0,0b00,0,0b00,2,4,1,0)]); c.set_xn(1,0xFFF0); c.set_xn(2,0xFF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFF0); }
#[test] fn orr_reg_32_lsl8()  { let (mut c, mut m) = cpu_exec(&[log_reg(0,0b01,0,0b00,2,8,1,0)]); c.set_xn(1,0xFF); c.set_xn(2,0xFF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFFFF); }

// ===================================================================
//  More bitfield variants — LSL/LSR/ASR with various widths
// ===================================================================

#[test] fn ubfm_lsr_32_by1()  { let (mut c, mut m) = cpu_exec(&[bitfield(0,0b10,0,1,31,1,0)]); c.set_xn(1,0xFF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0x7F); }
#[test] fn ubfm_lsr_32_by16() { let (mut c, mut m) = cpu_exec(&[bitfield(0,0b10,0,16,31,1,0)]); c.set_xn(1,0xFFFF_0000); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFFFF); }
#[test] fn ubfm_lsr_64_by4()  { let (mut c, mut m) = cpu_exec(&[bitfield(1,0b10,1,4,63,1,0)]); c.set_xn(1,0xF0); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xF); }
#[test] fn ubfm_lsr_64_by48() { let (mut c, mut m) = cpu_exec(&[bitfield(1,0b10,1,48,63,1,0)]); c.set_xn(1,0xFFFF_0000_0000_0000); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFFFF); }
#[test] fn ubfm_lsl_32_by1()  { let (mut c, mut m) = cpu_exec(&[bitfield(0,0b10,0,31,30,1,0)]); c.set_xn(1,0x7F); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFE); }
#[test] fn ubfm_lsl_32_by8()  { let (mut c, mut m) = cpu_exec(&[bitfield(0,0b10,0,24,23,1,0)]); c.set_xn(1,0xFF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFF00); }
#[test] fn ubfm_lsl_64_by1()  { let (mut c, mut m) = cpu_exec(&[bitfield(1,0b10,1,63,62,1,0)]); c.set_xn(1,0x7F); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFE); }
#[test] fn ubfm_lsl_64_by32() { let (mut c, mut m) = cpu_exec(&[bitfield(1,0b10,1,32,31,1,0)]); c.set_xn(1,0xFFFF_FFFF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFFFF_FFFF_0000_0000); }
#[test] fn sbfm_asr_64_by1()  { let (mut c, mut m) = cpu_exec(&[bitfield(1,0b00,1,1,63,1,0)]); c.set_xn(1,0x8000_0000_0000_0000); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xC000_0000_0000_0000); }
#[test] fn sbfm_asr_64_by32() { let (mut c, mut m) = cpu_exec(&[bitfield(1,0b00,1,32,63,1,0)]); c.set_xn(1,0x8000_0000_0000_0000); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFFFF_FFFF_8000_0000); }
#[test] fn sbfm_asr_32_by1()  { let (mut c, mut m) = cpu_exec(&[bitfield(0,0b00,0,1,31,1,0)]); c.set_xn(1,0x8000_0000); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xC000_0000); }
#[test] fn sbfm_asr_32_by16() { let (mut c, mut m) = cpu_exec(&[bitfield(0,0b00,0,16,31,1,0)]); c.set_xn(1,0x8000_0000); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFFFF_8000); }

// ===================================================================
//  Logical immediate — more rotation/replication patterns
// ===================================================================

#[test] fn and_imm_64_low32()  { let (mut c, mut m) = cpu_exec(&[log_imm(1,0b00,1,0,31,1,0)]); c.set_xn(1,u64::MAX); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFFFF_FFFF); }
#[test] fn and_imm_64_high32() { let (mut c, mut m) = cpu_exec(&[log_imm(1,0b00,1,32,31,1,0)]); c.set_xn(1,u64::MAX); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFFFF_FFFF_0000_0000); }
#[test] fn orr_imm_64_bit0()   { let (mut c, mut m) = cpu_exec(&[log_imm(1,0b01,1,0,0,1,0)]); c.set_xn(1,0); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),1); }
#[test] fn eor_imm_64_low8()   { let (mut c, mut m) = cpu_exec(&[log_imm(1,0b10,1,0,7,1,0)]); c.set_xn(1,0xAA); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0x55); }
#[test] fn ands_imm_64_z()     { let (mut c, mut m) = cpu_exec(&[log_imm(1,0b11,1,0,7,1,31)]); c.set_xn(1,0x100); c.step(&mut m).unwrap(); assert!(c.regs.z()); }
#[test] fn ands_imm_64_nz()    { let (mut c, mut m) = cpu_exec(&[log_imm(1,0b11,1,0,7,1,31)]); c.set_xn(1,0xFF); c.step(&mut m).unwrap(); assert!(!c.regs.z()); }
#[test] fn and_imm_32_low8()   { let (mut c, mut m) = cpu_exec(&[log_imm(0,0b00,0,0,7,1,0)]); c.set_xn(1,0xABCD); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xCD); }
#[test] fn orr_imm_32_high16() { let (mut c, mut m) = cpu_exec(&[log_imm(0,0b01,0,16,15,1,0)]); c.set_xn(1,0xFF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFFFF_00FF); }

// ===================================================================
//  More MOV wide edge cases
// ===================================================================

#[test] fn movz_clears_prev()  { let (mut c, mut m) = cpu_exec(&[mov_wide(1,0b10,0,0,0)]); c.set_xn(0,u64::MAX); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0); }
#[test] fn movk_hw1_preserve()  { let (mut c, mut m) = cpu_exec(&[mov_wide(1,0b11,1,0xABCD,0)]); c.set_xn(0,0x1234_5678_9ABC_DEF0); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0x1234_5678_ABCD_DEF0); }
#[test] fn movk_hw2_preserve()  { let (mut c, mut m) = cpu_exec(&[mov_wide(1,0b11,2,0x1111,0)]); c.set_xn(0,0xFFFF_FFFF_FFFF_FFFF); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFFFF_1111_FFFF_FFFF); }
#[test] fn movn_32_1()          { let (mut c, mut m) = cpu_exec(&[mov_wide(0,0b00,0,1,0)]); c.step(&mut m).unwrap(); assert_eq!(c.xn(0) & 0xFFFF_FFFF, 0xFFFF_FFFE); }
#[test] fn movn_64_hw2()        { let (mut c, mut m) = cpu_exec(&[mov_wide(1,0b00,2,0xFFFF,0)]); c.step(&mut m).unwrap(); assert_eq!(c.xn(0),0xFFFF_0000_FFFF_FFFF); }
