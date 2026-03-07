//! Decode name tests — verify the generated name decoders from .decode files
//! correctly identify instruction mnemonics from real A64 encodings.
//!
//! These test the DECODE side only (pattern matching), not execution.

// Include generated name decoders
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_dp_imm.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_dp_reg.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_branch.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_ldst.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_fp.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_simd.rs"));

// ═══════════════════════════════════════════════════════════════════
// DP-Immediate
// ═══════════════════════════════════════════════════════════════════

#[test] fn name_add_imm()   { assert_eq!(decode_aarch64_dp_imm(0x9100A820), "ADD_imm"); }
#[test] fn name_adds_imm()  { assert_eq!(decode_aarch64_dp_imm(0xB100A820), "ADDS_imm"); }
#[test] fn name_sub_imm()   { assert_eq!(decode_aarch64_dp_imm(0xD1000420), "SUB_imm"); }
#[test] fn name_subs_imm()  { assert_eq!(decode_aarch64_dp_imm(0xF1000420), "SUBS_imm"); }
#[test] fn name_and_imm()   { assert_eq!(decode_aarch64_dp_imm(0x92400420), "AND_imm"); }
#[test] fn name_orr_imm()   { assert_eq!(decode_aarch64_dp_imm(0xB2400020), "ORR_imm"); }
#[test] fn name_eor_imm()   { assert_eq!(decode_aarch64_dp_imm(0xD2400020), "EOR_imm"); }
#[test] fn name_movz()      { assert_eq!(decode_aarch64_dp_imm(0xD2824680), "MOVZ"); }
#[test] fn name_movn()      { assert_eq!(decode_aarch64_dp_imm(0x92800000), "MOVN"); }
#[test] fn name_movk()      { assert_eq!(decode_aarch64_dp_imm(0xF2A0ACF0), "MOVK"); }
#[test] fn name_adr()       { assert_eq!(decode_aarch64_dp_imm(0x10000000), "ADR"); }
#[test] fn name_adrp()      { assert_eq!(decode_aarch64_dp_imm(0x90000000), "ADRP"); }
#[test] fn name_sbfm()      { assert_eq!(decode_aarch64_dp_imm(0x93401C20), "SBFM"); }
#[test] fn name_ubfm()      { assert_eq!(decode_aarch64_dp_imm(0xD3401C20), "UBFM"); }
#[test] fn name_bfm()       { assert_eq!(decode_aarch64_dp_imm(0xB3401C20), "BFM"); }

// ═══════════════════════════════════════════════════════════════════
// DP-Register
// ═══════════════════════════════════════════════════════════════════

