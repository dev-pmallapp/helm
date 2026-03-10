//! Tests for SIMD/FP and CCMP instruction bugs found during binary debugging.

use crate::arm::aarch64::Aarch64Cpu;
use helm_memory::address_space::AddressSpace;

fn cpu_with_code(insns: &[u32]) -> (Aarch64Cpu, AddressSpace) {
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
    (cpu, mem)
}

// ─── CCMP immediate variant ───────────────────────────────────────────────

#[test]
fn ccmp_imm_eq_taken() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0xF100_003F, // CMP X1, #0
        0x7A42_1800, // CCMP W0, #2, #0, NE
    ]);
    cpu.set_xn(0, 2);
    cpu.set_xn(1, 5);
    cpu.step(&mut mem).unwrap();
    cpu.step(&mut mem).unwrap();
    assert!(cpu.regs.z(), "CCMP W0=#2 vs W0=2 should set Z");
    assert!(cpu.regs.c(), "CCMP W0=#2 vs W0=2 should set C");
}

#[test]
fn ccmp_imm_gt_taken() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0xF100_003F, // CMP X1, #0
        0x7A42_1800, // CCMP W0, #2, #0, NE
    ]);
    cpu.set_xn(0, 5);
    cpu.set_xn(1, 3);
    cpu.step(&mut mem).unwrap();
    cpu.step(&mut mem).unwrap();
    assert!(!cpu.regs.z(), "5 != 2, Z should be clear");
    assert!(cpu.regs.c(), "5 > 2 unsigned, C should be set");
}

#[test]
fn ccmp_imm_cond_false_uses_nzcv() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0xF100_003F, // CMP X1, #0 → sets Z if X1==0
        0x7A42_180A, // CCMP W0, #2, #0xA, NE → NE=false when Z=1
    ]);
    cpu.set_xn(0, 5);
    cpu.set_xn(1, 0);
    cpu.step(&mut mem).unwrap();
    cpu.step(&mut mem).unwrap();
    let nzcv = cpu.regs.nzcv;
    assert_eq!(
        nzcv >> 28,
        0xA,
        "When cond=NE is false, NZCV should be set to imm=0xA"
    );
}

#[test]
fn ccmp_reg_variant() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0xF100_003F, // CMP X1, #0
        0x7A42_1000, // CCMP W0, W2, #0, NE (register variant)
    ]);
    cpu.set_xn(0, 10);
    cpu.set_xn(1, 1);
    cpu.set_xn(2, 10);
    cpu.step(&mut mem).unwrap();
    cpu.step(&mut mem).unwrap();
    assert!(cpu.regs.z(), "CCMP W0=10, W2=10 should set Z");
}

// ─── SIMD load/store opc dispatch ─────────────────────────────────────────

#[test]
fn stur_q_stores_correctly() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x3C80_03E0, // STUR Q0, [SP]  (size=00, opc=10, V=1)
    ]);
    cpu.regs.v[0] = 0xDEAD_BEEF_CAFE_BABE_1234_5678_9ABC_DEF0;
    cpu.step(&mut mem).unwrap();
    let mut buf = [0u8; 16];
    mem.read(cpu.regs.sp, &mut buf).unwrap();
    let stored = u128::from_le_bytes(buf);
    assert_eq!(stored, 0xDEAD_BEEF_CAFE_BABE_1234_5678_9ABC_DEF0);
}

#[test]
fn ldur_q_loads_correctly() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x3CC0_03E0, // LDUR Q0, [SP]  (size=00, opc=11, V=1)
    ]);
    let val: u128 = 0x1111_2222_3333_4444_5555_6666_7777_8888;
    mem.write(cpu.regs.sp, &val.to_le_bytes()).unwrap();
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0], val);
}

#[test]
fn stur_d_stores_correctly() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0xFC00_03E0, // STUR D0, [SP]  (size=11, opc=00, V=1 → 8-byte store)
    ]);
    cpu.regs.v[0] = 0xCAFE_BABE_DEAD_BEEF;
    cpu.step(&mut mem).unwrap();
    let mut buf = [0u8; 8];
    mem.read(cpu.regs.sp, &mut buf).unwrap();
    let stored = u64::from_le_bytes(buf);
    assert_eq!(stored, 0xCAFE_BABE_DEAD_BEEF);
}

// ─── SIMD pair D-register ─────────────────────────────────────────────────

