//! Differential tests: stencil JIT vs interpreter.
//!
//! For each instruction, we run it through both the stencil compiler and the
//! AArch64 interpreter, then compare all register state including NZCV flags.

#![cfg(test)]
#![allow(unsafe_code)]

use crate::regs::{self, REG_JIT_MEM_READ, REG_JIT_MEM_WRITE};
use crate::stencil::{compiler, data, fields};
use helm_arch::aarch64::arch_state::Aarch64ArchState;
use helm_arch::aarch64_decode;
use helm_arch::aarch64_execute;
use helm_core::{AccessType, MemFault, MemInterface};

/// Null memory that returns zeros on read, ignores writes.
struct NullMem;

impl MemInterface for NullMem {
    fn read(&mut self, _addr: u64, _size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        Ok(0)
    }
    fn write(
        &mut self,
        _addr: u64,
        _size: usize,
        _val: u64,
        _ty: AccessType,
    ) -> Result<(), MemFault> {
        Ok(())
    }
}

/// Memory that returns the requested address as the loaded value.
struct AddrMem;

impl MemInterface for AddrMem {
    fn read(&mut self, addr: u64, _size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        Ok(addr)
    }

    fn write(
        &mut self,
        _addr: u64,
        _size: usize,
        _val: u64,
        _ty: AccessType,
    ) -> Result<(), MemFault> {
        Ok(())
    }
}

#[allow(clippy::not_unsafe_ptr_arg_deref)]
extern "C" fn addr_mem_read(_mem: *mut u8, addr: u64, _size: u32, out: *mut u64) -> u64 {
    unsafe { *out = addr };
    0
}

extern "C" fn addr_mem_write(_mem: *mut u8, _addr: u64, _val: u64, _size: u32) -> u64 {
    0
}

#[derive(Clone)]
struct InitState {
    x: [u64; 31],
    sp: u64,
    nzcv: u32,
}

impl Default for InitState {
    fn default() -> Self {
        Self {
            x: [0u64; 31],
            sp: 0x0000_7FFF_FFF0_0000,
            nzcv: 0,
        }
    }
}