#[test] fn name_add_reg()    { assert_eq!(decode_aarch64_dp_reg(0x8B020020), "ADD_reg"); }
#[test] fn name_sub_reg()    { assert_eq!(decode_aarch64_dp_reg(0xCB020020), "SUB_reg"); }
#[test] fn name_and_reg()    { assert_eq!(decode_aarch64_dp_reg(0x8A020020), "AND_reg"); }
#[test] fn name_orr_reg()    { assert_eq!(decode_aarch64_dp_reg(0xAA020020), "ORR_reg"); }
#[test] fn name_eor_reg()    { assert_eq!(decode_aarch64_dp_reg(0xCA020020), "EOR_reg"); }
#[test] fn name_bic()        { assert_eq!(decode_aarch64_dp_reg(0x8A220020), "BIC"); }
#[test] fn name_orn()        { assert_eq!(decode_aarch64_dp_reg(0xAA220020), "ORN"); }
#[test] fn name_madd()       { assert_eq!(decode_aarch64_dp_reg(0x9B020C20), "MADD"); }
#[test] fn name_msub()       { assert_eq!(decode_aarch64_dp_reg(0x9B028C20), "MSUB"); }
#[test] fn name_udiv()       { assert_eq!(decode_aarch64_dp_reg(0x9AC20820), "UDIV"); }
#[test] fn name_sdiv()       { assert_eq!(decode_aarch64_dp_reg(0x9AC20C20), "SDIV"); }
#[test] fn name_lslv()       { assert_eq!(decode_aarch64_dp_reg(0x9AC22020), "LSLV"); }
#[test] fn name_lsrv()       { assert_eq!(decode_aarch64_dp_reg(0x9AC22420), "LSRV"); }
#[test] fn name_asrv()       { assert_eq!(decode_aarch64_dp_reg(0x9AC22820), "ASRV"); }
#[test] fn name_rorv()       { assert_eq!(decode_aarch64_dp_reg(0x9AC22C20), "RORV"); }
#[test] fn name_csel()       { assert_eq!(decode_aarch64_dp_reg(0x9A820020), "CSEL"); }
#[test] fn name_csinc()      { assert_eq!(decode_aarch64_dp_reg(0x9A820420), "CSINC"); }
#[test] fn name_rbit()       { assert_eq!(decode_aarch64_dp_reg(0xDAC00020), "RBIT"); }
#[test] fn name_clz()        { assert_eq!(decode_aarch64_dp_reg(0xDAC01020), "CLZ"); }
#[test] fn name_cls()        { assert_eq!(decode_aarch64_dp_reg(0xDAC01420), "CLS"); }
#[test] fn name_rev()        { assert_eq!(decode_aarch64_dp_reg(0xDAC00C20), "REV"); }
// New additions:
#[test] fn name_adc()        { assert_eq!(decode_aarch64_dp_reg(0x9A020020), "ADC"); }
#[test] fn name_sbc()        { assert_eq!(decode_aarch64_dp_reg(0xDA020020), "SBC"); }
#[test] fn name_smaddl()     { assert_eq!(decode_aarch64_dp_reg(0x9B220C20), "SMADDL"); }
#[test] fn name_smsubl()     { assert_eq!(decode_aarch64_dp_reg(0x9B228C20), "SMSUBL"); }

// ═══════════════════════════════════════════════════════════════════
// Branches
// ═══════════════════════════════════════════════════════════════════

#[test] fn name_b()          { assert_eq!(decode_aarch64_branch(0x14000040), "B"); }
#[test] fn name_bl()         { assert_eq!(decode_aarch64_branch(0x94000040), "BL"); }
#[test] fn name_b_cond()     { assert_eq!(decode_aarch64_branch(0x54000080), "B_cond"); }
#[test] fn name_cbz()        { assert_eq!(decode_aarch64_branch(0xB4000080), "CBZ"); }
#[test] fn name_cbnz()       { assert_eq!(decode_aarch64_branch(0xB5000080), "CBNZ"); }
#[test] fn name_tbz()        { assert_eq!(decode_aarch64_branch(0x36280000), "TBZ"); }
#[test] fn name_tbnz()       { assert_eq!(decode_aarch64_branch(0x37280000), "TBNZ"); }
#[test] fn name_br()         { assert_eq!(decode_aarch64_branch(0xD61F0100), "BR"); }
#[test] fn name_blr()        { assert_eq!(decode_aarch64_branch(0xD63F0100), "BLR"); }
#[test] fn name_ret()        { assert_eq!(decode_aarch64_branch(0xD65F03C0), "RET"); }
#[test] fn name_svc()        { assert_eq!(decode_aarch64_branch(0xD4000001), "SVC"); }
#[test] fn name_brk()        { assert_eq!(decode_aarch64_branch(0xD4200000), "BRK"); }
#[test] fn name_nop()        { assert_eq!(decode_aarch64_branch(0xD503201F), "NOP"); }
// New additions:
#[test] fn name_dsb()        { assert_eq!(decode_aarch64_branch(0xD503309F), "DSB"); }
#[test] fn name_dmb()        { assert_eq!(decode_aarch64_branch(0xD50330BF), "DMB"); }
#[test] fn name_isb()        { assert_eq!(decode_aarch64_branch(0xD50330DF), "ISB"); }
#[test] fn name_clrex()      { assert_eq!(decode_aarch64_branch(0xD503305F), "CLREX"); }