#[test]
fn stp_d_pair() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x6D00_03E0, // STP D0, D0, [SP] (opc=01, D regs)
    ]);
    cpu.regs.v[0] = 0xAAAA_BBBB_CCCC_DDDDu128;
    cpu.step(&mut mem).unwrap();
    let mut buf = [0u8; 16];
    mem.read(cpu.regs.sp, &mut buf).unwrap();
    let lo = u64::from_le_bytes(buf[0..8].try_into().unwrap());
    let hi = u64::from_le_bytes(buf[8..16].try_into().unwrap());
    assert_eq!(lo, 0xAAAA_BBBB_CCCC_DDDD);
    assert_eq!(hi, 0xAAAA_BBBB_CCCC_DDDD);
}

#[test]
fn ldp_d_pair() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x6D40_07E0, // LDP D0, D1, [SP]
    ]);
    mem.write(cpu.regs.sp, &42u64.to_le_bytes()).unwrap();
    mem.write(cpu.regs.sp + 8, &99u64.to_le_bytes()).unwrap();
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0] as u64, 42);
    assert_eq!(cpu.regs.v[1] as u64, 99);
}

// ─── FMOV between FP and GP ──────────────────────────────────────────────

#[test]
fn fmov_x_to_d() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x9E67_0020, // FMOV D0, X1
    ]);
    cpu.set_xn(1, 0xDEAD_BEEF_CAFE_BABE);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0] as u64, 0xDEAD_BEEF_CAFE_BABE);
}

#[test]
fn fmov_d_to_x() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x9E66_0020, // FMOV X0, D1
    ]);
    cpu.regs.v[1] = 0x1234_5678_9ABC_DEF0u128;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0x1234_5678_9ABC_DEF0);
}

#[test]
fn fmov_w_to_s() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x1E27_0020, // FMOV S0, W1
    ]);
    cpu.set_xn(1, 0xDEAD_BEEF);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0] as u32, 0xDEAD_BEEF);
}

#[test]
fn fmov_s_to_w() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x1E26_0020, // FMOV W0, S1
    ]);
    cpu.regs.v[1] = 0xCAFE_BABEu128;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xCAFE_BABE);
}

// ─── SIMD vector operations ──────────────────────────────────────────────

#[test]
fn eor_v16b() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x6E21_1C00, // EOR V0.16B, V0.16B, V1.16B
    ]);
    cpu.regs.v[0] = 0xFF00_FF00_FF00_FF00_FF00_FF00_FF00_FF00;
    cpu.regs.v[1] = 0x0F0F_0F0F_0F0F_0F0F_0F0F_0F0F_0F0F_0F0F;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0], 0xF00F_F00F_F00F_F00F_F00F_F00F_F00F_F00F);
}

#[test]
fn cmeq_v8b_eq() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x2E21_8C20, // CMEQ V0.8B, V1.8B, V1.8B (all equal)
    ]);
    cpu.regs.v[1] = 0x0102030405060708;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0] as u64, 0xFFFF_FFFF_FFFF_FFFF);
}

#[test]
fn cmeq_v8b_neq() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x2E22_8C20, // CMEQ V0.8B, V1.8B, V2.8B
    ]);
    cpu.regs.v[1] = 0x0102030405060708;
    cpu.regs.v[2] = 0x0102030400060708;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0] as u64, 0xFFFFFFFF00FFFFFF);
}

#[test]
fn ins_d0_from_x() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x4E08_1C00, // INS V0.D[0], X0
    ]);
    cpu.regs.v[0] = 0xAAAA_BBBB_CCCC_DDDD_0000_0000_0000_0000;
    cpu.set_xn(0, 0x1234_5678_9ABC_DEF0);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0], 0xAAAA_BBBB_CCCC_DDDD_1234_5678_9ABC_DEF0);
}

#[test]
fn umov_x_from_d() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x4E08_3C20, // UMOV X0, V1.D[0]
    ]);
    cpu.regs.v[1] = 0xAAAA_BBBB_1234_5678u128;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.xn(0), 0xAAAA_BBBB_1234_5678);
}

#[test]
fn movi_v0_2d_zero() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x6F00_E400, // MOVI V0.2D, #0
    ]);
    cpu.regs.v[0] = 0xDEAD_BEEF_CAFE_BABEu128;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0], 0);
}

