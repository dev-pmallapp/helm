# helm-arch — LLD: RISC-V Decode

> **Status:** Draft — Phase 0 target
> **Covers:** `riscv::insn`, `riscv::decode`, `riscv::compress`, `riscv::exception`

---

## 1. `Instruction` Enum

The `Instruction` enum is the product of decoding a single 32-bit RV64GC instruction word (after C expansion). Every variant is `Copy`. No heap allocation occurs during decode.

Variants are organized by ISA extension. Within each extension they are ordered by opcode value (ascending), matching the table layout in the RISC-V Unprivileged Specification.

```rust
/// A fully decoded RISC-V instruction. All immediates are already sign-extended
/// to i64 / i32 at decode time. Register indices are in the range 0–31.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {

    // ── RV64I — Base Integer ─────────────────────────────────────────────────

    /// Load instructions: LB, LH, LW, LD, LBU, LHU, LWU
    Load  { rd: u8, rs1: u8, imm: i32, width: LoadWidth },
    /// Store instructions: SB, SH, SW, SD
    Store { rs1: u8, rs2: u8, imm: i32, width: StoreWidth },

    // Register-register ALU
    Add   { rd: u8, rs1: u8, rs2: u8 },
    Sub   { rd: u8, rs1: u8, rs2: u8 },
    Sll   { rd: u8, rs1: u8, rs2: u8 },
    Slt   { rd: u8, rs1: u8, rs2: u8 },
    Sltu  { rd: u8, rs1: u8, rs2: u8 },
    Xor   { rd: u8, rs1: u8, rs2: u8 },
    Srl   { rd: u8, rs1: u8, rs2: u8 },
    Sra   { rd: u8, rs1: u8, rs2: u8 },
    Or    { rd: u8, rs1: u8, rs2: u8 },
    And   { rd: u8, rs1: u8, rs2: u8 },

    // Immediate ALU
    Addi  { rd: u8, rs1: u8, imm: i32 },
    Slti  { rd: u8, rs1: u8, imm: i32 },
    Sltiu { rd: u8, rs1: u8, imm: i32 },
    Xori  { rd: u8, rs1: u8, imm: i32 },
    Ori   { rd: u8, rs1: u8, imm: i32 },
    Andi  { rd: u8, rs1: u8, imm: i32 },
    Slli  { rd: u8, rs1: u8, shamt: u8 },  // shamt 0–63 for RV64
    Srli  { rd: u8, rs1: u8, shamt: u8 },
    Srai  { rd: u8, rs1: u8, shamt: u8 },

    // RV64I word operations (operate on low 32 bits, sign-extend result to 64)
    Addw  { rd: u8, rs1: u8, rs2: u8 },
    Subw  { rd: u8, rs1: u8, rs2: u8 },
    Sllw  { rd: u8, rs1: u8, rs2: u8 },
    Srlw  { rd: u8, rs1: u8, rs2: u8 },
    Sraw  { rd: u8, rs1: u8, rs2: u8 },
    Addiw { rd: u8, rs1: u8, imm: i32 },
    Slliw { rd: u8, rs1: u8, shamt: u8 },
    Srliw { rd: u8, rs1: u8, shamt: u8 },
    Sraiw { rd: u8, rs1: u8, shamt: u8 },

    // Branches: imm is byte offset from PC, always even
    Beq   { rs1: u8, rs2: u8, imm: i32 },
    Bne   { rs1: u8, rs2: u8, imm: i32 },
    Blt   { rs1: u8, rs2: u8, imm: i32 },
    Bge   { rs1: u8, rs2: u8, imm: i32 },
    Bltu  { rs1: u8, rs2: u8, imm: i32 },
    Bgeu  { rs1: u8, rs2: u8, imm: i32 },

    // Jumps
    Jal   { rd: u8, imm: i32 },                 // imm is PC-relative byte offset
    Jalr  { rd: u8, rs1: u8, imm: i32 },

    // Upper immediates
    Lui   { rd: u8, imm: u32 },                 // raw upper-20 bits, bits [31:12]
    Auipc { rd: u8, imm: u32 },

    // System
    Ecall,
    Ebreak,
    Fence  { pred: u8, succ: u8 },              // memory ordering fence
    FenceI,                                     // instruction-fetch fence

    // ── M Extension — Integer Multiply/Divide ────────────────────────────────

    Mul    { rd: u8, rs1: u8, rs2: u8 },
    Mulh   { rd: u8, rs1: u8, rs2: u8 },        // signed × signed, upper 64 bits
    Mulhsu { rd: u8, rs1: u8, rs2: u8 },        // signed × unsigned, upper 64 bits
    Mulhu  { rd: u8, rs1: u8, rs2: u8 },        // unsigned × unsigned, upper 64 bits
    Div    { rd: u8, rs1: u8, rs2: u8 },        // signed division, truncate-toward-zero
    Divu   { rd: u8, rs1: u8, rs2: u8 },
    Rem    { rd: u8, rs1: u8, rs2: u8 },
    Remu   { rd: u8, rs1: u8, rs2: u8 },
    Mulw   { rd: u8, rs1: u8, rs2: u8 },        // RV64 word forms
    Divw   { rd: u8, rs1: u8, rs2: u8 },
    Divuw  { rd: u8, rs1: u8, rs2: u8 },
    Remw   { rd: u8, rs1: u8, rs2: u8 },
    Remuw  { rd: u8, rs1: u8, rs2: u8 },

    // ── A Extension — Atomic Instructions ────────────────────────────────────

    Lrw    { rd: u8, rs1: u8, aq: bool, rl: bool },
    Scw    { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    Lrd    { rd: u8, rs1: u8, aq: bool, rl: bool },
    Scd    { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },

    // AMOs — op field encodes the operation
    AmoswapW { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmoaddW  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmoxorW  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmoandW  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmoorW   { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmominW  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmomaxW  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmominuW { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmomaxuW { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    // Doubleword (D suffix) equivalents: AmoswapD, AmoaddD, etc. (same fields)
    AmoswapD { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmoaddD  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmoxorD  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmoandD  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmoorD   { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmominD  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmomaxD  { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmominuD { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },
    AmomaxuD { rd: u8, rs1: u8, rs2: u8, aq: bool, rl: bool },

    // ── F Extension — Single-Precision Floating Point ────────────────────────

    Flw    { frd: u8, rs1: u8, imm: i32 },
    Fsw    { rs1: u8, frs2: u8, imm: i32 },
    // FP ALU: rm = rounding mode (0–7; 7 = dynamic from fcsr)
    FaddS  { frd: u8, frs1: u8, frs2: u8, rm: u8 },
    FsubS  { frd: u8, frs1: u8, frs2: u8, rm: u8 },
    FmulS  { frd: u8, frs1: u8, frs2: u8, rm: u8 },
    FdivS  { frd: u8, frs1: u8, frs2: u8, rm: u8 },
    FsqrtS { frd: u8, frs1: u8, rm: u8 },
    FsgnjS { frd: u8, frs1: u8, frs2: u8 },
    FsgnjnS{ frd: u8, frs1: u8, frs2: u8 },
    FsgnjxS{ frd: u8, frs1: u8, frs2: u8 },
    FminS  { frd: u8, frs1: u8, frs2: u8 },
    FmaxS  { frd: u8, frs1: u8, frs2: u8 },
    FmaS   { frd: u8, frs1: u8, frs2: u8, frs3: u8, rm: u8 },  // FMADD.S
    FmsS   { frd: u8, frs1: u8, frs2: u8, frs3: u8, rm: u8 },  // FMSUB.S
    FnmsS  { frd: u8, frs1: u8, frs2: u8, frs3: u8, rm: u8 },  // FNMADD.S (negated)
    FnmaS  { frd: u8, frs1: u8, frs2: u8, frs3: u8, rm: u8 },  // FNMSUB.S (negated)
    FcvtWS { rd: u8, frs1: u8, rm: u8 },   // float → i32 SEXT to 64
    FcvtWuS{ rd: u8, frs1: u8, rm: u8 },   // float → u32 ZEXT to 64
    FcvtLS { rd: u8, frs1: u8, rm: u8 },   // float → i64 (RV64)
    FcvtLuS{ rd: u8, frs1: u8, rm: u8 },   // float → u64 (RV64)
    FcvtSW { frd: u8, rs1: u8, rm: u8 },   // i32 → float
    FcvtSWu{ frd: u8, rs1: u8, rm: u8 },
    FcvtSL { frd: u8, rs1: u8, rm: u8 },   // i64 → float (RV64)
    FcvtSLu{ frd: u8, rs1: u8, rm: u8 },
    FmvXW  { rd: u8, frs1: u8 },            // bit-transfer float reg → int reg (low 32)
    FmvWX  { frd: u8, rs1: u8 },            // bit-transfer int reg → float reg (NaN box)
    FeqS   { rd: u8, frs1: u8, frs2: u8 },
    FltS   { rd: u8, frs1: u8, frs2: u8 },
    FleS   { rd: u8, frs1: u8, frs2: u8 },
    FclassS{ rd: u8, frs1: u8 },

    // ── D Extension — Double-Precision Floating Point ────────────────────────
    // Same structure as F; fields use frd/frs for float register indices.

    Fld    { frd: u8, rs1: u8, imm: i32 },
    Fsd    { rs1: u8, frs2: u8, imm: i32 },
    FaddD  { frd: u8, frs1: u8, frs2: u8, rm: u8 },
    FsubD  { frd: u8, frs1: u8, frs2: u8, rm: u8 },
    FmulD  { frd: u8, frs1: u8, frs2: u8, rm: u8 },
    FdivD  { frd: u8, frs1: u8, frs2: u8, rm: u8 },
    FsqrtD { frd: u8, frs1: u8, rm: u8 },
    FsgnjD { frd: u8, frs1: u8, frs2: u8 },
    FsgnjnD{ frd: u8, frs1: u8, frs2: u8 },
    FsgnjxD{ frd: u8, frs1: u8, frs2: u8 },
    FminD  { frd: u8, frs1: u8, frs2: u8 },
    FmaxD  { frd: u8, frs1: u8, frs2: u8 },
    FmaD   { frd: u8, frs1: u8, frs2: u8, frs3: u8, rm: u8 },
    FmsD   { frd: u8, frs1: u8, frs2: u8, frs3: u8, rm: u8 },
    FnmsD  { frd: u8, frs1: u8, frs2: u8, frs3: u8, rm: u8 },
    FnmaD  { frd: u8, frs1: u8, frs2: u8, frs3: u8, rm: u8 },
    FcvtSD { frd: u8, frs1: u8, rm: u8 },  // double → single
    FcvtDS { frd: u8, frs1: u8, rm: u8 },  // single → double (exact, no rounding)
    FcvtWD { rd: u8, frs1: u8, rm: u8 },
    FcvtWuD{ rd: u8, frs1: u8, rm: u8 },
    FcvtLD { rd: u8, frs1: u8, rm: u8 },
    FcvtLuD{ rd: u8, frs1: u8, rm: u8 },
    FcvtDW { frd: u8, rs1: u8, rm: u8 },
    FcvtDWu{ frd: u8, rs1: u8, rm: u8 },
    FcvtDL { frd: u8, rs1: u8, rm: u8 },
    FcvtDLu{ frd: u8, rs1: u8, rm: u8 },
    FmvXD  { rd: u8, frs1: u8 },           // bit-transfer double → int (RV64)
    FmvDX  { frd: u8, rs1: u8 },
    FeqD   { rd: u8, frs1: u8, frs2: u8 },
    FltD   { rd: u8, frs1: u8, frs2: u8 },
    FleD   { rd: u8, frs1: u8, frs2: u8 },
    FclassD{ rd: u8, frs1: u8 },

    // ── Zicsr — Control and Status Register Instructions ─────────────────────

    Csrrw  { rd: u8, rs1: u8, csr: u16 },
    Csrrs  { rd: u8, rs1: u8, csr: u16 },
    Csrrc  { rd: u8, rs1: u8, csr: u16 },
    Csrrwi { rd: u8, uimm: u8, csr: u16 },
    Csrrsi { rd: u8, uimm: u8, csr: u16 },
    Csrrci { rd: u8, uimm: u8, csr: u16 },

    // ── Privileged instructions ──────────────────────────────────────────────

    Mret,
    Sret,
    Wfi,
    SfenceVma { rs1: u8, rs2: u8 },

    // ── Illegal / Unrecognized ───────────────────────────────────────────────

    /// Reserved encoding. Execute must raise HartException::IllegalInstruction.
    Illegal { raw: u32 },
}

// ── Auxiliary enums ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadWidth {
    Byte,           // LB — sign-extend byte to 64
    HalfWord,       // LH — sign-extend halfword to 64
    Word,           // LW — sign-extend word to 64
    DoubleWord,     // LD
    ByteUnsigned,   // LBU — zero-extend
    HalfWordUnsigned, // LHU — zero-extend
    WordUnsigned,   // LWU — zero-extend (RV64 only)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreWidth {
    Byte,      // SB
    HalfWord,  // SH
    Word,      // SW
    DoubleWord, // SD
}
```