// ═══════════════════════════════════════════════════════════════════
// Load/Store
// ═══════════════════════════════════════════════════════════════════

#[test] fn name_ldr_x_uimm() { assert_eq!(decode_aarch64_ldst(0xF9400420), "LDR_x_uimm"); }
#[test] fn name_str_x_uimm() { assert_eq!(decode_aarch64_ldst(0xF9000420), "STR_x_uimm"); }
#[test] fn name_ldrb_uimm()  { assert_eq!(decode_aarch64_ldst(0x39400020), "LDRB_uimm"); }
#[test] fn name_strb_uimm()  { assert_eq!(decode_aarch64_ldst(0x39000020), "STRB_uimm"); }
#[test] fn name_ldrh_uimm()  { assert_eq!(decode_aarch64_ldst(0x79400020), "LDRH_uimm"); }
#[test] fn name_strh_uimm()  { assert_eq!(decode_aarch64_ldst(0x79000020), "STRH_uimm"); }
#[test] fn name_ldp_x()      { assert_eq!(decode_aarch64_ldst(0xA94107E0), "LDP_x"); }
#[test] fn name_stp_x()      { assert_eq!(decode_aarch64_ldst(0xA90107E0), "STP_x"); }
// New additions:
#[test] fn name_ldr_x_pre()  { assert_eq!(decode_aarch64_ldst(0xF8408C20), "LDR_x_pre"); }
#[test] fn name_str_x_post() { assert_eq!(decode_aarch64_ldst(0xF8008420), "STR_x_post"); }
#[test] fn name_ldur_x()     { assert_eq!(decode_aarch64_ldst(0xF8400020), "LDUR_x"); }
#[test] fn name_ldr_x_reg()  { assert_eq!(decode_aarch64_ldst(0xF8626820), "LDR_x_reg"); }
#[test] fn name_ldr_lit_x()  { assert_eq!(decode_aarch64_ldst(0x58000800), "LDR_lit_x"); }
#[test] fn name_ldar_x()     { assert_eq!(decode_aarch64_ldst(0xC8DFFC20), "LDAR_x"); }
#[test] fn name_stlr_x()     { assert_eq!(decode_aarch64_ldst(0xC89FFC20), "STLR_x"); }
#[test] fn name_ldadd_x()    { assert_eq!(decode_aarch64_ldst(0xF8220020), "LDADD_x"); }
#[test] fn name_swp_x()      { assert_eq!(decode_aarch64_ldst(0xF8228020), "SWP_x"); }

// ═══════════════════════════════════════════════════════════════════
// Scalar FP (new decode file)
// ═══════════════════════════════════════════════════════════════════

#[test] fn name_fadd_s()     { assert_eq!(decode_aarch64_fp(0x1E222820), "FADD_s"); }
#[test] fn name_fsub_s()     { assert_eq!(decode_aarch64_fp(0x1E223820), "FSUB_s"); }
#[test] fn name_fmul_s()     { assert_eq!(decode_aarch64_fp(0x1E220820), "FMUL_s"); }
#[test] fn name_fdiv_s()     { assert_eq!(decode_aarch64_fp(0x1E221820), "FDIV_s"); }
#[test] fn name_fmov_s()     { assert_eq!(decode_aarch64_fp(0x1E204020), "FMOV_s"); }
#[test] fn name_fabs_s()     { assert_eq!(decode_aarch64_fp(0x1E20C020), "FABS_s"); }
#[test] fn name_fneg_s()     { assert_eq!(decode_aarch64_fp(0x1E214020), "FNEG_s"); }
#[test] fn name_fsqrt_s()    { assert_eq!(decode_aarch64_fp(0x1E21C020), "FSQRT_s"); }
#[test] fn name_fmadd_s()    { assert_eq!(decode_aarch64_fp(0x1F020C20), "FMADD"); }
#[test] fn name_fmov_wtof()  { assert_eq!(decode_aarch64_fp(0x1E270020), "FMOV_wtof"); }
#[test] fn name_fmov_ftow()  { assert_eq!(decode_aarch64_fp(0x1E260020), "FMOV_ftow"); }
#[test] fn name_fcvt_sd()    { assert_eq!(decode_aarch64_fp(0x1E22C020), "FCVT_sd"); }

