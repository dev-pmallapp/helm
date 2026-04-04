//! Instruction fusion detector for the dynasm JIT backend.
//!
//! Detects common 2-instruction patterns and fuses them into a single
//! x86-64 emission unit. Fused pairs avoid intermediate NZCV writes and
//! consume two slots from the decode window.
//!
//! # Supported patterns
//!
//! | ID | Pattern | Frequency | Savings |
//! |----|---------|-----------|---------|
//! | F1 | `CMP Xn, #imm` + `B.cond` | ~12% | No NZCV write; direct jcc |
//! | F2 | `SUBS Xd, Xn, #1` + `B.NE` | ~5% | Loop decrement; sub+jnz |
//!
//! F3 (LDR+ALU) is deferred — dynasm label management across two distinct
//! emitter modules is complex and the win is smaller.

use helm_arch::aarch64::insn::{Instruction, Opcode};

/// A detected fusable instruction pair.
///
/// Both variants reference borrowed instruction slices — no allocation needed.
#[derive(Debug)]
pub enum FusedPair<'a> {
    /// F1: `CMP Xn, #imm` immediately followed by `B.cond`.
    ///
    /// The comparison is a `SUBS XZR, Xn, #imm` (rd == 31 in SubsImm).
    CmpBranch {
        cmp: &'a Instruction,
        branch: &'a Instruction,
    },
    /// F2: `SUBS Xd, Xn, #1` immediately followed by `B.NE` (cond==1).
    ///
    /// Classic loop decrement pattern.
    SubsBne {
        subs: &'a Instruction,
        bne: &'a Instruction,
    },
}

/// Try to detect a fusable pair starting at `insns[0]`.
///
/// Returns `Some((pair, consumed))` if a fusion opportunity is found, where
/// `consumed` is the number of instructions that make up the pair (always 2).
pub fn try_fuse(insns: &[Instruction]) -> Option<(FusedPair<'_>, usize)> {
    let a = insns.first()?;
    let b = insns.get(1)?;

    // F1: CMP Xn, #imm + B.cond
    // CMP is encoded as SUBS XZR, Xn, #imm (rd == 31).
    if is_cmp_imm(a) && b.opcode == Opcode::BCond {
        return Some((FusedPair::CmpBranch { cmp: a, branch: b }, 2));
    }

    // F2: SUBS Xd, Xn, #1 + B.NE (cond 1 = NE)
    if a.opcode == Opcode::SubsImm
        && a.imm == 1
        && a.rd != 31
        && b.opcode == Opcode::BCond
        && b.cond == 1
    {
        return Some((FusedPair::SubsBne { subs: a, bne: b }, 2));
    }

    None
}

/// Returns true if `insn` is a CMP immediate (SUBS with rd == 31).
#[inline]
pub fn is_cmp_imm(insn: &Instruction) -> bool {
    insn.opcode == Opcode::SubsImm && insn.rd == 31
}