---

## 2. `DecodeError` and `HartException`

```rust
/// Returned by decode_rv64 / decode_rv64c when the input cannot be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Bits [1:0] indicate this is a 16-bit (C) instruction but the caller
    /// passed to decode_rv64 directly. Caller should use decode_rv64c first.
    CompressedInstruction { raw: u32 },
    /// The encoding is not a valid C-extension instruction.
    InvalidCompressed { raw: u16 },
}

/// Trap conditions raised by execute(). Corresponds to RISC-V mcause values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HartException {
    // Synchronous exceptions (traps)
    IllegalInstruction    { raw: u32 },
    Breakpoint            { pc: u64 },
    EnvironmentCallUMode,                       // ECALL from U-mode
    EnvironmentCallSMode,                       // ECALL from S-mode
    EnvironmentCallMMode,                       // ECALL from M-mode
    LoadAddressMisaligned { addr: u64 },
    LoadAccessFault       { addr: u64 },
    StoreAmoMisaligned    { addr: u64 },
    StoreAmoAccessFault   { addr: u64 },
    LoadPageFault         { addr: u64 },
    StoreAmoPageFault     { addr: u64 },
    InstructionPageFault  { addr: u64 },
    // Privilege violation
    CsrAccessFault       { csr: u16 },
}

impl HartException {
    /// Returns the RISC-V mcause value for this exception (interrupt bit = 0).
    pub fn mcause(&self) -> u64 { ... }
}
```

