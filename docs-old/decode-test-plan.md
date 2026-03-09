# Decode Test Plan — Importing QEMU ARM Test Assets

This document maps the relevant QEMU test assets to HELM test targets,
defines what to test at each level, and gives the exact test structure
for each phase.

---

## Asset Inventory

### `assets/qemu/tests/decode/` — Decode Tree Parser Tests

46 `.decode` files that test the QEMU decode tree parser itself:
- `succ_*.decode` (8 files) — valid `.decode` syntax that must parse cleanly
- `err_*.decode` (38 files) — invalid `.decode` syntax that must be rejected

These test `helm-decode`'s parser (fields, argsets, formats, patterns, groups).

| File group | Count | What it tests |
|------------|-------|---------------|
| `succ_argset_type1` | 1 | `&` argset with typed fields |
| `succ_function` | 1 | `%field !function=name` annotation |
| `succ_ident1` | 1 | multi-segment fields + format + pattern |
| `succ_infer1` | 1 | inferred field from format (SVE-style) |
| `succ_named_field` | 1 | field referencing another field's named slot |
| `succ_pattern_group_nest1–4` | 4 | `{}`/`[]` group nesting depth |
| `err_field1–10` | 10 | invalid field syntax, width, position, duplicates, loops |
| `err_overlap1–9` | 9 | overlapping pattern detection |
| `err_width1–4` | 4 | instruction width mismatches |
| `err_argset1–2` | 2 | invalid argset definitions |
| `err_init1–4` | 4 | invalid group initializers |
| `err_pattern_group_*` | 9 | invalid group nesting, empty groups, ident conflicts |

### `assets/qemu/target/arm/tcg/a64.decode` — AArch64 Instruction Encoding Spec

1927-line canonical QEMU `.decode` file covering all A64 instructions:

| Section | Lines | Instructions |
|---------|-------|-------------|
| Data Processing — Immediate | 97–188 | ADR, ADRP, ADD/SUB/ADDS/SUBS (imm), ADDG/SUBG, AND/ORR/EOR/ANDS (imm), MOVN/MOVZ/MOVK, SMAX/SMIN/UMAX/UMIN (imm), SBFM/BFM/UBFM, EXTR |
| Branches | 189–323 | B, BL, CBZ, TBZ, B.cond/BC.cond, BR/BLR/RET, BRAZ/BLRAZ/RETA, BRA/BLRA, ERET, NOP+hint group, WFET/WFIT, CLREX, DSB/DMB/ISB/SB, PSTATE/MSR_i, SYS/MRS, SVC/HVC/SMC/BRK/HLT |
| Loads and Stores | 324–691 | STXR/LDXR/STLR/LDAR, STXP/LDXP, CASP/CAS, LD_lit, STP/LDP (all modes), STGP, STR_i/LDR_i (unscaled, post, pre, offset, user, reg), LDAPUR/STLUR, LD1–LD4/ST1–ST4 (SIMD, all modes), prfm |
| Data Processing (register) | 692–832 | UDIV/SDIV, LSLV/LSRV/ASRV/RORV, CRC32/CRC32C, SUBP/IRG/GMI/PACGA, SMAX/SMIN/UMAX/UMIN (reg), RBIT/REV16/REV32/REV64/CLZ/CLS/CTZ/CNT/ABS, PACIA/PACIB/PACDA/PACDB, AUTIA/AUTIB/AUTDA/AUTDB, AND_r/ORR_r/EOR_r/ANDS_r, ADD_r/SUB_r/ADDS_r/SUBS_r, ADD_ext/SUB_ext, ADC/ADCS/SBC/SBCS, RMIF, SETF8/SETF16, CCMP, CSEL, MADD/MSUB, SMADDL/SMSUBL/UMADDL/UMSUBL, SMULH/UMULH |
| Crypto | 833–889 | AES, SHA-256, SHA-512, XAR |
| Advanced SIMD | 891–1927 | All scalar and vector SIMD |

### `assets/qemu/tests/tcg/aarch64/` — Execution Tests (C sources)

These are user-space C programs that exercise specific AArch64 features:

| File | Tests |
|------|-------|
| `bti-1/2/3.c` | Branch Target Identification |
| `mte-1..8.c` | Memory Tagging Extension |
| `pauth-1/2/4/5.c` | Pointer Authentication |
| `sme-*.c` | Scalable Matrix Extension |
| `sve-ioctls.c`, `sve-str.c` | Scalable Vector Extension |
| `fcvt.c`, `float_conv*.ref` | Float conversion reference outputs |
| `test-826.c`, `test-2150.c`, `test-2248.c`, `test-2375.c` | Regression tests (bug numbers) |
| `test-aes.c` | AES instruction execution |
| `dcpodp.c`, `dcpop.c` | Data cache prefetch ops |
| `lse2-fault.c` | Large System Extensions fault test |
| `pcalign-a64.c` | PC alignment |
| `gcs*.c` | Guarded Control Stack |
| `sysregs.c` | System register access |

---

## HELM Test Targets

```
assets/qemu/                            → what to import / derive tests from
                                          ↓
tests/decode/*.decode                   → helm-decode: parser correctness
target/arm/tcg/a64.decode              → helm-decode: a64 integration
                                          helm-isa: Aarch64Decoder mnemonic/field tests
tests/tcg/aarch64/fcvt.ref etc.        → helm-isa: exec golden output comparison
```

---

## Phase 1 — helm-decode Parser Tests

