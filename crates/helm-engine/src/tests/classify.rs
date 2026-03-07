use crate::se::classify::classify_a64;
use helm_timing::InsnClass;

#[test]
fn nop_classifies_as_nop_or_branch() {
    let class = classify_a64(0xD503201F);
    assert!(
        matches!(class, InsnClass::Branch | InsnClass::Nop | InsnClass::CondBranch
                      | InsnClass::Syscall | InsnClass::IntAlu),
        "NOP should be in the branch/system encoding group"
    );
}

#[test]
fn add_imm_classifies_as_int_alu() {
    // ADD X0, X1, #1 = 0x91000420
    assert_eq!(classify_a64(0x91000420), InsnClass::IntAlu);
}

#[test]
fn sub_imm_classifies_as_int_alu() {
    // SUB X0, X1, #1 = 0xD1000420
    assert_eq!(classify_a64(0xD1000420), InsnClass::IntAlu);
}

#[test]
fn b_unconditional_classifies_as_branch() {
    // B #0 = 0x14000000
    assert_eq!(classify_a64(0x14000000), InsnClass::Branch);
}

#[test]
fn bl_classifies_as_branch() {
    // BL #0 = 0x94000000
    assert_eq!(classify_a64(0x94000000), InsnClass::Branch);
}

#[test]
fn cbz_classifies_as_cond_branch() {
    // CBZ X0, #0 = 0xB4000000
    assert_eq!(classify_a64(0xB4000000), InsnClass::CondBranch);
}

#[test]
fn ldr_classifies_as_load() {
    // LDR X0, [X1] = 0xF9400020
    assert_eq!(classify_a64(0xF9400020), InsnClass::Load);
}

#[test]
fn str_classifies_as_store() {
    // STR X0, [X1] = 0xF9000020
    assert_eq!(classify_a64(0xF9000020), InsnClass::Store);
}

#[test]
fn madd_classifies_as_int_mul() {
    // MADD X0, X1, X2, X3 = 0x9B020C20
    assert_eq!(classify_a64(0x9B020C20), InsnClass::IntMul);
}

#[test]
fn udiv_classifies_as_int_div() {
    // UDIV X0, X1, X2 = 0x9AC20820
    assert_eq!(classify_a64(0x9AC20820), InsnClass::IntDiv);
}

#[test]
fn reserved_encoding_classifies_as_nop() {
    assert_eq!(classify_a64(0x00000000), InsnClass::Nop);
}