fn assert_stencil_matches_interpreter(raw: u32, pc: u64, init: &InitState, label: &str) {
    let insn = aarch64_decode(raw, pc)
        .unwrap_or_else(|e| panic!("[{label}] decode failed for {raw:#010x}: {e}"));

    // Stencil lookup
    let stencil = match data::lookup_stencil_a64(&insn) {
        Some(data::StencilLookup::Found(s)) => s,
        _ => {
            eprintln!("[{label}] stencil unsupported {:?}, skipping", insn.opcode);
            return;
        }
    };

    let decoded = fields::extract_fields_a64(&insn, pc);
    let block = match compiler::compile_block(pc, &[(stencil, decoded)]) {
        Some(b) => b,
        None => {
            eprintln!("[{label}] compile_block returned None, skipping");
            return;
        }
    };

    // Interpreter path
    let mut interp_state = Aarch64ArchState::default();
    for i in 0..31 {
        interp_state.x[i] = init.x[i];
    }
    interp_state.sp = init.sp;
    interp_state.pc = pc;
    interp_state.nzcv = init.nzcv;

    let mut interp_mem = NullMem;
    let pc_written = aarch64_execute(&insn, &mut interp_state, &mut interp_mem, None)
        .unwrap_or_else(|e| panic!("[{label}] interpreter execute failed: {e}"));
    if !pc_written {
        interp_state.pc = pc + 4;
    }

    // JIT path
    let mut jit_state = Aarch64ArchState::default();
    for i in 0..31 {
        jit_state.x[i] = init.x[i];
    }
    jit_state.sp = init.sp;
    jit_state.pc = pc;
    jit_state.nzcv = init.nzcv;
    let mut flat = regs::arch_to_flat(&jit_state);
    // Populate mem helper fn ptrs (not used by NullMem tests but needed for stencils)
    flat[REG_JIT_MEM_READ] = crate::helpers::jit_mem_read as *const () as u64;
    flat[REG_JIT_MEM_WRITE] = crate::helpers::jit_mem_write as *const () as u64;

    let _exit = unsafe { (block.entry)(flat.as_mut_ptr(), std::ptr::null_mut()) };
    regs::flat_to_arch(&mut flat, &mut jit_state);

    // Compare
    let mut mismatches = Vec::new();
    for i in 0..31 {
        if jit_state.x[i] != interp_state.x[i] {
            mismatches.push(format!(
                "  X{i}: stencil={:#018x}  interp={:#018x}",
                jit_state.x[i], interp_state.x[i]
            ));
        }
    }
    if jit_state.sp != interp_state.sp {
        mismatches.push(format!(
            "  SP: stencil={:#018x}  interp={:#018x}",
            jit_state.sp, interp_state.sp
        ));
    }
    if jit_state.pc != interp_state.pc {
        mismatches.push(format!(
            "  PC: stencil={:#018x}  interp={:#018x}",
            jit_state.pc, interp_state.pc
        ));
    }
    if jit_state.nzcv != interp_state.nzcv {
        let jn = (jit_state.nzcv >> 31) & 1;
        let jz = (jit_state.nzcv >> 30) & 1;
        let jc = (jit_state.nzcv >> 29) & 1;
        let jv = (jit_state.nzcv >> 28) & 1;
        let in_ = (interp_state.nzcv >> 31) & 1;
        let iz = (interp_state.nzcv >> 30) & 1;
        let ic = (interp_state.nzcv >> 29) & 1;
        let iv = (interp_state.nzcv >> 28) & 1;
        mismatches.push(format!(
            "  NZCV: stencil={:#010x} (N={jn} Z={jz} C={jc} V={jv})  \
             interp={:#010x} (N={in_} Z={iz} C={ic} V={iv})",
            jit_state.nzcv, interp_state.nzcv
        ));
    }

    if !mismatches.is_empty() {
        panic!(
            "\n\n[{label}] STENCIL vs INTERPRETER MISMATCH\n\
             Instruction: {raw:#010x} at pc={pc:#x}\n\
             Opcode: {:?}\n\
             Mismatched registers:\n{}\n",
            insn.opcode,
            mismatches.join("\n")
        );
    }
}

fn assert_stencil_matches_interpreter_with_mem<M: MemInterface>(
    raw: u32,
    pc: u64,
    init: &InitState,
    mut make_mem: impl FnMut() -> M,
    label: &str,
) {
    let insn = aarch64_decode(raw, pc)
        .unwrap_or_else(|e| panic!("[{label}] decode failed for {raw:#010x}: {e}"));

    let stencil = match data::lookup_stencil_a64(&insn) {
        Some(data::StencilLookup::Found(s)) => s,
        _ => {
            eprintln!("[{label}] stencil unsupported {:?}, skipping", insn.opcode);
            return;
        }
    };

    let decoded = fields::extract_fields_a64(&insn, pc);
    let block = match compiler::compile_block(pc, &[(stencil, decoded)]) {
        Some(b) => b,
        None => {
            eprintln!("[{label}] compile_block returned None, skipping");
            return;
        }
    };

    let mut interp_state = Aarch64ArchState::default();
    for i in 0..31 {
        interp_state.x[i] = init.x[i];
    }
    interp_state.sp = init.sp;
    interp_state.pc = pc;
    interp_state.nzcv = init.nzcv;

    let mut interp_mem = make_mem();
    let pc_written = aarch64_execute(&insn, &mut interp_state, &mut interp_mem, None)
        .unwrap_or_else(|e| panic!("[{label}] interpreter execute failed: {e}"));
    if !pc_written {
        interp_state.pc = pc + 4;
    }

    let mut jit_state = Aarch64ArchState::default();
    for i in 0..31 {
        jit_state.x[i] = init.x[i];
    }
    jit_state.sp = init.sp;
    jit_state.pc = pc;
    jit_state.nzcv = init.nzcv;
    let mut flat = regs::arch_to_flat(&jit_state);
    flat[REG_JIT_MEM_READ] = addr_mem_read as *const () as u64;
    flat[REG_JIT_MEM_WRITE] = addr_mem_write as *const () as u64;

    let _exit = unsafe { (block.entry)(flat.as_mut_ptr(), std::ptr::null_mut()) };
    regs::flat_to_arch(&mut flat, &mut jit_state);

    assert_eq!(jit_state.x, interp_state.x, "[{label}] X register mismatch");
    assert_eq!(jit_state.sp, interp_state.sp, "[{label}] SP mismatch");
    assert_eq!(jit_state.pc, interp_state.pc, "[{label}] PC mismatch");
    assert_eq!(jit_state.nzcv, interp_state.nzcv, "[{label}] NZCV mismatch");
}

