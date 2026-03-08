//! AArch64 instruction classifier for timing annotation.
//!
//! Maps the raw 32-bit A64 instruction word to [`InsnClass`] using the
//! top-level encoding groups from the Arm ARM (§C4.1).  This avoids a
//! dependency on `helm-isa` — it reads the raw instruction word directly.

use helm_timing::InsnClass;

/// Classify an A64 instruction word into a timing category.
///
/// The A64 top-level decode uses bits [28:25] (`op0`):
///
/// | op0    | Group              |
/// |--------|--------------------|
/// | `0000` | Reserved / UNALLOC |
/// | `100x` | Data-processing — immediate |
/// | `101x` | Branches, exception, system |
/// | `x1x0` | Loads and stores   |
/// | `x101` | Data-processing — register |
/// | `0111` | Data-processing — SIMD & FP |
/// | `1111` | Data-processing — SIMD & FP |
pub fn classify_a64(insn: u32) -> InsnClass {
    let op0 = (insn >> 25) & 0xF; // bits [28:25]

    match op0 {
        // Data-processing — immediate: 100x
        0b1000 | 0b1001 => classify_dp_imm(insn),

        // Branches, exception generation, system: 101x
        0b1010 | 0b1011 => classify_branch_sys(insn),

        // Loads and stores: x1x0
        0b0100 | 0b0110 | 0b1100 | 0b1110 => classify_ldst(insn),

        // Data-processing — register: x101
        0b0101 | 0b1101 => classify_dp_reg(insn),

        // Data-processing — SIMD & FP: 0111, 1111
        0b0111 | 0b1111 => classify_simd_fp(insn),

        // Reserved / unallocated
        _ => InsnClass::Nop,
    }
}

/// Data-processing — immediate.
///
/// Most are simple ALU ops.  We don't try to distinguish MUL/DIV here
/// since the immediate encoding space doesn't include multiply/divide.
fn classify_dp_imm(_insn: u32) -> InsnClass {
    InsnClass::IntAlu
}

/// Branches, exception generation, and system instructions.
fn classify_branch_sys(insn: u32) -> InsnClass {
    let op0_hi = (insn >> 29) & 0x7; // bits [31:29]

    match op0_hi {
        // 000 = B (unconditional)
        0b000 => InsnClass::Branch,
        // 001 = CBZ/CBNZ or B.cond
        0b001 => InsnClass::CondBranch,
        // 010 = B (unconditional) or BR/BLR/RET
        0b010 => InsnClass::Branch,
        // 100 = B (unconditional)
        0b100 => InsnClass::Branch,
        // 101 = CB/TB (conditional)
        0b101 => InsnClass::CondBranch,
        // 110 = BL (unconditional call), BR/BLR/RET, or system
        0b110 => {
            // System instructions (MSR, MRS, NOP, SVC, HVC, SMC)
            // SVC is bits [31:21] = 11010100_000
            let op_hi = (insn >> 21) & 0x7FF;
            if op_hi == 0b110_1010_0000 {
                InsnClass::Syscall
            } else {
                InsnClass::Branch
            }
        }
        // 011 = TB (test & branch)
        0b011 => InsnClass::CondBranch,
        // 111 = TB
        0b111 => InsnClass::CondBranch,
        _ => InsnClass::Nop,
    }
}

/// Loads and stores.
fn classify_ldst(insn: u32) -> InsnClass {
    // bit 22 = load (1) or store (0) for most encodings
    let is_load = (insn >> 22) & 1 != 0;

    // Special case: some atomics, prefetch, etc. don't follow this rule
    // but for timing purposes "Load" is a safe conservative classification
    // for anything with bit 22 set.
    if is_load {
        InsnClass::Load
    } else {
        InsnClass::Store
    }
}

/// Data-processing — register.
///
/// Further classify by the "op" sub-fields to separate MUL and DIV
/// from simple ALU operations.
fn classify_dp_reg(insn: u32) -> InsnClass {
    // bits [28:24] further refine the group
    let op1 = (insn >> 24) & 0x1; // bit 24
    let op2 = (insn >> 21) & 0xF; // bits [24:21] (low 4 of sub-encoding)

    // Data-processing (3 source) — MUL/MADD/MSUB: op1=1, op2[3]=1
    if op1 == 1 && (op2 >> 3) & 1 == 1 {
        return InsnClass::IntMul;
    }

    // Data-processing (2 source) — includes UDIV/SDIV: op1=0, op2=0b0110
    if op1 == 0 && op2 == 0b0110 {
        let opcode2 = (insn >> 10) & 0x3F; // bits [15:10]
                                           // UDIV = 00001x, SDIV = 00001x (bit 10 distinguishes)
        if opcode2 & 0b111110 == 0b000010 {
            return InsnClass::IntDiv;
        }
    }

    InsnClass::IntAlu
}

/// Data-processing — SIMD & FP.
fn classify_simd_fp(insn: u32) -> InsnClass {
    // bit 28 = 0 means scalar FP, bit 28 = 1 means advanced SIMD
    let is_simd = (insn >> 28) & 1 != 0;

    if is_simd {
        return InsnClass::Simd;
    }

    // For scalar FP, further distinguish MUL/DIV from add/sub
    let op3 = (insn >> 10) & 0x3F; // bits [15:10] — opcode field
                                   // FMUL variants have specific opcode bits
    if op3 & 0b001111 == 0b000010 {
        InsnClass::FpMul
    } else if op3 & 0b001111 == 0b000110 {
        // FDIV
        InsnClass::FpDiv
    } else {
        InsnClass::FpAlu
    }
}