// ═══════════════════════════════════════════════════════════════════
// SIMD
// ═══════════════════════════════════════════════════════════════════

#[test] fn name_add_v_16b()  { assert_eq!(decode_aarch64_simd(0x4E228420), "ADD_v"); }
#[test] fn name_sub_v_4s()   { assert_eq!(decode_aarch64_simd(0x6EA28420), "SUB_v"); }
#[test] fn name_and_v()      { assert_eq!(decode_aarch64_simd(0x4E221C20), "AND_v"); }
#[test] fn name_orr_v()      { assert_eq!(decode_aarch64_simd(0x4EA21C20), "ORR_v"); }
#[test] fn name_eor_v()      { assert_eq!(decode_aarch64_simd(0x6E221C20), "EOR_v"); }
#[test] fn name_mul_v()      { assert_eq!(decode_aarch64_simd(0x4EA29C20), "MUL_v"); }
#[test] fn name_not_v()      { assert_eq!(decode_aarch64_simd(0x6E205820), "NOT_v"); }
#[test] fn name_abs_v()      { assert_eq!(decode_aarch64_simd(0x4EA0B820), "ABS_v"); }
#[test] fn name_neg_v()      { assert_eq!(decode_aarch64_simd(0x6EA0B820), "NEG_v"); }
#[test] fn name_dup_gen()    { assert_eq!(decode_aarch64_simd(0x0E040C20), "DUP_general"); }
#[test] fn name_fmov_ws()    { assert_eq!(decode_aarch64_simd(0x1E260020), "FMOV_ws"); }
#[test] fn name_fmov_sw()    { assert_eq!(decode_aarch64_simd(0x1E270020), "FMOV_sw"); }

// SIMD — new three-same integer
#[test] fn name_sabd_v()     { assert_eq!(decode_aarch64_simd(0x4EA27420), "SABD_v"); }
#[test] fn name_uabd_v()     { assert_eq!(decode_aarch64_simd(0x6EA27420), "UABD_v"); }
#[test] fn name_sqadd_v()    { assert_eq!(decode_aarch64_simd(0x4EA20C20), "SQADD_v"); }
#[test] fn name_uqadd_v()    { assert_eq!(decode_aarch64_simd(0x6EA20C20), "UQADD_v"); }
#[test] fn name_sqsub_v()    { assert_eq!(decode_aarch64_simd(0x4EA22C20), "SQSUB_v"); }
#[test] fn name_sshl_v()     { assert_eq!(decode_aarch64_simd(0x4EA24420), "SSHL_v"); }
#[test] fn name_pmul_v()     { assert_eq!(decode_aarch64_simd(0x6EA29C20), "PMUL_v"); }

// SIMD — three-same FP
#[test] fn name_fadd_v()     { assert_eq!(decode_aarch64_simd(0x4E22D420), "FADD_v"); }
#[test] fn name_fsub_v()     { assert_eq!(decode_aarch64_simd(0x4EA2D420), "FSUB_v"); }
#[test] fn name_fmul_v()     { assert_eq!(decode_aarch64_simd(0x6E22DC20), "FMUL_v"); }
#[test] fn name_fdiv_v()     { assert_eq!(decode_aarch64_simd(0x6E22FC20), "FDIV_v"); }
#[test] fn name_fmla_v()     { assert_eq!(decode_aarch64_simd(0x4E22CC20), "FMLA_v"); }
#[test] fn name_fcmeq_v()    { assert_eq!(decode_aarch64_simd(0x4E22E420), "FCMEQ_v"); }

