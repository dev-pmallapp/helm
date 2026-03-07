use crate::arm::aarch64::decode::Aarch64Decoder;
use helm_core::ir::Opcode;

fn decode(insn: u32) -> (Opcode, bool, bool, bool) {
    let d = Aarch64Decoder::new();
    let uops = d.decode_insn(0x1000, insn).unwrap();
    let u = &uops[0];
    (u.opcode, u.flags.is_branch, u.flags.is_call, u.flags.is_return)
}

#[test]
fn decode_nop() {
    let (op, br, call, ret) = decode(0xD503201F);
    assert_eq!(op, Opcode::Nop);
    assert!(!br);
    assert!(!call);
    assert!(!ret);
}

#[test]
fn decode_add_imm_is_int_alu() {
    // ADD X0, X1, #1
    let (op, ..) = decode(0x91000420);
    assert_eq!(op, Opcode::IntAlu);
}

#[test]
fn decode_sub_imm_is_int_alu() {
    // SUB X0, X1, #1
    let (op, ..) = decode(0xD1000420);
    assert_eq!(op, Opcode::IntAlu);
}

#[test]
fn decode_b_is_branch() {
    // B #0 (offset 0)
    let (op, br, call, ret) = decode(0x14000000);
    assert_eq!(op, Opcode::Branch);
    assert!(br);
    assert!(!call);
    assert!(!ret);
}

#[test]
fn decode_bl_is_branch_call() {
    // BL #0
    let (op, br, call, ret) = decode(0x94000000);
    assert_eq!(op, Opcode::Branch);
    assert!(br);
    assert!(call);
    assert!(!ret);
}

#[test]
fn decode_ret_is_branch_return() {
    // RET (X30)
    let (op, br, _call, ret) = decode(0xD65F03C0);
    assert_eq!(op, Opcode::Branch);
    assert!(br);
    assert!(ret);
}

#[test]
fn decode_br_is_branch() {
    // BR X0
    let (op, br, call, _) = decode(0xD61F0000);
    assert_eq!(op, Opcode::Branch);
    assert!(br);
    assert!(!call);
}

#[test]
fn decode_blr_is_branch_call() {
    // BLR X0
    let (op, br, call, _) = decode(0xD63F0000);
    assert_eq!(op, Opcode::Branch);
    assert!(br);
    assert!(call);
}

#[test]
fn decode_cbz_is_cond_branch() {
    // CBZ X0, #0
    let (op, br, ..) = decode(0xB4000000);
    assert_eq!(op, Opcode::CondBranch);
    assert!(br);
}

#[test]
fn decode_svc_is_syscall() {
    // SVC #0
    let (op, ..) = decode(0xD4000001);
    assert_eq!(op, Opcode::Syscall);
}

#[test]
fn decode_ldr_is_load() {
    // LDR X0, [X1]
    let (op, ..) = decode(0xF9400020);
    assert_eq!(op, Opcode::Load);
}

#[test]
fn decode_str_is_store() {
    // STR X0, [X1]
    let (op, ..) = decode(0xF9000020);
    assert_eq!(op, Opcode::Store);
}

#[test]
fn decode_madd_is_int_mul() {
    // MADD X0, X1, X2, X3 = 0x9B020C20
    let (op, ..) = decode(0x9B020C20);
    assert_eq!(op, Opcode::IntMul);
}

#[test]
fn decode_udiv_is_int_div() {
    // UDIV X0, X1, X2 = 0x9AC20820
    let (op, ..) = decode(0x9AC20820);
    assert_eq!(op, Opcode::IntDiv);
}

#[test]
fn decode_dsb_is_fence() {
    // DSB SY = 0xD5033F9F
    let (op, ..) = decode(0xD5033F9F);
    assert_eq!(op, Opcode::Fence);
}

#[test]
fn decode_movz_is_int_alu() {
    // MOVZ X0, #0x1234
    let (op, ..) = decode(0xD2824680);
    assert_eq!(op, Opcode::IntAlu);
}

#[test]
fn decoder_default_trait() {
    let d = Aarch64Decoder::default();
    let uops = d.decode_insn(0, 0xD503201F).unwrap();
    assert_eq!(uops.len(), 1);
}

#[test]
fn decode_preserves_guest_pc() {
    let d = Aarch64Decoder::new();
    let uops = d.decode_insn(0xDEAD_0000, 0xD503201F).unwrap();
    assert_eq!(uops[0].guest_pc, 0xDEAD_0000);
}

#[test]
fn decode_add_imm_has_dest_and_source() {
    let d = Aarch64Decoder::new();
    // ADD X2, X3, #7  →  rd=2, rn=3, imm=7
    // Encoding: 0x91001C62
    let uops = d.decode_insn(0, 0x91001C62).unwrap();
    let u = &uops[0];
    assert_eq!(u.dest, Some(2));
    assert!(u.sources.contains(&3));
    assert_eq!(u.immediate, Some(7));
}
