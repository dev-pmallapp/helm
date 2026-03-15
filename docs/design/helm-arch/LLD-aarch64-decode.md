# helm-arch — LLD: AArch64 Decode

> **Status:** Draft — Phase 2 target
> **Covers:** `aarch64::insn`, `aarch64::decode`, `aarch64::sysreg`

---

## 1. Top-Level Dispatch

AArch64 uses a fixed 32-bit instruction width. The top-level decode key is bits [28:25] (`op0` field in ARM DDI 0487 §C4.1). This 4-bit field partitions the instruction space into five major encoding groups.

```rust
/// Decode a 32-bit AArch64 instruction word.
///
/// Returns `Err(DecodeError::Illegal { raw })` for reserved encodings or
/// encodings that are architecturally UNDEFINED. The execute function must
/// never be called with an `Aarch64Instruction::Illegal` variant.
pub fn decode_a64(raw: u32) -> Result<Aarch64Instruction, DecodeError> {
    // op0 = bits [28:25]
    let op0 = (raw >> 25) & 0xF;

    match op0 {
        0b0000 => decode_reserved(raw),
        0b0001 => decode_unallocated(raw),
        0b0010 => decode_svg(raw),          // SVE (out of scope for Phase 2)
        0b0011 => decode_unallocated(raw),

        // Data Processing — Immediate (op0 = 100x)
        0b1000 | 0b1001 => decode_dp_imm(raw),

        // Branches, Exception Generating, System (op0 = 101x)
        0b1010 | 0b1011 => decode_branch_exc_sys(raw),

        // Loads and Stores (op0 = x1x0)
        0b0100 | 0b0110 | 0b1100 | 0b1110 => decode_load_store(raw),

        // Data Processing — Register (op0 = x101)
        0b0101 | 0b1101 => decode_dp_reg(raw),

        // Data Processing — SIMD and FP (op0 = x111)
        0b0111 | 0b1111 => decode_dp_simd_fp(raw),

        _ => Ok(Aarch64Instruction::Illegal { raw }),
    }
}
```

---

## 2. `Aarch64Instruction` Enum