// SIMD — two-reg misc additional
// CLS_v: 0 q=1 0 0 1110 size=10 10000 00100 10 rn=00001 rd=00000
// = 0100_1110_1010_0000_0100_1000_0010_0000 = 0x4EA04820
#[test] fn name_cls_v()      { assert_eq!(decode_aarch64_simd(0x4EA04820), "CLS_v"); }
// CLZ_v: 0 q=1 1 0 1110 size=10 10000 00100 10 rn=00001 rd=00000
// = 0110_1110_1010_0000_0100_1000_0010_0000 = 0x6EA04820
#[test] fn name_clz_v()      { assert_eq!(decode_aarch64_simd(0x6EA04820), "CLZ_v"); }
#[test] fn name_xtn_v()      { assert_eq!(decode_aarch64_simd(0x0E612820), "XTN_v"); }
#[test] fn name_fabs_v()     { assert_eq!(decode_aarch64_simd(0x4EA0F820), "FABS_v"); }
#[test] fn name_fneg_v()     { assert_eq!(decode_aarch64_simd(0x6EA0F820), "FNEG_v"); }

// SIMD — widening/narrowing
#[test] fn name_saddl_v()    { assert_eq!(decode_aarch64_simd(0x0E620020), "SADDL_v"); }
#[test] fn name_uaddl_v()    { assert_eq!(decode_aarch64_simd(0x2E620020), "UADDL_v"); }
#[test] fn name_smull_v()    { assert_eq!(decode_aarch64_simd(0x0E62C020), "SMULL_v"); }
#[test] fn name_umull_v()    { assert_eq!(decode_aarch64_simd(0x2E62C020), "UMULL_v"); }
#[test] fn name_addhn_v()    { assert_eq!(decode_aarch64_simd(0x0E624020), "ADDHN_v"); }

// SIMD — scalar
#[test] fn name_sqadd_s()    { assert_eq!(decode_aarch64_simd(0x5EA20C20), "SQADD_s"); }
#[test] fn name_sqsub_s()    { assert_eq!(decode_aarch64_simd(0x5EA22C20), "SQSUB_s"); }
#[test] fn name_abs_s()      { assert_eq!(decode_aarch64_simd(0x5EA0B820), "ABS_s"); }
#[test] fn name_neg_s()      { assert_eq!(decode_aarch64_simd(0x7EA0B820), "NEG_s"); }

// SIMD — shift additional
#[test] fn name_srshr_v()    { assert_eq!(decode_aarch64_simd(0x4F282420), "SRSHR_v"); }
#[test] fn name_sri_v()      { assert_eq!(decode_aarch64_simd(0x6F284420), "SRI_v"); }

// Crypto
#[test] fn name_aese()       { assert_eq!(decode_aarch64_simd(0x4E284820), "AESE"); }
#[test] fn name_aesd()       { assert_eq!(decode_aarch64_simd(0x4E285820), "AESD"); }
#[test] fn name_aesmc()      { assert_eq!(decode_aarch64_simd(0x4E286820), "AESMC"); }
#[test] fn name_aesimc()     { assert_eq!(decode_aarch64_simd(0x4E287820), "AESIMC"); }

// Dot product
#[test] fn name_sdot_v()     { assert_eq!(decode_aarch64_simd(0x4E829420), "SDOT_v"); }
#[test] fn name_udot_v()     { assert_eq!(decode_aarch64_simd(0x6E829420), "UDOT_v"); }

// SIMD — element/indexed
// MUL_vi: 0 q=1 0 0 1111 size=10 l=0 m=0 rm=0010 1000 h=0 0 rn=00001 rd=00000
// = 0100_1111_1000_0010_1000_0000_0010_0000 = 0x4F828020
#[test] fn name_mul_vi()     { assert_eq!(decode_aarch64_simd(0x4F828020), "MUL_vi"); }

// SIMD — saturating multiply
#[test] fn name_sqdmulh_v()  { assert_eq!(decode_aarch64_simd(0x4EA2B420), "SQDMULH_v"); }
#[test] fn name_sqrdmulh_v() { assert_eq!(decode_aarch64_simd(0x6EA2B420), "SQRDMULH_v"); }