---

## 3. Bit-Extraction Helpers

One helper per encoding format. All return sign-extended values where appropriate. These are `#[inline(always)]` functions in `riscv::decode`.

```rust
/// Extract the 7-bit opcode from bits [6:0].
#[inline(always)]
pub fn opcode(raw: u32) -> u8 { (raw & 0x7F) as u8 }

/// Extract funct3 from bits [14:12].
#[inline(always)]
pub fn funct3(raw: u32) -> u8 { ((raw >> 12) & 0x7) as u8 }

/// Extract funct7 from bits [31:25].
#[inline(always)]
pub fn funct7(raw: u32) -> u8 { ((raw >> 25) & 0x7F) as u8 }

/// Extract rd from bits [11:7].
#[inline(always)]
pub fn rd(raw: u32) -> u8 { ((raw >> 7) & 0x1F) as u8 }

/// Extract rs1 from bits [19:15].
#[inline(always)]
pub fn rs1(raw: u32) -> u8 { ((raw >> 15) & 0x1F) as u8 }

/// Extract rs2 from bits [24:20].
#[inline(always)]
pub fn rs2(raw: u32) -> u8 { ((raw >> 20) & 0x1F) as u8 }

/// Extract rs3 from bits [31:27] (used in FP fused instructions).
#[inline(always)]
pub fn rs3(raw: u32) -> u8 { ((raw >> 27) & 0x1F) as u8 }

/// I-type immediate: sign-extend imm[11:0] from bits [31:20].
#[inline(always)]
pub fn decode_i_imm(raw: u32) -> i32 {
    (raw as i32) >> 20
}

/// S-type immediate: imm[11:5] from bits [31:25], imm[4:0] from bits [11:7].
#[inline(always)]
pub fn decode_s_imm(raw: u32) -> i32 {
    let lo = (raw >> 7) & 0x1F;
    let hi = (raw >> 25) & 0x7F;
    let imm = (hi << 5) | lo;
    ((imm as i32) << 20) >> 20
}

/// B-type immediate: scrambled bit layout, always even (bit 0 implicit 0).
#[inline(always)]
pub fn decode_b_imm(raw: u32) -> i32 {
    let imm12   = (raw >> 31) & 1;
    let imm11   = (raw >> 7)  & 1;
    let imm10_5 = (raw >> 25) & 0x3F;
    let imm4_1  = (raw >> 8)  & 0xF;
    let imm = (imm12 << 12) | (imm11 << 11) | (imm10_5 << 5) | (imm4_1 << 1);
    ((imm as i32) << 19) >> 19
}

/// U-type immediate: bits [31:12] placed at [31:12] of result, [11:0] zeroed.
#[inline(always)]
pub fn decode_u_imm(raw: u32) -> u32 {
    raw & 0xFFFFF000
}

/// J-type immediate: scrambled bit layout, always even. Range ±1 MiB.
#[inline(always)]
pub fn decode_j_imm(raw: u32) -> i32 {
    let imm20    = (raw >> 31) & 1;
    let imm10_1  = (raw >> 21) & 0x3FF;
    let imm11    = (raw >> 20) & 1;
    let imm19_12 = (raw >> 12) & 0xFF;
    let imm = (imm20 << 20) | (imm19_12 << 12) | (imm11 << 11) | (imm10_1 << 1);
    ((imm as i32) << 11) >> 11
}

/// Sign-extend a value of `width` bits to i64.
#[inline(always)]
pub fn sext(val: u64, width: u32) -> i64 {
    let shift = 64 - width;
    ((val as i64) << shift) >> shift
}
```