Organized by the five top-level encoding groups. Within each group, variants are named after the instruction mnemonic. All extracted fields are in decoded form: register indices (0–30 for GPRs, 31 for XZR or SP depending on context), immediates already shifted/signed, and shift/extend type as an enum.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aarch64Instruction {

    // ── Group 1: Data Processing — Immediate ─────────────────────────────────

    // PC-relative address generation
    /// ADR Xd, label  — PC + imm21 (byte offset)
    Adr    { rd: u8, imm: i32 },
    /// ADRP Xd, label — (PC aligned to 4K) + imm21*4096
    Adrp   { rd: u8, imm: i32 },

    // Add/subtract immediate
    /// ADD  Xd/Wd, Xn/Wn, #imm{, shift}  (no flags)
    AddImm { sf: bool, rd: u8, rn: u8, imm: u16, shift: u8 },  // shift: 0 or 12
    /// ADDS Xd/Wd, Xn/Wn, #imm{, shift}  (sets NZCV)
    AddsImm{ sf: bool, rd: u8, rn: u8, imm: u16, shift: u8 },
    /// SUB  Xd/Wd, Xn/Wn, #imm{, shift}
    SubImm { sf: bool, rd: u8, rn: u8, imm: u16, shift: u8 },
    /// SUBS Xd/Wd, Xn/Wn, #imm{, shift}
    SubsImm{ sf: bool, rd: u8, rn: u8, imm: u16, shift: u8 },

    // Logical immediate — immr/imms decoded to the expanded bitmask at decode time
    /// AND  Xd/Wd, Xn/Wn, #imm (bitmask)
    AndImm { sf: bool, rd: u8, rn: u8, imm: u64 },
    /// ORR  Xd/Wd, Xn/Wn, #imm
    OrrImm { sf: bool, rd: u8, rn: u8, imm: u64 },
    /// EOR  Xd/Wd, Xn/Wn, #imm
    EorImm { sf: bool, rd: u8, rn: u8, imm: u64 },
    /// ANDS Xd/Wd, Xn/Wn, #imm (sets NZCV)
    AndsImm{ sf: bool, rd: u8, rn: u8, imm: u64 },

    // Move wide
    /// MOVN Xd/Wd, #imm16, LSL #shift  — move inverted
    Movn   { sf: bool, rd: u8, imm16: u16, hw: u8 },  // hw: 0/1/2/3 = shift 0/16/32/48
    /// MOVZ Xd/Wd, #imm16, LSL #shift  — move zeroing
    Movz   { sf: bool, rd: u8, imm16: u16, hw: u8 },
    /// MOVK Xd/Wd, #imm16, LSL #shift  — move keeping other bits
    Movk   { sf: bool, rd: u8, imm16: u16, hw: u8 },

    // Bitfield
    /// SBFM Xd/Wd, Xn/Wn, #immr, #imms  — signed bitfield move
    Sbfm   { sf: bool, rd: u8, rn: u8, immr: u8, imms: u8 },
    /// BFM  Xd/Wd, Xn/Wn, #immr, #imms  — bitfield move (no sign)
    Bfm    { sf: bool, rd: u8, rn: u8, immr: u8, imms: u8 },
    /// UBFM Xd/Wd, Xn/Wn, #immr, #imms  — unsigned bitfield move
    Ubfm   { sf: bool, rd: u8, rn: u8, immr: u8, imms: u8 },

    // Extract
    /// EXTR Xd/Wd, Xn/Wn, Xm/Wm, #lsb  — extract register
    Extr   { sf: bool, rd: u8, rn: u8, rm: u8, lsb: u8 },

    // ── Group 2: Branches, Exception Generating, System ───────────────────────

    // Unconditional branches (immediate)
    /// B label   — PC-relative branch (26-bit imm, ×4, signed)
    B      { imm: i32 },
    /// BL label  — branch with link (X30 = PC+4)
    Bl     { imm: i32 },

    // Unconditional branches (register)
    /// BR Xn   — branch to register
    Br     { rn: u8 },
    /// BLR Xn  — branch with link to register
    Blr    { rn: u8 },
    /// RET {Xn} — return from subroutine (default: X30)
    Ret    { rn: u8 },
    /// ERET    — exception return (ELR_ELn → PC, SPSR_ELn → PSTATE)
    Eret,

    // Compare and branch
    /// CBZ Xn/Wn, label  — branch if zero
    Cbz    { sf: bool, rt: u8, imm: i32 },
    /// CBNZ Xn/Wn, label — branch if non-zero
    Cbnz   { sf: bool, rt: u8, imm: i32 },
    /// TBZ  Xn/Wn, #bit, label — test bit and branch if zero
    Tbz    { rt: u8, bit: u8, imm: i32 },
    /// TBNZ Xn/Wn, #bit, label — test bit and branch if non-zero
    Tbnz   { rt: u8, bit: u8, imm: i32 },

    // Conditional branch
    /// B.cond label — branch if condition (cond encodes NZCV condition, 4 bits)
    BCond  { cond: u8, imm: i32 },

    // Exception generating
    /// SVC #imm16 — supervisor call
    Svc    { imm16: u16 },
    /// HVC #imm16 — hypervisor call
    Hvc    { imm16: u16 },
    /// BRK #imm16 — breakpoint instruction
    Brk    { imm16: u16 },
    /// HLT #imm16 — halt (debug)
    Hlt    { imm16: u16 },

    // System instructions
    /// MRS Xt, sysreg  — move from system register to Xt
    Mrs    { rt: u8, sysreg: SysregEncoding },
    /// MSR sysreg, Xt  — move from Xt to system register
    Msr    { sysreg: SysregEncoding, rt: u8 },
    /// MSR pstatefield, #imm  — move immediate to PSTATE field
    MsrImm { op1: u8, op2: u8, crm: u8 },
    /// HINT #imm7  — NOP and related hint instructions
    Hint   { imm7: u8 },
    /// NOP — architecturally defined no-operation
    Nop,

    // Barriers
    /// DSB #option  — data synchronization barrier
    Dsb    { option: u8 },
    /// DMB #option  — data memory barrier
    Dmb    { option: u8 },
    /// ISB #option  — instruction synchronization barrier
    Isb    { option: u8 },

    // System register access (cache/TLB maintenance, AT, DC, IC, TLBI)
    /// SYS #op1, Cn, Cm, #op2{, Xt}  — generic system instruction
    Sys    { op1: u8, crn: u8, crm: u8, op2: u8, rt: u8 },
    /// SYSL Xt, #op1, Cn, Cm, #op2
    Sysl   { rt: u8, op1: u8, crn: u8, crm: u8, op2: u8 },

    // WFI / WFE / SEV / SEVL
    Wfi,
    Wfe,

    // ── Group 3: Loads and Stores ─────────────────────────────────────────────

    // Load/store register (unsigned offset) — base + offset*scale
    /// LDR Xt/Wt, [Xn, #imm]  (all size variants)
    Ldr    { size: LdStSize, rt: u8, rn: u8, offset: u16 },
    /// STR Xt/Wt, [Xn, #imm]
    Str    { size: LdStSize, rt: u8, rn: u8, offset: u16 },
    /// LDRB Wt, [Xn, #imm]  — byte, zero-extend
    Ldrb   { rt: u8, rn: u8, offset: u16 },
    /// LDRSB Xt/Wt, [Xn, #imm]  — byte, sign-extend
    Ldrsb  { sf: bool, rt: u8, rn: u8, offset: u16 },
    /// LDRH Wt, [Xn, #imm]  — halfword, zero-extend
    Ldrh   { rt: u8, rn: u8, offset: u16 },
    /// LDRSH Xt/Wt, [Xn, #imm]  — halfword, sign-extend
    Ldrsh  { sf: bool, rt: u8, rn: u8, offset: u16 },
    /// LDRSW Xt, [Xn, #imm]  — word, sign-extend to 64
    Ldrsw  { rt: u8, rn: u8, offset: u16 },
    /// STRB Wt, [Xn, #imm]
    Strb   { rt: u8, rn: u8, offset: u16 },
    /// STRH Wt, [Xn, #imm]
    Strh   { rt: u8, rn: u8, offset: u16 },

    // Load/store register (register offset)
    /// LDR Xt, [Xn, Xm{, extend shift}]
    LdrReg { size: LdStSize, rt: u8, rn: u8, rm: u8, extend: ExtendType, amount: u8 },
    StrReg { size: LdStSize, rt: u8, rn: u8, rm: u8, extend: ExtendType, amount: u8 },

    // Load/store register (immediate, pre/post-index and signed offset)
    /// LDR Xt, [Xn, #imm]! — pre-index
    LdrPre { size: LdStSize, rt: u8, rn: u8, simm: i16 },
    /// LDR Xt, [Xn], #imm — post-index
    LdrPost{ size: LdStSize, rt: u8, rn: u8, simm: i16 },
    StrPre { size: LdStSize, rt: u8, rn: u8, simm: i16 },
    StrPost{ size: LdStSize, rt: u8, rn: u8, simm: i16 },

    // Load/store PC-relative
    /// LDR Xt, label  — literal load (PC-relative, 19-bit imm × 4)
    LdrLit { size: LdStSize, rt: u8, imm: i32 },

    // Load/store pair
    /// LDP Xt1, Xt2, [Xn, #imm]  (signed pairwise load)
    Ldp    { size: LdpStpSize, rt1: u8, rt2: u8, rn: u8, simm: i16, mode: PairMode },
    /// STP Xt1, Xt2, [Xn, #imm]
    Stp    { size: LdpStpSize, rt1: u8, rt2: u8, rn: u8, simm: i16, mode: PairMode },

    // Load-acquire / store-release
    Ldar   { size: LdStSize, rt: u8, rn: u8 },
    Stlr   { size: LdStSize, rt: u8, rn: u8 },
    Ldaxr  { size: LdStSize, rt: u8, rn: u8 },            // load-acquire exclusive
    Stlxr  { size: LdStSize, rs: u8, rt: u8, rn: u8 },   // store-release exclusive

    // ── Group 4: Data Processing — Register ───────────────────────────────────

    // Two-source: ADD, SUB, AND, ORR, EOR, etc. with optional shift
    /// ADD Xd/Wd, Xn/Wn, Xm/Wm{, shift #amount}
    AddReg { sf: bool, rd: u8, rn: u8, rm: u8, shift: ShiftType, amount: u8 },
    AddsReg{ sf: bool, rd: u8, rn: u8, rm: u8, shift: ShiftType, amount: u8 },
    SubReg { sf: bool, rd: u8, rn: u8, rm: u8, shift: ShiftType, amount: u8 },
    SubsReg{ sf: bool, rd: u8, rn: u8, rm: u8, shift: ShiftType, amount: u8 },

    AndReg { sf: bool, rd: u8, rn: u8, rm: u8, shift: ShiftType, amount: u8 },
    AndsReg{ sf: bool, rd: u8, rn: u8, rm: u8, shift: ShiftType, amount: u8 },
    OrrReg { sf: bool, rd: u8, rn: u8, rm: u8, shift: ShiftType, amount: u8 },
    EorReg { sf: bool, rd: u8, rn: u8, rm: u8, shift: ShiftType, amount: u8 },
    BicReg { sf: bool, rd: u8, rn: u8, rm: u8, shift: ShiftType, amount: u8 },
    BicrReg{ sf: bool, rd: u8, rn: u8, rm: u8, shift: ShiftType, amount: u8 },  // BIC + sets flags

    // Add/subtract with carry
    Adc    { sf: bool, rd: u8, rn: u8, rm: u8 },
    Adcs   { sf: bool, rd: u8, rn: u8, rm: u8 },
    Sbc    { sf: bool, rd: u8, rn: u8, rm: u8 },
    Sbcs   { sf: bool, rd: u8, rn: u8, rm: u8 },

    // Add/subtract with extend (Wm/Xm is extended before adding)
    AddExt { sf: bool, rd: u8, rn: u8, rm: u8, extend: ExtendType, shift: u8 },
    AddsExt{ sf: bool, rd: u8, rn: u8, rm: u8, extend: ExtendType, shift: u8 },
    SubExt { sf: bool, rd: u8, rn: u8, rm: u8, extend: ExtendType, shift: u8 },
    SubsExt{ sf: bool, rd: u8, rn: u8, rm: u8, extend: ExtendType, shift: u8 },

    // Multiply (three-source)
    /// MADD Xd/Wd, Xn/Wn, Xm/Wm, Xa/Wa  — Xd = Xa + Xn*Xm
    Madd   { sf: bool, rd: u8, rn: u8, rm: u8, ra: u8 },
    Msub   { sf: bool, rd: u8, rn: u8, rm: u8, ra: u8 },
    Smaddl { rd: u8, rn: u8, rm: u8, ra: u8 },   // signed 32×32+64→64
    Smsubl { rd: u8, rn: u8, rm: u8, ra: u8 },
    Smulh  { rd: u8, rn: u8, rm: u8 },            // upper 64 bits of 64×64
    Umaddl { rd: u8, rn: u8, rm: u8, ra: u8 },
    Umsubl { rd: u8, rn: u8, rm: u8, ra: u8 },
    Umulh  { rd: u8, rn: u8, rm: u8 },

    // Divide
    Sdiv   { sf: bool, rd: u8, rn: u8, rm: u8 },
    Udiv   { sf: bool, rd: u8, rn: u8, rm: u8 },

    // Shift (variable amount, encoded as aliases of LSLV/ASRV/LSRV/RORV)
    Lslv   { sf: bool, rd: u8, rn: u8, rm: u8 },
    Lsrv   { sf: bool, rd: u8, rn: u8, rm: u8 },
    Asrv   { sf: bool, rd: u8, rn: u8, rm: u8 },
    Rorv   { sf: bool, rd: u8, rn: u8, rm: u8 },

    // Bit operations
    Cls    { sf: bool, rd: u8, rn: u8 },   // count leading sign bits
    Clz    { sf: bool, rd: u8, rn: u8 },   // count leading zeros
    Rbit   { sf: bool, rd: u8, rn: u8 },   // reverse bits
    Rev    { sf: bool, rd: u8, rn: u8 },   // reverse bytes
    Rev16  { sf: bool, rd: u8, rn: u8 },
    Rev32  { rd: u8, rn: u8 },

    // Conditional select / compare
    Csel   { sf: bool, rd: u8, rn: u8, rm: u8, cond: u8 },
    Csinc  { sf: bool, rd: u8, rn: u8, rm: u8, cond: u8 },
    Csinv  { sf: bool, rd: u8, rn: u8, rm: u8, cond: u8 },
    Csneg  { sf: bool, rd: u8, rn: u8, rm: u8, cond: u8 },
    Ccmp   { sf: bool, rn: u8, rm: u8, nzcv: u8, cond: u8 },   // conditional compare
    Ccmn   { sf: bool, rn: u8, rm: u8, nzcv: u8, cond: u8 },
    CcmpImm{ sf: bool, rn: u8, imm5: u8, nzcv: u8, cond: u8 },
    CcmnImm{ sf: bool, rn: u8, imm5: u8, nzcv: u8, cond: u8 },

    // ── Group 5: Data Processing — SIMD and FP ────────────────────────────────
    // (Subset for SE-mode; full SIMD requires dozens of additional variants)

    /// FMOV Xd/Wd, Dn/Sn  — bit transfer FP→GPR
    FmovF2I { sf: bool, rd: u8, rn: u8, ftype: u8 },
    /// FMOV Dn/Sn, Xd/Wd  — bit transfer GPR→FP
    FmovI2F { sf: bool, rd: u8, rn: u8, ftype: u8 },
    /// FMOV Dn/Sn, #imm8   — floating-point immediate
    FmovImm { rd: u8, ftype: u8, imm8: u8 },

    FaddF  { rd: u8, rn: u8, rm: u8, ftype: u8 },
    FsubF  { rd: u8, rn: u8, rm: u8, ftype: u8 },
    FmulF  { rd: u8, rn: u8, rm: u8, ftype: u8 },
    FdivF  { rd: u8, rn: u8, rm: u8, ftype: u8 },
    FabsF  { rd: u8, rn: u8, ftype: u8 },
    FnegF  { rd: u8, rn: u8, ftype: u8 },
    FsqrtF { rd: u8, rn: u8, ftype: u8 },
    FcmpF  { rn: u8, rm: u8, ftype: u8 },     // sets NZCV from FP compare
    FcmpeF { rn: u8, rm: u8, ftype: u8 },     // compare with signaling NaN
    FcmpZ  { rn: u8, ftype: u8 },             // compare with zero
    FcmpeZ { rn: u8, ftype: u8 },
    FmaddF { rd: u8, rn: u8, rm: u8, ra: u8, ftype: u8 },
    FmsubF { rd: u8, rn: u8, rm: u8, ra: u8, ftype: u8 },

    FcvtZsF { sf: bool, rd: u8, rn: u8, ftype: u8 },   // FP → integer, round toward zero
    FcvtZuF { sf: bool, rd: u8, rn: u8, ftype: u8 },
    ScvtfF  { sf: bool, rd: u8, rn: u8, ftype: u8 },   // integer → FP (signed)
    UcvtfF  { sf: bool, rd: u8, rn: u8, ftype: u8 },

    FcvtSH  { rd: u8, rn: u8 },  // half → single
    FcvtHS  { rd: u8, rn: u8 },  // single → half
    FcvtSD  { rd: u8, rn: u8 },  // double → single
    FcvtDS  { rd: u8, rn: u8 },  // single → double

    // SIMD (NEON) vector instructions — scalar subset for Phase 2
    DupElement  { rd: u8, rn: u8, imm5: u8 },
    InsElement  { rd: u8, ri: u8, rn: u8, rj: u8, imm5: u8 },
    Umov        { rd: u8, rn: u8, imm5: u8, q: bool },
    Smov        { rd: u8, rn: u8, imm5: u8, q: bool },

    // ── Illegal / Unallocated ──────────────────────────────────────────────────
    Illegal { raw: u32 },
}
```

### Auxiliary Enums

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LdStSize {
    Byte,       // 1 byte
    HalfWord,   // 2 bytes
    Word,       // 4 bytes (Wt)
    DoubleWord, // 8 bytes (Xt)
    QuadWord,   // 16 bytes (Qt, SIMD)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LdpStpSize { Word, DoubleWord, QuadWord }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairMode { Offset, PreIndex, PostIndex }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftType { Lsl, Lsr, Asr, Ror }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtendType {
    Uxtb, Uxth, Uxtw, Uxtx,   // zero-extend byte/halfword/word/doubleword
    Sxtb, Sxth, Sxtw, Sxtx,   // sign-extend
    Lsl,                        // logical shift left (treated as UXTX + shift)
}
```