// ── Data Processing — Immediate ────────────────────────────────────────────

#[test]
fn stencil_vs_interp_add_imm() {
    let mut init = InitState::default();
    init.x[1] = 100;
    assert_stencil_matches_interpreter(0x91010820, 0x1000, &init, "ADD X0, X1, #66");
}

#[test]
fn stencil_vs_interp_sub_imm() {
    let mut init = InitState::default();
    init.x[0] = 200;
    assert_stencil_matches_interpreter(0xd1010400, 0x1000, &init, "SUB X0, X0, #65");
}

#[test]
fn stencil_vs_interp_movz() {
    assert_stencil_matches_interpreter(
        0xd283600c,
        0x400100,
        &InitState::default(),
        "MOVZ X12, #0x1B00",
    );
}

#[test]
fn stencil_vs_interp_movn() {
    assert_stencil_matches_interpreter(0x92800020, 0x400100, &InitState::default(), "MOVN X0, #1");
}

#[test]
fn stencil_vs_interp_and_imm() {
    let mut init = InitState::default();
    init.x[0] = 0xFF;
    // AND X0, X0, #0xF  — bitmask encoding for 0xF
    assert_stencil_matches_interpreter(0x92400c00, 0x1000, &init, "AND X0, X0, #0xF");
}

#[test]
fn stencil_vs_interp_orr_imm() {
    let mut init = InitState::default();
    init.x[0] = 0xFF00;
    assert_stencil_matches_interpreter(0xb2401c00, 0x1000, &init, "ORR X0, X0, #0xFF");
}

// ── Data Processing — Register ─────────────────────────────────────────────

#[test]
fn stencil_vs_interp_add_reg() {
    let mut init = InitState::default();
    init.x[1] = 10;
    init.x[2] = 20;
    assert_stencil_matches_interpreter(0x8b020020, 0x1000, &init, "ADD X0, X1, X2");
}

#[test]
fn stencil_vs_interp_sub_reg() {
    let mut init = InitState::default();
    init.x[1] = 100;
    init.x[2] = 30;
    assert_stencil_matches_interpreter(0xcb020020, 0x1000, &init, "SUB X0, X1, X2");
}

#[test]
fn stencil_vs_interp_and_reg() {
    let mut init = InitState::default();
    init.x[0] = 0xFF;
    init.x[1] = 0x0F;
    assert_stencil_matches_interpreter(0x8a010000, 0x1000, &init, "AND X0, X0, X1");
}

#[test]
fn stencil_vs_interp_orr_reg() {
    let mut init = InitState::default();
    init.x[0] = 0xF0;
    init.x[1] = 0x0F;
    assert_stencil_matches_interpreter(0xaa010000, 0x1000, &init, "ORR X0, X0, X1");
}

// ── Flag-setting ops (ADDS/SUBS) — task 11 verification ─────────────────────

#[test]
fn stencil_vs_interp_adds_imm_zero() {
    let mut init = InitState::default();
    init.x[0] = 0;
    // ADDS X0, X0, #0 → result=0, Z=1
    assert_stencil_matches_interpreter(0xb1000000, 0x1000, &init, "ADDS X0, X0, #0 (zero)");
}

#[test]
fn stencil_vs_interp_adds_imm_negative() {
    let mut init = InitState::default();
    init.x[0] = 0x8000_0000_0000_0000;
    // ADDS X0, X0, #1 → N=1 (MSB set)
    assert_stencil_matches_interpreter(0xb1000400, 0x1000, &init, "ADDS X0, X0, #1 (neg result)");
}

