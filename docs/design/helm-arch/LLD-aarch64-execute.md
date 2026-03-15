# helm-arch — LLD: AArch64 Execute

> **Status:** Draft — Phase 2 target
> **Covers:** `aarch64::execute`, `aarch64::flags`, condition codes, all encoding group semantics

---

## 1. Execute Function Signature

```rust
/// Execute a single decoded AArch64 instruction against the given context.
///
/// # Returns
/// - `Ok(())` — instruction executed; ctx state updated; PC advanced by 4.
/// - `Err(HartException)` — trap raised; PC is NOT advanced; engine handles it.
///
/// # Key AArch64 correctness rules enforced here
/// - W register writes zero-extend into the full 64-bit Xn (not preserve upper bits).
/// - Register 31 is XZR (zero) as source, or SP as base in addressing — depending on
///   instruction field. This context-sensitivity is resolved by the enum variant field
///   names (rt/rd for data, rn for base address).
/// - XZR as destination: writes are silently discarded.
/// - NZCV flags: only set by flag-setting variants (ADDS, SUBS, ANDS, etc.), not by
///   plain ADD, SUB, AND.
pub fn execute_a64<C: ExecContext>(insn: Aarch64Instruction, ctx: &mut C) -> Result<(), HartException> {
    match insn {
        // ... all arms below
    }
}
```

---

## 2. Register Read/Write Helpers

### XZR / SP Disambiguation

Register index 31 means XZR (zero) for data processing and load/store `rt`/`rd` fields, and SP for base register (`rn`) fields in load/store instructions. The enum variant carries the right field name by convention:

- `rd`, `rn`, `rm`, `ra`, `rt`, `rt1`, `rt2`: data registers — index 31 = XZR (read 0, discard write)
- Base register in load/store addressing: index 31 = SP (tracked separately as `ctx.read_sp()`)

```rust
/// Read a general-purpose register. Index 31 = XZR (always 0).
/// For 32-bit mode (sf=false), result is zero-extended from 32 bits.
#[inline(always)]
fn read_gpr(ctx: &impl ExecContext, idx: u8, sf: bool) -> u64 {
    let val = if idx == 31 { 0 } else { ctx.read_int_reg(idx as usize) };
    if sf { val } else { val & 0xFFFF_FFFF }
}

/// Write a general-purpose register. Index 31 = XZR (discard).
/// For 32-bit mode (sf=false), value is zero-extended before storing (W-register rule).
#[inline(always)]
fn write_gpr(ctx: &mut impl ExecContext, idx: u8, val: u64, sf: bool) {
    if idx == 31 { return; }
    let stored = if sf { val } else { val & 0xFFFF_FFFF };  // W reg: zero-extend
    ctx.write_int_reg(idx as usize, stored);
}

/// Read the stack pointer (SP_EL0 or SP_EL1 based on PSTATE.SP).
#[inline(always)]
fn read_sp(ctx: &impl ExecContext) -> u64 {
    ctx.read_sp()
}

/// Read base register: index 31 = SP (not XZR). Used for load/store addressing.
#[inline(always)]
fn read_base(ctx: &impl ExecContext, rn: u8) -> u64 {
    if rn == 31 { ctx.read_sp() } else { ctx.read_int_reg(rn as usize) }
}

/// Write base register (post/pre-index update): index 31 = SP update.
#[inline(always)]
fn write_base(ctx: &mut impl ExecContext, rn: u8, val: u64) {
    if rn == 31 { ctx.write_sp(val) } else { ctx.write_int_reg(rn as usize, val) }
}
```

---

## 3. NZCV Flag Helpers

All flag-setting instructions use these helpers. They are defined in `aarch64::flags`.

### `add_with_carry`

The fundamental operation underlying ADD, ADDS, ADC, ADCS, SUB (as ADD with carry-in 1 and inverted second operand), SUBS, SBC, SBCS, CSEL, CMP, CMN.