---

## 3. deku Struct Examples

`deku` maps struct fields to bit ranges. The `#[deku]` derive generates `from_reader` and `to_writer` implementations. For `helm-arch`, we use `deku` in read-only mode (decode only).

### Example: ADD (immediate) — Data Processing Immediate

ARM DDI 0487 §C3.4: `sf:1 | op:1 | S:1 | 10001 | shift:2 | imm12:12 | Rn:5 | Rd:5`

```rust
use deku::prelude::*;

#[derive(Debug, Clone, Copy, DekuRead)]
#[deku(endian = "little")]
pub struct AddImmEncoding {
    #[deku(bits = 5)]  pub rd: u8,
    #[deku(bits = 5)]  pub rn: u8,
    #[deku(bits = 12)] pub imm12: u16,
    #[deku(bits = 2)]  pub shift: u8,    // 0 = LSL #0, 1 = LSL #12, 2/3 reserved
    #[deku(bits = 5)]  pub _op: u8,      // must be 0b10001
    #[deku(bits = 1)]  pub s: u8,        // 0 = ADD, 1 = ADDS
    #[deku(bits = 1)]  pub op: u8,       // 0 = ADD/ADDS, 1 = SUB/SUBS
    #[deku(bits = 1)]  pub sf: u8,       // 0 = 32-bit, 1 = 64-bit
}

// Usage in decode_dp_imm:
fn decode_add_sub_imm(raw: u32) -> Result<Aarch64Instruction, DecodeError> {
    let enc = AddImmEncoding::from_bytes((&raw.to_le_bytes(), 0))
        .map_err(|_| DecodeError::Illegal { raw })?.1;

    if enc.shift > 1 { return Ok(Aarch64Instruction::Illegal { raw }); }

    let sf = enc.sf != 0;
    let rd = enc.rd;
    let rn = enc.rn;
    let imm = enc.imm12 as u16;
    let shift = enc.shift * 12;  // 0 or 12

    Ok(match (enc.op, enc.s) {
        (0, 0) => Aarch64Instruction::AddImm  { sf, rd, rn, imm, shift },
        (0, 1) => Aarch64Instruction::AddsImm { sf, rd, rn, imm, shift },
        (1, 0) => Aarch64Instruction::SubImm  { sf, rd, rn, imm, shift },
        (1, 1) => Aarch64Instruction::SubsImm { sf, rd, rn, imm, shift },
        _ => unreachable!(),
    })
}
```