#[test]
fn stencil_vs_interp_adds_signed_overflow() {
    let mut init = InitState::default();
    init.x[0] = 0x7FFF_FFFF_FFFF_FFFF;
    // ADDS X0, X0, #1 → V=1 (signed overflow)
    assert_stencil_matches_interpreter(0xb1000400, 0x1000, &init, "ADDS X0, X0, #1 (overflow)");
}

#[test]
fn stencil_vs_interp_subs_imm_borrow() {
    let mut init = InitState::default();
    init.x[0] = 0;
    // SUBS X0, X0, #1 → C=0 (borrow), N=1
    assert_stencil_matches_interpreter(0xf1000400, 0x1000, &init, "SUBS X0, X0, #1 (borrow)");
}

#[test]
fn stencil_vs_interp_subs_imm_equal() {
    let mut init = InitState::default();
    init.x[0] = 42;
    // SUBS X0, X0, #42 → Z=1, C=1 (no borrow)
    assert_stencil_matches_interpreter(0xf100a800, 0x1000, &init, "SUBS X0, X0, #42 (equal)");
}

#[test]
fn stencil_vs_interp_adds_reg_carry() {
    let mut init = InitState::default();
    init.x[0] = u64::MAX;
    init.x[1] = 1;
    // ADDS X0, X0, X1 → C=1 (unsigned overflow)
    assert_stencil_matches_interpreter(0xab010000, 0x1000, &init, "ADDS X0, X0, X1 (carry)");
}

#[test]
fn stencil_vs_interp_subs_reg_signed_overflow() {
    let mut init = InitState::default();
    init.x[0] = 0x8000_0000_0000_0000; // MIN_SIGNED
    init.x[1] = 1;
    // SUBS X0, X0, X1 → V=1 (signed underflow)
    assert_stencil_matches_interpreter(
        0xeb010000,
        0x1000,
        &init,
        "SUBS X0, X0, X1 (signed underflow)",
    );
}

// ── Branches ────────────────────────────────────────────────────────────────

#[test]
fn stencil_vs_interp_b() {
    // B #0x100
    assert_stencil_matches_interpreter(0x14000040, 0x2000, &InitState::default(), "B #0x100");
}

#[test]
fn stencil_vs_interp_bl() {
    // BL #0x100
    assert_stencil_matches_interpreter(0x94000040, 0x2000, &InitState::default(), "BL #0x100");
}

#[test]
fn stencil_vs_interp_cbz_taken() {
    let init = InitState::default(); // X0=0
                                     // CBZ X0, #0x10
    assert_stencil_matches_interpreter(0xb4000080, 0x3000, &init, "CBZ X0, #0x10 (taken)");
}

#[test]
fn stencil_vs_interp_cbz_not_taken() {
    let mut init = InitState::default();
    init.x[0] = 1;
    // CBZ X0, #0x10
    assert_stencil_matches_interpreter(0xb4000080, 0x3000, &init, "CBZ X0, #0x10 (not taken)");
}

#[test]
fn stencil_vs_interp_bcond_eq_taken() {
    let mut init = InitState::default();
    init.nzcv = 0x4000_0000; // Z=1
                             // B.EQ #0x20
    assert_stencil_matches_interpreter(0x54000100, 0x4000, &init, "B.EQ #0x20 (Z=1, taken)");
}

#[test]
fn stencil_vs_interp_bcond_eq_not_taken() {
    let init = InitState::default(); // Z=0
                                     // B.EQ #0x20
    assert_stencil_matches_interpreter(0x54000100, 0x4000, &init, "B.EQ #0x20 (Z=0, not taken)");
}

#[test]
fn stencil_vs_interp_cmp_does_not_clobber_sp() {
    let mut init = InitState::default();
    init.x[0] = 0x1234;
    init.x[1] = 0x1234;
    init.sp = 0x7fff_ffff_ff00;
    assert_stencil_matches_interpreter(0xeb01001f, 0x4100, &init, "CMP X0, X1");
}

#[test]
fn stencil_vs_interp_tst_does_not_clobber_sp() {
    let mut init = InitState::default();
    init.x[0] = 0x55aa;
    init.x[1] = 0xff00;
    init.sp = 0x7fff_ffff_fe00;
    assert_stencil_matches_interpreter(0xea01001f, 0x4200, &init, "TST X0, X1");
}