```rust
/// Perform `result = x + y + carry_in` and compute NZCV flags.
///
/// The ARM DDI 0487 pseudocode `AddWithCarry(x, y, carry_in)` is:
///   unsigned_sum = UInt(x) + UInt(y) + UInt(carry_in)
///   signed_sum   = SInt(x) + SInt(y) + UInt(carry_in)
///   result = unsigned_sum[N-1:0]
///   N = result[N-1]   (sign bit)
///   Z = IsZero(result)
///   C = unsigned_sum[N]    (unsigned overflow = carry out)
///   V = signed_sum != SInt(result)  (signed overflow)
///
/// Returns (result, nzcv_bits) where nzcv_bits is N:Z:C:V in bits [3:0].
pub fn add_with_carry(x: u64, y: u64, carry_in: u64, sf: bool) -> (u64, u8) {
    if sf {
        // 64-bit operation
        let (r1, c1) = x.overflowing_add(y);
        let (result, c2) = r1.overflowing_add(carry_in);
        let n = ((result >> 63) & 1) as u8;
        let z = if result == 0 { 1u8 } else { 0 };
        let c = if c1 || c2 { 1u8 } else { 0 };
        // Signed overflow: sign(x) == sign(y) && sign(result) != sign(x)
        let v = (!(x ^ y) & (x ^ result)) >> 63;
        (result, (n << 3) | (z << 2) | (c << 1) | (v as u8))
    } else {
        // 32-bit operation: work in 32 bits, zero-extend result
        let x = (x & 0xFFFF_FFFF) as u32;
        let y = (y & 0xFFFF_FFFF) as u32;
        let carry = carry_in as u32;
        let (r1, c1) = x.overflowing_add(y);
        let (result32, c2) = r1.overflowing_add(carry);
        let result = result32 as u64;
        let n = ((result >> 31) & 1) as u8;
        let z = if result32 == 0 { 1u8 } else { 0 };
        let c = if c1 || c2 { 1u8 } else { 0 };
        let v = (!(x ^ y) & (x ^ result32)) >> 31;
        (result, (n << 3) | (z << 2) | (c << 1) | (v as u8))
    }
}
```

### `check_cond` — Condition Code Evaluation

```rust
/// Evaluate a 4-bit AArch64 condition code against the current NZCV flags.
///
/// NZCV is packed as bits [3:0]: N=bit3, Z=bit2, C=bit1, V=bit0.
/// Condition codes (ARM DDI 0487 Table C1-1):
///   0b0000 EQ — Z==1         0b0001 NE — Z==0
///   0b0010 CS — C==1         0b0011 CC — C==0
///   0b0100 MI — N==1         0b0101 PL — N==0
///   0b0110 VS — V==1         0b0111 VC — V==0
///   0b1000 HI — C==1 && Z==0 0b1001 LS — C==0 || Z==1
///   0b1010 GE — N==V         0b1011 LT — N!=V
///   0b1100 GT — Z==0 && N==V 0b1101 LE — Z==1 || N!=V
///   0b1110 AL — always true  0b1111 NV — always true (UNPREDICTABLE in most uses)
pub fn check_cond(nzcv: u8, cond: u8) -> bool {
    let n = (nzcv >> 3) & 1;
    let z = (nzcv >> 2) & 1;
    let c = (nzcv >> 1) & 1;
    let v = nzcv & 1;

    let result = match cond >> 1 {
        0b000 => z == 1,           // EQ / NE
        0b001 => c == 1,           // CS / CC
        0b010 => n == 1,           // MI / PL
        0b011 => v == 1,           // VS / VC
        0b100 => c == 1 && z == 0, // HI / LS
        0b101 => n == v,           // GE / LT
        0b110 => z == 0 && n == v, // GT / LE
        0b111 => true,             // AL / NV
        _ => unreachable!(),
    };
    // Odd condition codes are the inverse of even ones (except AL/NV).
    if cond & 1 == 1 && cond != 0b1111 { !result } else { result }
}
```

### `update_nzcv`

```rust
/// Write NZCV bits to PSTATE. Only flag-setting instructions call this.
#[inline(always)]
fn update_nzcv(ctx: &mut impl ExecContext, nzcv: u8) {
    ctx.write_nzcv(nzcv);
}
```

---

## 4. Data Processing — Immediate Execute

### ADD/SUB Immediate