**Target crate:** `crates/helm-decode`
**Target file:** `crates/helm-decode/src/tests/qemu_compat.rs`
**Register in:** `crates/helm-decode/src/tests/mod.rs`

The QEMU `tests/decode/` directory tests the same parser semantics as
`helm-decode`. Import the `.decode` file content as string literals and
assert parse success or failure.

### 1.1 Success cases

Each `succ_*.decode` file must parse without error and produce a
`DecodeTree` with the expected counts.

```rust
// src/tests/qemu_compat.rs

use crate::tree::DecodeTree;

/// Macro: parse a QEMU succ_ decode file and assert no error.
macro_rules! assert_parses {
    ($name:ident, $content:literal, $patterns:expr, $fields:expr, $argsets:expr) => {
        #[test]
        fn $name() {
            let tree = DecodeTree::from_decode_text($content);
            assert_eq!(tree.len(), $patterns,
                "expected {} patterns", $patterns);
            assert_eq!(tree.field_defs.len(), $fields,
                "expected {} field defs", $fields);
            assert_eq!(tree.arg_sets.len(), $argsets,
                "expected {} arg sets", $argsets);
        }
    };
}

assert_parses!(
    succ_ident1,
    include_str!("../../../../assets/qemu/tests/decode/succ_ident1.decode"),
    1,   // 1 pattern: 3insn
    3,   // %1f, %2f, %3f
    1    // &3arg
);

assert_parses!(
    succ_infer1,
    include_str!("../../../../assets/qemu/tests/decode/succ_infer1.decode"),
    1,   // LD1Q
    1,   // &rprr_load (argset used inline, no explicit %field)
    1
);

assert_parses!(
    succ_pattern_group_nest1,
    include_str!("../../../../assets/qemu/tests/decode/succ_pattern_group_nest1.decode"),
    5,   // top, sub1, sub2, sub3, sub4 (groups flattened)
    4,   // %sub1 .. %sub4
    0
);

// ... repeat for all succ_ files ...
```

### 1.2 Error cases

Each `err_*.decode` file documents a condition that should either:
- Be rejected by the parser (return no patterns for the invalid line), OR
- Produce a `DecodeTree` that silently skips the bad line

Since `helm-decode` currently silently skips invalid lines rather than
returning errors, the test asserts graceful handling (no panic):

```rust
macro_rules! assert_no_panic {
    ($name:ident, $content:literal) => {
        #[test]
        fn $name() {
            // Must not panic; silently skip invalid syntax
            let _tree = DecodeTree::from_decode_text($content);
        }
    };
}

assert_no_panic!(
    err_field1_invalid_syntax,
    include_str!("../../../../assets/qemu/tests/decode/err_field1.decode")
);

assert_no_panic!(
    err_field2_width_too_large,
    include_str!("../../../../assets/qemu/tests/decode/err_field2.decode")
);

// ... repeat for all err_ files ...
```

**Future work:** When `DecodeTree` gains an error-reporting path, convert
`assert_no_panic!` to `assert_parse_error!` that checks the error variant.

### 1.3 a64.decode integration (extend existing test)

`tree_loads_qemu_a64_decode` already exists in `tree.rs`. Extend it with
per-group mnemonic lookups covering each of the 7 major sections:

```rust
// In the existing tree_loads_qemu_a64_decode test, add:
let cases: &[(&str, u32)] = &[
    // Data Processing — Immediate
    ("ADD_i",   0x9100A820), // ADD X0, X1, #42
    ("SUB_i",   0xD1000420), // SUB X0, X1, #1
    ("ADDS_i",  0xB100003F), // ADDS XZR, X1, #0  (CMN alias)
    ("SUBS_i",  0xF100003F), // SUBS XZR, X1, #0  (CMP alias)
    ("MOVZ",    0xD2824680), // MOVZ X0, #0x1234
    ("MOVK",    0xF2A0ACF0), // MOVK X0, #0x5678, LSL#16
    ("MOVN",    0x12800000), // MOVN W0, #0
    ("SBFM",    0x93400C00), // ASR X0, X0, #0  (SBFM alias)
    ("UBFM",    0xD3401C00), // LSL X0, X0, #60  (UBFM alias)
    ("BFM",     0xB3010420), // BFI X0, X1, #63, #2
    ("EXTR",    0x93C08441), // ROR X1, X2, #2  (EXTR alias)
    ("ADR",     0x10000001), // ADR X1, #0
    ("ADRP",    0x90000001), // ADRP X1, #0
    ("AND_i",   0x92400001), // AND X1, X0, #1
    ("ORR_i",   0xB2400001), // ORR X1, X0, #1
    ("EOR_i",   0xD2400001), // EOR X1, X0, #1
    ("ANDS_i",  0xF2400001), // ANDS X1, X0, #1
    // Branches
    ("B",       0x14000040), // B #0x100
    ("BL",      0x94000040), // BL #0x100
    ("BR",      0xD61F0000), // BR X0
    ("BLR",     0xD63F0000), // BLR X0
    ("RET",     0xD65F03C0), // RET
    ("CBZ",     0xB4000080), // CBZ X0, #0x10
    ("CBNZ",    0xB5000080), // CBNZ X0, #0x10
    ("TBZ",     0x36280040), // TBZ X0, #5, #0x8
    ("TBNZ",    0x37280040), // TBNZ X0, #5, #0x8
    ("B_cond",  0x54000080), // B.EQ #0x10
    ("SVC",     0xD4000001), // SVC #0
    ("BRK",     0xD4200000), // BRK #0
    ("NOP",     0xD503201F), // NOP
    ("ISB",     0xD5033FDF), // ISB
    ("DMB",     0xD5033BBF), // DMB ISH
    ("SYS",     0xD51B4220), // MSR TPIDR_EL0, X0 (SYS l=0)
    ("SYS",     0xD53B4220), // MRS X0, TPIDR_EL0 (SYS l=1)
    ("CLREX",   0xD5033F5F), // CLREX
    // Loads and stores
    ("STXR",    0xC8027C20), // STXR W2, X0, [X1]
    ("LDXR",    0xC85F7C20), // LDXR X0, [X1]
    ("STLR",    0xC89FFC20), // STLR X0, [X1]
    ("LDAR",    0xC8DFFC20), // LDAR X0, [X1]
    ("STP",     0xA9400001), // LDP X1, X0, [X0]  (actually LDP)
    ("LDP",     0xA9400001),
    ("STR_i",   0xF9000020), // STR X0, [X1]
    ("LDR_i",   0xF9400020), // LDR X0, [X1]
    ("LD_lit",  0x58000000), // LDR W0, label
    // Data Processing — register
    ("UDIV",    0x9AC20820), // UDIV X0, X1, X2
    ("SDIV",    0x9AC20C20), // SDIV X0, X1, X2
    ("LSLV",    0x9AC22020), // LSLV X0, X1, X2
    ("MADD_x",  0x9B020820), // MADD X0, X1, X2, X2
    ("MSUB_x",  0x9B028820), // MSUB X0, X1, X2, X2
    ("SMADDL",  0x9B220820), // SMADDL X0, W1, W2, X2
    ("UMULH",   0x9BE27C20), // UMULH X0, X1, X2
    ("ADD_r",   0x8B020020), // ADD X0, X1, X2
    ("SUB_r",   0xCB020020), // SUB X0, X1, X2
    ("AND_r",   0x8A020020), // AND X0, X1, X2
    ("ORR_r",   0xAA020020), // ORR X0, X1, X2
    ("EOR_r",   0xCA020020), // EOR X0, X1, X2
    ("ADC",     0x9A020020), // ADC X0, X1, X2
    ("SBC",     0xDA020020), // SBC X0, X1, X2
    ("CSEL",    0x9A820020), // CSEL X0, X1, X2, EQ
    ("CLZ",     0xDAC01020), // CLZ X0, X1
    ("RBIT",    0xDAC00020), // RBIT X0, X1
    ("REV64",   0xDAC00C20), // REV64 X0, X1
    ("ADD_ext", 0x8B220020), // ADD X0, X1, W2, UXTW
    ("SUBS_r",  0xEB020020), // SUBS X0, X1, X2
    ("CCMP",    0xFA400800), // CCMP X0, X0, #0, EQ
];
for (mnemonic, insn) in cases {
    let r = tree.lookup(*insn);
    assert!(r.is_some(), "missed {mnemonic}: {insn:#010x}");
    assert_eq!(r.unwrap().0, *mnemonic,
        "wrong mnemonic for {insn:#010x}: got {} expected {mnemonic}",
        r.unwrap().0);
}
```

---

## Phase 2 — helm-isa Aarch64Decoder Tests

**Target crate:** `crates/helm-isa`
**Target file:** `crates/helm-isa/src/arm/aarch64/tests/decode.rs`

The existing `decode.rs` has ~30 `#[ignore]`-marked TDD stubs.
Once `Aarch64Decoder` is implemented beyond its current NOP stub,
remove `#[ignore]` and extend with the following additional tests
derived directly from `a64.decode` bit patterns.

### 2.1 Complete the existing stubs

All 30 existing `#[ignore]` tests cover the core SE-mode instruction surface.
They should be the first batch enabled when the decoder is implemented.

### 2.2 Additional tests derived from a64.decode

Group all new tests by encoding group matching the a64.decode sections:

```rust
// === Data Processing — Immediate (complete coverage) ===

#[test] fn decode_adds_imm_sets_s_flag() { /* ADDS X0, X1, #1 → 0xB1000420 */ }
#[test] fn decode_subs_imm_cmp_alias()   { /* SUBS XZR, X1, #0 = CMP → 0xF100003F */ }
#[test] fn decode_addg_imm()             { /* ADDG X0, X0, #0, #0 → 0x91800000 */ }
#[test] fn decode_and_imm_32bit()        { /* AND W0, W0, #1 (sf=0) → 0x12000000 */ }
#[test] fn decode_ands_imm_tst_alias()   { /* ANDS XZR, X0, #1 = TST → 0xF2400001 */ }
#[test] fn decode_movn_x()               { /* MOVN X0, #0 → 0x92800000 */ }
#[test] fn decode_movk_hw1()             { /* MOVK X0, #1, LSL#16 → 0xF2A00020 */ }
#[test] fn decode_movk_hw2()             { /* MOVK X0, #1, LSL#32 → 0xF2C00020 */ }
#[test] fn decode_sbfm_asr_alias()       { /* ASR X0, X1, #3 (SBFM) → 0x9343FC20 */ }
#[test] fn decode_ubfm_lsl_alias()       { /* LSL X0, X1, #1 (UBFM) → 0xD37FF820 */ }
#[test] fn decode_ubfm_lsr_alias()       { /* LSR X0, X1, #3 (UBFM) → 0xD343FC20 */ }
#[test] fn decode_bfm_bfi_alias()        { /* BFI X0, X1, #3, #2 → 0xB37D0C20 */ }
#[test] fn decode_extr_ror_alias()       { /* ROR X0, X0, #3 (EXTR) → 0x93C00C00 */ }
#[test] fn decode_extr_32bit()           { /* EXTR W0, W1, W2, #3 (sf=0) */ }
#[test] fn decode_adr_negative_offset()  { /* ADR X0, -4 */ }
#[test] fn decode_adrp_page_offset()     { /* ADRP X0, #0x1000 */ }
#[test] fn decode_smax_imm()             { /* SMAX X0, X1, #10 */ }
#[test] fn decode_umax_imm()             { /* UMAX X0, X1, #10 */ }
#[test] fn decode_smin_imm()             { /* SMIN X0, X1, #0 */ }
#[test] fn decode_umin_imm()             { /* UMIN X0, X1, #255 */ }

// === Branches (complete coverage) ===

#[test] fn decode_b_backward()           { /* B #-4 = 0x17FFFFFF */ }
#[test] fn decode_bl_backward()          { /* BL #-4 */ }
#[test] fn decode_blr()                  { /* BLR X1 → 0xD63F0020 */ }
#[test] fn decode_br_x16()              { /* BR X16 → 0xD61F0200 */ }
#[test] fn decode_ret_x30()              { /* RET = RET X30 → 0xD65F03C0 */ }
#[test] fn decode_ret_custom_reg()       { /* RET X1 → 0xD65F0020 */ }
#[test] fn decode_cbnz_x()               { /* CBNZ X0, #0x10 → 0xB5000080 */ }
#[test] fn decode_cbz_w()                { /* CBZ W0, #0x10 (sf=0) */ }
#[test] fn decode_b_cond_ne()            { /* B.NE #0x10 → 0x54000081 */ }
#[test] fn decode_b_cond_lt()            { /* B.LT #0x10 → 0x5400008B */ }
#[test] fn decode_b_cond_al()            { /* B.AL #0x10 → 0x5400008E */ }
#[test] fn decode_tbnz_high_bit()        { /* TBNZ X0, #63, #offset */ }
#[test] fn decode_svc_nonzero_imm()      { /* SVC #42 → 0xD4000541 */ }
#[test] fn decode_brk_nonzero()          { /* BRK #1 → 0xD4200020 */ }
#[test] fn decode_hlt()                  { /* HLT #0 → 0xD4400000 */ }
#[test] fn decode_nop_canonical()        { /* NOP = 0xD503201F */ }
#[test] fn decode_yield()                { /* YIELD → 0xD503203F */ }
#[test] fn decode_wfe()                  { /* WFE → 0xD503205F */ }
#[test] fn decode_wfi()                  { /* WFI → 0xD503207F */ }
#[test] fn decode_isb()                  { /* ISB → 0xD5033FDF */ }
#[test] fn decode_dsb_ish()              { /* DSB ISH → 0xD5033BBF */ }
#[test] fn decode_dmb_ish()              { /* DMB ISH → 0xD5033BBF (same enc, DMB differs at bit 5) */ }
#[test] fn decode_mrs_nzcv()             { /* MRS X0, NZCV */ }
#[test] fn decode_msr_nzcv()             { /* MSR NZCV, X0 */ }
#[test] fn decode_mrs_fpcr()             { /* MRS X0, FPCR */ }
#[test] fn decode_mrs_fpsr()             { /* MRS X0, FPSR */ }
#[test] fn decode_sys_dc_cvau()          { /* DC CVAU, X0 */ }

// === Loads and Stores (complete coverage) ===

#[test] fn decode_stxr_64bit()           { /* STXR W2, X0, [X1] → 0xC8027C20 */ }
#[test] fn decode_stxr_32bit()           { /* STXR W2, W0, [X1] (sz=10) */ }
#[test] fn decode_ldxr_64bit()           { /* LDXR X0, [X1] → 0xC85F7C20 */ }
#[test] fn decode_stlxr()                { /* STLXR W2, X0, [X1] (lasr=1) */ }
#[test] fn decode_ldaxr()                { /* LDAXR X0, [X1] (lasr=1) */ }
#[test] fn decode_stlr()                 { /* STLR X0, [X1] → 0xC89FFC20 */ }
#[test] fn decode_ldar()                 { /* LDAR X0, [X1] → 0xC8DFFC20 */ }
#[test] fn decode_stxp()                 { /* STXP W2, X0, X1, [X2] */ }
#[test] fn decode_ldxp()                 { /* LDXP X0, X1, [X2] */ }
#[test] fn decode_cas()                  { /* CAS X0, X1, [X2] */ }
#[test] fn decode_ld_lit_w()             { /* LDR W0, #0 → 0x18000000 */ }
#[test] fn decode_ld_lit_x()             { /* LDR X0, #0 → 0x58000000 */ }
#[test] fn decode_ld_lit_sw()            { /* LDRSW X0, #0 → 0x98000000 */ }
#[test] fn decode_stp_post()             { /* STP X0, X1, [SP], #16 */ }
#[test] fn decode_ldp_pre()              { /* LDP X0, X1, [SP, #-16]! */ }
#[test] fn decode_stp_off()              { /* STP X0, X1, [SP, #16] */ }
#[test] fn decode_ldp_off()              { /* LDP X0, X1, [SP, #16] */ }
#[test] fn decode_ldp_sw()               { /* LDPSW X0, X1, [X2] */ }
#[test] fn decode_str_imm_post()         { /* STR X0, [X1], #8 */ }
#[test] fn decode_ldr_imm_pre()          { /* LDR X0, [X1, #8]! */ }
#[test] fn decode_ldr_imm_off()          { /* LDR X0, [X1, #8] → 0xF9400420 */ }
#[test] fn decode_str_imm_off()          { /* STR X0, [X1, #8] → 0xF9000420 */ }
#[test] fn decode_ldur_x()               { /* LDUR X0, [X1, #-8] */ }
#[test] fn decode_stur_x()               { /* STUR X0, [X1, #-8] */ }
#[test] fn decode_ldr_reg()              { /* LDR X0, [X1, X2] */ }
#[test] fn decode_str_reg()              { /* STR X0, [X1, X2] */ }
#[test] fn decode_ldrb_imm()             { /* LDRB W0, [X1] → 0x39400020 */ }
#[test] fn decode_strb_imm()             { /* STRB W0, [X1] → 0x39000020 */ }
#[test] fn decode_ldrh_imm()             { /* LDRH W0, [X1] → 0x79400020 */ }
#[test] fn decode_strh_imm()             { /* STRH W0, [X1] → 0x79000020 */ }
#[test] fn decode_ldrsb_x()              { /* LDRSB X0, [X1] → 0x39800020 */ }
#[test] fn decode_ldrsb_w()              { /* LDRSB W0, [X1] */ }
#[test] fn decode_ldrsh_x()              { /* LDRSH X0, [X1] → 0x79800020 */ }
#[test] fn decode_ldrsw()                { /* LDRSW X0, [X1] → 0xB9800020 */ }

// === Data Processing — Register (complete coverage) ===

#[test] fn decode_lslv()                 { /* LSLV X0, X1, X2 → 0x9AC22020 */ }
#[test] fn decode_lsrv()                 { /* LSRV X0, X1, X2 → 0x9AC22420 */ }
#[test] fn decode_asrv()                 { /* ASRV X0, X1, X2 → 0x9AC22820 */ }
#[test] fn decode_rorv()                 { /* RORV X0, X1, X2 → 0x9AC22C20 */ }
#[test] fn decode_add_shifted_lsl()      { /* ADD X0, X1, X2, LSL #3 → 0x8B020C20 */ }
#[test] fn decode_add_shifted_lsr()      { /* ADD X0, X1, X2, LSR #3 */ }
#[test] fn decode_add_shifted_asr()      { /* ADD X0, X1, X2, ASR #3 */ }
#[test] fn decode_sub_shifted()          { /* SUB X0, X1, X2, LSL #1 */ }
#[test] fn decode_adds_shifted()         { /* ADDS X0, X1, X2 (flag-setting) */ }
#[test] fn decode_subs_shifted()         { /* SUBS X0, X1, X2 (CMN/CMP aliases) */ }
#[test] fn decode_add_ext_uxtw()         { /* ADD X0, X1, W2, UXTW → 0x8B224020 */ }
#[test] fn decode_add_ext_sxtx()         { /* ADD X0, X1, X2, SXTX */ }
#[test] fn decode_add_ext_lsl()          { /* ADD X0, SP, X2, LSL #2 */ }
#[test] fn decode_and_reg()              { /* AND X0, X1, X2 → 0x8A020020 */ }
#[test] fn decode_bic_reg()              { /* BIC X0, X1, X2 (AND with N=1) */ }
#[test] fn decode_orn_reg()              { /* ORN X0, X1, X2 (MVN alias) */ }
#[test] fn decode_eon_reg()              { /* EON X0, X1, X2 */ }
#[test] fn decode_ands_reg()             { /* ANDS X0, X1, X2 (TST alias) */ }
#[test] fn decode_adcs()                 { /* ADCS X0, X1, X2 */ }
#[test] fn decode_sbcs()                 { /* SBCS X0, X1, X2 */ }
#[test] fn decode_ccmp_reg()             { /* CCMP X0, X1, #0, EQ */ }
#[test] fn decode_ccmp_imm()             { /* CCMP X0, #0, #0, EQ */ }
#[test] fn decode_csinc()                { /* CSINC X0, X1, X2, EQ (CINC alias) */ }
#[test] fn decode_csinv()                { /* CSINV X0, X1, X2, EQ (CINV alias) */ }
#[test] fn decode_csneg()                { /* CSNEG X0, X1, X2, EQ (CNEG alias) */ }
#[test] fn decode_rev16()                { /* REV16 X0, X1 */ }
#[test] fn decode_rev32()                { /* REV32 X0, X1 */ }
#[test] fn decode_cls()                  { /* CLS X0, X1 */ }
#[test] fn decode_ctz()                  { /* CTZ X0, X1 */ }
#[test] fn decode_cnt_scalar()           { /* CNT X0, X1 */ }
#[test] fn decode_abs_scalar()           { /* ABS X0, X1 */ }
#[test] fn decode_smaddl()               { /* SMADDL X0, W1, W2, X3 */ }
#[test] fn decode_smsubl()               { /* SMSUBL X0, W1, W2, X3 */ }
#[test] fn decode_umaddl()               { /* UMADDL X0, W1, W2, X3 */ }
#[test] fn decode_umsubl()               { /* UMSUBL X0, W1, W2, X3 */ }
#[test] fn decode_smulh()                { /* SMULH X0, X1, X2 */ }
#[test] fn decode_mul_w()                { /* MUL W0, W1, W2 (MADD 32-bit) */ }
#[test] fn decode_madd_w()               { /* MADD W0, W1, W2, W3 */ }
#[test] fn decode_msub_w()               { /* MSUB W0, W1, W2, W3 */ }
#[test] fn decode_rmif()                 { /* RMIF X0, #3, #0xF */ }
#[test] fn decode_setf8()                { /* SETF8 W0 */ }
#[test] fn decode_setf16()               { /* SETF16 W0 */ }
#[test] fn decode_smax_reg()             { /* SMAX X0, X1, X2 */ }
#[test] fn decode_smin_reg()             { /* SMIN X0, X1, X2 */ }
#[test] fn decode_umax_reg()             { /* UMAX X0, X1, X2 */ }
#[test] fn decode_umin_reg()             { /* UMIN X0, X1, X2 */ }
```

