# AArch64 Instruction Coverage

674 unique mnemonics in QEMU's `a64.decode`. This document tracks
decode and execution coverage across `helm-isa` and `helm-decode`.

**Legend:** helm = in HELM `.decode` file, qemu = in `qemu/a64.decode` only,
exec = hand-implemented in `exec.rs`

---

## Data Processing — Immediate (17 helm / 19 qemu)

| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| ADR | helm | yes | implemented |
| ADRP | helm | yes | implemented |
| ADD_imm | helm | yes | implemented |
| ADDS_imm | helm | yes | implemented |
| SUB_imm | helm | yes | implemented |
| SUBS_imm | helm | yes | implemented |
| AND_imm | helm | yes | implemented |
| ORR_imm | helm | yes | implemented |
| EOR_imm | helm | yes | implemented |
| ANDS_imm | helm | yes | implemented |
| MOVN | helm | yes | implemented |
| MOVZ | helm | yes | implemented |
| MOVK | helm | yes | implemented |
| SBFM | helm | yes | implemented |
| BFM | helm | yes | implemented |
| UBFM | helm | yes | implemented |
| EXTR | helm | yes | implemented |
| ADDG_i | qemu | no | missing |
| SUBG_i | qemu | no | missing |

## Branches, Exception, System (14 helm / 40+ qemu)

| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| B | helm | yes | implemented |
| BL | helm | yes | implemented |
| B_cond | helm | yes | implemented |
| CBZ | helm | yes | implemented |
| CBNZ | helm | yes | implemented |
| TBZ | helm | yes | implemented |
| TBNZ | helm | yes | implemented |
| BR | helm | yes | implemented |
| BLR | helm | yes | implemented |
| RET | helm | yes | implemented |
| SVC | helm | yes | implemented |
| HVC | helm | no | stub |
| BRK | helm | yes | implemented |
| NOP | helm | yes | implemented |
| BRA/BRAZ | qemu | no | missing (PAuth) |
| BLRA/BLRAZ | qemu | no | missing (PAuth) |
| RETA | qemu | no | missing (PAuth) |
| ERET/ERETA | qemu | no | missing (EL2+) |
| CCMP | qemu | yes | implemented |
| CSEL | qemu | yes | implemented |
| SYS | qemu | no | missing |
| MSR_i_* | qemu | partial | MRS/MSR TPIDR_EL0 only |
| CLREX | qemu | no | missing |
| DSB_DMB | qemu | no | missing |
| ISB | qemu | no | missing |
| HLT | qemu | no | missing |
| SMC | qemu | no | missing (EL3) |
| RMIF | qemu | no | missing |
| SETF8/16 | qemu | no | missing |
| CFINV | qemu | no | missing |
| AXFLAG | qemu | no | missing |
| WFET/WFIT | qemu | no | missing |
| SB | qemu | no | missing |

## Data Processing — Register (33 helm / 65+ qemu)

| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| ADD_reg | helm | yes | implemented |
| ADDS_reg | helm | yes | implemented |
| SUB_reg | helm | yes | implemented |
| SUBS_reg | helm | yes | implemented |
| AND_reg | helm | yes | implemented |
| ORR_reg | helm | yes | implemented |
| EOR_reg | helm | yes | implemented |
| ANDS_reg | helm | yes | implemented |
| BIC | helm | yes | implemented |
| ORN | helm | yes | implemented |
| EON | helm | yes | implemented |
| BICS | helm | yes | implemented |
| MADD | helm | yes | implemented |
| MSUB | helm | yes | implemented |
| SMADDL | helm | yes | implemented |
| UMADDL | helm | yes | implemented |
| SMULH | helm | yes | implemented |
| UMULH | helm | yes | implemented |
| UDIV | helm | yes | implemented |
| SDIV | helm | yes | implemented |
| LSLV | helm | yes | implemented |
| LSRV | helm | yes | implemented |
| ASRV | helm | yes | implemented |
| CSEL | helm | yes | implemented |
| CSINC | helm | yes | implemented |
| CSINV | helm | yes | implemented |
| CSNEG | helm | yes | implemented |
| RBIT | helm | yes | implemented |
| REV16 | helm | yes | implemented |
| REV32 | helm | yes | implemented |
| REV | helm | yes | implemented |
| CLZ | helm | yes | implemented |
| CLS | helm | yes | implemented |
| ADD_ext | qemu | yes | implemented |
| ADDS_ext | qemu | yes | implemented |
| SUB_ext | qemu | no | missing |
| SUBS_ext | qemu | no | missing |
| ADC | qemu | yes | implemented |
| ADCS | qemu | yes | implemented |
| SBC | qemu | yes | implemented |
| SBCS | qemu | yes | implemented |
| RORV | qemu | yes | implemented |
| SMSUBL | qemu | no | missing |
| UMSUBL | qemu | no | missing |
| CRC32 | qemu | no | missing |
| CRC32C | qemu | no | missing |
| CTZ | qemu | no | missing |
| CNT | qemu | no | missing |
| PACGA/PACIA/PACIB/PACDA/PACDB | qemu | no | missing (PAuth) |
| AUTIA/AUTIB/AUTDA/AUTDB | qemu | no | missing (PAuth) |
| XPACI/XPACD | qemu | no | missing (PAuth) |
| IRG/GMI | qemu | no | missing (MTE) |
| SUBP/SUBPS | qemu | no | missing (MTE) |
| SMAX/SMIN/UMAX/UMIN | qemu | no | missing (CSSC) |
| SMAX_i/SMIN_i/UMAX_i/UMIN_i | qemu | no | missing (CSSC) |