### Example: LDR (unsigned offset) — Loads and Stores

ARM DDI 0487 §C3.3: `size:2 | 111001 | 01 | imm12:12 | Rn:5 | Rt:5`

```rust
#[derive(Debug, Clone, Copy, DekuRead)]
#[deku(endian = "little")]
pub struct LdrUnsignedOffset {
    #[deku(bits = 5)]  pub rt: u8,
    #[deku(bits = 5)]  pub rn: u8,
    #[deku(bits = 12)] pub imm12: u16,   // unsigned; actual byte offset = imm12 * size_bytes
    #[deku(bits = 2)]  pub opc: u8,      // 01 = load, 00 = store
    #[deku(bits = 6)]  pub _vr: u8,      // 111001 for integer loads
    #[deku(bits = 2)]  pub size: u8,     // 00=byte, 01=halfword, 10=word, 11=doubleword
}
```

### Example: B / BL — Unconditional Branch (immediate)

ARM DDI 0487 §C6.2.29: `op:1 | 00101 | imm26:26`

```rust
#[derive(Debug, Clone, Copy, DekuRead)]
#[deku(endian = "little")]
pub struct BranchImm {
    #[deku(bits = 26)] pub imm26: u32,  // signed, shifted left 2 = byte offset
    #[deku(bits = 5)]  pub _op2: u8,    // 00101
    #[deku(bits = 1)]  pub op: u8,      // 0 = B, 1 = BL
}

fn decode_uncond_branch_imm(raw: u32) -> Result<Aarch64Instruction, DecodeError> {
    let enc = BranchImm::from_bytes((&raw.to_le_bytes(), 0))?.1;
    // Sign-extend 26-bit value and shift left 2 for byte offset.
    let imm = (((enc.imm26 as i32) << 6) >> 6) << 2;
    Ok(if enc.op == 0 {
        Aarch64Instruction::B  { imm }
    } else {
        Aarch64Instruction::Bl { imm }
    })
}
```