#[test]
fn movi_v0_2d_allones() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x6F07_E7E0, // MOVI V0.2D, #0xFFFFFFFFFFFFFFFF
    ]);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0], u128::MAX);
}

#[test]
fn cmlt_v8b_zero() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x0E20_A820, // CMLT V0.8B, V1.8B, #0
    ]);
    cpu.regs.v[1] = 0x00FF_0180_7F01_FE00;
    cpu.step(&mut mem).unwrap();
    let r = cpu.regs.v[0] as u64;
    assert_eq!(r & 0xFF, 0x00, "byte 0 (0x00) is not < 0");
    assert_eq!((r >> 8) & 0xFF, 0xFF, "byte 1 (0xFE) is < 0 (signed)");
    assert_eq!((r >> 48) & 0xFF, 0xFF, "byte 6 (0xFF) is < 0 (signed)");
}

#[test]
fn add_v2d() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x4EE1_8400, // ADD V0.2D, V0.2D, V1.2D
    ]);
    cpu.regs.v[0] = ((100u128) << 64) | 200u128;
    cpu.regs.v[1] = ((300u128) << 64) | 400u128;
    cpu.step(&mut mem).unwrap();
    let lo = cpu.regs.v[0] as u64;
    let hi = (cpu.regs.v[0] >> 64) as u64;
    assert_eq!(lo, 600);
    assert_eq!(hi, 400);
}

#[test]
fn scalar_addp_d() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x5EF1_B800, // ADDP D0, V0.2D
    ]);
    cpu.regs.v[0] = ((100u128) << 64) | 200u128;
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0] as u64, 300);
}

#[test]
fn ushr_v2d_1() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x6F40_0420, // USHR V0.2D, V1.2D, #64? Let me use a simpler one
    ]);
    // USHR V0.2D, V1.2D, #63 = 0x6F410420
    let (mut cpu, mut mem) = cpu_with_code(&[0x6F41_0420]);
    cpu.regs.v[1] = ((0x8000_0000_0000_0000u128) << 64) | 0x4000_0000_0000_0000u128;
    cpu.step(&mut mem).unwrap();
    let lo = cpu.regs.v[0] as u64;
    let hi = (cpu.regs.v[0] >> 64) as u64;
    assert_eq!(lo, 0);
    assert_eq!(hi, 1);
}

// ─── DUP element size handling ────────────────────────────────────────────

#[test]
fn dup_v16b_byte() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x4E01_0C00, // DUP V0.16B, W0 (imm5=00001 → byte)
    ]);
    cpu.set_xn(0, 0xDEAD_BEEF_CAFE_BA42);
    cpu.step(&mut mem).unwrap();
    assert_eq!(cpu.regs.v[0], 0x42424242424242424242424242424242);
}

#[test]
fn dup_v4s_word() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x4E04_0C00, // DUP V0.4S, W0 (imm5=00100 → word)
    ]);
    cpu.set_xn(0, 0xDEAD_BEEF);
    cpu.step(&mut mem).unwrap();
    let lo = cpu.regs.v[0] as u64;
    let hi = (cpu.regs.v[0] >> 64) as u64;
    assert_eq!(lo, 0xDEAD_BEEF_DEAD_BEEF);
    assert_eq!(hi, 0xDEAD_BEEF_DEAD_BEEF);
}

#[test]
fn dup_v2d_doubleword() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x4E08_0C00, // DUP V0.2D, X0 (imm5=01000 → doubleword)
    ]);
    cpu.set_xn(0, 0xCAFE_BABE_DEAD_BEEF);
    cpu.step(&mut mem).unwrap();
    let lo = cpu.regs.v[0] as u64;
    let hi = (cpu.regs.v[0] >> 64) as u64;
    assert_eq!(lo, 0xCAFE_BABE_DEAD_BEEF);
    assert_eq!(hi, 0xCAFE_BABE_DEAD_BEEF);
}

#[test]
fn dup_v2d_not_truncated_to_byte() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x4E08_0C00, // DUP V0.2D, X0
    ]);
    cpu.set_xn(0, 0x1234_5678_9ABC_DEF0);
    cpu.step(&mut mem).unwrap();
    assert_ne!(
        cpu.regs.v[0] as u64, 0xF0F0_F0F0_F0F0_F0F0,
        "DUP V0.2D should NOT truncate to byte"
    );
    assert_eq!(cpu.regs.v[0] as u64, 0x1234_5678_9ABC_DEF0);
}