## Loads and Stores (25 helm / 70+ qemu)

| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| LDR unsigned | helm | yes | implemented |
| STR unsigned | helm | yes | implemented |
| LDP/STP | helm | yes | implemented |
| LDP/STP pre/post | helm | yes | implemented |
| LDXR/STXR | helm | yes | implemented |
| LDAXR/STLXR | helm | yes | implemented |
| SWP | helm | yes | implemented |
| LDADD | helm | yes | implemented |
| LDR/STR pre/post/unscaled | exec only | yes | implemented |
| LDR/STR register offset | exec only | yes | implemented |
| Load literal | exec only | yes | implemented |
| LDAR/LDAPR | qemu | no | missing |
| STLR | qemu | no | missing |
| CAS/CASP | qemu | no | missing |
| LDCLR/LDEOR/LDSET | qemu | no | missing |
| LDSMAX/LDSMIN/LDUMAX/LDUMIN | qemu | no | missing |
| LD/ST multiple structures | qemu | no | missing |
| LD/ST single structure | qemu | no | missing |
| LDR/STR SIMD | exec only | partial | common variants |
| LDP/STP SIMD | exec only | partial | Q/D/S |
| MTE: LDG/STG/LDGM/STGM/ST2G/STZG/STZGM/STGP | qemu | no | missing |
| CPY*/SET* | qemu | no | missing (MOPS) |
| LDRA (PAuth LDR) | qemu | no | missing |

## SIMD / NEON — Integer (~68 helm / ~250 qemu)

### Three Same (integer)
| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| ADD_v | helm | partial | 16B/8B via exec_simd_dp |
| SUB_v | helm | partial | via exec_simd_dp |
| AND/BIC/ORR/ORN/EOR/BSL/BIT/BIF_v | helm | no | stub (logged) |
| CMGT/CMHI/CMGE/CMHS/CMTST/CMEQ_v | helm | no | stub |
| SMAX/UMAX/SMIN/UMIN_v | helm | no | stub |
| MUL/MLA/MLS_v | helm | no | stub |
| ADDP_v | helm | no | stub |
| SMAXP/UMAXP/SMINP/UMINP_v | helm | no | stub |
| SABD/UABD/SABA/UABA_v | qemu | no | missing |
| SHADD/UHADD/SHSUB/UHSUB_v | qemu | no | missing |
| SRHADD/URHADD_v | qemu | no | missing |
| SQADD/UQADD/SQSUB/UQSUB_v | qemu | no | missing |
| SQSHL/UQSHL/SQRSHL/UQRSHL_v | qemu | no | missing |
| SSHL/USHL/SRSHL/URSHL_v | qemu | no | missing |
| PMUL_v | qemu | no | missing |

### Two-Register Misc
| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| ABS/NEG_v | helm | no | stub |
| NOT/RBIT_v | helm | no | stub |
| REV16/REV32/REV64_v | helm | no | stub |
| CNT_v | helm | no | stub |
| CMGT0/CMEQ0/CMLT0/CMGE0/CMLE0_v | helm | no | stub |
| CLS/CLZ_v | qemu | no | missing |
| SQABS/SQNEG_v | qemu | no | missing |
| SUQADD/USQADD_v | qemu | no | missing |
| SADDLP/UADDLP/SADALP/UADALP_v | qemu | no | missing |
| SHLL_v | qemu | no | missing |
| FABS/FNEG/FSQRT_v | qemu | no | missing |
| FCVTL/FCVTN/FCVTXN/BFCVTN_v | qemu | no | missing |
| FRECPE/FRSQRTE/URECPE/URSQRTE_v | qemu | no | missing |
| FRINT*_v (7 variants) | qemu | no | missing |
| FCMEQ0/FCMGE0/FCMGT0/FCMLE0/FCMLT0_v | qemu | no | missing |

### Across Lanes
| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| ADDV | helm | no | stub |
| SMAXV/UMAXV/SMINV/UMINV | helm | no | stub |
| SADDLV/UADDLV | qemu | no | missing |
| FMAXNMV/FMAXV/FMINNMV/FMINV | qemu | no | missing |