// SIMD — widening
#[test] fn name_pmull_v()    { assert_eq!(decode_aarch64_simd(0x0E62E020), "PMULL_v"); }
#[test] fn name_sqdmull_v()  { assert_eq!(decode_aarch64_simd(0x0E62D020), "SQDMULL_v"); }

// SIMD — narrowing
#[test] fn name_sqxtn_v()    { assert_eq!(decode_aarch64_simd(0x0EA14820), "SQXTN_v"); }
#[test] fn name_uqxtn_v()    { assert_eq!(decode_aarch64_simd(0x2EA14820), "UQXTN_v"); }

// SIMD — FP conversions (vector)
#[test] fn name_scvtf_vf()   { assert_eq!(decode_aarch64_simd(0x4E21D820), "SCVTF_vf"); }
#[test] fn name_ucvtf_vf()   { assert_eq!(decode_aarch64_simd(0x6E21D820), "UCVTF_vf"); }
#[test] fn name_fcvtl_v()    { assert_eq!(decode_aarch64_simd(0x0E217820), "FCVTL_v"); }
#[test] fn name_fcvtn_v()    { assert_eq!(decode_aarch64_simd(0x0E216820), "FCVTN_v"); }

// SIMD — scalar shift
#[test] fn name_shl_s()      { assert_eq!(decode_aarch64_simd(0x5F405420), "SHL_s"); }

// SIMD — scalar FP compare
#[test] fn name_fcmeq_s()    { assert_eq!(decode_aarch64_simd(0x5E22E420), "FCMEQ_s"); }
#[test] fn name_frecpe_s()   { assert_eq!(decode_aarch64_simd(0x5EA1D820), "FRECPE_s"); }
#[test] fn name_frsqrte_s()  { assert_eq!(decode_aarch64_simd(0x7EA1D820), "FRSQRTE_s"); }

// Crypto — SHA512
#[test] fn name_sha512h()    { assert_eq!(decode_aarch64_simd(0xCE628020), "SHA512H"); }

// Crypto — misc
#[test] fn name_eor3()       { assert_eq!(decode_aarch64_simd(0xCE020020), "EOR3"); }
#[test] fn name_bcax()       { assert_eq!(decode_aarch64_simd(0xCE220020), "BCAX"); }

// BFloat16
#[test] fn name_bfdot_v()    { assert_eq!(decode_aarch64_simd(0x6E42FC20), "BFDOT_v"); }
#[test] fn name_bfmmla()     { assert_eq!(decode_aarch64_simd(0x6E42EC20), "BFMMLA"); }

// Matrix multiply
#[test] fn name_smmla()      { assert_eq!(decode_aarch64_simd(0x4E82A420), "SMMLA"); }
#[test] fn name_ummla()      { assert_eq!(decode_aarch64_simd(0x6E82A420), "UMMLA"); }

// Scalar FP — indexed multiply (complex encoding, skip for now)
#[test] fn name_sqdmulh_s()  { assert_eq!(decode_aarch64_fp(0x5EA2B420), "SQDMULH_s"); }

// FP — fixed-point convert
#[test] fn name_scvtf_f()    { assert_eq!(decode_aarch64_fp(0x1E020020), "SCVTF_f"); }
#[test] fn name_fcvtzs_f()   { assert_eq!(decode_aarch64_fp(0x1E180020), "FCVTZS_f"); }

// Authenticated branches (ARMv8.3-PAuth)
#[test] fn name_braz()       { assert_eq!(decode_aarch64_branch(0xD61F083F), "BRAZ"); }
#[test] fn name_blraz()      { assert_eq!(decode_aarch64_branch(0xD63F083F), "BLRAZ"); }
#[test] fn name_reta()       { assert_eq!(decode_aarch64_branch(0xD65F0BFF), "RETA"); }
#[test] fn name_bra()        { assert_eq!(decode_aarch64_branch(0xD71F0822), "BRA"); }
#[test] fn name_blra()       { assert_eq!(decode_aarch64_branch(0xD73F0822), "BLRA"); }

