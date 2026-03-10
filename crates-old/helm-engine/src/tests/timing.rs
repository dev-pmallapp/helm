use crate::se::classify::classify_a64;
use helm_timing::InsnClass;

// ---------------------------------------------------------------------------
// Test helpers — encode minimal A64 instruction words
// ---------------------------------------------------------------------------

/// Encode bits [28:25] (op0) with optional high bits.
fn encode_op0(op0: u32) -> u32 {
    op0 << 25
}

/// Build a minimal A64 branch instruction with specific bits [31:29].
fn encode_branch(op0_hi: u32) -> u32 {
    // op0 = 101x → bits [28:25] = 0b1010
    let base = 0b1010 << 25;
    base | (op0_hi << 29)
}

// ---------------------------------------------------------------------------
// Data-processing — immediate (100x)
// ---------------------------------------------------------------------------

#[test]
fn dp_imm_classifies_as_int_alu() {
    // op0 = 1000
    assert_eq!(classify_a64(encode_op0(0b1000)), InsnClass::IntAlu);
    // op0 = 1001
    assert_eq!(classify_a64(encode_op0(0b1001)), InsnClass::IntAlu);
}

// ---------------------------------------------------------------------------
// Branches (101x)
// ---------------------------------------------------------------------------

#[test]
fn unconditional_branch_classifies_as_branch() {
    // bits [31:29] = 000 → B (unconditional)
    assert_eq!(classify_a64(encode_branch(0b000)), InsnClass::Branch);
}

#[test]
fn conditional_branch_classifies_as_cond_branch() {
    // bits [31:29] = 001 → CBZ/CBNZ, B.cond
    assert_eq!(classify_a64(encode_branch(0b001)), InsnClass::CondBranch);
    // bits [31:29] = 011 → TB (test & branch)
    assert_eq!(classify_a64(encode_branch(0b011)), InsnClass::CondBranch);
    // bits [31:29] = 101 → CB/TB (conditional)
    assert_eq!(classify_a64(encode_branch(0b101)), InsnClass::CondBranch);
}

// ---------------------------------------------------------------------------
// Loads and stores (x1x0)
// ---------------------------------------------------------------------------

#[test]
fn load_classifies_correctly() {
    // op0 = 0100, bit 22 set (load)
    let insn = encode_op0(0b0100) | (1 << 22);
    assert_eq!(classify_a64(insn), InsnClass::Load);
}

#[test]
fn store_classifies_correctly() {
    // op0 = 0100, bit 22 clear (store)
    let insn = encode_op0(0b0100);
    assert_eq!(classify_a64(insn), InsnClass::Store);
}

#[test]
fn ldst_variants() {
    // All x1x0 patterns should classify as load or store
    for op0 in [0b0100u32, 0b0110, 0b1100, 0b1110] {
        let insn_load = (op0 << 25) | (1 << 22);
        let insn_store = op0 << 25;
        assert_eq!(classify_a64(insn_load), InsnClass::Load);
        assert_eq!(classify_a64(insn_store), InsnClass::Store);
    }
}

// ---------------------------------------------------------------------------
// Data-processing — register (x101)
// ---------------------------------------------------------------------------

#[test]
fn dp_reg_simple_alu() {
    // op0 = 0101, simple encoding (no MUL/DIV bits)
    assert_eq!(classify_a64(encode_op0(0b0101)), InsnClass::IntAlu);
    assert_eq!(classify_a64(encode_op0(0b1101)), InsnClass::IntAlu);
}

#[test]
fn dp_reg_mul_classifies() {
    // Data-processing (3 source) — op1=1 (bit 24), op2[3]=1 (bit 24 of sub-encoding)
    // op0 = 0101, bit 24 set, bit 24 of op2 (bit 24) = already set
    // Actually: op1 = bit 24, op2 = bits [24:21]
    // For MUL: op1=1, op2[3]=1 → bit 24=1, bit 24=1 (already), plus bit 24 (op2[3]) = bit 24
    // Simplify: set bit 24 and set bit 24 → op2 >> 3 & 1 == 1 means bit 24 set
    // But op1 = (insn >> 24) & 1 and op2 = (insn >> 21) & 0xF
    // For MUL: op1=1 → bit 24=1, op2[3]=1 → bit 24=1 (same bit!)
    // So any instruction with op0=x101 and bit 24 set is classified as MUL
    let insn = encode_op0(0b0101) | (1 << 24);
    assert_eq!(classify_a64(insn), InsnClass::IntMul);
}