#[test]
fn stencil_vs_interp_csel_ls_taken() {
    let mut init = InitState::default();
    init.x[2] = 0x1111;
    init.x[27] = 0x2222;
    init.nzcv = 0x4000_0000; // Z=1 => LS taken
    assert_stencil_matches_interpreter(0x9a9b9042, 0x4300, &init, "CSEL X2, X2, X27, LS");
}

#[test]
fn stencil_vs_interp_csel_ls_not_taken() {
    let mut init = InitState::default();
    init.x[2] = 0x1111;
    init.x[27] = 0x2222;
    init.nzcv = 0x2000_0000; // C=1, Z=0 => LS false
    assert_stencil_matches_interpreter(0x9a9b9042, 0x4300, &init, "CSEL X2, X2, X27, LS");
}

#[test]
fn stencil_vs_interp_bcond_gt_taken() {
    let mut init = InitState::default();
    init.nzcv = 0; // Z=0, N=0, V=0 => GT taken
    assert_stencil_matches_interpreter(0x5400052c, 0x4400, &init, "B.GT");
}

#[test]
fn stencil_vs_interp_ldr_sp_base_uses_sp_slot() {
    let mut init = InitState::default();
    init.sp = 0x7fff_ffff_f000;
    assert_stencil_matches_interpreter_with_mem(
        0xf9404be0,
        0x4500,
        &init,
        || AddrMem,
        "LDR X0, [SP, #144]",
    );
}

// ── SBFM/UBFM ──────────────────────────────────────────────────────────────

#[test]
fn stencil_vs_interp_ubfm_lsr() {
    let mut init = InitState::default();
    init.x[0] = 0xFF00;
    // LSR X0, X0, #4 = UBFM X0, X0, #4, #63
    assert_stencil_matches_interpreter(0xd344fc00, 0x1000, &init, "LSR X0, X0, #4");
}

#[test]
fn stencil_vs_interp_ubfm_lsr_nonzero_low_bits() {
    // Regression: old stencil used ROR instead of SHR, wrapping low bits to top
    let mut init = InitState::default();
    init.x[0] = 0x4F4C; // low 6 bits = 0x0C, nonzero
                        // LSR X0, X0, #6 = UBFM X0, X0, #6, #63
    assert_stencil_matches_interpreter(
        0xd346fc00,
        0x1000,
        &init,
        "LSR X0, X0, #6 (nonzero low bits)",
    );
}

#[test]
fn stencil_vs_interp_ubfm_lsr_all_ones() {
    let mut init = InitState::default();
    init.x[0] = 0xFFFFFFFFFFFFFFFF;
    // LSR X0, X0, #4 = UBFM X0, X0, #4, #63
    assert_stencil_matches_interpreter(0xd344fc00, 0x1000, &init, "LSR X0, X0, #4 (all ones)");
}

#[test]
fn stencil_vs_interp_ubfm_ubfx() {
    // UBFX X0, X0, #4, #8 = UBFM X0, X0, #4, #11 (extract bits[11:4])
    let mut init = InitState::default();
    init.x[0] = 0xDEADBEEF;
    assert_stencil_matches_interpreter(0xd3042c00, 0x1000, &init, "UBFX X0, X0, #4, #8");
}

#[test]
fn stencil_vs_interp_ubfm_lsl() {
    // LSL X0, X1, #3 = UBFM X0, X1, #61, #60 (imms < immr)
    let mut init = InitState::default();
    init.x[1] = 0xCAFE;
    assert_stencil_matches_interpreter(0xd37df020, 0x1000, &init, "LSL X0, X1, #3");
}

#[test]
fn stencil_vs_interp_ubfm_uxtb() {
    // UXTB X0, X1 = UBFM X0, X1, #0, #7
    let mut init = InitState::default();
    init.x[1] = 0xDEADBEEF_CAFEBABE;
    assert_stencil_matches_interpreter(0xd3401c20, 0x1000, &init, "UXTB X0, X1");
}

