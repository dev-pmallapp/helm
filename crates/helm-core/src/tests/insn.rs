use crate::insn::*;
use std::mem;

#[test]
fn decoded_insn_fits_two_cache_lines() {
    assert!(
        mem::size_of::<DecodedInsn>() <= 128,
        "DecodedInsn too large: {} bytes",
        mem::size_of::<DecodedInsn>()
    );
}

#[test]
fn exec_outcome_reasonable_size() {
    // ExecOutcome includes Option<ExceptionInfo>, so it exceeds 64 bytes.
    // 2 cache lines (128) is the practical limit.
    assert!(
        mem::size_of::<ExecOutcome>() <= 128,
        "ExecOutcome too large: {} bytes",
        mem::size_of::<ExecOutcome>()
    );
}

#[test]
fn insn_flags_orthogonality() {
    let combined = InsnFlags::LOAD | InsnFlags::STORE;
    assert!(combined.contains(InsnFlags::LOAD));
    assert!(combined.contains(InsnFlags::STORE));
    // LOAD_STORE is a separate bit, not the combination of LOAD|STORE
    assert!(!combined.contains(InsnFlags::LOAD_STORE));
}

#[test]
fn insn_flags_all_bits_unique() {
    // Verify all flag bits are powers of two (no accidental overlap)
    let all = InsnFlags::all();
    assert_eq!(all.bits().count_ones(), 32); // all 32 bits used
}

#[test]
fn insn_class_debug() {
    assert_eq!(format!("{:?}", InsnClass::IntAlu), "IntAlu");
    assert_eq!(format!("{:?}", InsnClass::SimdFpMul), "SimdFpMul");
}

#[test]
fn decoded_insn_default_values() {
    let insn = DecodedInsn::default();
    assert_eq!(insn.pc, 0);
    assert_eq!(insn.len, 0);
    assert_eq!(insn.src_count, 0);
    assert_eq!(insn.dst_count, 0);
    assert_eq!(insn.imm, 0);
    assert!(insn.flags.is_empty());
    assert_eq!(insn.uop_count, 1);
    assert_eq!(insn.mem_count, 0);
    assert_eq!(insn.class, InsnClass::Nop);
}

#[test]
fn exec_outcome_default() {
    let outcome = ExecOutcome::default();
    assert_eq!(outcome.mem_access_count, 0);
    assert!(!outcome.branch_taken);
    assert!(outcome.exception.is_none());
    assert!(!outcome.rep_ongoing);
    assert_eq!(outcome.next_pc, 0);
}

#[test]
fn mem_access_info_default() {
    let ma = MemAccessInfo::default();
    assert_eq!(ma.addr, 0);
    assert_eq!(ma.size, 0);
    assert!(!ma.is_write);
}

#[test]
fn decoded_insn_encoding_bytes_capacity() {
    let mut insn = DecodedInsn::default();
    // x86 instructions can be up to 15 bytes
    insn.encoding_bytes = [0xFF; 15];
    insn.len = 15;
    assert_eq!(insn.encoding_bytes.len(), 15);
}