---

## 4. `decode_rv64` — Primary Opcode Dispatch

```rust
/// Decode a 32-bit RV64GC instruction word.
///
/// Precondition: bits [1:0] must not be `0b11` followed by `0b00`, `0b01`, or `0b10`
/// (those are C-extension quadrants). If bits [1:0] != 0b11, return
/// DecodeError::CompressedInstruction — caller must use decode_rv64c instead.
///
/// All immediates in the returned variant are fully decoded and sign-extended.
pub fn decode_rv64(raw: u32) -> Result<Instruction, DecodeError> {
    // Detect compressed instruction
    if (raw & 0x3) != 0x3 {
        return Err(DecodeError::CompressedInstruction { raw });
    }

    let op = opcode(raw);
    let f3 = funct3(raw);
    let f7 = funct7(raw);

    match op {
        // ── LOAD ─────────────────────────────────────────────────────────────
        0b000_0011 => {
            let width = match f3 {
                0b000 => LoadWidth::Byte,
                0b001 => LoadWidth::HalfWord,
                0b010 => LoadWidth::Word,
                0b011 => LoadWidth::DoubleWord,
                0b100 => LoadWidth::ByteUnsigned,
                0b101 => LoadWidth::HalfWordUnsigned,
                0b110 => LoadWidth::WordUnsigned,
                _     => return Ok(Instruction::Illegal { raw }),
            };
            Ok(Instruction::Load { rd: rd(raw), rs1: rs1(raw), imm: decode_i_imm(raw), width })
        }

        // ── LOAD-FP ───────────────────────────────────────────────────────────
        0b000_0111 => match f3 {
            0b010 => Ok(Instruction::Flw { frd: rd(raw), rs1: rs1(raw), imm: decode_i_imm(raw) }),
            0b011 => Ok(Instruction::Fld { frd: rd(raw), rs1: rs1(raw), imm: decode_i_imm(raw) }),
            _     => Ok(Instruction::Illegal { raw }),
        },

        // ── MISC-MEM (FENCE, FENCE.I) ─────────────────────────────────────────
        0b000_1111 => match f3 {
            0b000 => {
                let pred = ((raw >> 24) & 0xF) as u8;
                let succ = ((raw >> 20) & 0xF) as u8;
                Ok(Instruction::Fence { pred, succ })
            }
            0b001 => Ok(Instruction::FenceI),
            _     => Ok(Instruction::Illegal { raw }),
        },

        // ── OP-IMM ────────────────────────────────────────────────────────────
        0b001_0011 => match f3 {
            0b000 => Ok(Instruction::Addi  { rd: rd(raw), rs1: rs1(raw), imm: decode_i_imm(raw) }),
            0b010 => Ok(Instruction::Slti  { rd: rd(raw), rs1: rs1(raw), imm: decode_i_imm(raw) }),
            0b011 => Ok(Instruction::Sltiu { rd: rd(raw), rs1: rs1(raw), imm: decode_i_imm(raw) }),
            0b100 => Ok(Instruction::Xori  { rd: rd(raw), rs1: rs1(raw), imm: decode_i_imm(raw) }),
            0b110 => Ok(Instruction::Ori   { rd: rd(raw), rs1: rs1(raw), imm: decode_i_imm(raw) }),
            0b111 => Ok(Instruction::Andi  { rd: rd(raw), rs1: rs1(raw), imm: decode_i_imm(raw) }),
            0b001 => {
                // SLLI: imm[11:6] must be 0b000000 for RV64
                let shamt = ((raw >> 20) & 0x3F) as u8;
                if (raw >> 26) & 0x3F != 0 { return Ok(Instruction::Illegal { raw }); }
                Ok(Instruction::Slli { rd: rd(raw), rs1: rs1(raw), shamt })
            }
            0b101 => {
                let shamt = ((raw >> 20) & 0x3F) as u8;
                match (raw >> 26) & 0x3F {
                    0b00_0000 => Ok(Instruction::Srli { rd: rd(raw), rs1: rs1(raw), shamt }),
                    0b01_0000 => Ok(Instruction::Srai { rd: rd(raw), rs1: rs1(raw), shamt }),
                    _         => Ok(Instruction::Illegal { raw }),
                }
            }
            _ => Ok(Instruction::Illegal { raw }),
        },

        // ── AUIPC ─────────────────────────────────────────────────────────────
        0b001_0111 => Ok(Instruction::Auipc { rd: rd(raw), imm: decode_u_imm(raw) }),

        // ── OP-IMM-32 (RV64 word immediate shifts) ───────────────────────────
        0b001_1011 => match f3 {
            0b000 => Ok(Instruction::Addiw { rd: rd(raw), rs1: rs1(raw), imm: decode_i_imm(raw) }),
            0b001 => {
                let shamt = ((raw >> 20) & 0x1F) as u8;
                Ok(Instruction::Slliw { rd: rd(raw), rs1: rs1(raw), shamt })
            }
            0b101 => {
                let shamt = ((raw >> 20) & 0x1F) as u8;
                match f7 {
                    0b000_0000 => Ok(Instruction::Srliw { rd: rd(raw), rs1: rs1(raw), shamt }),
                    0b010_0000 => Ok(Instruction::Sraiw { rd: rd(raw), rs1: rs1(raw), shamt }),
                    _          => Ok(Instruction::Illegal { raw }),
                }
            }
            _ => Ok(Instruction::Illegal { raw }),
        },

        // ── STORE ─────────────────────────────────────────────────────────────
        0b010_0011 => {
            let width = match f3 {
                0b000 => StoreWidth::Byte,
                0b001 => StoreWidth::HalfWord,
                0b010 => StoreWidth::Word,
                0b011 => StoreWidth::DoubleWord,
                _     => return Ok(Instruction::Illegal { raw }),
            };
            Ok(Instruction::Store { rs1: rs1(raw), rs2: rs2(raw), imm: decode_s_imm(raw), width })
        }

        // ── STORE-FP ─────────────────────────────────────────────────────────
        0b010_0111 => match f3 {
            0b010 => Ok(Instruction::Fsw { rs1: rs1(raw), frs2: rs2(raw), imm: decode_s_imm(raw) }),
            0b011 => Ok(Instruction::Fsd { rs1: rs1(raw), frs2: rs2(raw), imm: decode_s_imm(raw) }),
            _     => Ok(Instruction::Illegal { raw }),
        },

        // ── AMO (A extension) ─────────────────────────────────────────────────
        0b010_1111 => decode_amo(raw, f3),

        // ── OP ────────────────────────────────────────────────────────────────
        0b011_0011 => decode_op(raw, f3, f7),

        // ── LUI ───────────────────────────────────────────────────────────────
        0b011_0111 => Ok(Instruction::Lui { rd: rd(raw), imm: decode_u_imm(raw) }),

        // ── OP-32 (M extension word forms) ───────────────────────────────────
        0b011_1011 => decode_op32(raw, f3, f7),

        // ── MADD/MSUB/NMSUB/NMADD (FP fused) ─────────────────────────────────
        0b100_0011 => decode_fma(raw, false, false),   // FMADD
        0b100_0111 => decode_fma(raw, true,  false),   // FMSUB
        0b100_1011 => decode_fma(raw, true,  true),    // FNMSUB
        0b100_1111 => decode_fma(raw, false, true),    // FNMADD

        // ── OP-FP ─────────────────────────────────────────────────────────────
        0b101_0011 => decode_op_fp(raw, f7),

        // ── BRANCH ────────────────────────────────────────────────────────────
        0b110_0011 => {
            let imm = decode_b_imm(raw);
            match f3 {
                0b000 => Ok(Instruction::Beq  { rs1: rs1(raw), rs2: rs2(raw), imm }),
                0b001 => Ok(Instruction::Bne  { rs1: rs1(raw), rs2: rs2(raw), imm }),
                0b100 => Ok(Instruction::Blt  { rs1: rs1(raw), rs2: rs2(raw), imm }),
                0b101 => Ok(Instruction::Bge  { rs1: rs1(raw), rs2: rs2(raw), imm }),
                0b110 => Ok(Instruction::Bltu { rs1: rs1(raw), rs2: rs2(raw), imm }),
                0b111 => Ok(Instruction::Bgeu { rs1: rs1(raw), rs2: rs2(raw), imm }),
                _     => Ok(Instruction::Illegal { raw }),
            }
        }

        // ── JALR ──────────────────────────────────────────────────────────────
        0b110_0111 => match f3 {
            0b000 => Ok(Instruction::Jalr { rd: rd(raw), rs1: rs1(raw), imm: decode_i_imm(raw) }),
            _     => Ok(Instruction::Illegal { raw }),
        },

        // ── JAL ───────────────────────────────────────────────────────────────
        0b110_1111 => Ok(Instruction::Jal { rd: rd(raw), imm: decode_j_imm(raw) }),

        // ── SYSTEM (CSR, ECALL, EBREAK, WFI, MRET, SRET, SFENCE.VMA) ─────────
        0b111_0011 => decode_system(raw, f3),

        _ => Ok(Instruction::Illegal { raw }),
    }
}
```