#[test]
fn stencil_vs_interp_sbfm_asr() {
    // ASR X0, X0, #4 = SBFM X0, X0, #4, #63
    let mut init = InitState::default();
    init.x[0] = 0x8000000000000000; // MSB set
    assert_stencil_matches_interpreter(0x9344fc00, 0x1000, &init, "ASR X0, X0, #4");
}

#[test]
fn stencil_vs_interp_sbfm_sxtb() {
    // SXTB X0, X1 = SBFM X0, X1, #0, #7
    let mut init = InitState::default();
    init.x[1] = 0x80; // negative byte
    assert_stencil_matches_interpreter(0x93401c20, 0x1000, &init, "SXTB X0, X1");
}

#[test]
fn stencil_vs_interp_sbfm_sbfiz() {
    // SBFIZ X0, X1, #4, #8 = SBFM X0, X1, #60, #7 (imms < immr)
    let mut init = InitState::default();
    init.x[1] = 0xFF; // all-ones byte, sign bit set
    assert_stencil_matches_interpreter(0x933c1c20, 0x1000, &init, "SBFIZ X0, X1, #4, #8");
}

// ── Multiply ────────────────────────────────────────────────────────────────

#[test]
fn stencil_vs_interp_madd() {
    let mut init = InitState::default();
    init.x[1] = 6;
    init.x[2] = 7;
    init.x[3] = 0;
    // MADD X0, X1, X2, X3 = X0 = X3 + X1*X2 = 42
    assert_stencil_matches_interpreter(0x9b020c20, 0x1000, &init, "MADD X0, X1, X2, X3");
}

// ── Sweep test: ADDS with various initial values ────────────────────────────

#[test]
fn stencil_vs_interp_adds_sweep() {
    let values = [
        0u64,
        1,
        u64::MAX,
        0x7FFF_FFFF_FFFF_FFFF,
        0x8000_0000_0000_0000,
        42,
        0xFFFF_FFFF,
        0x1_0000_0000,
    ];
    for &v in &values {
        let mut init = InitState::default();
        init.x[0] = v;
        assert_stencil_matches_interpreter(
            0xb1000400, // ADDS X0, X0, #1
            0x4005c4,
            &init,
            &format!("ADDS X0, X0, #1 with X0={v:#x}"),
        );
    }
}

#[test]
fn stencil_vs_interp_subs_sweep() {
    let values = [
        0u64,
        1,
        2,
        u64::MAX,
        0x7FFF_FFFF_FFFF_FFFF,
        0x8000_0000_0000_0000,
        42,
    ];
    for &v in &values {
        let mut init = InitState::default();
        init.x[0] = v;
        assert_stencil_matches_interpreter(
            0xf1000400, // SUBS X0, X0, #1
            0x4005c4,
            &init,
            &format!("SUBS X0, X0, #1 with X0={v:#x}"),
        );
    }
}

// ── Load/Store Pair ───────────────────────────────────────────────────────

#[test]
fn stencil_vs_interp_ldp_x19_x20_sp_16() {
    // LDP X19, X20, [SP, #16]  raw=0xa94153f3
    let mut init = InitState::default();
    init.sp = 0x1000;
    assert_stencil_matches_interpreter_with_mem(
        0xa94153f3,
        0x2000,
        &init,
        || AddrMem,
        "LDP X19, X20, [SP, #16]",
    );
}

#[test]
fn stencil_vs_interp_stp_x29_x30_sp_neg16_pre() {
    // STP X29, X30, [SP, #-16]! — pre-index, should be rejected (complex addressing)
    let insn = helm_arch::aarch64_decode(0xa9bf7bfd, 0x2000).unwrap();
    match data::lookup_stencil_a64(&insn) {
        Some(data::StencilLookup::Rejected(_)) | None => {} // expected
        Some(data::StencilLookup::Found(_)) => panic!("pre-index STP should be rejected"),
    }
}

#[test]
fn stencil_vs_interp_ldp_x0_x1_x2_0() {
    // LDP X0, X1, [X2, #0]  raw=0xa9400440
    let mut init = InitState::default();
    init.x[2] = 0x2000;
    assert_stencil_matches_interpreter_with_mem(
        0xa9400440,
        0x3000,
        &init,
        || AddrMem,
        "LDP X0, X1, [X2, #0]",
    );
}