### Shift by Immediate
| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| SHL/SSHR/USHR_v | helm | no | stub |
| SSRA/USRA_v | helm | no | stub |
| SSHLL/USHLL_v | helm | no | stub |
| SHRN_v | helm | no | stub |
| SRI/SLI_v | qemu | no | missing |
| SRSHR/URSHR/SRSRA/URSRA_v | qemu | no | missing |
| RSHRN_v | qemu | no | missing |
| SQSHRN/UQSHRN/SQRSHRN/UQRSHRN_v | qemu | no | missing |
| SQSHRUN/SQRSHRUN_v | qemu | no | missing |
| SQSHL/UQSHL/SQSHLU_vi | qemu | no | missing |

### Copy / Insert / Extract
| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| DUP_general | helm | no | stub |
| INS_general | helm | no | stub |
| UMOV | helm | no | stub |
| INS_element | helm | no | stub |
| DUP_element_s/v | qemu | no | missing |
| SMOV | qemu | no | missing |

### Permute
| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| UZP1/UZP2/TRN1/TRN2/ZIP1/ZIP2 | helm | no | stub |

### Table Lookup
| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| TBL_TBX | helm | no | stub |

### Extract
| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| EXT_d/EXT_q | helm | no | stub |

### Modified Immediate
| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| MOVI | helm | no | stub |
| Vimm | qemu | no | missing |
| FMOVI_v_h | qemu | no | missing |

### Widening / Narrowing (0 helm / 27 qemu)
SADDL/UADDL/SSUBL/USUBL/SADDW/UADDW/SSUBW/USUBW/SABDL/UABDL/SABAL/UABAL/SMLAL/UMLAL/SMLSL/UMLSL/SMULL/UMULL/ADDHN/SUBHN/RADDHN/RSUBHN/PMULL/SQDMULL/SQDMLAL/SQDMLSL/XTN — all missing

### Element / Indexed (0 helm / 30+ qemu)
MUL/MLA/MLS/FMUL/FMLA/FMLS/SQDMULH/SQRDMULH/SMLAL/UMLAL/SMLSL/UMLSL/SMULL/UMULL/SQDMULL/SQDMLAL/SQDMLSL/FMULX/SDOT/UDOT/SUDOT/USDOT/BFDOT/FMLAL/FMLSL_vi — all missing

### Scalar (0 helm / 50+ qemu)
All *_s scalar SIMD variants — ABS_s, NEG_s, SQADD_s, UQADD_s, etc. — all missing

## Scalar Floating-Point (0 helm / 120+ qemu)

| Group | Instructions | Status |
|-------|-------------|--------|
| Arithmetic | FADD/FSUB/FMUL/FDIV/FNMUL_s | exec.rs has partial support |
| Fused MAC | FMADD/FMSUB/FNMADD/FNMSUB | missing in decode, may be in exec |
| Compare | FCMP/FCCMP/FCSEL | missing |
| Move | FMOV_s/FMOVI_s | missing |
| Unary | FABS/FNEG/FSQRT_s | missing |
| Convert | FCVT_s_* (6 variants) | missing |
| Round | FRINT*_s (7 variants) | missing |
| Int↔FP | SCVTF/UCVTF_g/f | missing |
| FP→Int | FCVTAS/AU/MS/MU/NS/NU/PS/PU/ZS/ZU_g/f | missing |
| Reciprocal | FRECPE/FRECPS/FRECPX/FRSQRTE/FRSQRTS_s | missing |
| Min/Max | FMAX/FMIN/FMAXNM/FMINNM_s | missing |

## FP/GP Transfers (8 helm / 12 qemu)

| Instruction | HELM .decode | exec.rs | Status |
|-------------|-------------|---------|--------|
| FMOV_ws | helm | yes | implemented |
| FMOV_sw | helm | yes | implemented |
| FMOV_xd | helm | yes | implemented |
| FMOV_dx | helm | yes | implemented |
| FMOV_xh/hx | qemu | no | missing |
| FMOV_xu/ux | qemu | no | missing |

## Crypto (0 helm / 30 qemu)

AES: AESE/AESD/AESIMC/AESMC
SHA1: SHA1C/SHA1H/SHA1M/SHA1P/SHA1SU0/SHA1SU1
SHA256: SHA256H/SHA256H2/SHA256SU0/SHA256SU1
SHA512: SHA512H/SHA512H2/SHA512SU0/SHA512SU1
SM3: SM3PARTW1/SM3PARTW2/SM3SS1/SM3TT1A/SM3TT1B/SM3TT2A/SM3TT2B
SM4: SM4E/SM4EKEY
Misc: EOR3/BCAX/RAX1/XAR

All missing from both decode and exec.

## Summary

| Category | HELM .decode | exec.rs | QEMU a64.decode |
|----------|-------------|---------|-----------------|
| DP-Immediate | 17 | 17 | 19 |
| Branches/System | 14 | 14 | 40+ |
| DP-Register | 33 | 33 | 65+ |
| Load/Store | 25 | 25+ | 70+ |
| SIMD Integer | 68 | ~5 | 250+ |
| Scalar FP | 4 | partial | 120+ |
| Crypto | 0 | 0 | 30 |
| **Total** | **~161** | **~95** | **674** |