Sub-decoders (`decode_amo`, `decode_op`, `decode_op32`, `decode_fma`, `decode_op_fp`, `decode_system`) follow the same pattern: `match` on relevant funct bits, return the appropriate `Instruction` variant or `Instruction::Illegal { raw }`.

---

## 5. C Extension Decode — `decode_rv64c`

```rust
/// Decode a 16-bit compressed instruction and return the equivalent 32-bit
/// instruction word. The result can be passed directly to decode_rv64.
///
/// Returns Err(DecodeError::InvalidCompressed) for reserved or unknown encodings.
///
/// C extension quadrants are identified by bits [1:0]:
///   00 = Quadrant 0, 01 = Quadrant 1, 11 = not C (but 11 is not valid here either
///        since 32-bit instructions have bits [1:0] = 11)
pub fn decode_rv64c(raw: u16) -> Result<u32, DecodeError> {
    let quad = raw & 0x3;
    let op   = (raw >> 13) & 0x7;

    match (quad, op) {
        // ── Quadrant 0 ───────────────────────────────────────────────────────
        (0b00, 0b000) => {
            // C.ADDI4SPN → ADDI rd', x2, nzuimm
            // rd' = bits [4:2] + 8 (CL/CS/CB/CIW register prime encoding)
            let rd_prime = ((raw >> 2) & 0x7) as u8 + 8;
            let imm = c_addi4spn_imm(raw);
            if imm == 0 { return Err(DecodeError::InvalidCompressed { raw }); }
            Ok(encode_i(rd_prime, 2, imm as i32, 0b000, 0b001_0011))  // ADDI
        }
        (0b00, 0b010) => {
            // C.LW → LW rd', offset(rs1')
            let rd_prime  = ((raw >> 2) & 0x7) as u8 + 8;
            let rs1_prime = ((raw >> 7) & 0x7) as u8 + 8;
            let imm = c_lw_imm(raw) as i32;
            Ok(encode_i(rd_prime, rs1_prime, imm, 0b010, 0b000_0011))  // LW
        }
        (0b00, 0b011) => {
            // C.LD → LD rd', offset(rs1')
            let rd_prime  = ((raw >> 2) & 0x7) as u8 + 8;
            let rs1_prime = ((raw >> 7) & 0x7) as u8 + 8;
            let imm = c_ld_imm(raw) as i32;
            Ok(encode_i(rd_prime, rs1_prime, imm, 0b011, 0b000_0011))  // LD
        }
        (0b00, 0b110) => {
            // C.SW → SW rs2', offset(rs1')
            let rs2_prime = ((raw >> 2) & 0x7) as u8 + 8;
            let rs1_prime = ((raw >> 7) & 0x7) as u8 + 8;
            let imm = c_lw_imm(raw) as i32;
            Ok(encode_s(rs1_prime, rs2_prime, imm, 0b010, 0b010_0011))  // SW
        }
        (0b00, 0b111) => {
            // C.SD → SD rs2', offset(rs1')
            let rs2_prime = ((raw >> 2) & 0x7) as u8 + 8;
            let rs1_prime = ((raw >> 7) & 0x7) as u8 + 8;
            let imm = c_ld_imm(raw) as i32;
            Ok(encode_s(rs1_prime, rs2_prime, imm, 0b011, 0b010_0011))  // SD
        }

        // ── Quadrant 1 ───────────────────────────────────────────────────────
        (0b01, 0b000) => {
            // C.ADDI → ADDI rd, rd, nzimm  (nzimm != 0)
            let rd = ((raw >> 7) & 0x1F) as u8;
            let imm = c_ci_imm(raw);
            Ok(encode_i(rd, rd, imm, 0b000, 0b001_0011))   // ADDI
        }
        (0b01, 0b001) => {
            // C.ADDIW → ADDIW rd, rd, imm  (rd != 0)
            let rd = ((raw >> 7) & 0x1F) as u8;
            if rd == 0 { return Err(DecodeError::InvalidCompressed { raw }); }
            let imm = c_ci_imm(raw);
            Ok(encode_i(rd, rd, imm, 0b000, 0b001_1011))   // ADDIW
        }
        (0b01, 0b010) => {
            // C.LI → ADDI rd, x0, imm
            let rd = ((raw >> 7) & 0x1F) as u8;
            let imm = c_ci_imm(raw);
            Ok(encode_i(rd, 0, imm, 0b000, 0b001_0011))    // ADDI rd, x0, imm
        }
        (0b01, 0b011) => {
            let rd = ((raw >> 7) & 0x1F) as u8;
            if rd == 2 {
                // C.ADDI16SP → ADDI x2, x2, nzimm*16
                let imm = c_addi16sp_imm(raw);
                Ok(encode_i(2, 2, imm, 0b000, 0b001_0011))
            } else {
                // C.LUI → LUI rd, nzimm
                let imm = c_lui_imm(raw);
                Ok(encode_u(rd, imm as u32, 0b011_0111))   // LUI
            }
        }
        (0b01, 0b100) => decode_rv64c_misc_alu(raw),       // CB-format arithmetic
        (0b01, 0b101) => {
            // C.J → JAL x0, offset
            let imm = c_j_imm(raw);
            Ok(encode_j(0, imm, 0b110_1111))               // JAL x0, offset
        }
        (0b01, 0b110) => {
            // C.BEQZ → BEQ rs1', x0, offset
            let rs1_prime = ((raw >> 7) & 0x7) as u8 + 8;
            let imm = c_b_imm(raw);
            Ok(encode_b(rs1_prime, 0, imm, 0b000, 0b110_0011))  // BEQ
        }
        (0b01, 0b111) => {
            // C.BNEZ → BNE rs1', x0, offset
            let rs1_prime = ((raw >> 7) & 0x7) as u8 + 8;
            let imm = c_b_imm(raw);
            Ok(encode_b(rs1_prime, 0, imm, 0b001, 0b110_0011))  // BNE
        }

        // ── Quadrant 2 ───────────────────────────────────────────────────────
        (0b10, 0b000) => {
            // C.SLLI → SLLI rd, rd, shamt
            let rd = ((raw >> 7) & 0x1F) as u8;
            let shamt = c_ci_imm(raw) as u8 & 0x3F;
            // Encode as SLLI I-type: imm[5:0] = shamt, imm[11:6] = 0
            let imm = shamt as i32;
            Ok(encode_i(rd, rd, imm, 0b001, 0b001_0011))   // SLLI
        }
        (0b10, 0b010) => {
            // C.LWSP → LW rd, offset(x2)
            let rd = ((raw >> 7) & 0x1F) as u8;
            if rd == 0 { return Err(DecodeError::InvalidCompressed { raw }); }
            let imm = c_lwsp_imm(raw) as i32;
            Ok(encode_i(rd, 2, imm, 0b010, 0b000_0011))    // LW
        }
        (0b10, 0b011) => {
            // C.LDSP → LD rd, offset(x2)
            let rd = ((raw >> 7) & 0x1F) as u8;
            if rd == 0 { return Err(DecodeError::InvalidCompressed { raw }); }
            let imm = c_ldsp_imm(raw) as i32;
            Ok(encode_i(rd, 2, imm, 0b011, 0b000_0011))    // LD
        }
        (0b10, 0b100) => {
            let bit12 = (raw >> 12) & 1;
            let rs1   = ((raw >> 7) & 0x1F) as u8;
            let rs2   = ((raw >> 2) & 0x1F) as u8;
            match (bit12, rs1, rs2) {
                (0, rs1, 0) => {
                    // C.JR → JALR x0, rs1, 0
                    Ok(encode_i(0, rs1, 0, 0b000, 0b110_0111))  // JALR
                }
                (0, rd, rs2) => {
                    // C.MV → ADD rd, x0, rs2
                    Ok(encode_r(rd, 0, rs2, 0b000, 0b000_0000, 0b011_0011))  // ADD
                }
                (1, 0, 0) => {
                    // C.EBREAK
                    Ok(0x00100073)  // raw encoding of EBREAK
                }
                (1, rs1, 0) => {
                    // C.JALR → JALR x1, rs1, 0
                    Ok(encode_i(1, rs1, 0, 0b000, 0b110_0111))
                }
                (1, rd, rs2) => {
                    // C.ADD → ADD rd, rd, rs2
                    Ok(encode_r(rd, rd, rs2, 0b000, 0b000_0000, 0b011_0011))
                }
                _ => Err(DecodeError::InvalidCompressed { raw }),
            }
        }
        (0b10, 0b110) => {
            // C.SWSP → SW rs2, offset(x2)
            let rs2 = ((raw >> 2) & 0x1F) as u8;
            let imm = c_swsp_imm(raw) as i32;
            Ok(encode_s(2, rs2, imm, 0b010, 0b010_0011))
        }
        (0b10, 0b111) => {
            // C.SDSP → SD rs2, offset(x2)
            let rs2 = ((raw >> 2) & 0x1F) as u8;
            let imm = c_sdsp_imm(raw) as i32;
            Ok(encode_s(2, rs2, imm, 0b011, 0b010_0011))
        }

        _ => Err(DecodeError::InvalidCompressed { raw }),
    }
}

// ── Encoding helpers (reconstitute 32-bit instruction words) ─────────────────

fn encode_i(rd: u8, rs1: u8, imm: i32, funct3: u8, opcode: u32) -> u32 {
    let imm = (imm as u32) & 0xFFF;
    (imm << 20) | ((rs1 as u32) << 15) | ((funct3 as u32) << 12) | ((rd as u32) << 7) | opcode
}

fn encode_s(rs1: u8, rs2: u8, imm: i32, funct3: u8, opcode: u32) -> u32 {
    let imm = (imm as u32) & 0xFFF;
    let hi = (imm >> 5) & 0x7F;
    let lo = imm & 0x1F;
    (hi << 25) | ((rs2 as u32) << 20) | ((rs1 as u32) << 15) | ((funct3 as u32) << 12) | (lo << 7) | opcode
}

fn encode_r(rd: u8, rs1: u8, rs2: u8, funct3: u8, funct7: u8, opcode: u32) -> u32 {
    ((funct7 as u32) << 25) | ((rs2 as u32) << 20) | ((rs1 as u32) << 15)
    | ((funct3 as u32) << 12) | ((rd as u32) << 7) | opcode
}

fn encode_j(rd: u8, imm: i32, opcode: u32) -> u32 {
    let imm = (imm as u32) & 0x1FFFFE;
    let imm20    = (imm >> 20) & 1;
    let imm10_1  = (imm >> 1)  & 0x3FF;
    let imm11    = (imm >> 11) & 1;
    let imm19_12 = (imm >> 12) & 0xFF;
    (imm20 << 31) | (imm10_1 << 21) | (imm11 << 20) | (imm19_12 << 12) | ((rd as u32) << 7) | opcode
}

fn encode_b(rs1: u8, rs2: u8, imm: i32, funct3: u8, opcode: u32) -> u32 {
    let imm = (imm as u32) & 0x1FFE;
    let imm12   = (imm >> 12) & 1;
    let imm11   = (imm >> 11) & 1;
    let imm10_5 = (imm >> 5)  & 0x3F;
    let imm4_1  = (imm >> 1)  & 0xF;
    (imm12 << 31) | (imm10_5 << 25) | ((rs2 as u32) << 20) | ((rs1 as u32) << 15)
    | ((funct3 as u32) << 12) | (imm4_1 << 8) | (imm11 << 7) | opcode
}

fn encode_u(rd: u8, imm: u32, opcode: u32) -> u32 {
    (imm & 0xFFFFF000) | ((rd as u32) << 7) | opcode
}
```