### Example: CBZ / CBNZ — Compare and Branch

ARM DDI 0487 §C6.2.47: `sf:1 | 011010 | op:1 | imm19:19 | Rt:5`

```rust
#[derive(Debug, Clone, Copy, DekuRead)]
#[deku(endian = "little")]
pub struct CbzEncoding {
    #[deku(bits = 5)]  pub rt: u8,
    #[deku(bits = 19)] pub imm19: u32,   // PC-relative, ×4
    #[deku(bits = 1)]  pub op: u8,       // 0 = CBZ, 1 = CBNZ
    #[deku(bits = 6)]  pub _fixed: u8,   // 011010
    #[deku(bits = 1)]  pub sf: u8,       // 0 = 32-bit Wt, 1 = 64-bit Xt
}
```

### Example: SVC — Supervisor Call

ARM DDI 0487 §C6.2.294: `11010100 | 000 | imm16:16 | 00001`

```rust
#[derive(Debug, Clone, Copy, DekuRead)]
#[deku(endian = "little")]
pub struct SvcEncoding {
    #[deku(bits = 5)]  pub _ll: u8,     // 00001
    #[deku(bits = 16)] pub imm16: u16,
    #[deku(bits = 3)]  pub _opc: u8,    // 000
    #[deku(bits = 8)]  pub _fixed: u8,  // 11010100
}
```