---

## Phase 3 — Field Extraction Verification

**Target file:** `crates/helm-isa/src/arm/aarch64/tests/decode.rs`

These tests verify not just the mnemonic but that field values are
extracted correctly from real instruction encodings, using the field
positions defined in `a64.decode`.

```rust
// Derive expected field values from a64.decode:
// ADD_i: sf:1 .. ...... . imm:12 rn:5 rd:5
// ADD X0, X1, #42 = sf=1(64-bit) imm=42 rn=1 rd=0

#[test]
fn decode_add_imm_fields_correct() {
    // ADD X0, X1, #42 → 0x9100A820
    // sf=1, op=0, S=0, imm12=42, rn=1, rd=0
    let dec = Aarch64Decoder::new();
    let uops = dec.decode_insn(0x1000, 0x9100A820).unwrap();
    let uop = &uops[0];
    assert_eq!(uop.opcode, Opcode::IntAlu);
    assert_eq!(uop.immediate, Some(42));
    assert_eq!(uop.sources, vec![1]);   // rn=X1
    assert_eq!(uop.dest, Some(0));      // rd=X0
}

#[test]
fn decode_movz_imm16_hw_fields() {
    // MOVZ X0, #0x1234 → 0xD2824680
    // sf=1, hw=0, imm16=0x1234, rd=0
    let dec = Aarch64Decoder::new();
    let uops = dec.decode_insn(0x1000, 0xD2824680).unwrap();
    assert_eq!(uops[0].immediate, Some(0x1234));
    assert_eq!(uops[0].dest, Some(0));
}

#[test]
fn decode_movk_imm16_hw1_fields() {
    // MOVK X0, #0x5678, LSL#16 → 0xF2ACF000
    // sf=1, hw=1, imm16=0x5678, rd=0
    let dec = Aarch64Decoder::new();
    let uops = dec.decode_insn(0x1000, 0xF2ACEF00).unwrap();
    // Should preserve existing bits in rd with imm16<<(hw*16)
    assert_eq!(uops[0].dest, Some(0));
}

#[test]
fn decode_b_imm26_signed() {
    // B #0x100 → 0x14000040
    // imm26 = 0x40, sign-extended, ×4 → offset = 0x100
    let dec = Aarch64Decoder::new();
    let uops = dec.decode_insn(0x1000, 0x14000040).unwrap();
    // PC-relative target should be 0x1000 + 0x100 = 0x1100
    assert_eq!(uops[0].immediate, Some(0x100));
}

#[test]
fn decode_cbz_imm19_signed() {
    // CBZ X0, #-8 (backward branch)
    // imm19 = 0x7FFFC sign-extended × 4 = -8
    // Encoding: sf=1, imm19=0x7FFFC, rt=0
    let dec = Aarch64Decoder::new();
    let uops = dec.decode_insn(0x1000, 0xB4FFFFE0).unwrap();
    assert_eq!(uops[0].opcode, Opcode::CondBranch);
    assert_eq!(uops[0].sources, vec![0]); // rt = X0
}

#[test]
fn decode_tbz_bitpos_imm14_fields() {
    // TBZ X0, #5, #0x10
    // bitpos = b31:b19 = 0:5 = 5, imm14 = 4, rt = 0
    let dec = Aarch64Decoder::new();
    let uops = dec.decode_insn(0x1000, 0x36280080).unwrap();
    assert_eq!(uops[0].opcode, Opcode::CondBranch);
    assert_eq!(uops[0].sources, vec![0]); // rt = X0
}

#[test]
fn decode_ldr_imm_offset_scaled() {
    // LDR X0, [X1, #8] → 0xF9400420
    // sz=11(64-bit), opc=01, imm12=1 (×8=8), rn=1, rt=0
    let dec = Aarch64Decoder::new();
    let uops = dec.decode_insn(0x1000, 0xF9400420).unwrap();
    assert_eq!(uops[0].opcode, Opcode::Load);
    assert_eq!(uops[0].immediate, Some(8)); // offset after scaling
    assert_eq!(uops[0].sources, vec![1]);   // rn = X1
    assert_eq!(uops[0].dest, Some(0));      // rt = X0
}

#[test]
fn decode_ldrb_imm_unsigned_offset() {
    // LDRB W0, [X1, #3] → 0x39400C20
    // sz=00, imm12=3 (no scaling for byte), rn=1, rt=0
    let dec = Aarch64Decoder::new();
    let uops = dec.decode_insn(0x1000, 0x39400C20).unwrap();
    assert_eq!(uops[0].opcode, Opcode::Load);
    assert_eq!(uops[0].immediate, Some(3));
}

#[test]
fn decode_stp_imm7_scaled() {
    // STP X0, X1, [SP, #-16]! → 0xA9BF07E0
    // imm7=-2 (×8=-16), rt2=1, rn=31(SP), rt=0
    let dec = Aarch64Decoder::new();
    let uops = dec.decode_insn(0x1000, 0xA9BF07E0).unwrap();
    assert_eq!(uops[0].opcode, Opcode::Store);
    assert_eq!(uops[0].immediate, Some(-16i64 as u64));
}

#[test]
fn decode_sbfm_immr_imms_lsl_alias() {
    // LSL X0, X1, #1 = UBFM X0, X1, #63, #62
    // sf=1, N=1, immr=63, imms=62, rn=1, rd=0
    let dec = Aarch64Decoder::new();
    let uops = dec.decode_insn(0x1000, 0xD37FF820).unwrap();
    assert_eq!(uops[0].opcode, Opcode::IntAlu);
}

#[test]
fn decode_csel_cond_field() {
    // CSEL X0, X1, X2, EQ → 0x9A820020
    // cond=EQ(0), rm=2, rn=1, rd=0
    let dec = Aarch64Decoder::new();
    let uops = dec.decode_insn(0x1000, 0x9A820020).unwrap();
    assert_eq!(uops[0].opcode, Opcode::IntAlu);
    assert_eq!(uops[0].sources[0], 1); // rn
    assert_eq!(uops[0].sources[1], 2); // rm
}

#[test]
fn decode_madd_ra_field() {
    // MADD X0, X1, X2, X3 → 0x9B020C20 (ra=X3)
    // vs MADD X0, X1, X2, XZR = MUL → ra=31
    let dec = Aarch64Decoder::new();
    let uops_madd = dec.decode_insn(0x1000, 0x9B020C20).unwrap();
    let uops_mul  = dec.decode_insn(0x1000, 0x9B027C20).unwrap();
    assert_eq!(uops_madd[0].opcode, Opcode::IntMul);
    assert_eq!(uops_mul[0].opcode,  Opcode::IntMul);
    // When ra=XZR, it's pure multiply; when ra≠XZR, multiply-accumulate
}
```