### C extension immediate decoders

```rust
// C.ADDI4SPN immediate: bits scattered across [12:5] of the 16-bit word
fn c_addi4spn_imm(raw: u16) -> i32 {
    let b = raw as u32;
    let imm = ((b >> 6) & 0x4) | ((b >> 4) & 0x38) | ((b >> 2) & 0x1) | ((b >> 1) & 0x2);
    // Scale by 4 (always byte-aligned stack offset)
    (imm << 2) as i32
}

// C.LW/C.SW offset
fn c_lw_imm(raw: u16) -> u32 {
    let b = raw as u32;
    ((b >> 7) & 0x38) | ((b >> 4) & 0x4) | ((b << 1) & 0x40)
    // actually: [5:3] from [12:10], [2] from [6], [6] from [5]
    // simpler: ((b >> 4) & 0x4) | ((b >> 7) & 0x38) | ((b << 1) & 0x40)
}

// C.LD/C.SD offset (scale by 8)
fn c_ld_imm(raw: u16) -> u32 {
    let b = raw as u32;
    ((b >> 7) & 0x38) | ((b << 1) & 0xC0)
}

// CI-format immediate (sign-extended 6-bit)
fn c_ci_imm(raw: u16) -> i32 {
    let b = raw as u32;
    let imm = ((b >> 7) & 0x20) | ((b >> 2) & 0x1F);
    (((imm as i32) << 26) >> 26)  // sign-extend from bit 5
}

// C.ADDI16SP immediate (scale by 16, sign-extended)
fn c_addi16sp_imm(raw: u16) -> i32 {
    let b = raw as u32;
    let imm = ((b >> 3) & 0x200) | ((b >> 2) & 0x10) | ((b << 1) & 0x40)
             | ((b << 4) & 0x180) | ((b << 3) & 0x20);
    (((imm as i32) << 22) >> 22)
}

// C.LUI immediate (upper 20 bits, shifted left 12)
fn c_lui_imm(raw: u16) -> i32 {
    let b = raw as u32;
    let imm = ((b >> 7) & 0x20) | ((b >> 2) & 0x1F);
    ((imm as i32) << 26) >> 14   // sign-extend then shift left 12
}

// C.J / C.JAL offset
fn c_j_imm(raw: u16) -> i32 {
    let b = raw as u32;
    let imm = ((b >> 1) & 0x800) | ((b >> 7) & 0x10) | ((b >> 1) & 0x300)
             | ((b << 2) & 0x400) | ((b >> 1) & 0x40) | ((b << 1) & 0x80)
             | ((b >> 2) & 0xE) | ((b << 3) & 0x20);
    (((imm as i32) << 20) >> 20)  // sign-extend from bit 11
}

// C.BEQZ / C.BNEZ offset
fn c_b_imm(raw: u16) -> i32 {
    let b = raw as u32;
    let imm = ((b >> 4) & 0x100) | ((b >> 7) & 0x18) | ((b << 1) & 0xC0)
             | ((b >> 2) & 0x6) | ((b << 3) & 0x20);
    (((imm as i32) << 23) >> 23)
}

fn c_lwsp_imm(raw: u16) -> u32 {
    let b = raw as u32;
    ((b >> 7) & 0x20) | ((b >> 2) & 0x1C) | ((b << 4) & 0xC0)
}
fn c_ldsp_imm(raw: u16) -> u32 {
    let b = raw as u32;
    ((b >> 7) & 0x20) | ((b >> 2) & 0x18) | ((b << 4) & 0x1C0)
}
fn c_swsp_imm(raw: u16) -> u32 {
    let b = raw as u32;
    ((b >> 7) & 0x3C) | ((b >> 1) & 0xC0)
}
fn c_sdsp_imm(raw: u16) -> u32 {
    let b = raw as u32;
    ((b >> 7) & 0x38) | ((b >> 1) & 0x1C0)
}
```

---

## 6. Illegal Instruction Detection

An instruction returns `Instruction::Illegal { raw }` (not a `DecodeError`) in these cases:

- Known opcode, but `funct3`/`funct7` combination is reserved
- Valid opcode group, but all-zero encoding within it
- `HINT` instructions (functionally NOPs; treated as illegal for simplicity)
- C extension: `rd == 0` where prohibited, or `imm == 0` where `nzimm` is required

The `execute` function raises `HartException::IllegalInstruction { raw }` when it encounters `Instruction::Illegal`.

---

## 7. Module Layout

```
riscv/
├── mod.rs          — pub use
├── insn.rs         — Instruction, LoadWidth, StoreWidth
├── decode.rs       — decode_rv64, all sub-decoders, bit helpers
├── compress.rs     — decode_rv64c, immediate decoders, encode_* helpers
├── execute.rs      — execute(insn, ctx)
├── csr.rs          — CSR address constants, side-effect dispatch
└── exception.rs    — HartException, DecodeError, mcause conversion
```