// ─── SIMD string-processing pattern (strlen / memchr) ─────────────────

#[test]
fn cmeq_zero_finds_nul_byte() {
    // Simulate: load "hello\0XX..." into V0, CMEQ V0.16B, V0.16B, #0
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x4e209800, // CMEQ V0.16B, V0.16B, #0
    ]);
    // "hello\0" + padding (byte 5 is NUL)
    let data: [u8; 16] = [
        b'h', b'e', b'l', b'l', b'o', 0, b'X', b'X', b'X', b'X', b'X', b'X', b'X', b'X', b'X', b'X',
    ];
    cpu.regs.v[0] = u128::from_le_bytes(data);
    cpu.step(&mut mem).unwrap();
    // After CMEQ #0, byte 5 should be 0xFF, all others 0x00
    let result = cpu.regs.v[0].to_le_bytes();
    assert_eq!(result[0], 0x00, "h != 0");
    assert_eq!(result[1], 0x00, "e != 0");
    assert_eq!(result[4], 0x00, "o != 0");
    assert_eq!(result[5], 0xFF, "NUL byte should match");
    assert_eq!(result[6], 0x00, "X != 0");
}

#[test]
fn umaxv_detects_nul_after_cmeq() {
    // CMEQ V0.16B, V0.16B, #0 then UMAXV B0, V0.16B
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x4e209800, // CMEQ V0.16B, V0.16B, #0
        0x6e30a800, // UMAXV B0, V0.16B
        0x1e260000, // FMOV W0, S0
    ]);
    // String with NUL at byte 5
    let data: [u8; 16] = [
        b'h', b'e', b'l', b'l', b'o', 0, b'X', b'X', b'X', b'X', b'X', b'X', b'X', b'X', b'X', b'X',
    ];
    cpu.regs.v[0] = u128::from_le_bytes(data);
    cpu.step(&mut mem).unwrap(); // CMEQ
    cpu.step(&mut mem).unwrap(); // UMAXV
    cpu.step(&mut mem).unwrap(); // FMOV
                                 // UMAXV should find 0xFF (the NUL match), FMOV puts it in W0
    assert_eq!(cpu.xn(0), 0xFF, "UMAXV should find the 0xFF NUL-match byte");
}

#[test]
fn umaxv_no_nul_in_string() {
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x4e209800, // CMEQ V0.16B, V0.16B, #0
        0x6e30a800, // UMAXV B0, V0.16B
        0x1e260000, // FMOV W0, S0
    ]);
    // No NUL bytes
    let data: [u8; 16] = *b"abcdefghijklmnop";
    cpu.regs.v[0] = u128::from_le_bytes(data);
    cpu.step(&mut mem).unwrap(); // CMEQ
    cpu.step(&mut mem).unwrap(); // UMAXV
    cpu.step(&mut mem).unwrap(); // FMOV
    assert_eq!(cpu.xn(0), 0, "No NUL bytes means UMAXV should be 0");
}

#[test]
fn cmeq_v_compares_equal_bytes() {
    // CMEQ V2.16B, V0.16B, V1.16B
    // 0x6e218c02  => Q=1 U=1 01110 00 1 Rm=01 opcode=10001 1 Rn=00 Rd=02
    let (mut cpu, mut mem) = cpu_with_code(&[
        0x6e218c02, // CMEQ V2.16B, V0.16B, V1.16B  (U=1, opcode=10001)
    ]);
    let a: [u8; 16] = [
        b'a', b'b', b'c', b'd', b'e', b'f', b'g', b'h', b'i', b'j', b'k', b'l', b'm', b'n', b'o',
        b'p',
    ];
    let b: [u8; 16] = [
        b'a', b'X', b'c', b'X', b'e', b'X', b'g', b'X', b'i', b'X', b'k', b'X', b'm', b'X', b'o',
        b'X',
    ];
    cpu.regs.v[0] = u128::from_le_bytes(a);
    cpu.regs.v[1] = u128::from_le_bytes(b);
    cpu.step(&mut mem).unwrap();
    let result = cpu.regs.v[2].to_le_bytes();
    assert_eq!(result[0], 0xFF, "a == a");
    assert_eq!(result[1], 0x00, "b != X");
    assert_eq!(result[2], 0xFF, "c == c");
    assert_eq!(result[3], 0x00, "d != X");
}
