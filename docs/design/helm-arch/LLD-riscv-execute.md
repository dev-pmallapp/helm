# helm-arch — LLD: RISC-V Execute

> **Status:** Draft — Phase 0 target
> **Covers:** `riscv::execute`, `riscv::csr`, M/A/F/D/Zicsr semantics, CSR side effects

---

## 1. Execute Function Signature

```rust
/// Execute a single decoded RISC-V instruction against the given execution context.
///
/// # Returns
/// - `Ok(())` — instruction executed; ctx state updated; PC advanced by 4 (or by
///   branch offset for control-flow instructions).
/// - `Err(HartException)` — trap raised; PC is NOT advanced; the engine's trap
///   handler takes over.
///
/// # Invariants
/// - x0 writes are silently discarded (enforced by write_int_reg implementation).
/// - FP instructions access the float register file via ctx.read_float_reg /
///   ctx.write_float_reg (indices 0–31).
/// - CSR instructions check privilege before accessing; CsrAccessFault is possible.
pub fn execute<C: ExecContext>(insn: Instruction, ctx: &mut C) -> Result<(), HartException> {
    match insn {
        // ... all arms below
    }
}
```

The function is one large `match`. Each arm handles exactly one `Instruction` variant. Helper functions are called from arm bodies for repetitive logic (sign extension, FP rounding, carry computation). There is no intermediate data structure between decoding and execution.

---

## 2. RV64I — Base Integer Execute

### Integer ALU — Register-Register

```rust
Instruction::Add { rd, rs1, rs2 } => {
    let result = ctx.read_int_reg(rs1 as usize)
        .wrapping_add(ctx.read_int_reg(rs2 as usize));
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Sub { rd, rs1, rs2 } => {
    let result = ctx.read_int_reg(rs1 as usize)
        .wrapping_sub(ctx.read_int_reg(rs2 as usize));
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// SLL shifts by rs2[5:0] (low 6 bits of rs2 for RV64).
Instruction::Sll { rd, rs1, rs2 } => {
    let shamt = ctx.read_int_reg(rs2 as usize) & 0x3F;
    let result = ctx.read_int_reg(rs1 as usize) << shamt;
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// SLT — set if rs1 < rs2 (signed comparison). Result is 0 or 1.
Instruction::Slt { rd, rs1, rs2 } => {
    let a = ctx.read_int_reg(rs1 as usize) as i64;
    let b = ctx.read_int_reg(rs2 as usize) as i64;
    ctx.write_int_reg(rd as usize, if a < b { 1 } else { 0 });
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// SLTU — unsigned comparison.
Instruction::Sltu { rd, rs1, rs2 } => {
    let a = ctx.read_int_reg(rs1 as usize);
    let b = ctx.read_int_reg(rs2 as usize);
    ctx.write_int_reg(rd as usize, if a < b { 1 } else { 0 });
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// XOR / SRL / SRA / OR / AND follow the same pattern.
```

### Integer ALU — Immediate