```rust
Aarch64Instruction::AddImm { sf, rd, rn, imm, shift } => {
    let operand1 = read_gpr(ctx, rn, sf);
    let operand2 = if shift == 12 { (imm as u64) << 12 } else { imm as u64 };
    let (result, _) = add_with_carry(operand1, operand2, 0, sf);
    write_gpr(ctx, rd, result, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::AddsImm { sf, rd, rn, imm, shift } => {
    let operand1 = read_gpr(ctx, rn, sf);
    let operand2 = if shift == 12 { (imm as u64) << 12 } else { imm as u64 };
    let (result, nzcv) = add_with_carry(operand1, operand2, 0, sf);
    write_gpr(ctx, rd, result, sf);
    update_nzcv(ctx, nzcv);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// SubImm: same as AddImm but with bitwise NOT of operand2 and carry_in=1
// (two's-complement subtraction = add with inverted operand and carry in 1).
Aarch64Instruction::SubImm { sf, rd, rn, imm, shift } => {
    let operand1 = read_gpr(ctx, rn, sf);
    let operand2 = if shift == 12 { (imm as u64) << 12 } else { imm as u64 };
    let (result, _) = add_with_carry(operand1, !operand2, 1, sf);
    write_gpr(ctx, rd, result, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::SubsImm { sf, rd, rn, imm, shift } => {
    let operand1 = read_gpr(ctx, rn, sf);
    let operand2 = if shift == 12 { (imm as u64) << 12 } else { imm as u64 };
    let (result, nzcv) = add_with_carry(operand1, !operand2, 1, sf);
    write_gpr(ctx, rd, result, sf);
    update_nzcv(ctx, nzcv);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### Move Wide

```rust
Aarch64Instruction::Movz { sf, rd, imm16, hw } => {
    // Zero the register and place imm16 at position hw*16.
    let val = (imm16 as u64) << (hw * 16);
    write_gpr(ctx, rd, val, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::Movn { sf, rd, imm16, hw } => {
    // Move inverted immediate.
    let val = !((imm16 as u64) << (hw * 16));
    write_gpr(ctx, rd, val, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::Movk { sf, rd, imm16, hw } => {
    // Keep other bits; insert imm16 at hw*16 position.
    let shift = hw * 16;
    let mask  = !(0xFFFFu64 << shift);
    let old   = read_gpr(ctx, rd, sf);
    let val   = (old & mask) | ((imm16 as u64) << shift);
    write_gpr(ctx, rd, val, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### Logical Immediate

```rust
Aarch64Instruction::AndImm { sf, rd, rn, imm } => {
    let result = read_gpr(ctx, rn, sf) & imm;
    write_gpr(ctx, rd, result, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::AndsImm { sf, rd, rn, imm } => {
    let result = read_gpr(ctx, rn, sf) & imm;
    write_gpr(ctx, rd, result, sf);
    // ANDS sets N and Z; C and V are cleared.
    let n = if sf { (result >> 63) as u8 & 1 } else { (result >> 31) as u8 & 1 };
    let z = if result == 0 { 1u8 } else { 0 };
    update_nzcv(ctx, (n << 3) | (z << 2));
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// OrrImm, EorImm: same as AndImm with different operators, no flag setting.
```

### Bitfield (SBFM / BFM / UBFM)

SBFM/UBFM/BFM are general bitfield operations. Common aliases: LSL, LSR, ASR, SXTB, SXTH, SXTW, UXTB, UXTH are encoded as specific SBFM/UBFM variants.

```rust
Aarch64Instruction::Ubfm { sf, rd, rn, immr, imms } => {
    // UBFM: unsigned bitfield move. No sign extension.
    let src = read_gpr(ctx, rn, sf);
    let width = if sf { 64u32 } else { 32u32 };
    let result = if imms >= immr {
        // Extract bits [imms:immr] from src, place at bits [(imms-immr):0].
        (src >> immr) & ((1u64 << (imms - immr + 1)) - 1)
    } else {
        // Rotate right by immr, mask to imms+1 bits.
        let rotated = src.rotate_right(immr as u32);
        rotated & ((1u64 << (imms + 1)) - 1)
    };
    write_gpr(ctx, rd, result, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::Sbfm { sf, rd, rn, immr, imms } => {
    // SBFM: sign-extend the extracted field.
    let src = read_gpr(ctx, rn, sf) as i64;
    let width = if sf { 64i32 } else { 32i32 };
    // Extract field, sign-extend.
    let result = if imms >= immr {
        let field_len = (imms - immr + 1) as i32;
        let shift = 64 - field_len;
        ((src >> immr) << shift >> shift) as u64
    } else {
        // EXTS: sign-extend bit imms of src to full width.
        let field_len = (imms + 1) as i32;
        let shift = 64 - field_len;
        (src << shift >> (shift + immr as i32)) as u64
    };
    write_gpr(ctx, rd, result, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

---

## 5. Data Processing — Register Execute

### Shift Application Helper

```rust
#[inline(always)]
fn apply_shift(val: u64, shift: ShiftType, amount: u8, sf: bool) -> u64 {
    let amount = amount as u32;
    let mask = if sf { u64::MAX } else { 0xFFFF_FFFF };
    let val = val & mask;
    (match shift {
        ShiftType::Lsl => val << amount,
        ShiftType::Lsr => val >> amount,
        ShiftType::Asr => ((val as i64) >> amount) as u64,
        ShiftType::Ror => val.rotate_right(amount),
    }) & mask
}
```

### Extend Application Helper

```rust
#[inline(always)]
fn apply_extend(val: u64, extend: ExtendType, shift: u8) -> u64 {
    let extended = match extend {
        ExtendType::Uxtb => val & 0xFF,
        ExtendType::Uxth => val & 0xFFFF,
        ExtendType::Uxtw => val & 0xFFFF_FFFF,
        ExtendType::Uxtx => val,
        ExtendType::Sxtb => (val as i8)  as i64 as u64,
        ExtendType::Sxth => (val as i16) as i64 as u64,
        ExtendType::Sxtw => (val as i32) as i64 as u64,
        ExtendType::Sxtx => val,
        ExtendType::Lsl  => val,
    };
    extended << shift
}
```

### ADD/SUB Register

```rust
Aarch64Instruction::AddReg { sf, rd, rn, rm, shift, amount } => {
    let op1 = read_gpr(ctx, rn, sf);
    let op2 = apply_shift(read_gpr(ctx, rm, sf), shift, amount, sf);
    let (result, _) = add_with_carry(op1, op2, 0, sf);
    write_gpr(ctx, rd, result, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::AddsReg { sf, rd, rn, rm, shift, amount } => {
    let op1 = read_gpr(ctx, rn, sf);
    let op2 = apply_shift(read_gpr(ctx, rm, sf), shift, amount, sf);
    let (result, nzcv) = add_with_carry(op1, op2, 0, sf);
    write_gpr(ctx, rd, result, sf);
    update_nzcv(ctx, nzcv);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// SubReg: op2 = NOT(shifted_rm), carry_in=1.
Aarch64Instruction::SubReg { sf, rd, rn, rm, shift, amount } => {
    let op1 = read_gpr(ctx, rn, sf);
    let op2 = apply_shift(read_gpr(ctx, rm, sf), shift, amount, sf);
    let (result, _) = add_with_carry(op1, !op2, 1, sf);
    write_gpr(ctx, rd, result, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### Multiply

```rust
Aarch64Instruction::Madd { sf, rd, rn, rm, ra } => {
    // Xd = Xa + Xn * Xm  (MUL is MADD with ra=XZR)
    let a = read_gpr(ctx, ra, sf);
    let n = read_gpr(ctx, rn, sf);
    let m = read_gpr(ctx, rm, sf);
    write_gpr(ctx, rd, a.wrapping_add(n.wrapping_mul(m)), sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::Smulh { rd, rn, rm } => {
    // Upper 64 bits of signed 64×64 product.
    let a = ctx.read_int_reg(rn as usize) as i64 as i128;
    let b = ctx.read_int_reg(rm as usize) as i64 as i128;
    let result = ((a * b) >> 64) as u64;
    write_gpr(ctx, rd, result, true);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::Umulh { rd, rn, rm } => {
    let a = ctx.read_int_reg(rn as usize) as u128;
    let b = ctx.read_int_reg(rm as usize) as u128;
    let result = ((a * b) >> 64) as u64;
    write_gpr(ctx, rd, result, true);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### Divide

```rust
Aarch64Instruction::Sdiv { sf, rd, rn, rm } => {
    let a = read_gpr(ctx, rn, sf) as i64;
    let b = read_gpr(ctx, rm, sf) as i64;
    // AArch64: division by zero returns 0, no exception.
    let result = if b == 0 {
        0i64
    } else if a == i64::MIN && b == -1 {
        i64::MIN   // overflow: return MIN
    } else {
        a / b      // truncate toward zero
    };
    write_gpr(ctx, rd, result as u64, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::Udiv { sf, rd, rn, rm } => {
    let a = read_gpr(ctx, rn, sf);
    let b = read_gpr(ctx, rm, sf);
    let result = if b == 0 { 0 } else { a / b };
    write_gpr(ctx, rd, result, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### Conditional Select

```rust
Aarch64Instruction::Csel { sf, rd, rn, rm, cond } => {
    let nzcv = ctx.read_nzcv();
    let result = if check_cond(nzcv, cond) {
        read_gpr(ctx, rn, sf)
    } else {
        read_gpr(ctx, rm, sf)
    };
    write_gpr(ctx, rd, result, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::Csinc { sf, rd, rn, rm, cond } => {
    let nzcv = ctx.read_nzcv();
    // CSINC rd, rn, rm, cond: rd = cond ? rn : rm+1.
    // CINC rd, rn, cond is CSINC rd, rn, rn, invert(cond).
    // CSET rd, cond is CSINC rd, XZR, XZR, invert(cond).
    let result = if check_cond(nzcv, cond) {
        read_gpr(ctx, rn, sf)
    } else {
        read_gpr(ctx, rm, sf).wrapping_add(1)
    };
    write_gpr(ctx, rd, result, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

---

## 6. Loads and Stores

### Address Computation

```rust
// Unsigned offset (LDR Xt, [Xn, #imm]):
let addr = read_base(ctx, rn).wrapping_add(offset as u64 * size_bytes);

// Signed offset (pre/post-index):
let base = read_base(ctx, rn);
let addr = base.wrapping_add(simm as i64 as u64);

// Pre-index: update Xn = addr, then access addr.
// Post-index: access base, then update Xn = base + simm.
```

### Load Variants

```rust
Aarch64Instruction::Ldr { size, rt, rn, offset } => {
    let size_bytes = size.bytes();
    let addr = read_base(ctx, rn).wrapping_add(offset as u64 * size_bytes as u64);
    let raw = ctx.read_mem(addr, size_bytes)?;
    let val = match size {
        LdStSize::Byte      => raw & 0xFF,
        LdStSize::HalfWord  => raw & 0xFFFF,
        LdStSize::Word      => raw & 0xFFFF_FFFF,
        LdStSize::DoubleWord => raw,
        LdStSize::QuadWord  => raw,  // SIMD 128-bit handled separately
    };
    write_gpr(ctx, rt, val, size != LdStSize::Word);  // Word → Wt (32-bit)
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// LDRSB: byte, sign-extend to 32 or 64 bits.
Aarch64Instruction::Ldrsb { sf, rt, rn, offset } => {
    let addr = read_base(ctx, rn).wrapping_add(offset as u64);
    let raw = ctx.read_mem(addr, 1)?;
    let val = if sf {
        (raw as i8) as i64 as u64
    } else {
        (raw as i8) as i32 as u64
    };
    write_gpr(ctx, rt, val, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// LDRSW: word, sign-extend to 64 bits.
Aarch64Instruction::Ldrsw { rt, rn, offset } => {
    let addr = read_base(ctx, rn).wrapping_add(offset as u64 * 4);
    let raw = ctx.read_mem(addr, 4)?;
    let val = (raw as i32) as i64 as u64;
    write_gpr(ctx, rt, val, true);  // always 64-bit result
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### Pre/Post-Index

```rust
Aarch64Instruction::LdrPre { size, rt, rn, simm } => {
    let base = read_base(ctx, rn);
    let addr = base.wrapping_add(simm as i64 as u64);
    // Update base register first, then load.
    write_base(ctx, rn, addr);
    let raw = ctx.read_mem(addr, size.bytes())?;
    write_gpr(ctx, rt, raw, size.is_64bit());
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::LdrPost { size, rt, rn, simm } => {
    let addr = read_base(ctx, rn);
    // Load from original base, then update.
    let raw = ctx.read_mem(addr, size.bytes())?;
    write_gpr(ctx, rt, raw, size.is_64bit());
    write_base(ctx, rn, addr.wrapping_add(simm as i64 as u64));
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### Load Pair

```rust
Aarch64Instruction::Ldp { size, rt1, rt2, rn, simm, mode } => {
    let size_bytes = size.bytes();
    let base = read_base(ctx, rn);
    let (addr, wback_addr) = match mode {
        PairMode::Offset    => (base.wrapping_add(simm as i64 as u64 * size_bytes as u64), None),
        PairMode::PreIndex  => {
            let a = base.wrapping_add(simm as i64 as u64 * size_bytes as u64);
            (a, Some(a))
        }
        PairMode::PostIndex => (base, Some(base.wrapping_add(simm as i64 as u64 * size_bytes as u64))),
    };
    let val1 = ctx.read_mem(addr, size_bytes)?;
    let val2 = ctx.read_mem(addr.wrapping_add(size_bytes as u64), size_bytes)?;
    write_gpr(ctx, rt1, val1, size.is_64bit());
    write_gpr(ctx, rt2, val2, size.is_64bit());
    if let Some(wb) = wback_addr { write_base(ctx, rn, wb); }
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### Register-Offset Addressing

```rust
Aarch64Instruction::LdrReg { size, rt, rn, rm, extend, amount } => {
    let base  = read_base(ctx, rn);
    let index = apply_extend(read_gpr(ctx, rm, true), extend, amount);
    let addr  = base.wrapping_add(index);
    let raw   = ctx.read_mem(addr, size.bytes())?;
    write_gpr(ctx, rt, raw, size.is_64bit());
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### Load-Acquire / Store-Release / Exclusive

```rust
Aarch64Instruction::Ldaxr { size, rt, rn } => {
    // Load-acquire exclusive: set the exclusive monitor for this address.
    let addr = read_base(ctx, rn);
    let raw  = ctx.read_mem(addr, size.bytes())?;
    write_gpr(ctx, rt, raw, size.is_64bit());
    ctx.set_exclusive_monitor(Some(addr));
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::Stlxr { size, rs, rt, rn } => {
    // Store-release exclusive: check monitor, rs = 0 on success, 1 on failure.
    let addr = read_base(ctx, rn);
    let success = ctx.get_exclusive_monitor() == Some(addr);
    if success {
        let val = read_gpr(ctx, rt, size.is_64bit());
        ctx.write_mem(addr, size.bytes(), val)?;
        write_gpr(ctx, rs, 0, true);
    } else {
        write_gpr(ctx, rs, 1, true);
    }
    ctx.set_exclusive_monitor(None);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

---

## 7. Branches and Control Flow

### Unconditional Branch

```rust
Aarch64Instruction::B { imm } => {
    let pc = ctx.read_pc();
    ctx.write_pc(pc.wrapping_add(imm as i64 as u64));
    Ok(())
}
Aarch64Instruction::Bl { imm } => {
    let pc = ctx.read_pc();
    ctx.write_int_reg(30, pc.wrapping_add(4));  // X30 = return address
    ctx.write_pc(pc.wrapping_add(imm as i64 as u64));
    Ok(())
}
Aarch64Instruction::Br  { rn } => {
    ctx.write_pc(ctx.read_int_reg(rn as usize));
    Ok(())
}
Aarch64Instruction::Blr { rn } => {
    let target = ctx.read_int_reg(rn as usize);
    ctx.write_int_reg(30, ctx.read_pc().wrapping_add(4));
    ctx.write_pc(target);
    Ok(())
}
Aarch64Instruction::Ret { rn } => {
    // Default: RET = BR X30. The rn field allows RET X29, etc.
    ctx.write_pc(ctx.read_int_reg(rn as usize));
    Ok(())
}
```

### Conditional Branch

```rust
Aarch64Instruction::BCond { cond, imm } => {
    let pc   = ctx.read_pc();
    let nzcv = ctx.read_nzcv();
    let target = if check_cond(nzcv, cond) {
        pc.wrapping_add(imm as i64 as u64)
    } else {
        pc.wrapping_add(4)
    };
    ctx.write_pc(target);
    Ok(())
}
```

### Compare and Branch

```rust
Aarch64Instruction::Cbz { sf, rt, imm } => {
    let pc  = ctx.read_pc();
    let val = read_gpr(ctx, rt, sf);
    ctx.write_pc(if val == 0 {
        pc.wrapping_add(imm as i64 as u64)
    } else {
        pc.wrapping_add(4)
    });
    Ok(())
}
Aarch64Instruction::Cbnz { sf, rt, imm } => {
    let pc  = ctx.read_pc();
    let val = read_gpr(ctx, rt, sf);
    ctx.write_pc(if val != 0 {
        pc.wrapping_add(imm as i64 as u64)
    } else {
        pc.wrapping_add(4)
    });
    Ok(())
}
Aarch64Instruction::Tbz { rt, bit, imm } => {
    let pc  = ctx.read_pc();
    let val = ctx.read_int_reg(rt as usize);
    let tst = (val >> bit) & 1;
    ctx.write_pc(if tst == 0 {
        pc.wrapping_add(imm as i64 as u64)
    } else {
        pc.wrapping_add(4)
    });
    Ok(())
}
```

---

## 8. System Instructions

### SVC — Supervisor Call

```rust
Aarch64Instruction::Svc { imm16 } => {
    // Raise a synchronous exception to EL1. In SE mode, the engine intercepts
    // this and dispatches to SyscallHandler. In FS mode, the simulated kernel
    // exception handler runs.
    Err(HartException::Svc { imm16, pc: ctx.read_pc() })
}
```

### ERET — Exception Return

```rust
Aarch64Instruction::Eret => {
    // Restore PC from ELR_EL1, PSTATE from SPSR_EL1.
    // Only valid from EL1 or higher; raises UndefinedException from EL0.
    if ctx.current_el() == 0 {
        return Err(HartException::UndefinedException { pc: ctx.read_pc() });
    }
    let elr   = ctx.read_sysreg(SysregEncoding::ELR_EL1)?;
    let spsr  = ctx.read_sysreg(SysregEncoding::SPSR_EL1)?;
    // Restore PSTATE from SPSR.
    ctx.write_nzcv(((spsr >> 28) & 0xF) as u8);
    ctx.set_current_el(((spsr >> 2) & 0x3) as u8);    // EL field from SPSR
    ctx.write_pc(elr);
    Ok(())
}
```

### MRS / MSR — System Register Access

```rust
Aarch64Instruction::Mrs { rt, sysreg } => {
    let val = ctx.read_sysreg(sysreg)?;
    write_gpr(ctx, rt, val, true);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::Msr { sysreg, rt } => {
    let val = read_gpr(ctx, rt, true);
    ctx.write_sysreg(sysreg, val)?;
    // Post-write side effects (e.g., SCTLR_EL1.M → MMU enable).
    sysreg_post_write_effects(sysreg, val, ctx);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

#### System Register Side Effects

```rust
fn sysreg_post_write_effects<C: ExecContext>(sysreg: SysregEncoding, val: u64, ctx: &mut C) {
    match (sysreg.op0, sysreg.op1, sysreg.crn, sysreg.crm, sysreg.op2) {
        // TTBR0_EL1 or TTBR1_EL1 write → TLB flush required.
        (3, 0, 2, 0, 0) | (3, 0, 2, 0, 1) => {
            ctx.tlb_flush_all();
        }
        // SCTLR_EL1 write → MMU enable/disable, cache enable/disable.
        // In Phase 0 SE mode: MMU is always off; SCTLR writes are recorded but
        // no functional change to address translation.
        (3, 0, 1, 0, 0) => {
            // Future: ctx.set_mmu_enabled(val & 1 != 0);
        }
        // CPACR_EL1 write → SIMD/FP access at EL0/EL1 enabled/disabled.
        // In Phase 0: always enabled for SE mode.
        (3, 0, 1, 0, 2) => {}
        // DAIF write → interrupt mask. In SE mode: no interrupts, so no-op.
        (3, 3, 4, 2, 1) => {}
        _ => {}
    }
}
```

### WFI

```rust
Aarch64Instruction::Wfi => {
    // Wait for interrupt. In SE mode: NOP. In FS mode: suspend hart.
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### Memory Barriers

```rust
Aarch64Instruction::Dsb { option: _ } | Aarch64Instruction::Dmb { option: _ } => {
    // In a single-hart ISS, all memory barriers are no-ops: there is no
    // instruction reordering and no other observers.
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Aarch64Instruction::Isb { option: _ } => {
    // Instruction synchronization barrier. In Phase 0: no I-cache to flush.
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### BRK — Breakpoint

```rust
Aarch64Instruction::Brk { imm16 } => {
    Err(HartException::Breakpoint { pc: ctx.read_pc(), imm16 })
}
```

---

## 9. FP Scalar Execute

AArch64 FP instructions use `ftype` to select precision: `0b00` = single (S), `0b01` = half (H, ARMv8.2+), `0b11` = double (D).

```rust
Aarch64Instruction::FaddF { rd, rn, rm, ftype } => {
    match ftype {
        0b00 => {
            let a = f32::from_bits(read_vreg_s(ctx, rn));
            let b = f32::from_bits(read_vreg_s(ctx, rm));
            write_vreg_s(ctx, rd, (a + b).to_bits());
        }
        0b11 => {
            let a = f64::from_bits(ctx.read_vreg(rn as usize));
            let b = f64::from_bits(ctx.read_vreg(rm as usize));
            ctx.write_vreg(rd as usize, (a + b).to_bits());
        }
        _ => return Err(HartException::Undef { pc: ctx.read_pc() }),
    }
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### FMOV — FP ↔ GPR Bit Transfer

```rust
Aarch64Instruction::FmovF2I { sf, rd, rn, ftype } => {
    let val = match (ftype, sf) {
        (0b00, false) => read_vreg_s(ctx, rn) as u64,  // Sn → Wd (zero-extend)
        (0b11, true)  => ctx.read_vreg(rn as usize),    // Dn → Xt
        (0b10, true)  => ctx.read_vreg_upper(rn as usize), // V[127:64] → Xt (Q upper half)
        _ => return Err(HartException::Undef { pc: ctx.read_pc() }),
    };
    write_gpr(ctx, rd, val, sf);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### FCMP — Floating-Point Compare (sets NZCV)

```rust
Aarch64Instruction::FcmpF { rn, rm, ftype } => {
    let (n_flag, z_flag, c_flag, v_flag) = match ftype {
        0b00 => {
            let a = f32::from_bits(read_vreg_s(ctx, rn));
            let b = f32::from_bits(read_vreg_s(ctx, rm));
            fp_compare(a.total_cmp(&b), a.is_nan() || b.is_nan())
        }
        0b11 => {
            let a = f64::from_bits(ctx.read_vreg(rn as usize));
            let b = f64::from_bits(ctx.read_vreg(rm as usize));
            fp_compare(a.total_cmp(&b), a.is_nan() || b.is_nan())
        }
        _ => return Err(HartException::Undef { pc: ctx.read_pc() }),
    };
    let nzcv = (n_flag << 3) | (z_flag << 2) | (c_flag << 1) | v_flag;
    update_nzcv(ctx, nzcv);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}

fn fp_compare(ord: std::cmp::Ordering, unordered: bool) -> (u8, u8, u8, u8) {
    // Returns (N, Z, C, V) per ARM DDI 0487 §C.7.195.
    if unordered {
        (0, 0, 1, 1)   // unordered: C=1, V=1
    } else {
        match ord {
            std::cmp::Ordering::Equal   => (0, 1, 1, 0),   // EQ: Z=1, C=1
            std::cmp::Ordering::Less    => (1, 0, 0, 0),   // LT: N=1
            std::cmp::Ordering::Greater => (0, 0, 1, 0),   // GT: C=1
        }
    }
}
```

---

## 10. Illegal Instruction

```rust
Aarch64Instruction::Illegal { raw } => {
    Err(HartException::UndefinedException { pc: ctx.read_pc(), raw })
}
```

---

## 11. `HartException` for AArch64

```rust
/// AArch64 synchronous exceptions. The engine converts these to
/// ESR_EL1 / ELR_EL1 / SPSR_EL1 state and dispatches to the exception vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HartException {
    /// SVC instruction from EL0.
    Svc              { imm16: u16, pc: u64 },
    /// HVC instruction.
    Hvc              { imm16: u16, pc: u64 },
    /// BRK instruction (software breakpoint).
    Breakpoint       { pc: u64, imm16: u16 },
    /// UNDEFINED instruction or architectural UNPREDICTABLE.
    UndefinedException { pc: u64, raw: u32 },
    /// Instruction Abort (PC-fetch page fault or access fault).
    InstructionAbort { pc: u64, ipa: u64 },
    /// Data Abort (load/store page fault or access fault).
    DataAbort        { pc: u64, ipa: u64, is_write: bool, size: u8 },
    /// SP misalignment (SP not 16-byte aligned on EL-change).
    SpAlignmentFault { pc: u64 },
    /// PC misalignment (PC not 4-byte aligned).
    PcAlignmentFault { pc: u64 },
    /// System register access to a register not implemented or not accessible at current EL.
    SysregTrap       { pc: u64, sysreg: SysregEncoding },
}

impl HartException {
    /// Compute ESR_EL1 encoding for this exception (ARM DDI 0487 §D17.2.37).
    pub fn esr_el1(&self) -> u32 {
        match self {
            Self::Svc { imm16, .. } => (0b010101 << 26) | *imm16 as u32,
            Self::Breakpoint { imm16, .. } => (0b110000 << 26) | *imm16 as u32,
            Self::UndefinedException { .. } => 0b000000 << 26,
            Self::DataAbort { is_write, size, .. } => {
                (0b100100 << 26) | ((*is_write as u32) << 6) | (*size as u32)
            }
            _ => 0,
        }
    }
}
```

---

## 12. Module Layout

```
aarch64/
├── execute.rs     — execute_a64(insn, ctx): main match + all arm implementations
│                    Helpers: read_gpr, write_gpr, read_base, write_base,
│                    apply_shift, apply_extend, read_vreg_s, write_vreg_s,
│                    fp_compare, sysreg_post_write_effects
├── flags.rs       — add_with_carry(x, y, carry_in, sf) -> (u64, u8)
│                    check_cond(nzcv: u8, cond: u8) -> bool
│                    update_nzcv (inline helper)
└── exception.rs   — HartException enum, esr_el1() conversion
```