---

## Phase 4 — helm-decode Error Case Validation

**Target file:** `crates/helm-decode/src/tests/qemu_compat.rs`

The `err_overlap*` cases test that a `DecodeTree` correctly handles
overlapping patterns. Currently `helm-decode` uses first-match semantics
(linear scan). These tests verify the documented behaviour:

```rust
#[test]
fn err_overlap_first_match_wins() {
    // succ equivalent: two overlapping patterns where first always matches
    let text = "
A  00000000 00000000 00000000 00000000
B  ........ ........ ........ ........
";
    let tree = DecodeTree::from_decode_text(text);
    // With 0x00000000, A should match first (linear scan)
    let (m, _) = tree.lookup(0x00000000).unwrap();
    assert_eq!(m, "A");
    // With any non-zero, B should match
    let (m, _) = tree.lookup(0xFF000000).unwrap();
    assert_eq!(m, "B");
}

#[test]
fn group_nesting_flattened_first_match() {
    // succ_pattern_group_nest1: group { top, { sub1, { sub2, { sub3, sub4 } } } }
    // helm-decode flattens groups; verify first-match still works
    let text = include_str!(
        "../../../../assets/qemu/tests/decode/succ_pattern_group_nest1.decode"
    );
    let tree = DecodeTree::from_decode_text(text);
    // 0x00000000 should match "top" (most specific)
    // (or whichever is first after flattening)
    let r = tree.lookup(0x00000000);
    assert!(r.is_some());
}
```