```rust
Instruction::Addi { rd, rs1, imm } => {
    let result = ctx.read_int_reg(rs1 as usize)
        .wrapping_add(imm as i64 as u64);
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// SLTI: signed comparison with sign-extended immediate.
Instruction::Slti { rd, rs1, imm } => {
    let a = ctx.read_int_reg(rs1 as usize) as i64;
    ctx.write_int_reg(rd as usize, if a < imm as i64 { 1 } else { 0 });
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// SLTIU: compare as unsigned — but imm is still sign-extended first, then treated as u64.
Instruction::Sltiu { rd, rs1, imm } => {
    let a = ctx.read_int_reg(rs1 as usize);
    let b = imm as i64 as u64;  // sign-extend 12-bit imm to 64 bits, then treat as u64
    ctx.write_int_reg(rd as usize, if a < b { 1 } else { 0 });
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### RV64 Word Operations

The `*W` instructions operate on the low 32 bits of rs1/rs2, produce a 32-bit result, and sign-extend it to 64 bits. This is a critical correctness point — the result is always an SEXT, not a ZEXT.

```rust
Instruction::Addw { rd, rs1, rs2 } => {
    let a = ctx.read_int_reg(rs1 as usize) as i32;
    let b = ctx.read_int_reg(rs2 as usize) as i32;
    let result = a.wrapping_add(b) as i64 as u64;  // i32→i64 = SEXT; i64→u64 = bitcast
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Subw { rd, rs1, rs2 } => {
    let a = ctx.read_int_reg(rs1 as usize) as i32;
    let b = ctx.read_int_reg(rs2 as usize) as i32;
    let result = a.wrapping_sub(b) as i64 as u64;
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// SLLW: shift amount from rs2[4:0] (5 bits for 32-bit word); result SEXT.
Instruction::Sllw { rd, rs1, rs2 } => {
    let shamt = ctx.read_int_reg(rs2 as usize) & 0x1F;
    let a = ctx.read_int_reg(rs1 as usize) as u32;
    let result = (a << shamt) as i32 as i64 as u64;
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// ADDIW: imm sign-extended, add to low 32 bits, SEXT result.
Instruction::Addiw { rd, rs1, imm } => {
    let a = ctx.read_int_reg(rs1 as usize) as i32;
    let result = a.wrapping_add(imm as i32) as i64 as u64;
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### Loads

```rust
Instruction::Load { rd, rs1, imm, width } => {
    let addr = ctx.read_int_reg(rs1 as usize).wrapping_add(imm as i64 as u64);
    let raw = ctx.read_mem(addr, width.byte_count())?;
    let val = match width {
        LoadWidth::Byte           => (raw as i8)  as i64 as u64,  // SEXT
        LoadWidth::HalfWord       => (raw as i16) as i64 as u64,
        LoadWidth::Word           => (raw as i32) as i64 as u64,
        LoadWidth::DoubleWord     => raw,
        LoadWidth::ByteUnsigned   => raw & 0xFF,                   // ZEXT
        LoadWidth::HalfWordUnsigned => raw & 0xFFFF,
        LoadWidth::WordUnsigned   => raw & 0xFFFF_FFFF,
    };
    ctx.write_int_reg(rd as usize, val);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

Memory errors from `ctx.read_mem` propagate as `HartException` variants (the `ExecContext` implementation converts `MemFault` to the appropriate RISC-V exception code).

### Stores

```rust
Instruction::Store { rs1, rs2, imm, width } => {
    let addr = ctx.read_int_reg(rs1 as usize).wrapping_add(imm as i64 as u64);
    let val  = ctx.read_int_reg(rs2 as usize);
    ctx.write_mem(addr, width.byte_count(), val)?;
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

### Branches

```rust
Instruction::Beq { rs1, rs2, imm } => {
    let pc = ctx.read_pc();
    let taken = ctx.read_int_reg(rs1 as usize) == ctx.read_int_reg(rs2 as usize);
    ctx.write_pc(if taken { pc.wrapping_add(imm as i64 as u64) } else { pc.wrapping_add(4) });
    Ok(())
}
Instruction::Bne  { rs1, rs2, imm } => { /* !taken = ne */ ... }
Instruction::Blt  { rs1, rs2, imm } => { /* signed < */ ... }
Instruction::Bge  { rs1, rs2, imm } => { /* signed >= */ ... }
Instruction::Bltu { rs1, rs2, imm } => { /* unsigned < */ ... }
Instruction::Bgeu { rs1, rs2, imm } => { /* unsigned >= */ ... }
```

PC alignment: the branch target must be 4-byte aligned (or 2-byte for C extension). This is checked in the engine's fetch, not in execute. Execute only computes the target; the engine catches misaligned fetch.

### Jumps

```rust
Instruction::Jal { rd, imm } => {
    let pc  = ctx.read_pc();
    let ret = pc.wrapping_add(4);            // link value = PC+4
    let tgt = pc.wrapping_add(imm as i64 as u64);
    ctx.write_int_reg(rd as usize, ret);     // x0 write silently discarded
    ctx.write_pc(tgt);
    Ok(())
}
Instruction::Jalr { rd, rs1, imm } => {
    let pc  = ctx.read_pc();
    let ret = pc.wrapping_add(4);
    // JALR clears bit 0 of the target address (RISC-V spec §2.5).
    let tgt = ctx.read_int_reg(rs1 as usize)
        .wrapping_add(imm as i64 as u64) & !1u64;
    ctx.write_int_reg(rd as usize, ret);
    ctx.write_pc(tgt);
    Ok(())
}
```

### Upper Immediates

```rust
Instruction::Lui { rd, imm } => {
    // imm already has bits [31:12] set; bits [11:0] are zero from decode_u_imm.
    // Sign-extend the 32-bit value to 64 bits.
    ctx.write_int_reg(rd as usize, imm as i32 as i64 as u64);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Auipc { rd, imm } => {
    let pc = ctx.read_pc();
    let result = pc.wrapping_add(imm as i32 as i64 as u64);
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(pc.wrapping_add(4));
    Ok(())
}
```

### System

```rust
Instruction::Ecall => {
    // The exception code depends on the current privilege level.
    // ExecContext::current_privilege() returns the current PrivLevel.
    Err(match ctx.current_privilege() {
        PrivLevel::User     => HartException::EnvironmentCallUMode,
        PrivLevel::Supervisor => HartException::EnvironmentCallSMode,
        PrivLevel::Machine  => HartException::EnvironmentCallMMode,
    })
}
Instruction::Ebreak => {
    Err(HartException::Breakpoint { pc: ctx.read_pc() })
}
Instruction::Fence  { .. } => {
    // In a single-hart simulator, FENCE is a no-op.
    // Multi-hart: signal memory barrier to MemoryMap (Phase 3).
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::FenceI => {
    // Instruction-fetch fence. In an ISS without an I-cache, this is a no-op.
    // With a simulated I-cache, this would flush it.
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Wfi => {
    // WFI: wait for interrupt. In SE mode, treat as NOP.
    // In FS mode: signal to the event loop that this hart should be descheduled
    // until an interrupt is pending.
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Mret => {
    // Restore PC from mepc, privilege from mstatus.MPP, MIE from MPIE.
    // Only legal in M-mode; raise IllegalInstruction in other modes.
    if ctx.current_privilege() != PrivLevel::Machine {
        return Err(HartException::IllegalInstruction { raw: 0x30200073 });
    }
    let mepc   = ctx.read_csr(CsrAddr::MEPC)?;
    let mstatus = ctx.read_csr(CsrAddr::MSTATUS)?;
    let mpp    = (mstatus >> 11) & 0x3;
    let mpie   = (mstatus >> 7) & 0x1;
    // Set MIE = MPIE, MPP = U, MPIE = 1
    let new_mstatus = (mstatus & !0x1888u64)
        | (mpie << 3)           // MIE = old MPIE
        | (0b00 << 11)          // MPP = U-mode
        | (1 << 7);             // MPIE = 1
    ctx.write_csr(CsrAddr::MSTATUS, new_mstatus)?;
    ctx.set_privilege(PrivLevel::from_bits(mpp as u8));
    ctx.write_pc(mepc & !1u64);  // clear bit 0 per spec
    Ok(())
}
Instruction::Sret => { /* similar to MRET but using sepc / sstatus / SPP */ ... }
Instruction::SfenceVma { rs1, rs2 } => {
    // Flush TLB. Four variants depending on rs1 (VA) and rs2 (ASID) being x0 or not.
    ctx.sfence_vma(
        if rs1 == 0 { None } else { Some(ctx.read_int_reg(rs1 as usize)) },
        if rs2 == 0 { None } else { Some(ctx.read_int_reg(rs2 as usize)) },
    );
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Illegal { raw } => {
    Err(HartException::IllegalInstruction { raw })
}
```

---

## 3. M Extension — Integer Multiply/Divide

Key correctness rules:

- All division and remainder by zero produce defined results (no exception): `rd = -1` for DIV/DIVU when divisor is zero, `rd = dividend` for REM/REMU.
- Integer overflow (only case: `i64::MIN / -1`) produces `rd = i64::MIN` for DIV, `rd = 0` for REM.
- `MULH`, `MULHSU`, `MULHU` return the upper 64 bits of a 128-bit product.

```rust
Instruction::Mul { rd, rs1, rs2 } => {
    // Lower 64 bits of signed × signed (same as unsigned for lower half).
    let result = ctx.read_int_reg(rs1 as usize)
        .wrapping_mul(ctx.read_int_reg(rs2 as usize));
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Mulh { rd, rs1, rs2 } => {
    // Signed × signed, upper 64 bits.
    let a = ctx.read_int_reg(rs1 as usize) as i64 as i128;
    let b = ctx.read_int_reg(rs2 as usize) as i64 as i128;
    let result = ((a * b) >> 64) as u64;
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Mulhsu { rd, rs1, rs2 } => {
    // Signed rs1 × unsigned rs2, upper 64 bits.
    let a = ctx.read_int_reg(rs1 as usize) as i64 as i128;
    let b = ctx.read_int_reg(rs2 as usize) as u128 as i128;
    let result = ((a * b) >> 64) as u64;
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Mulhu { rd, rs1, rs2 } => {
    let a = ctx.read_int_reg(rs1 as usize) as u128;
    let b = ctx.read_int_reg(rs2 as usize) as u128;
    let result = ((a * b) >> 64) as u64;
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Div { rd, rs1, rs2 } => {
    let a = ctx.read_int_reg(rs1 as usize) as i64;
    let b = ctx.read_int_reg(rs2 as usize) as i64;
    let result = if b == 0 {
        u64::MAX                         // -1 as u64
    } else if a == i64::MIN && b == -1 {
        i64::MIN as u64                  // overflow: return MIN
    } else {
        (a / b) as u64
    };
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Divu { rd, rs1, rs2 } => {
    let a = ctx.read_int_reg(rs1 as usize);
    let b = ctx.read_int_reg(rs2 as usize);
    let result = if b == 0 { u64::MAX } else { a / b };
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Rem { rd, rs1, rs2 } => {
    let a = ctx.read_int_reg(rs1 as usize) as i64;
    let b = ctx.read_int_reg(rs2 as usize) as i64;
    let result = if b == 0 {
        a as u64                         // dividend
    } else if a == i64::MIN && b == -1 {
        0                                // overflow: remainder = 0
    } else {
        (a % b) as u64
    };
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// REMU: unsigned remainder. Zero-by-zero = dividend. No overflow case.
Instruction::Remu { rd, rs1, rs2 } => {
    let a = ctx.read_int_reg(rs1 as usize);
    let b = ctx.read_int_reg(rs2 as usize);
    let result = if b == 0 { a } else { a % b };
    ctx.write_int_reg(rd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// Word variants (MULW, DIVW, DIVUW, REMW, REMUW):
// Operate on low 32 bits, sign-extend result to 64 bits. Same edge cases as above
// but using i32/u32 arithmetic.
```

---

## 4. A Extension — Atomic Instructions

### LR/SC — Load-Reserved / Store-Conditional

The reservation set is a single address stored in `RiscvHart::lr_addr`. This is the minimal correct model for a uniprocessor simulator. Multi-hart support would require a shared reservation granule structure.

```rust
Instruction::Lrw { rd, rs1, aq: _, rl: _ } => {
    let addr = ctx.read_int_reg(rs1 as usize);
    if addr & 0x3 != 0 {
        return Err(HartException::LoadAddressMisaligned { addr });
    }
    let val = ctx.read_mem(addr, 4)?;
    let val = (val as i32) as i64 as u64;  // SEXT to 64
    ctx.write_int_reg(rd as usize, val);
    ctx.set_lr(Some(addr));                 // reserve this address
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Scw { rd, rs1, rs2, aq: _, rl: _ } => {
    let addr = ctx.read_int_reg(rs1 as usize);
    if addr & 0x3 != 0 {
        return Err(HartException::StoreAmoMisaligned { addr });
    }
    let success = ctx.get_lr() == Some(addr);  // check reservation
    if success {
        let val = ctx.read_int_reg(rs2 as usize);
        ctx.write_mem(addr, 4, val & 0xFFFF_FFFF)?;
        ctx.write_int_reg(rd as usize, 0);     // success = 0
    } else {
        ctx.write_int_reg(rd as usize, 1);     // failure = non-zero
    }
    ctx.set_lr(None);                          // reservation expires
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// LRD / SCD: same but 8-byte aligned and 64-bit.
```

### AMO — Atomic Memory Operations

AMO semantics: load current value from addr, apply operation with rs2, store result. Return original value in rd. Atomicity is trivially preserved in a single-hart ISS.

```rust
// Generic AMO helper
fn amo<C, F>(ctx: &mut C, rd: u8, rs1: u8, rs2: u8, width: usize, op: F)
    -> Result<(), HartException>
where
    C: ExecContext,
    F: Fn(u64, u64) -> u64,
{
    let addr = ctx.read_int_reg(rs1 as usize);
    let orig = ctx.read_mem(addr, width)?;
    let rs2v = ctx.read_int_reg(rs2 as usize);
    let new_val = op(orig, rs2v);
    ctx.write_mem(addr, width, new_val)?;
    // Return the *original* value sign-extended (for 32-bit AMOs).
    let ret = if width == 4 { (orig as i32) as i64 as u64 } else { orig };
    ctx.write_int_reg(rd as usize, ret);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}

Instruction::AmoswapW { rd, rs1, rs2, .. } =>
    amo(ctx, rd, rs1, rs2, 4, |_, b| b & 0xFFFF_FFFF),
Instruction::AmoaddW  { rd, rs1, rs2, .. } =>
    amo(ctx, rd, rs1, rs2, 4, |a, b| (a as u32).wrapping_add(b as u32) as u64),
Instruction::AmoxorW  { rd, rs1, rs2, .. } =>
    amo(ctx, rd, rs1, rs2, 4, |a, b| a ^ b),
Instruction::AmoandW  { rd, rs1, rs2, .. } =>
    amo(ctx, rd, rs1, rs2, 4, |a, b| a & b),
Instruction::AmoorW   { rd, rs1, rs2, .. } =>
    amo(ctx, rd, rs1, rs2, 4, |a, b| a | b),
// AMAMIN.W: signed minimum of memory word and rs2.
Instruction::AmominW  { rd, rs1, rs2, .. } =>
    amo(ctx, rd, rs1, rs2, 4, |a, b| {
        let ai = a as i32; let bi = b as i32;
        if ai < bi { a } else { b & 0xFFFF_FFFF }
    }),
// AMAXIMU.W: unsigned maximum.
Instruction::AmomaxuW { rd, rs1, rs2, .. } =>
    amo(ctx, rd, rs1, rs2, 4, |a, b| {
        let a = a & 0xFFFF_FFFF; let b = b & 0xFFFF_FFFF;
        if a > b { a } else { b }
    }),
// D-suffix variants: width = 8, no masking needed.
Instruction::AmoswapD { rd, rs1, rs2, .. } =>
    amo(ctx, rd, rs1, rs2, 8, |_, b| b),
```

---

## 5. F/D Extension — Floating Point

### Float Register Storage

Floating-point registers are stored as `u64` (for F, NaN-boxed) and `u64` (for D). The `read_float_reg` / `write_float_reg` methods on `ExecContext` handle the physical `[u64; 32]` storage.

**NaN boxing (F extension in D-capable hart):** A 32-bit float stored in a 64-bit FP register has bits [63:32] = all 1s. When a 32-bit FP instruction reads a register, if bits [63:32] are not all 1s, the value is treated as canonical NaN.

```rust
fn read_f32(ctx: &impl ExecContext, idx: usize) -> f32 {
    let raw = ctx.read_float_reg(idx);
    if (raw >> 32) != 0xFFFF_FFFF {
        f32::NAN      // not NaN-boxed — treat as canonical NaN
    } else {
        f32::from_bits(raw as u32)
    }
}

fn write_f32(ctx: &mut impl ExecContext, idx: usize, val: f32) {
    // NaN-box: upper 32 bits = 0xFFFF_FFFF
    let raw = 0xFFFF_FFFF_0000_0000u64 | val.to_bits() as u64;
    ctx.write_float_reg(idx, raw);
}

fn read_f64(ctx: &impl ExecContext, idx: usize) -> f64 {
    f64::from_bits(ctx.read_float_reg(idx))
}

fn write_f64(ctx: &mut impl ExecContext, idx: usize, val: f64) {
    ctx.write_float_reg(idx, val.to_bits());
}
```

### Rounding Mode

RISC-V defines 5 static rounding modes (0–4) plus dynamic (7 = from `fcsr.FRM`). The execute function resolves the effective rounding mode before any FP computation.

```rust
fn effective_rm(rm_field: u8, ctx: &impl ExecContext) -> Result<u8, HartException> {
    match rm_field {
        0b111 => {
            // Dynamic: read FRM from fcsr
            let fcsr = ctx.read_csr(CsrAddr::FCSR).unwrap_or(0);
            let frm  = ((fcsr >> 5) & 0x7) as u8;
            if frm > 4 { Err(HartException::IllegalInstruction { raw: 0 }) }
            else { Ok(frm) }
        }
        0..=4 => Ok(rm_field),
        _     => Err(HartException::IllegalInstruction { raw: 0 }),
    }
}
```

RISC-V FP rounding modes map to IEEE 754:

| RISC-V RM | Name | IEEE 754 |
|-----------|------|----------|
| 0 | RNE | Round to nearest, ties to even |
| 1 | RTZ | Round toward zero |
| 2 | RDN | Round down (toward -∞) |
| 3 | RUP | Round up (toward +∞) |
| 4 | RMM | Round to nearest, ties to max magnitude |

Currently Rust's `f32`/`f64` operations use the host FPU rounding mode. Phase 0 sets the host mode to match the target mode via platform-specific FPU control. A future phase may use a software FP library for full portability.

### FADD.S example

```rust
Instruction::FaddS { frd, frs1, frs2, rm } => {
    let _rm = effective_rm(rm, ctx)?;
    let a = read_f32(ctx, frs1 as usize);
    let b = read_f32(ctx, frs2 as usize);
    let result = a + b;  // TODO: apply rounding mode
    update_fflags(ctx, result.is_nan(), false, false, false, false);
    write_f32(ctx, frd as usize, result);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
```

**`update_fflags`** sets bits in `fcsr.FFLAGS` (bits [4:0]) for: NV (invalid), DZ (divide by zero), OF (overflow), UF (underflow), NX (inexact). This is queried from `std::fenv` in Phase 0 or computed manually for each operation.

```rust
fn update_fflags(ctx: &mut impl ExecContext, nv: bool, dz: bool, of: bool, uf: bool, nx: bool) {
    let bits = (nv as u64) << 4 | (dz as u64) << 3 | (of as u64) << 2
             | (uf as u64) << 1 | (nx as u64);
    // FFLAGS is fcsr[4:0]; OR in the new flags (sticky).
    let old = ctx.read_csr(CsrAddr::FFLAGS).unwrap_or(0);
    let _ = ctx.write_csr(CsrAddr::FFLAGS, old | bits);
}
```

### FMV.X.W / FMV.W.X — Bit Transfers

```rust
Instruction::FmvXW { rd, frs1 } => {
    // Transfer low 32 bits of float reg to integer reg, sign-extended.
    let raw = ctx.read_float_reg(frs1 as usize) as u32;
    ctx.write_int_reg(rd as usize, raw as i32 as i64 as u64);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::FmvWX { frd, rs1 } => {
    // Transfer low 32 bits of integer reg to float reg; NaN-box upper 32.
    let raw = ctx.read_int_reg(rs1 as usize) as u32;
    write_f32_raw(ctx, frd as usize, raw);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
fn write_f32_raw(ctx: &mut impl ExecContext, idx: usize, bits: u32) {
    ctx.write_float_reg(idx, 0xFFFF_FFFF_0000_0000u64 | bits as u64);
}
```

### FCLASS.S / FCLASS.D

```rust
Instruction::FclassS { rd, frs1 } => {
    let val = read_f32(ctx, frs1 as usize);
    let class_bits = fclass_f32(val);
    ctx.write_int_reg(rd as usize, class_bits as u64);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}

fn fclass_f32(v: f32) -> u32 {
    // Returns a one-hot 10-bit value per RISC-V spec §11.2
    if v.is_nan() {
        if v.to_bits() & 0x0040_0000 != 0 { 1 << 9 } else { 1 << 8 }  // quiet / signaling NaN
    } else if v == f32::NEG_INFINITY { 1 << 0 }
    else if v.is_sign_negative() && v.is_normal() { 1 << 1 }
    else if v.is_sign_negative() && v.is_subnormal() { 1 << 2 }
    else if v == 0.0 && v.is_sign_negative() { 1 << 3 }
    else if v == 0.0 { 1 << 4 }
    else if v.is_subnormal() { 1 << 5 }
    else if v.is_normal() { 1 << 6 }
    else /* +inf */ { 1 << 7 }
}
```

---

## 6. Zicsr — CSR Read-Modify-Write

All CSR instructions follow the same pattern: read old value, compute new value, write new value, write old value to rd.

```rust
Instruction::Csrrw { rd, rs1, csr } => {
    // Atomically write rs1 to CSR, return old CSR to rd.
    // If rd = x0, do NOT read the CSR (avoids read side effects on WO CSRs).
    let old = if rd != 0 { ctx.read_csr(csr)? } else { 0 };
    ctx.write_csr(csr, ctx.read_int_reg(rs1 as usize))?;
    ctx.write_int_reg(rd as usize, old);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Csrrs { rd, rs1, csr } => {
    // Atomically OR rs1 into CSR bits; return old CSR to rd.
    // If rs1 = x0, do NOT write the CSR (avoids spurious write side effects).
    let old = ctx.read_csr(csr)?;
    if rs1 != 0 {
        ctx.write_csr(csr, old | ctx.read_int_reg(rs1 as usize))?;
    }
    ctx.write_int_reg(rd as usize, old);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
Instruction::Csrrc { rd, rs1, csr } => {
    // Atomically clear CSR bits set in rs1; return old CSR.
    // If rs1 = x0, do NOT write the CSR.
    let old = ctx.read_csr(csr)?;
    if rs1 != 0 {
        ctx.write_csr(csr, old & !ctx.read_int_reg(rs1 as usize))?;
    }
    ctx.write_int_reg(rd as usize, old);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}
// Immediate variants: same logic but imm field (5-bit, zero-extended) used instead of rs1.
Instruction::Csrrwi { rd, uimm, csr } => { ... }
Instruction::Csrrsi { rd, uimm, csr } => { ... }
Instruction::Csrrci { rd, uimm, csr } => { ... }
```

---

## 7. CSR Side Effects (Q20)

**Design decision Q20:** CSR side effects are handled inside `execute`, not inside `ExecContext::write_csr`. After every `write_csr` call, the execute arm for each CSR instruction performs explicit side-effect dispatch based on the CSR address. This keeps the side effects visible and auditable, co-located with the instruction semantics.

`write_csr` in `ExecContext` only validates privilege and WARL fields (write-any-read-legal masking). It does not trigger any state machine transitions.

### CSR Address Constants

```rust
/// CSR address constants (source: RISC-V Privileged Specification Table 2.2–2.4).
pub mod CsrAddr {
    // Unprivileged / User
    pub const FFLAGS:  u16 = 0x001;  // FP accrued exceptions
    pub const FRM:     u16 = 0x002;  // FP rounding mode
    pub const FCSR:    u16 = 0x003;  // FP control and status (FRM + FFLAGS)
    pub const CYCLE:   u16 = 0xC00;  // Cycle counter (read-only from U-mode)
    pub const TIME:    u16 = 0xC01;
    pub const INSTRET: u16 = 0xC02;

    // Supervisor
    pub const SSTATUS: u16 = 0x100;
    pub const SIE:     u16 = 0x104;
    pub const STVEC:   u16 = 0x105;
    pub const SCOUNTEREN: u16 = 0x106;
    pub const SSCRATCH: u16 = 0x140;
    pub const SEPC:    u16 = 0x141;
    pub const SCAUSE:  u16 = 0x142;
    pub const STVAL:   u16 = 0x143;
    pub const SIP:     u16 = 0x144;
    pub const SATP:    u16 = 0x180;  // Supervisor Address Translation and Protection

    // Machine
    pub const MSTATUS:  u16 = 0x300;
    pub const MISA:     u16 = 0x301;
    pub const MEDELEG:  u16 = 0x302;
    pub const MIDELEG:  u16 = 0x303;
    pub const MIE:      u16 = 0x304;
    pub const MTVEC:    u16 = 0x305;
    pub const MCOUNTEREN: u16 = 0x306;
    pub const MSCRATCH: u16 = 0x340;
    pub const MEPC:     u16 = 0x341;
    pub const MCAUSE:   u16 = 0x342;
    pub const MTVAL:    u16 = 0x343;
    pub const MIP:      u16 = 0x344;
    pub const PMPCFG0:  u16 = 0x3A0;
    pub const PMPADDR0: u16 = 0x3B0;
    pub const MCYCLE:   u16 = 0xB00;
    pub const MINSTRET: u16 = 0xB02;
    pub const MHARTID:  u16 = 0xF14;
}
```

### Side-Effect Dispatch Table

Called after `write_csr` returns `Ok(())` inside the execute arm for each CSR instruction:

```rust
fn csr_post_write_effects<C: ExecContext>(csr: u16, new_val: u64, ctx: &mut C) {
    match csr {
        // satp write → TLB flush required (new page table base or ASID change).
        CsrAddr::SATP => {
            ctx.sfence_vma(None, None);   // flush all TLB entries
            // The page table mode (Sv39/Sv48) encoded in satp[63:60] is also updated.
        }

        // mstatus / sstatus write → privilege mode or endianness may change.
        // The ExecContext tracks privilege internally; a write to mstatus.MPP
        // does NOT immediately change privilege (that happens on MRET).
        // But mstatus.MXL / SXL (extension bits) affect decode in future instructions.
        CsrAddr::MSTATUS | CsrAddr::SSTATUS => {
            // No immediate action needed in Phase 0 single-hart ISS.
            // Multi-hart: may need to signal other harts.
        }

        // mtvec / stvec write → update trap vector base. No immediate action needed;
        // the trap handler reads mtvec/stvec from the CSR file when a trap fires.
        CsrAddr::MTVEC | CsrAddr::STVEC => {}

        // fcsr write → FRM and FFLAGS sub-registers change. If using host FPU
        // for rounding, update host FPU control word to match new FRM.
        CsrAddr::FCSR => {
            // let frm = (new_val >> 5) & 0x7;
            // platform::set_fp_rounding_mode(frm);
        }
        CsrAddr::FRM => {
            // platform::set_fp_rounding_mode(new_val & 0x7);
        }

        // mideleg / medeleg changes affect interrupt/exception routing on next trap.
        // No immediate action in a single-hart ISS — routing is computed at trap time.
        _ => {}
    }
}
```

This function is called inside the execute arm for CSRRW/CSRRS/CSRRC after the write succeeds:

```rust
// Inside the Csrrw arm, after write_csr:
csr_post_write_effects(csr, ctx.read_int_reg(rs1 as usize), ctx);
```

---

## 8. ECALL / EBREAK / Illegal Instruction

```rust
Instruction::Ecall => {
    Err(match ctx.current_privilege() {
        PrivLevel::User       => HartException::EnvironmentCallUMode,
        PrivLevel::Supervisor => HartException::EnvironmentCallSMode,
        PrivLevel::Machine    => HartException::EnvironmentCallMMode,
    })
}
Instruction::Ebreak => {
    Err(HartException::Breakpoint { pc: ctx.read_pc() })
}
Instruction::Illegal { raw } => {
    Err(HartException::IllegalInstruction { raw })
}
```

These raise `HartException` and return it to the engine. The engine's exception handler then:

1. In SE mode: intercepts `EnvironmentCallUMode` and dispatches to `SyscallHandler`.
2. In FE mode: raises `EnvironmentCallUMode` up to the simulation runner (test frameworks catch it).
3. `Breakpoint`: sends a `SIGTRAP`-equivalent to the GDB stub.
4. `IllegalInstruction`: raises a fatal signal to the simulation.

The execute function itself does not write `mcause`, `mepc`, or `mtval` — that is the engine's responsibility in the exception handler, where it has access to the full hart context.

---

## 9. x0 Write Semantics

`x0` is hardwired to zero. Writes to `x0` are silently discarded. This is enforced in `ExecContext::write_int_reg`:

```rust
// Inside RiscvHart's ExecContext implementation:
fn write_int_reg(&mut self, idx: usize, val: u64) {
    if idx != 0 {
        self.regs[idx] = val;
    }
    // writes to idx=0 are silently discarded
}
fn read_int_reg(&self, idx: usize) -> u64 {
    if idx == 0 { 0 } else { self.regs[idx] }
}
```

The execute function does not need to guard against `rd == 0` in each arm. The `write_int_reg` implementation handles it.

---

## 10. Module Layout

```
riscv/
├── execute.rs     — execute(insn, ctx): the main match + all arm implementations
│                    Helper functions: amo(), fclass_f32(), fclass_f64(),
│                    read_f32/write_f32, read_f64/write_f64, effective_rm(),
│                    update_fflags(), csr_post_write_effects()
└── csr.rs         — CsrAddr constants, privilege check helpers
```