// ---------------------------------------------------------------------------
// SIMD & FP (0111, 1111)
// ---------------------------------------------------------------------------

#[test]
fn simd_fp_classifies() {
    // op0 = 0111
    let insn = encode_op0(0b0111);
    let class = classify_a64(insn);
    assert!(
        matches!(
            class,
            InsnClass::FpAlu | InsnClass::FpMul | InsnClass::FpDiv | InsnClass::Simd
        ),
        "expected FP/SIMD class, got {:?}",
        class
    );
}

#[test]
fn simd_with_bit28_set() {
    // op0 = 1111, bit 28 set → advanced SIMD
    let insn = encode_op0(0b1111);
    // bit 28 is part of op0, already set in 1111
    assert_eq!(classify_a64(insn), InsnClass::Simd);
}

// ---------------------------------------------------------------------------
// Reserved / unallocated
// ---------------------------------------------------------------------------

#[test]
fn reserved_classifies_as_nop() {
    // op0 = 0000 → reserved
    assert_eq!(classify_a64(encode_op0(0b0000)), InsnClass::Nop);
}

// ---------------------------------------------------------------------------
// Real instruction encodings
// ---------------------------------------------------------------------------

#[test]
fn real_add_immediate() {
    // ADD X0, X1, #1  →  0x91000420
    // op0 = bits [28:25] = 0b1001 → dp_imm → IntAlu
    let insn: u32 = 0x9100_0420;
    assert_eq!(classify_a64(insn), InsnClass::IntAlu);
}

#[test]
fn real_ldr_register() {
    // LDR X0, [X1]  →  0xF9400020
    // op0 = bits [28:25] of 0xF9400020 = 0b1100 → ldst, bit 22 set → Load
    let insn: u32 = 0xF940_0020;
    assert_eq!(classify_a64(insn), InsnClass::Load);
}

#[test]
fn real_str_register() {
    // STR X0, [X1]  →  0xF9000020
    // op0 = bits [28:25] = 0b1100 → ldst, bit 22 clear → Store
    let insn: u32 = 0xF900_0020;
    assert_eq!(classify_a64(insn), InsnClass::Store);
}

#[test]
fn real_b_unconditional() {
    // B #4  →  0x14000001
    // op0 = bits [28:25] = 0b1010 → branch, bits [31:29] = 000 → Branch
    let insn: u32 = 0x1400_0001;
    assert_eq!(classify_a64(insn), InsnClass::Branch);
}

#[test]
fn real_b_cond() {
    // B.EQ #8  →  0x54000040
    // op0 = bits [28:25] = 0b1010 → branch, bits [31:29] = 010 → Branch
    let insn: u32 = 0x5400_0040;
    assert_eq!(classify_a64(insn), InsnClass::Branch);
}

#[test]
fn real_cbz() {
    // CBZ X0, #8  →  0xB4000040
    // op0 = bits [28:25] = 0b1010 → branch, bits [31:29] = 101 → CondBranch
    let insn: u32 = 0xB400_0040;
    assert_eq!(classify_a64(insn), InsnClass::CondBranch);
}

#[test]
fn real_nop() {
    // NOP  →  0xD503201F
    // op0 = bits [28:25] = 0b1010 → branch/system
    // bits [31:29] = 110 → system group
    let insn: u32 = 0xD503_201F;
    let class = classify_a64(insn);
    // NOP is in the system/branch group — classified as Branch or Syscall
    assert!(
        matches!(class, InsnClass::Branch | InsnClass::Syscall),
        "NOP classified as {:?}",
        class
    );
}
