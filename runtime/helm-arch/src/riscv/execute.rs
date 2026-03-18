//! RISC-V RV64GC instruction execution.
//!
//! Entry point: [`execute`] — takes a decoded [`Instruction`] and an `&mut impl ExecContext`.
//! Returns `Ok(())` on normal completion or `Err(HartException)` on traps/syscalls.
//!
//! # Phase 0 implementation status
//! - [x] RV64I base (all 47 instructions)
//! - [x] Zicsr (6 instructions)
//! - [ ] RV64M multiply/divide (implement next)
//! - [ ] RV64A atomics  (implement next)
//! - [ ] RV64F/D floating-point (implement after M/A)
//! - [ ] Privileged (MRET/SRET/WFI/SFENCE.VMA)
//! - [ ] C extension (expand in engine before calling execute)

use helm_core::{AccessType, ExecContext, HartException, MemFault};

use super::insn::Instruction;

/// Execute one decoded instruction.
///
/// On a memory fault, converts it to the appropriate `HartException` variant
/// before returning, so the caller only needs to handle `HartException`.
pub fn execute(
    insn: Instruction,
    ctx: &mut impl ExecContext,
) -> Result<(), HartException> {
    use Instruction::*;

    match insn {
        // ── RV64I — Base Integer ─────────────────────────────────────────────
        LUI { rd, imm } => {
            ctx.write_int_reg(rd as usize, imm as u64);
            ctx.write_pc(ctx.read_pc().wrapping_add(4));
        }
        AUIPC { rd, imm } => {
            let val = ctx.read_pc().wrapping_add(imm as u64);
            ctx.write_int_reg(rd as usize, val);
            ctx.write_pc(ctx.read_pc().wrapping_add(4));
        }

        JAL { rd, imm } => {
            let pc = ctx.read_pc();
            ctx.write_int_reg(rd as usize, pc.wrapping_add(4));
            let target = pc.wrapping_add(imm as u64);
            if target & 3 != 0 {
                return Err(HartException::InstructionAddressMisaligned { addr: target });
            }
            ctx.write_pc(target);
        }
        JALR { rd, rs1, imm } => {
            let base = ctx.read_int_reg(rs1 as usize);
            let ret = ctx.read_pc().wrapping_add(4);
            let target = base.wrapping_add(imm as u64) & !1u64;
            if target & 3 != 0 {
                return Err(HartException::InstructionAddressMisaligned { addr: target });
            }
            ctx.write_int_reg(rd as usize, ret);
            ctx.write_pc(target);
        }

        BEQ  { rs1, rs2, imm } => branch(ctx, rs1, rs2, imm, |a, b| a == b)?,
        BNE  { rs1, rs2, imm } => branch(ctx, rs1, rs2, imm, |a, b| a != b)?,
        BLT  { rs1, rs2, imm } => branch(ctx, rs1, rs2, imm, |a, b| (a as i64) < (b as i64))?,
        BGE  { rs1, rs2, imm } => branch(ctx, rs1, rs2, imm, |a, b| (a as i64) >= (b as i64))?,
        BLTU { rs1, rs2, imm } => branch(ctx, rs1, rs2, imm, |a, b| a < b)?,
        BGEU { rs1, rs2, imm } => branch(ctx, rs1, rs2, imm, |a, b| a >= b)?,

        LB  { rd, rs1, imm } => load(ctx, rd, rs1, imm, 1, |v| (v as i8)  as u64)?,
        LH  { rd, rs1, imm } => load(ctx, rd, rs1, imm, 2, |v| (v as i16) as u64)?,
        LW  { rd, rs1, imm } => load(ctx, rd, rs1, imm, 4, |v| (v as i32) as u64)?,
        LD  { rd, rs1, imm } => load(ctx, rd, rs1, imm, 8, |v| v)?,
        LBU { rd, rs1, imm } => load(ctx, rd, rs1, imm, 1, |v| v)?,
        LHU { rd, rs1, imm } => load(ctx, rd, rs1, imm, 2, |v| v)?,
        LWU { rd, rs1, imm } => load(ctx, rd, rs1, imm, 4, |v| v)?,

        SB { rs1, rs2, imm } => store(ctx, rs1, rs2, imm, 1)?,
        SH { rs1, rs2, imm } => store(ctx, rs1, rs2, imm, 2)?,
        SW { rs1, rs2, imm } => store(ctx, rs1, rs2, imm, 4)?,
        SD { rs1, rs2, imm } => store(ctx, rs1, rs2, imm, 8)?,

        ADDI  { rd, rs1, imm } => { let v = ctx.read_int_reg(rs1 as usize).wrapping_add(imm as u64);                             wri(ctx, rd, v); }
        SLTI  { rd, rs1, imm } => { let v = ((ctx.read_int_reg(rs1 as usize) as i64) < imm) as u64; wri(ctx, rd, v); }
        SLTIU { rd, rs1, imm } => { let v = (ctx.read_int_reg(rs1 as usize) < (imm as u64)) as u64; wri(ctx, rd, v); }
        XORI  { rd, rs1, imm } => { let v = ctx.read_int_reg(rs1 as usize) ^ (imm as u64);          wri(ctx, rd, v); }
        ORI   { rd, rs1, imm } => { let v = ctx.read_int_reg(rs1 as usize) | (imm as u64);           wri(ctx, rd, v); }
        ANDI  { rd, rs1, imm } => { let v = ctx.read_int_reg(rs1 as usize) & (imm as u64);           wri(ctx, rd, v); }
        SLLI  { rd, rs1, shamt } => { let v = ctx.read_int_reg(rs1 as usize) << shamt;               wri(ctx, rd, v); }
        SRLI  { rd, rs1, shamt } => { let v = ctx.read_int_reg(rs1 as usize) >> shamt;               wri(ctx, rd, v); }
        SRAI  { rd, rs1, shamt } => { let v = ((ctx.read_int_reg(rs1 as usize) as i64) >> shamt) as u64; wri(ctx, rd, v); }

        ADD  { rd, rs1, rs2 } => { let v = ctx.read_int_reg(rs1 as usize).wrapping_add(ctx.read_int_reg(rs2 as usize));  wri(ctx, rd, v); }
        SUB  { rd, rs1, rs2 } => { let v = ctx.read_int_reg(rs1 as usize).wrapping_sub(ctx.read_int_reg(rs2 as usize));  wri(ctx, rd, v); }
        SLL  { rd, rs1, rs2 } => { let shamt = ctx.read_int_reg(rs2 as usize) & 63; let v = ctx.read_int_reg(rs1 as usize) << shamt; wri(ctx, rd, v); }
        SLT  { rd, rs1, rs2 } => { let v = ((ctx.read_int_reg(rs1 as usize) as i64) < (ctx.read_int_reg(rs2 as usize) as i64)) as u64; wri(ctx, rd, v); }
        SLTU { rd, rs1, rs2 } => { let v = (ctx.read_int_reg(rs1 as usize) < ctx.read_int_reg(rs2 as usize)) as u64; wri(ctx, rd, v); }
        XOR  { rd, rs1, rs2 } => { let v = ctx.read_int_reg(rs1 as usize) ^ ctx.read_int_reg(rs2 as usize);           wri(ctx, rd, v); }
        SRL  { rd, rs1, rs2 } => { let shamt = ctx.read_int_reg(rs2 as usize) & 63; let v = ctx.read_int_reg(rs1 as usize) >> shamt; wri(ctx, rd, v); }
        SRA  { rd, rs1, rs2 } => { let shamt = ctx.read_int_reg(rs2 as usize) & 63; let v = ((ctx.read_int_reg(rs1 as usize) as i64) >> shamt) as u64; wri(ctx, rd, v); }
        OR   { rd, rs1, rs2 } => { let v = ctx.read_int_reg(rs1 as usize) | ctx.read_int_reg(rs2 as usize);           wri(ctx, rd, v); }
        AND  { rd, rs1, rs2 } => { let v = ctx.read_int_reg(rs1 as usize) & ctx.read_int_reg(rs2 as usize);           wri(ctx, rd, v); }

        // Word (32-bit sign-extended) ops
        ADDIW { rd, rs1, imm } => { let v = sext32(ctx.read_int_reg(rs1 as usize).wrapping_add(imm as u64) as u32); wri(ctx, rd, v); }
        SLLIW { rd, rs1, shamt } => { let v = sext32((ctx.read_int_reg(rs1 as usize) as u32) << shamt); wri(ctx, rd, v); }
        SRLIW { rd, rs1, shamt } => { let v = sext32((ctx.read_int_reg(rs1 as usize) as u32) >> shamt); wri(ctx, rd, v); }
        SRAIW { rd, rs1, shamt } => { let v = sext32((((ctx.read_int_reg(rs1 as usize) as u32) as i32) >> shamt) as u32); wri(ctx, rd, v); }
        ADDW  { rd, rs1, rs2 }  => { let v = sext32((ctx.read_int_reg(rs1 as usize) as u32).wrapping_add(ctx.read_int_reg(rs2 as usize) as u32)); wri(ctx, rd, v); }
        SUBW  { rd, rs1, rs2 }  => { let v = sext32((ctx.read_int_reg(rs1 as usize) as u32).wrapping_sub(ctx.read_int_reg(rs2 as usize) as u32)); wri(ctx, rd, v); }
        SLLW  { rd, rs1, rs2 }  => { let s = ctx.read_int_reg(rs2 as usize) & 31; let v = sext32((ctx.read_int_reg(rs1 as usize) as u32) << s); wri(ctx, rd, v); }
        SRLW  { rd, rs1, rs2 }  => { let s = ctx.read_int_reg(rs2 as usize) & 31; let v = sext32((ctx.read_int_reg(rs1 as usize) as u32) >> s); wri(ctx, rd, v); }
        SRAW  { rd, rs1, rs2 }  => { let s = ctx.read_int_reg(rs2 as usize) & 31; let v = sext32(((ctx.read_int_reg(rs1 as usize) as i32) >> s) as u32); wri(ctx, rd, v); }

        FENCE   { .. } | FENCE_I => { /* no-op in SE mode */ }

        ECALL  => {
            let pc  = ctx.read_pc();
            let nr  = ctx.read_int_reg(17); // a7
            return Err(HartException::EnvironmentCall { pc, nr });
        }
        EBREAK => {
            return Err(HartException::Breakpoint { pc: ctx.read_pc() });
        }

        // ── Zicsr ─────────────────────────────────────────────────────────
        CSRRW { rd, rs1, csr } => {
            let old = ctx.read_csr(csr);
            let new = ctx.read_int_reg(rs1 as usize);
            ctx.write_csr(csr, new);
            wri(ctx, rd, old);
        }
        CSRRS { rd, rs1, csr } => {
            let old = ctx.read_csr(csr);
            if rs1 != 0 { ctx.write_csr(csr, old | ctx.read_int_reg(rs1 as usize)); }
            wri(ctx, rd, old);
        }
        CSRRC { rd, rs1, csr } => {
            let old = ctx.read_csr(csr);
            if rs1 != 0 { ctx.write_csr(csr, old & !ctx.read_int_reg(rs1 as usize)); }
            wri(ctx, rd, old);
        }
        CSRRWI { rd, uimm, csr } => {
            let old = ctx.read_csr(csr);
            ctx.write_csr(csr, uimm as u64);
            wri(ctx, rd, old);
        }
        CSRRSI { rd, uimm, csr } => {
            let old = ctx.read_csr(csr);
            if uimm != 0 { ctx.write_csr(csr, old | uimm as u64); }
            wri(ctx, rd, old);
        }
        CSRRCI { rd, uimm, csr } => {
            let old = ctx.read_csr(csr);
            if uimm != 0 { ctx.write_csr(csr, old & !(uimm as u64)); }
            wri(ctx, rd, old);
        }

        // ── M extension ───────────────────────────────────────────────────
        MUL    { rd, rs1, rs2 } => { let v = ctx.read_int_reg(rs1 as usize).wrapping_mul(ctx.read_int_reg(rs2 as usize)); wri(ctx, rd, v); }
        MULH   { rd, rs1, rs2 } => { let a = ctx.read_int_reg(rs1 as usize) as i64 as i128; let b = ctx.read_int_reg(rs2 as usize) as i64 as i128; wri(ctx, rd, ((a * b) >> 64) as u64); }
        MULHSU { rd, rs1, rs2 } => { let a = ctx.read_int_reg(rs1 as usize) as i64 as i128; let b = ctx.read_int_reg(rs2 as usize) as u128 as i128; wri(ctx, rd, ((a * b) >> 64) as u64); }
        MULHU  { rd, rs1, rs2 } => { let a = ctx.read_int_reg(rs1 as usize) as u128; let b = ctx.read_int_reg(rs2 as usize) as u128; wri(ctx, rd, ((a * b) >> 64) as u64); }
        DIV    { rd, rs1, rs2 } => { let a = ctx.read_int_reg(rs1 as usize) as i64; let b = ctx.read_int_reg(rs2 as usize) as i64; wri(ctx, rd, if b == 0 { u64::MAX } else { a.wrapping_div(b) as u64 }); }
        DIVU   { rd, rs1, rs2 } => { let a = ctx.read_int_reg(rs1 as usize); let b = ctx.read_int_reg(rs2 as usize); wri(ctx, rd, if b == 0 { u64::MAX } else { a / b }); }
        REM    { rd, rs1, rs2 } => { let a = ctx.read_int_reg(rs1 as usize) as i64; let b = ctx.read_int_reg(rs2 as usize) as i64; wri(ctx, rd, if b == 0 { a as u64 } else { a.wrapping_rem(b) as u64 }); }
        REMU   { rd, rs1, rs2 } => { let a = ctx.read_int_reg(rs1 as usize); let b = ctx.read_int_reg(rs2 as usize); wri(ctx, rd, if b == 0 { a } else { a % b }); }
        MULW   { rd, rs1, rs2 } => { let v = sext32((ctx.read_int_reg(rs1 as usize) as u32).wrapping_mul(ctx.read_int_reg(rs2 as usize) as u32)); wri(ctx, rd, v); }
        DIVW   { rd, rs1, rs2 } => { let a = ctx.read_int_reg(rs1 as usize) as i32; let b = ctx.read_int_reg(rs2 as usize) as i32; wri(ctx, rd, sext32(if b == 0 { -1i32 as u32 } else { a.wrapping_div(b) as u32 })); }
        DIVUW  { rd, rs1, rs2 } => { let a = ctx.read_int_reg(rs1 as usize) as u32; let b = ctx.read_int_reg(rs2 as usize) as u32; wri(ctx, rd, sext32(if b == 0 { u32::MAX } else { a / b })); }
        REMW   { rd, rs1, rs2 } => { let a = ctx.read_int_reg(rs1 as usize) as i32; let b = ctx.read_int_reg(rs2 as usize) as i32; wri(ctx, rd, sext32(if b == 0 { a as u32 } else { a.wrapping_rem(b) as u32 })); }
        REMUW  { rd, rs1, rs2 } => { let a = ctx.read_int_reg(rs1 as usize) as u32; let b = ctx.read_int_reg(rs2 as usize) as u32; wri(ctx, rd, sext32(if b == 0 { a } else { a % b })); }

        // ── A extension, F/D, privileged ─────────────────────────────────
        // TODO(phase-0): implement atomics, floating-point, and privileged instructions.
        insn => {
            return Err(HartException::Unsupported);
            #[allow(unreachable_code)]
            let _ = insn;
        }
    }

    // Default PC advance for non-control-flow instructions is handled inline above.
    // For instructions that do NOT explicitly write PC, advance by 4.
    // NOTE: all branches and jumps above write PC themselves, so we cannot do a
    // blanket `pc += 4` here. Each instruction must advance PC explicitly.

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Sign-extend a 32-bit value to 64 bits.
#[inline(always)]
fn sext32(v: u32) -> u64 { (v as i32) as u64 }

/// Write integer register and advance PC by 4 (convenience for non-CF instructions).
#[inline(always)]
fn wri(ctx: &mut impl ExecContext, rd: u8, val: u64) {
    ctx.write_int_reg(rd as usize, val);
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
}

/// Branch helper — takes comparison predicate.
#[inline(always)]
fn branch(
    ctx: &mut impl ExecContext,
    rs1: u8,
    rs2: u8,
    imm: i64,
    pred: impl Fn(u64, u64) -> bool,
) -> Result<(), HartException> {
    let a = ctx.read_int_reg(rs1 as usize);
    let b = ctx.read_int_reg(rs2 as usize);
    if pred(a, b) {
        let target = ctx.read_pc().wrapping_add(imm as u64);
        if target & 3 != 0 {
            return Err(HartException::InstructionAddressMisaligned { addr: target });
        }
        ctx.write_pc(target);
    } else {
        ctx.write_pc(ctx.read_pc().wrapping_add(4));
    }
    Ok(())
}

/// Load helper — reads `size` bytes from `rs1 + imm`, applies `extend` fn, writes to `rd`.
#[inline(always)]
fn load(
    ctx: &mut impl ExecContext,
    rd: u8,
    rs1: u8,
    imm: i64,
    size: usize,
    extend: impl Fn(u64) -> u64,
) -> Result<(), HartException> {
    let addr = ctx.read_int_reg(rs1 as usize).wrapping_add(imm as u64);
    let raw = ctx.read_mem(addr, size, AccessType::Load).map_err(|e| mem_fault_to_load(e, addr))?;
    wri(ctx, rd, extend(raw));
    Ok(())
}

/// Store helper — writes `rs2` (size bytes) to `rs1 + imm`.
#[inline(always)]
fn store(
    ctx: &mut impl ExecContext,
    rs1: u8,
    rs2: u8,
    imm: i64,
    size: usize,
) -> Result<(), HartException> {
    let addr = ctx.read_int_reg(rs1 as usize).wrapping_add(imm as u64);
    let val  = ctx.read_int_reg(rs2 as usize);
    ctx.write_mem(addr, size, val, AccessType::Store)
        .map_err(|e| mem_fault_to_store(e, addr))?;
    ctx.write_pc(ctx.read_pc().wrapping_add(4));
    Ok(())
}

fn mem_fault_to_load(e: MemFault, addr: u64) -> HartException {
    match e {
        MemFault::AccessFault { .. } | MemFault::ReadOnly { .. } => {
            HartException::LoadAccessFault { addr }
        }
        MemFault::PageFault { .. } => HartException::LoadAccessFault { addr },
        MemFault::AlignmentFault { .. } => HartException::LoadAccessFault { addr },
    }
}

fn mem_fault_to_store(e: MemFault, addr: u64) -> HartException {
    match e {
        _ => HartException::StoreAccessFault { addr },
    }
}
