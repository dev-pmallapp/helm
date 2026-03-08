//! Threaded TCG interpreter — flat bytecode + function-pointer dispatch.
//!
//! Converts `Vec<TcgOp>` into a compact bytecode representation and
//! executes it via a function pointer table, avoiding the overhead of
//! `match` dispatch on every op.
//!
//! Layout: each "instruction" is a fixed-size slot:
//!   `[opcode: u8, dst: u16, src1: u16, src2: u16, imm: i64]`
//! packed into `[u64; 2]` for alignment.

use crate::block::TcgBlock;
use crate::interp::{InterpExit, InterpResult, MemAccess, NUM_REGS};
use crate::ir::{TcgOp, TcgTemp};
use helm_core::HelmResult;
use helm_memory::address_space::AddressSpace;
use std::collections::HashMap;

// ── Compact bytecode encoding ──────────────────────────────────────

/// Opcodes for the threaded interpreter bytecode.
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
enum Op {
    Movi = 0,
    Mov,
    Add,
    Sub,
    Mul,
    Div,
    Addi,
    And,
    Or,
    Xor,
    Not,
    Shl,
    Shr,
    Sar,
    Load,
    Store,
    ReadReg,
    WriteReg,
    SetEq,
    SetNe,
    SetLt,
    SetGe,
    Sext,
    Zext,
    Label,
    Br,
    BrCond,
    GotoTb,
    Syscall,
    ExitTb,
    ReadSysReg,
    WriteSysReg,
    DaifSet,
    DaifClr,
    SetSpSel,
    SvcExc,
    Eret,
    Wfi,
    DcZva,
    Tlbi,
    At,
    Barrier,
    Clrex,
    HvcExc,
    SmcExc,
    BrkExc,
    HltExc,
    Cfinv,
    Nop,
}

/// A single bytecode instruction: opcode + 3 operands + immediate.
#[derive(Clone, Copy)]
struct ByteOp {
    op: u8,
    dst: u16,
    src1: u16,
    src2: u16,
    imm: i64,
}

/// Compiled bytecode block — ready for threaded execution.
pub struct CompiledBlock {
    ops: Vec<ByteOp>,
    /// label_id → op index mapping.
    labels: Vec<(u32, usize)>,
    /// Original block metadata.
    pub guest_pc: u64,
    pub insn_count: usize,
}

/// Compile a TcgBlock into flat bytecode.
pub fn compile_block(block: &TcgBlock) -> CompiledBlock {
    let mut ops = Vec::with_capacity(block.ops.len());
    let mut labels = Vec::new();

    for tcg_op in &block.ops {
        let bop = encode_op(tcg_op);
        if let TcgOp::Label { id } = tcg_op {
            labels.push((*id, ops.len()));
        }
        ops.push(bop);
    }

    CompiledBlock {
        ops,
        labels,
        guest_pc: block.guest_pc,
        insn_count: block.insn_count,
    }
}

