# Instruction Coverage

AArch64 instruction support in HELM.

## Coverage by Group

| Group | Instructions | Executor | TCG Emitter |
|-------|-------------|----------|-------------|
| **Integer ALU** | ADD/ADDS/SUB/SUBS (imm/reg/ext), AND/ORR/EOR/BIC/ORN, TST, MOV, MVN | ✅ | ✅ |
| **Shift / Bitfield** | LSL/LSR/ASR (imm), UBFM, SBFM, BFM, EXTR | ✅ | ✅ |
| **Multiply** | MUL, MADD, MSUB, SMULL, SMULH, UMULH, MNEG | ✅ | ✅ |
| **Divide** | UDIV, SDIV | ✅ | ✅ |
| **Wide Moves** | MOVZ, MOVN, MOVK (all hw shifts) | ✅ | ✅ |
| **Address** | ADR, ADRP | ✅ | ✅ |
| **Compare / Select** | CMP, CMN, CSEL, CSET, CSINC, CSINV, CSNEG, CCMP, CCMN | ✅ | ✅ |
| **Carry** | ADC, ADCS, SBC, SBCS | ✅ | ✅ |
| **Conditional Branch** | B.cond, CBZ, CBNZ, TBZ, TBNZ | ✅ | ✅ |
| **Unconditional Branch** | B, BL, BR, BLR, RET | ✅ | ✅ |
| **Load/Store** | LDR/STR (imm/reg/pre/post-index/literal), LDP/STP | ✅ | ✅ |
| **Load/Store Ext** | LDRB/LDRH/LDRSB/LDRSH/LDRSW, STRB/STRH | ✅ | ✅ |
| **Load/Store Unscaled** | LDUR/STUR and byte/half/signed variants | ✅ | ✅ |
| **Exclusive** | LDXR, STXR, LDAXR, STLXR (32/64) | ✅ | Partial |
| **SIMD/FP** | FMOV, FADD, FSUB, FMUL, FDIV, FCMP, FCSEL, FCVT, FABS, FNEG, FSQRT | ✅ | Partial |
| **SIMD Integer** | ADD, SUB, MUL, AND, ORR, EOR, shift vectors | ✅ | Partial |
| **System** | SVC, NOP, MRS, MSR, ISB, DSB, DMB, WFI, WFE, YIELD | ✅ | ✅ |
| **Exception** | ERET, HVC, SMC | ✅ | ✅ |
| **Barriers** | DSB, DMB, ISB, SB, CLREX | ✅ | ✅ |
| **PAC** | PACIA, PACIB, AUTIA, AUTIB, XPACLRI | ✅ (NOP) | ✅ (NOP) |

## Decoder Coverage

The decode tree supports ~200 AArch64 instruction encodings across:
- Data processing (immediate)
- Data processing (register)
- Branch / exception / system
- Load / store
- Scalar floating-point
- Advanced SIMD