// Exception return
#[test] fn name_eret()       { assert_eq!(decode_aarch64_branch(0xD69F03E0), "ERET"); }
#[test] fn name_ereta()      { assert_eq!(decode_aarch64_branch(0xD69F0BFF), "ERETA"); }

// Pointer authentication hints
#[test] fn name_xpaclri()    { assert_eq!(decode_aarch64_branch(0xD50320FF), "XPACLRI"); }
#[test] fn name_pacia1716()  { assert_eq!(decode_aarch64_branch(0xD503211F), "PACIA1716"); }
#[test] fn name_pacib1716()  { assert_eq!(decode_aarch64_branch(0xD503215F), "PACIB1716"); }
#[test] fn name_autia1716()  { assert_eq!(decode_aarch64_branch(0xD503219F), "AUTIA1716"); }
#[test] fn name_autib1716()  { assert_eq!(decode_aarch64_branch(0xD50321DF), "AUTIB1716"); }
#[test] fn name_paciaz()     { assert_eq!(decode_aarch64_branch(0xD503231F), "PACIAZ"); }
#[test] fn name_paciasp()    { assert_eq!(decode_aarch64_branch(0xD503233F), "PACIASP"); }
#[test] fn name_pacibz()     { assert_eq!(decode_aarch64_branch(0xD503235F), "PACIBZ"); }
#[test] fn name_pacibsp()    { assert_eq!(decode_aarch64_branch(0xD503237F), "PACIBSP"); }
#[test] fn name_autiaz()     { assert_eq!(decode_aarch64_branch(0xD503239F), "AUTIAZ"); }
#[test] fn name_autiasp()    { assert_eq!(decode_aarch64_branch(0xD50323BF), "AUTIASP"); }
#[test] fn name_autibz()     { assert_eq!(decode_aarch64_branch(0xD50323DF), "AUTIBZ"); }
#[test] fn name_autibsp()    { assert_eq!(decode_aarch64_branch(0xD50323FF), "AUTIBSP"); }

// Misc hints
#[test] fn name_esb()        { assert_eq!(decode_aarch64_branch(0xD503221F), "ESB"); }
#[test] fn name_chkfeat()    { assert_eq!(decode_aarch64_branch(0xD503251F), "CHKFEAT"); }

// System instructions with register argument
#[test] fn name_wfet()       { assert_eq!(decode_aarch64_branch(0xD5031001), "WFET"); }
#[test] fn name_wfit()       { assert_eq!(decode_aarch64_branch(0xD5031021), "WFIT"); }

// Additional barriers
#[test] fn name_sb()         { assert_eq!(decode_aarch64_branch(0xD50330FF), "SB"); }

// PSTATE flag manipulation
#[test] fn name_cfinv()      { assert_eq!(decode_aarch64_branch(0xD500401F), "CFINV"); }
#[test] fn name_xaflag()     { assert_eq!(decode_aarch64_branch(0xD500403F), "XAFLAG"); }
#[test] fn name_axflag()     { assert_eq!(decode_aarch64_branch(0xD500405F), "AXFLAG"); }

// MSR (immediate) variants
#[test] fn name_msr_i_daifset()   { assert_eq!(decode_aarch64_branch(0xD5034FDF), "MSR_i_DAIFSET"); }
#[test] fn name_msr_i_daifclear() { assert_eq!(decode_aarch64_branch(0xD5034FFF), "MSR_i_DAIFCLEAR"); }
#[test] fn name_msr_i_spsel()     { assert_eq!(decode_aarch64_branch(0xD50041BF), "MSR_i_SPSEL"); }
#[test] fn name_msr_i_pan()       { assert_eq!(decode_aarch64_branch(0xD500419F), "MSR_i_PAN"); }
#[test] fn name_msr_i_uao()       { assert_eq!(decode_aarch64_branch(0xD500417F), "MSR_i_UAO"); }

// SYS / SYSL
#[test] fn name_sys()        { assert_eq!(decode_aarch64_branch(0xD5087620), "SYS"); }
#[test] fn name_sysl()       { assert_eq!(decode_aarch64_branch(0xD52B0020), "SYSL"); }