### Example: MRS — Move from System Register

ARM DDI 0487 §C6.2.184: `1101010100 | 1 | op0[1] | op1:3 | CRn:4 | CRm:4 | op2:3 | Rt:5`

The system register is identified by the 5-tuple `(op0, op1, CRn, CRm, op2)`.

```rust
#[derive(Debug, Clone, Copy, DekuRead)]
#[deku(endian = "little")]
pub struct MrsEncoding {
    #[deku(bits = 5)]  pub rt: u8,
    #[deku(bits = 3)]  pub op2: u8,
    #[deku(bits = 4)]  pub crm: u8,
    #[deku(bits = 4)]  pub crn: u8,
    #[deku(bits = 3)]  pub op1: u8,
    #[deku(bits = 1)]  pub op0_lsb: u8,   // bit 19: op0[0]; op0[1]=1 (fixed at 1 for MRS)
    #[deku(bits = 1)]  pub l: u8,          // 1 = MRS (read), 0 = MSR (write)
    #[deku(bits = 10)] pub _fixed: u16,    // 1101010100
}

/// System register encoding: the 5-tuple that uniquely identifies a register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SysregEncoding {
    pub op0: u8,   // 2 or 3 (effectively: 2 = S2_*, 3 = S3_*)
    pub op1: u8,   // 3 bits
    pub crn: u8,   // 4 bits (Cn field)
    pub crm: u8,   // 4 bits (Cm field)
    pub op2: u8,   // 3 bits
}

impl SysregEncoding {
    /// Known system registers — checked against ARM DDI 0487 Table D17-2.
    pub fn name(&self) -> &'static str {
        match (self.op0, self.op1, self.crn, self.crm, self.op2) {
            (3, 0, 0, 0, 0) => "MIDR_EL1",
            (3, 0, 0, 0, 5) => "MPIDR_EL1",
            (3, 0, 1, 0, 0) => "SCTLR_EL1",
            (3, 0, 1, 0, 2) => "CPACR_EL1",
            (3, 0, 2, 0, 0) => "TTBR0_EL1",
            (3, 0, 2, 0, 1) => "TTBR1_EL1",
            (3, 0, 2, 0, 2) => "TCR_EL1",
            (3, 0, 4, 0, 0) => "SPSR_EL1",
            (3, 0, 4, 0, 1) => "ELR_EL1",
            (3, 0, 4, 1, 0) => "SP_EL0",
            (3, 0, 4, 2, 0) => "CurrentEL",
            (3, 0, 4, 2, 1) => "DAIF",
            (3, 0, 4, 2, 2) => "SPSel",
            (3, 3, 4, 2, 0) => "NZCV",
            (3, 3, 4, 2, 1) => "DAIF",
            (3, 3, 13, 0, 2) => "TPIDR_EL0",
            (3, 0, 13, 0, 1) => "TPIDR_EL1",
            (3, 0, 5, 1, 0) => "ESR_EL1",
            (3, 0, 6, 0, 0) => "FAR_EL1",
            (3, 0, 10, 2, 0) => "MAIR_EL1",
            (3, 0, 12, 0, 0) => "VBAR_EL1",
            (3, 0, 0, 5, 1) => "DCZID_EL0",
            _ => "UNKNOWN_SYSREG",
        }
    }
}
```