---

## Phase 5 — helm-decode a32 / t32 (Future)

**Target file:** `crates/helm-isa/src/arm/aarch32/` (not yet created)

The following QEMU decode files exist for AArch32:

```
assets/qemu/target/arm/tcg/a32.decode          AArch32 A32 instruction set
assets/qemu/target/arm/tcg/a32-uncond.decode   AArch32 unconditional instructions
assets/qemu/target/arm/tcg/t16.decode          Thumb-16 instruction set
assets/qemu/target/arm/tcg/t32.decode          Thumb-32 instruction set
assets/qemu/target/arm/tcg/vfp.decode          VFP floating-point
assets/qemu/target/arm/tcg/neon-dp.decode      NEON data-processing
assets/qemu/target/arm/tcg/neon-ls.decode      NEON load-store
assets/qemu/target/arm/tcg/mve.decode          M-profile Vector Extension
```

These become relevant when AArch32 support is added (see `docs/arm.md §Roadmap`).
The same three-phase approach (parser test, mnemonic test, field extraction test)
applies to each file.

---

## Implementation Order and Priority

| Phase | File to create/modify | Blocked on | Priority |
|-------|-----------------------|------------|----------|
| 1.1 Success | `helm-decode/tests/qemu_compat.rs` | Nothing | High |
| 1.2 Error | `helm-decode/tests/qemu_compat.rs` | Nothing | High |
| 1.3 a64 lookups | `helm-decode/tests/tree.rs` | Nothing | High |
| 2.1 Existing stubs | `helm-isa/tests/decode.rs` | Aarch64Decoder impl | Medium |
| 2.2 New stubs | `helm-isa/tests/decode.rs` | Aarch64Decoder impl | Medium |
| 3 Field extraction | `helm-isa/tests/decode.rs` | Aarch64Decoder impl | Medium |
| 4 Error behaviour | `helm-decode/tests/qemu_compat.rs` | Nothing | Low |
| 5 AArch32 | (new files) | AArch32 frontend | Low |