fn encode_op(op: &TcgOp) -> ByteOp {
    match op {
        TcgOp::Movi { dst, value } => ByteOp {
            op: Op::Movi as u8, dst: dst.0 as u16, src1: 0, src2: 0, imm: *value as i64,
        },
        TcgOp::Mov { dst, src } => ByteOp {
            op: Op::Mov as u8, dst: dst.0 as u16, src1: src.0 as u16, src2: 0, imm: 0,
        },
        TcgOp::Add { dst, a, b } => ByteOp {
            op: Op::Add as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::Sub { dst, a, b } => ByteOp {
            op: Op::Sub as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::Mul { dst, a, b } => ByteOp {
            op: Op::Mul as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::Div { dst, a, b } => ByteOp {
            op: Op::Div as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::Addi { dst, a, imm } => ByteOp {
            op: Op::Addi as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: 0, imm: *imm,
        },
        TcgOp::And { dst, a, b } => ByteOp {
            op: Op::And as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::Or { dst, a, b } => ByteOp {
            op: Op::Or as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::Xor { dst, a, b } => ByteOp {
            op: Op::Xor as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::Not { dst, src } => ByteOp {
            op: Op::Not as u8, dst: dst.0 as u16, src1: src.0 as u16, src2: 0, imm: 0,
        },
        TcgOp::Shl { dst, a, b } => ByteOp {
            op: Op::Shl as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::Shr { dst, a, b } => ByteOp {
            op: Op::Shr as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::Sar { dst, a, b } => ByteOp {
            op: Op::Sar as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::Load { dst, addr, size } => ByteOp {
            op: Op::Load as u8, dst: dst.0 as u16, src1: addr.0 as u16, src2: 0, imm: *size as i64,
        },
        TcgOp::Store { addr, val, size } => ByteOp {
            op: Op::Store as u8, dst: 0, src1: addr.0 as u16, src2: val.0 as u16, imm: *size as i64,
        },
        TcgOp::ReadReg { dst, reg_id } => ByteOp {
            op: Op::ReadReg as u8, dst: dst.0 as u16, src1: *reg_id, src2: 0, imm: 0,
        },
        TcgOp::WriteReg { reg_id, src } => ByteOp {
            op: Op::WriteReg as u8, dst: *reg_id, src1: src.0 as u16, src2: 0, imm: 0,
        },
        TcgOp::SetEq { dst, a, b } => ByteOp {
            op: Op::SetEq as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::SetNe { dst, a, b } => ByteOp {
            op: Op::SetNe as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::SetLt { dst, a, b } => ByteOp {
            op: Op::SetLt as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::SetGe { dst, a, b } => ByteOp {
            op: Op::SetGe as u8, dst: dst.0 as u16, src1: a.0 as u16, src2: b.0 as u16, imm: 0,
        },
        TcgOp::Sext { dst, src, from_bits } => ByteOp {
            op: Op::Sext as u8, dst: dst.0 as u16, src1: src.0 as u16, src2: 0, imm: *from_bits as i64,
        },
        TcgOp::Zext { dst, src, from_bits } => ByteOp {
            op: Op::Zext as u8, dst: dst.0 as u16, src1: src.0 as u16, src2: 0, imm: *from_bits as i64,
        },
        TcgOp::Label { id } => ByteOp {
            op: Op::Label as u8, dst: 0, src1: 0, src2: 0, imm: *id as i64,
        },
        TcgOp::Br { label } => ByteOp {
            op: Op::Br as u8, dst: 0, src1: 0, src2: 0, imm: *label as i64,
        },
        TcgOp::BrCond { cond, label } => ByteOp {
            op: Op::BrCond as u8, dst: 0, src1: cond.0 as u16, src2: 0, imm: *label as i64,
        },
        TcgOp::GotoTb { target_pc } => ByteOp {
            op: Op::GotoTb as u8, dst: 0, src1: 0, src2: 0, imm: *target_pc as i64,
        },
        TcgOp::Syscall { nr } => ByteOp {
            op: Op::Syscall as u8, dst: 0, src1: nr.0 as u16, src2: 0, imm: 0,
        },
        TcgOp::ExitTb => ByteOp {
            op: Op::ExitTb as u8, dst: 0, src1: 0, src2: 0, imm: 0,
        },
        TcgOp::ReadSysReg { dst, sysreg_id } => ByteOp {
            op: Op::ReadSysReg as u8, dst: dst.0 as u16, src1: 0, src2: 0, imm: *sysreg_id as i64,
        },
        TcgOp::WriteSysReg { sysreg_id, src } => ByteOp {
            op: Op::WriteSysReg as u8, dst: 0, src1: src.0 as u16, src2: 0, imm: *sysreg_id as i64,
        },
        TcgOp::DaifSet { imm } => ByteOp {
            op: Op::DaifSet as u8, dst: 0, src1: 0, src2: 0, imm: *imm as i64,
        },
        TcgOp::DaifClr { imm } => ByteOp {
            op: Op::DaifClr as u8, dst: 0, src1: 0, src2: 0, imm: *imm as i64,
        },
        TcgOp::SetSpSel { imm } => ByteOp {
            op: Op::SetSpSel as u8, dst: 0, src1: 0, src2: 0, imm: *imm as i64,
        },
        TcgOp::SvcExc { imm16 } => ByteOp {
            op: Op::SvcExc as u8, dst: 0, src1: 0, src2: 0, imm: *imm16 as i64,
        },
        TcgOp::Eret => ByteOp {
            op: Op::Eret as u8, dst: 0, src1: 0, src2: 0, imm: 0,
        },
        TcgOp::Wfi => ByteOp {
            op: Op::Wfi as u8, dst: 0, src1: 0, src2: 0, imm: 0,
        },
        TcgOp::DcZva { addr } => ByteOp {
            op: Op::DcZva as u8, dst: 0, src1: addr.0 as u16, src2: 0, imm: 0,
        },
        TcgOp::Tlbi { op, addr } => ByteOp {
            op: Op::Tlbi as u8, dst: 0, src1: addr.0 as u16, src2: 0, imm: *op as i64,
        },
        TcgOp::At { op, addr } => ByteOp {
            op: Op::At as u8, dst: 0, src1: addr.0 as u16, src2: 0, imm: *op as i64,
        },
        TcgOp::Barrier { kind } => ByteOp {
            op: Op::Barrier as u8, dst: 0, src1: 0, src2: 0, imm: *kind as i64,
        },
        TcgOp::Clrex => ByteOp {
            op: Op::Clrex as u8, dst: 0, src1: 0, src2: 0, imm: 0,
        },
        TcgOp::HvcExc { imm16 } => ByteOp {
            op: Op::HvcExc as u8, dst: 0, src1: 0, src2: 0, imm: *imm16 as i64,
        },
        TcgOp::SmcExc { imm16 } => ByteOp {
            op: Op::SmcExc as u8, dst: 0, src1: 0, src2: 0, imm: *imm16 as i64,
        },
        TcgOp::BrkExc { imm16 } => ByteOp {
            op: Op::BrkExc as u8, dst: 0, src1: 0, src2: 0, imm: *imm16 as i64,
        },
        TcgOp::HltExc { imm16 } => ByteOp {
            op: Op::HltExc as u8, dst: 0, src1: 0, src2: 0, imm: *imm16 as i64,
        },
        TcgOp::Cfinv => ByteOp {
            op: Op::Cfinv as u8, dst: 0, src1: 0, src2: 0, imm: 0,
        },
    }
}

// ── Threaded interpreter ───────────────────────────────────────────

/// Execution state passed through the threaded dispatch.
struct ExecState<'a> {
    temps: &'a mut [u64],
    regs: &'a mut [u64; NUM_REGS],
    mem: &'a mut AddressSpace,
    sysregs: &'a mut HashMap<u32, u64>,
    labels: &'a [(u32, usize)],
    mem_accesses: Vec<MemAccess>,
    exit: Option<InterpExit>,
}

/// Execute a compiled block using the threaded dispatch.
pub fn exec_threaded(
    block: &CompiledBlock,
    regs: &mut [u64; NUM_REGS],
    mem: &mut AddressSpace,
    sysregs: &mut HashMap<u32, u64>,
) -> HelmResult<InterpResult> {
    let max_temp = block.ops.iter().fold(0u16, |m, op| {
        m.max(op.dst).max(op.src1).max(op.src2)
    });
    let mut temps = vec![0u64; (max_temp as usize) + 1];

    let mut state = ExecState {
        temps: &mut temps,
        regs,
        mem,
        sysregs,
        labels: &block.labels,
        mem_accesses: Vec::new(),
        exit: None,
    };

    let ops = &block.ops;
    let mut ip = 0usize;

    // The core dispatch loop — uses a lookup table instead of match
    while ip < ops.len() {
        let bop = &ops[ip];
        // Direct dispatch via opcode index
        ip = dispatch(&mut state, bop, ip);
        if state.exit.is_some() {
            break;
        }
    }

    let pc = state.regs[crate::interp::REG_PC as usize];
    let exit = state.exit.take().unwrap_or(InterpExit::EndOfBlock { next_pc: pc });
    let mem_accesses = std::mem::take(&mut state.mem_accesses);

    Ok(InterpResult {
        insns_executed: block.insn_count,
        exit,
        mem_accesses,
    })
}

/// Dispatch a single bytecode op. Returns the next IP.
#[inline(always)]
fn dispatch(s: &mut ExecState, bop: &ByteOp, ip: usize) -> usize {
    use crate::interp::*;

    match bop.op {
        x if x == Op::Movi as u8 => {
            s.temps[bop.dst as usize] = bop.imm as u64;
        }
        x if x == Op::Mov as u8 => {
            s.temps[bop.dst as usize] = s.temps[bop.src1 as usize];
        }
        x if x == Op::Add as u8 => {
            s.temps[bop.dst as usize] = s.temps[bop.src1 as usize].wrapping_add(s.temps[bop.src2 as usize]);
        }
        x if x == Op::Sub as u8 => {
            s.temps[bop.dst as usize] = s.temps[bop.src1 as usize].wrapping_sub(s.temps[bop.src2 as usize]);
        }
        x if x == Op::Mul as u8 => {
            s.temps[bop.dst as usize] = s.temps[bop.src1 as usize].wrapping_mul(s.temps[bop.src2 as usize]);
        }
        x if x == Op::Div as u8 => {
            let b = s.temps[bop.src2 as usize];
            s.temps[bop.dst as usize] = if b == 0 { 0 } else { s.temps[bop.src1 as usize] / b };
        }
        x if x == Op::Addi as u8 => {
            s.temps[bop.dst as usize] = s.temps[bop.src1 as usize].wrapping_add(bop.imm as u64);
        }
        x if x == Op::And as u8 => {
            s.temps[bop.dst as usize] = s.temps[bop.src1 as usize] & s.temps[bop.src2 as usize];
        }
        x if x == Op::Or as u8 => {
            s.temps[bop.dst as usize] = s.temps[bop.src1 as usize] | s.temps[bop.src2 as usize];
        }
        x if x == Op::Xor as u8 => {
            s.temps[bop.dst as usize] = s.temps[bop.src1 as usize] ^ s.temps[bop.src2 as usize];
        }
        x if x == Op::Not as u8 => {
            s.temps[bop.dst as usize] = !s.temps[bop.src1 as usize];
        }
        x if x == Op::Shl as u8 => {
            let shift = s.temps[bop.src2 as usize] & 63;
            s.temps[bop.dst as usize] = s.temps[bop.src1 as usize] << shift;
        }
        x if x == Op::Shr as u8 => {
            let shift = s.temps[bop.src2 as usize] & 63;
            s.temps[bop.dst as usize] = s.temps[bop.src1 as usize] >> shift;
        }
        x if x == Op::Sar as u8 => {
            let shift = s.temps[bop.src2 as usize] & 63;
            s.temps[bop.dst as usize] = (s.temps[bop.src1 as usize] as i64 >> shift) as u64;
        }
        x if x == Op::Load as u8 => {
            let addr = s.temps[bop.src1 as usize];
            let sz = bop.imm as usize;
            s.mem_accesses.push(MemAccess { addr, size: sz, is_write: false });
            let mut buf = [0u8; 8];
            if s.mem.read(addr, &mut buf[..sz]).is_ok() {
                s.temps[bop.dst as usize] = match sz {
                    1 => buf[0] as u64,
                    2 => u16::from_le_bytes([buf[0], buf[1]]) as u64,
                    4 => u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64,
                    8 => u64::from_le_bytes(buf),
                    _ => 0,
                };
            }
        }
        x if x == Op::Store as u8 => {
            let addr = s.temps[bop.src1 as usize];
            let val = s.temps[bop.src2 as usize];
            let sz = bop.imm as usize;
            s.mem_accesses.push(MemAccess { addr, size: sz, is_write: true });
            let bytes = val.to_le_bytes();
            let _ = s.mem.write(addr, &bytes[..sz]);
        }
        x if x == Op::ReadReg as u8 => {
            s.temps[bop.dst as usize] = s.regs[bop.src1 as usize];
        }
        x if x == Op::WriteReg as u8 => {
            s.regs[bop.dst as usize] = s.temps[bop.src1 as usize];
        }
        x if x == Op::SetEq as u8 => {
            s.temps[bop.dst as usize] = if s.temps[bop.src1 as usize] == s.temps[bop.src2 as usize] { 1 } else { 0 };
        }
        x if x == Op::SetNe as u8 => {
            s.temps[bop.dst as usize] = if s.temps[bop.src1 as usize] != s.temps[bop.src2 as usize] { 1 } else { 0 };
        }
        x if x == Op::SetLt as u8 => {
            s.temps[bop.dst as usize] = if (s.temps[bop.src1 as usize] as i64) < (s.temps[bop.src2 as usize] as i64) { 1 } else { 0 };
        }
        x if x == Op::SetGe as u8 => {
            s.temps[bop.dst as usize] = if (s.temps[bop.src1 as usize] as i64) >= (s.temps[bop.src2 as usize] as i64) { 1 } else { 0 };
        }
        x if x == Op::Sext as u8 => {
            let val = s.temps[bop.src1 as usize];
            let bits = bop.imm as u32;
            let shift = 64 - bits;
            s.temps[bop.dst as usize] = ((val << shift) as i64 >> shift) as u64;
        }
        x if x == Op::Zext as u8 => {
            let val = s.temps[bop.src1 as usize];
            let bits = bop.imm as u32;
            let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
            s.temps[bop.dst as usize] = val & mask;
        }
        x if x == Op::Label as u8 => {} // no-op
        x if x == Op::Br as u8 => {
            let label = bop.imm as u32;
            if let Some((_, idx)) = s.labels.iter().find(|(id, _)| *id == label) {
                return *idx;
            }
        }
        x if x == Op::BrCond as u8 => {
            if s.temps[bop.src1 as usize] != 0 {
                let label = bop.imm as u32;
                if let Some((_, idx)) = s.labels.iter().find(|(id, _)| *id == label) {
                    return *idx;
                }
            }
        }
        x if x == Op::GotoTb as u8 => {
            s.exit = Some(InterpExit::Chain { target_pc: bop.imm as u64 });
            return ip + 1;
        }
        x if x == Op::Syscall as u8 => {
            s.exit = Some(InterpExit::Syscall { nr: s.temps[bop.src1 as usize] });
            return ip + 1;
        }
        x if x == Op::ExitTb as u8 => {
            s.exit = Some(InterpExit::Exit);
            return ip + 1;
        }
        x if x == Op::ReadSysReg as u8 => {
            let id = bop.imm as u32;
            s.temps[bop.dst as usize] = s.sysregs.get(&id).copied().unwrap_or(0);
        }
        x if x == Op::WriteSysReg as u8 => {
            let id = bop.imm as u32;
            let val = s.temps[bop.src1 as usize];
            s.sysregs.insert(id, val);
        }
        x if x == Op::DaifSet as u8 => {
            s.regs[REG_DAIF as usize] |= ((bop.imm as u64) & 0xF) << 6;
        }
        x if x == Op::DaifClr as u8 => {
            s.regs[REG_DAIF as usize] &= !(((bop.imm as u64) & 0xF) << 6);
        }
        x if x == Op::SetSpSel as u8 => {
            s.regs[REG_SPSEL as usize] = (bop.imm as u64) & 1;
        }
        x if x == Op::SvcExc as u8 => {
            // Save state and generate SVC exception
            let pc = s.regs[REG_PC as usize];
            s.regs[REG_ELR_EL1 as usize] = pc.wrapping_add(4);
            let nzcv = s.regs[REG_NZCV as usize] as u32;
            let daif = s.regs[REG_DAIF as usize] as u32;
            let el = (s.regs[REG_CURRENT_EL as usize] >> 2) as u32;
            let sp_sel = s.regs[REG_SPSEL as usize] as u32;
            let spsr = (nzcv & 0xF000_0000) | (daif & 0x3C0) | ((el & 3) << 2) | (sp_sel & 1);
            s.regs[REG_SPSR_EL1 as usize] = spsr as u64;
            s.regs[REG_ESR_EL1 as usize] = (0x15u64 << 26) | (1u64 << 25) | (bop.imm as u64 & 0xFFFF);
            s.regs[REG_DAIF as usize] |= 0x3C0;
            let vbar = s.regs[REG_VBAR_EL1 as usize];
            let offset: u64 = if el == 0 { 0x400 } else { 0x200 };
            s.regs[REG_PC as usize] = vbar.wrapping_add(offset);
            s.regs[REG_CURRENT_EL as usize] = 1 << 2;
            s.exit = Some(InterpExit::Exception { class: 0x15, iss: bop.imm as u32 });
            return ip + 1;
        }
        x if x == Op::Eret as u8 => {
            let elr = s.regs[REG_ELR_EL1 as usize];
            let spsr = s.regs[REG_SPSR_EL1 as usize] as u32;
            s.regs[REG_PC as usize] = elr;
            s.regs[REG_NZCV as usize] = (spsr & 0xF000_0000) as u64;
            s.regs[REG_DAIF as usize] = (spsr & 0x3C0) as u64;
            s.regs[REG_CURRENT_EL as usize] = (((spsr >> 2) & 3) as u64) << 2;
            s.regs[REG_SPSEL as usize] = (spsr & 1) as u64;
            s.exit = Some(InterpExit::ExceptionReturn);
            return ip + 1;
        }
        x if x == Op::Wfi as u8 => {
            s.exit = Some(InterpExit::Wfi);
            return ip + 1;
        }
        x if x == Op::DcZva as u8 => {
            let va = s.temps[bop.src1 as usize];
            let aligned = va & !63;
            s.mem_accesses.push(MemAccess { addr: aligned, size: 64, is_write: true });
            let _ = s.mem.write(aligned, &[0u8; 64]);
        }
        x if x == Op::Tlbi as u8 || x == Op::Barrier as u8 || x == Op::Clrex as u8 => {
            // No-ops in single-threaded interpreter
        }
        x if x == Op::At as u8 => {
            let va = s.temps[bop.src1 as usize];
            s.sysregs.insert(0xC3A0, va & !0xFFF);
        }
        x if x == Op::HvcExc as u8 => {
            s.exit = Some(InterpExit::Exception { class: 0x16, iss: bop.imm as u32 });
            return ip + 1;
        }
        x if x == Op::SmcExc as u8 => {
            s.exit = Some(InterpExit::Exception { class: 0x17, iss: bop.imm as u32 });
            return ip + 1;
        }
        x if x == Op::BrkExc as u8 => {
            s.exit = Some(InterpExit::Exception { class: 0x3C, iss: bop.imm as u32 });
            return ip + 1;
        }
        x if x == Op::HltExc as u8 => {
            s.exit = Some(InterpExit::Exception { class: 0x3E, iss: bop.imm as u32 });
            return ip + 1;
        }
        x if x == Op::Cfinv as u8 => {
            s.regs[REG_NZCV as usize] ^= 1 << 29;
        }
        _ => {} // unknown op — skip
    }
    ip + 1
}