---

## 4. Encoding Group Sub-Decoders

### Data Processing — Immediate

Bits [28:25] = `100x`. Op field is bits [25:23] (3 bits, after the `100` prefix).

```rust
fn decode_dp_imm(raw: u32) -> Result<Aarch64Instruction, DecodeError> {
    // Sub-op: bits [25:23]
    let op = (raw >> 23) & 0x7;
    match op {
        0b000 | 0b001 => decode_pc_rel(raw),           // ADR, ADRP
        0b010 | 0b011 => decode_add_sub_imm(raw),      // ADD/SUB immediate
        0b100 => decode_add_sub_imm_tags(raw),          // ADDG/SUBG (MTE, out of scope)
        0b101 => decode_logical_imm(raw),               // AND/ORR/EOR/ANDS
        0b110 => decode_move_wide_imm(raw),             // MOVN/MOVZ/MOVK
        0b111 => decode_bitfield(raw),                  // SBFM/BFM/UBFM + EXTR
        _ => Ok(Aarch64Instruction::Illegal { raw }),
    }
}
```

### Logical Immediate — Bitmask Decode

ARM's logical immediates use `N:immr:imms` to encode a bitmask. Decoding requires the `DecodeBitMasks` algorithm from ARM DDI 0487 §C.4.

```rust
/// Decode an ARM64 logical immediate (N:immr:imms → 64-bit bitmask).
/// Returns None if the encoding is reserved.
pub fn decode_bitmask(n: u8, immr: u8, imms: u8, sf: bool) -> Option<u64> {
    // Length of the element: find the highest set bit in N:~imms
    let len = if n != 0 {
        6u32
    } else {
        let x = !(imms as u32) & 0x3F;
        if x == 0 { return None; }  // reserved
        31u32 - x.leading_zeros()
    };
    if len < 1 { return None; }

    let levels = (1u32 << len) - 1;            // 2^len - 1: used as mask
    let s = (imms as u32) & levels;            // actual element width - 1
    let r = (immr as u32) & levels;

    let esize = 1u64 << len;                  // element size in bits
    // Build the base bit pattern: s+1 ones
    let welem = (1u64 << (s + 1)) - 1;
    // Rotate right by r within esize bits
    let telem = ror(welem, r as u64, esize);

    // Replicate the element to fill 64 bits
    let mut mask = telem;
    let mut e = esize;
    while e < 64 {
        mask |= mask << e;
        e *= 2;
    }
    if !sf { mask &= 0xFFFF_FFFF; }
    Some(mask)
}

fn ror(x: u64, shift: u64, width: u64) -> u64 {
    (x >> shift) | (x << (width - shift)) & ((1u64 << width) - 1)
}
```

