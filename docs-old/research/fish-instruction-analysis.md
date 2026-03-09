# fish-shell AArch64 Binary — Instruction Analysis

Binary: `binaries/fish` (14 MB, static, musl-linked)
Total .text instructions: **702,070**

## Encoding Group Summary

| Group | Count | % | Notes |
|-------|-------|---|-------|
| Integer ALU (imm) | 197,720 | 28.2% | ADD, SUB, CMP, MOV, bitfield, ADR |
| Loads & stores | 188,435 | 26.8% | GPR, pair, byte/half, exclusive, LSE atomics |
| Branches | 169,229 | 24.1% | B, BL, B.cond, CBZ, TBZ, BR, BLR, RET |
| Integer ALU (reg) | 130,661 | 18.6% | MOV, ADD, CMP, MUL, DIV, CSEL, shift |
| SIMD/FP | 5,820 | 0.8% | FMOV, FNEG, FABS, FCVT, AdvSIMD |
| System | 1,730 | 0.2% | SVC, MRS, MSR, NOP, barriers |
| Reserved/unknown | 1,020 | 0.1% | |

## Top 40 Instructions by Count

| # | Instruction | Count | % | Category |
|---|-------------|-------|---|----------|
| 1 | MOV Xd, Xn (ORR alias) | 72,259 | 10.3% | ALU reg |
| 2 | STP X (pre-index) | 65,913 | 9.4% | Store pair |
| 3 | ADD Xd, Xn, #imm | 60,534 | 8.6% | ALU imm |
| 4 | BL #offset | 51,387 | 7.3% | Branch |
| 5 | LDR Xd, [Xn, #uimm] | 48,026 | 6.8% | Load |
| 6 | MOVZ Xd, #imm | 44,461 | 6.3% | ALU imm |
| 7 | B.cond #offset | 41,925 | 6.0% | Branch |
| 8 | B #offset | 36,267 | 5.2% | Branch |
| 9 | ADRP Xd, #page | 26,824 | 3.8% | ALU imm |
| 10 | LDP X [Xn, #imm] | 26,090 | 3.7% | Load pair |
| 11 | STP X [Xn, #imm] | 24,445 | 3.5% | Store pair |
| 12 | CBZ Xn, #offset | 22,035 | 3.1% | Branch |
| 13 | STR Xd, [Xn, #uimm] | 21,035 | 3.0% | Store |
| 14 | CMP Xn, #imm (SUBS alias) | 20,353 | 2.9% | ALU imm |
| 15 | CMP Xn, Xm (SUBS alias) | 18,669 | 2.7% | ALU reg |
| 16 | SUB Xd, Xn, #imm | 17,252 | 2.5% | ALU imm |
| 17 | ADD Xd, Xn, Xm | 12,467 | 1.8% | ALU reg |
| 18 | LDR Xd, [Xn, #simm] | 10,121 | 1.4% | Load |
| 19 | LDR Wd, [Xn, #uimm] | 7,514 | 1.1% | Load |
| 20 | UBFM (LSL/LSR/UXTB) | 7,330 | 1.0% | ALU imm |
| 21 | LDRB Wd, [Xn, #uimm] | 7,020 | 1.0% | Load |
| 22 | CBNZ Xn, #offset | 5,991 | 0.9% | Branch |
| 23 | TBNZ Xn, #bit, #off | 5,501 | 0.8% | Branch |
| 24 | STR Xd, [Xn, #simm] | 5,377 | 0.8% | Store |
| 25 | RET | 5,228 | 0.7% | Branch |
| 26 | AND Xd, Xn, #imm | 4,885 | 0.7% | ALU imm |
| 27 | STRB Wd, [Xn, #uimm] | 4,648 | 0.7% | Store |
| 28 | STR Wd, [Xn, #uimm] | 4,272 | 0.6% | Store |
| 29 | TBZ Xn, #bit, #off | 3,990 | 0.6% | Branch |
| 30 | SUB Xd, Xn, Xm | 3,676 | 0.5% | ALU reg |
| 31 | SWP (atomic swap) | 3,645 | 0.5% | Atomic |
| 32 | AdvSIMD (vector ops) | 3,292 | 0.5% | SIMD |
| 33 | ORR Xd, Xn, Xm | 2,983 | 0.4% | ALU reg |
| 34 | MOVN Xd, #imm | 2,611 | 0.4% | ALU imm |
| 35 | LDP X post-index | 2,587 | 0.4% | Load pair |
| 36 | BLR Xn | 2,176 | 0.3% | Branch |
| 37 | MOVK Xd, #imm, LSL | 2,177 | 0.3% | ALU imm |
| 38 | CMN Xn, #imm | 2,018 | 0.3% | ALU imm |
| 39 | TST Xn, #imm | 1,967 | 0.3% | ALU imm |
| 40 | SUBS Xd, Xn, #imm | 1,872 | 0.3% | ALU imm |

## Atomics Breakdown

fish uses musl's lock-free allocator heavily:

| Instruction | Count | Notes |
|-------------|-------|-------|
| SWP | 3,645 | Atomic swap (LSE) |
| LDUMAX | 1,717 | Atomic unsigned max (LSE) |
| LDADD | 1,198 | Atomic add (LSE) |
| LDSMAX | 879 | Atomic signed max (LSE) |
| LDUMIN | 651 | Atomic unsigned min (LSE) |
| LDAXR/LDXR | 562 | Load-exclusive (LL/SC fallback) |
| STLXR/STXR | 93 | Store-exclusive |
| LDCLR | 179 | Atomic bit clear (LSE) |
| LDEOR | 113 | Atomic XOR (LSE) |
| LDSET | 71 | Atomic bit set (LSE) |
| LDSMIN | 67 | Atomic signed min (LSE) |
| LDAR/STLR | 4 | Load-acquire / store-release |

**Total atomics: ~9,200 (1.3%)** — LSE atomics dominate over LL/SC.
HELM must implement the full LSE set for musl-static fish.

## SIMD/FP Breakdown

| Instruction | Count | Notes |
|-------------|-------|-------|
| AdvSIMD (vector) | 3,292 | Used by musl memcpy/memset/strcmp |
| FMOV | 1,657 | GP ↔ FP register moves |
| FNEG | 345 | Negate |
| FABS | 221 | Absolute value |
| FSQRT | 171 | Square root |
| FCVT | 130 | FP format conversion |
| FCVT_int | 4 | FP → integer conversion |

**Total FP/SIMD: ~5,800 (0.8%)** — mostly memcpy/memset AdvSIMD
and printf float formatting.  The AdvSIMD subset needed is small:
MOVI, LD1, ST1, EOR (for memset/memcpy/strcmp patterns).

## Implementation Priority

Based on instruction frequency, implement in this order to unlock
the most code paths earliest:

1. **MOV/ADD/SUB/CMP/ADRP** (imm + reg) — covers 45% alone
2. **LDR/STR** (unsigned-offset, unscaled) — covers 15%
3. **B/BL/B.cond/CBZ/RET** — covers 24%
4. **STP/LDP** (X, pre/post-index) — covers 13%
5. **MOVZ/MOVK/MOVN** — covers 7%
6. **UBFM/SBFM (LSL/LSR/ASR aliases)** — covers 1%
7. **Atomics (SWP, LDADD, etc.)** — covers 1.3% (needed for malloc)
8. **SVC + MRS/MSR** — covers 0.2% (needed for syscalls)
9. **SIMD load/store + FMOV** — covers 0.8% (needed for memcpy)