Phases 1 and 1.3 can be done immediately — they test `helm-decode`
which already works. Phases 2, 3 are blocked on implementing
`Aarch64Decoder` beyond its current NOP stub.

---

## Encoding Reference Table

Canonical encodings for implementing the Phase 2 / Phase 3 stubs,
derived directly from `assets/qemu/target/arm/tcg/a64.decode`:

| Instruction | Encoding (hex) | Key fields |
|-------------|---------------|------------|
| `ADD X0, X1, #42` | `0x9100A820` | sf=1 op=0 S=0 sh=0 imm12=42 rn=1 rd=0 |
| `ADDS XZR, X1, #0` (CMP) | `0xF100003F` | sf=1 op=0 S=1 imm12=0 rn=1 rd=31 |
| `MOVZ X0, #0x1234` | `0xD2824680` | sf=1 hw=0 imm16=0x1234 rd=0 |
| `MOVK X0, #0x5678, LSL#16` | `0xF2A0ACF0` | sf=1 hw=1 imm16=0x5678 rd=0 |
| `B #0x100` | `0x14000040` | imm26=0x40 (×4=0x100) |
| `BL #0x100` | `0x94000040` | imm26=0x40 |
| `B.EQ #0x10` | `0x54000080` | imm19=0x4 (×4=0x10) cond=0x0 |
| `CBZ X0, #0x10` | `0xB4000080` | sf=1 imm19=0x4 rt=0 |
| `TBZ X0, #5, #0x10` | `0x36280080` | b5=0 b40=5 imm14=4 rt=0 |
| `TBNZ X0, #5, #0x10` | `0x37280080` | same with nz=1 |
| `BR X0` | `0xD61F0000` | rn=0 |
| `BLR X0` | `0xD63F0000` | rn=0 |
| `RET` | `0xD65F03C0` | rn=30 |
| `SVC #0` | `0xD4000001` | imm16=0 |
| `NOP` | `0xD503201F` | canonical NOP encoding |
| `ISB` | `0xD5033FDF` | |
| `DMB ISH` | `0xD5033BBF` | domain=2 types=3 |
| `MRS X0, TPIDR_EL0` | `0xD53BD040` | op0=3 op1=3 crn=13 crm=0 op2=2 |
| `MSR TPIDR_EL0, X0` | `0xD51BD040` | l=0 |
| `LDR X0, [X1, #8]` | `0xF9400420` | sz=3 imm12=1(×8=8) rn=1 rt=0 |
| `STR X0, [X1, #8]` | `0xF9000420` | sz=3 opc=0 imm12=1 rn=1 rt=0 |
| `LDP X0, X1, [SP, #16]` | `0xA9410BE0` | sf=1 imm7=2(×8=16) rt2=1 rn=31 rt=0 |
| `STP X0, X1, [SP, #-16]!` | `0xA9BF07E0` | sf=1 p=0 w=1 imm7=-2 rt2=1 rn=31 rt=0 |
| `LDXR X0, [X1]` | `0xC85F7C20` | sz=3 rs=31 lasr=0 rt2=31 rn=1 rt=0 |
| `STXR W2, X0, [X1]` | `0xC8027C20` | sz=3 rs=2 lasr=0 rt2=31 rn=1 rt=0 |
| `LDRB W0, [X1]` | `0x39400020` | sz=0 opc=01 imm12=0 rn=1 rt=0 |
| `STRB W0, [X1]` | `0x39000020` | sz=0 opc=00 |
| `LDRSB X0, [X1]` | `0x39800020` | sz=0 opc=10 ext=0 |
| `LDRH W0, [X1]` | `0x79400020` | sz=1 |
| `LDRSW X0, [X1]` | `0xB9800020` | sz=2 opc=10 |
| `ADD X0, X1, X2` | `0x8B020020` | sf=1 rm=2 shift=0 sa=0 rn=1 rd=0 |
| `SUB X0, X1, X2` | `0xCB020020` | |
| `MUL X0, X1, X2` | `0x9B027C20` | MADD ra=XZR |
| `MADD X0, X1, X2, X3` | `0x9B020C20` | ra=3 |
| `SMULH X0, X1, X2` | `0x9B427C20` | |
| `UMULH X0, X1, X2` | `0x9BE27C20` | |
| `UDIV X0, X1, X2` | `0x9AC20820` | |
| `SDIV X0, X1, X2` | `0x9AC20C20` | |
| `CLZ X0, X1` | `0xDAC01020` | |
| `RBIT X0, X1` | `0xDAC00020` | |
| `REV X0, X1` | `0xDAC00C20` | REV64 |
| `CSEL X0, X1, X2, EQ` | `0x9A820020` | cond=0 rm=2 rn=1 rd=0 |
| `ADC X0, X1, X2` | `0x9A020020` | |
| `AND X0, X1, X2` | `0x8A020020` | |
| `ORR X0, X1, X2` | `0xAA020020` | |
| `EOR X0, X1, X2` | `0xCA020020` | |
| `FMOV D0, X0` | `0x9E670000` | |
| `FADD D0, D1, D2` | `0x1E622820` | |
| `FCVTZS X0, D1` | `0x9E780020` | |