### Branches, Exception Generating, System

Bits [28:25] = `101x`. Sub-op from bits [31:29]:

```rust
fn decode_branch_exc_sys(raw: u32) -> Result<Aarch64Instruction, DecodeError> {
    let op1 = (raw >> 29) & 0x7;
    let op2 = (raw >> 22) & 0xF;

    match op1 {
        0b000 | 0b100 => decode_uncond_branch_imm(raw),   // B, BL
        0b001 | 0b101 => decode_cond_branch(raw),         // B.cond
        0b010 => match op2 {
            0b0000..=0b0011 => decode_exception_gen(raw), // SVC, HVC, SMC, BRK, HLT
            0b1000..=0b1111 => decode_system(raw),        // MSR, MRS, SYS, barriers
            _ => Ok(Aarch64Instruction::Illegal { raw }),
        },
        0b110 | 0b111 => {
            match (raw >> 24) & 0x1F {
                0b11010 => decode_uncond_branch_reg(raw), // BR, BLR, RET, ERET
                0b11011 => decode_compare_branch(raw),    // CBZ, CBNZ, TBZ, TBNZ
                _ => Ok(Aarch64Instruction::Illegal { raw }),
            }
        }
        _ => Ok(Aarch64Instruction::Illegal { raw }),
    }
}
```

### Loads and Stores

Bits [28:25] = `x1x0`. The 4-bit op0 identifies the load/store sub-group:

```rust
fn decode_load_store(raw: u32) -> Result<Aarch64Instruction, DecodeError> {
    let op1 = (raw >> 26) & 1;  // VR bit (1 = FP/SIMD)
    let op2 = (raw >> 23) & 0x3;
    let op3 = (raw >> 16) & 0x3F;
    let op4 = (raw >> 10) & 0x3;

    if op1 == 0 {
        match op2 {
            0b00 => decode_exclusive(raw),              // LDXR, STXR, LDAXR, STLXR
            0b01 => decode_load_register_literal(raw),  // LDR (literal)
            0b10 => decode_load_store_pair(raw),        // LDP, STP
            0b11 => decode_load_store_single(raw, op3, op4), // LDR, STR variants
            _ => unreachable!(),
        }
    } else {
        decode_load_store_simd(raw)                    // SIMD LDR/STR
    }
}
```

---

## 5. DecodeError

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The encoding is architecturally UNDEFINED or reserved. The execute
    /// function must not be called with an Illegal instruction.
    Illegal { raw: u32 },
    /// AArch32 encoding detected (op0 bits not valid for AArch64).
    /// Only returned if somehow a 32-bit AArch32 encoding reaches this decoder.
    Aarch32Unsupported { raw: u32 },
}
```

---

## 6. Module Layout

```
aarch64/
├── mod.rs         — pub use
├── insn.rs        — Aarch64Instruction enum, LdStSize, ShiftType, ExtendType,
│                    PairMode, SysregEncoding, LdpStpSize
├── decode.rs      — decode_a64(raw: u32) + all sub-group decoders
│                    decode_bitmask, ror, helper functions
├── execute.rs     — execute_a64(insn, ctx)
├── sysreg.rs      — SysregEncoding lookup table, SysregFile, read/write dispatch
├── flags.rs       — add_with_carry, sub_borrow, check_cond, update_nzcv
└── exception.rs   — HartException (AArch64), DecodeError, ESR_EL1 encoding
```
