//! TCG interpreter — walks a `TcgBlock`'s op sequence and executes it.
//!
//! The interpreter maintains a temporary register file (`Vec<u64>`) and
//! reads/writes guest architectural state via a flat register array.
//! Memory accesses go through `AddressSpace`.

use crate::block::TcgBlock;
use crate::ir::{TcgOp, TcgTemp};
use helm_core::HelmResult;
use helm_memory::address_space::AddressSpace;

/// Guest register IDs used by `ReadReg`/`WriteReg`.
/// 0-30 = X0-X30, 31 = SP, 32 = PC, 33 = NZCV.
pub const REG_SP: u16 = 31;
pub const REG_PC: u16 = 32;
pub const REG_NZCV: u16 = 33;
pub const NUM_REGS: usize = 34;

/// A memory access recorded during interpretation.
#[derive(Debug, Clone)]
pub struct MemAccess {
    pub addr: u64,
    pub size: usize,
    pub is_write: bool,
}

/// How the interpreter exited a block.
#[derive(Debug, Clone)]
pub enum InterpExit {
    /// Ran off the end of the op list — continue at next_pc.
    EndOfBlock { next_pc: u64 },
    /// `GotoTb` — chain to the given guest PC.
    Chain { target_pc: u64 },
    /// `Syscall` — host must handle syscall with the given number.
    Syscall { nr: u64 },
    /// `ExitTb` — return to the outer dispatcher.
    Exit,
}

/// Result of executing one translated block.
#[derive(Debug, Clone)]
pub struct InterpResult {
    /// How many guest instructions were covered by this block.
    pub insns_executed: usize,
    /// How the block ended.
    pub exit: InterpExit,
    /// Memory accesses performed during execution.
    pub mem_accesses: Vec<MemAccess>,
}

/// TCG block interpreter.
pub struct TcgInterp {
    temps: Vec<u64>,
}

impl TcgInterp {
    pub fn new() -> Self {
        Self { temps: Vec::new() }
    }

    /// Execute a translated block.
    ///
    /// `regs` is a flat array of 34 u64 values (X0-X30, SP, PC, NZCV).
    /// The interpreter modifies it in place via `ReadReg`/`WriteReg` ops.
    pub fn exec_block(
        &mut self,
        block: &TcgBlock,
        regs: &mut [u64; NUM_REGS],
        mem: &mut AddressSpace,
    ) -> HelmResult<InterpResult> {
        let ops = &block.ops;

        // Pre-allocate temps — find max temp index
        let max_temp = ops.iter().fold(0u32, |m, op| m.max(max_temp_in_op(op)));
        self.temps.clear();
        self.temps.resize((max_temp as usize) + 1, 0);

        // Build label → op-index map for branch targets
        let mut labels: Vec<(u32, usize)> = Vec::new();
        for (i, op) in ops.iter().enumerate() {
            if let TcgOp::Label { id } = op {
                labels.push((*id, i));
            }
        }

        let mut mem_accesses = Vec::new();
        let mut ip = 0usize; // instruction pointer into ops

        while ip < ops.len() {
            match &ops[ip] {
                // -- Moves and constants --
                TcgOp::Movi { dst, value } => {
                    self.set(dst, *value);
                }
                TcgOp::Mov { dst, src } => {
                    self.set(dst, self.get(src));
                }

                // -- Arithmetic --
                TcgOp::Add { dst, a, b } => {
                    self.set(dst, self.get(a).wrapping_add(self.get(b)));
                }
                TcgOp::Sub { dst, a, b } => {
                    self.set(dst, self.get(a).wrapping_sub(self.get(b)));
                }
                TcgOp::Mul { dst, a, b } => {
                    self.set(dst, self.get(a).wrapping_mul(self.get(b)));
                }
                TcgOp::Div { dst, a, b } => {
                    let bv = self.get(b);
                    self.set(dst, if bv == 0 { 0 } else { self.get(a) / bv });
                }
                TcgOp::Addi { dst, a, imm } => {
                    self.set(dst, self.get(a).wrapping_add(*imm as u64));
                }

                // -- Bitwise --
                TcgOp::And { dst, a, b } => {
                    self.set(dst, self.get(a) & self.get(b));
                }
                TcgOp::Or { dst, a, b } => {
                    self.set(dst, self.get(a) | self.get(b));
                }
                TcgOp::Xor { dst, a, b } => {
                    self.set(dst, self.get(a) ^ self.get(b));
                }
                TcgOp::Not { dst, src } => {
                    self.set(dst, !self.get(src));
                }
                TcgOp::Shl { dst, a, b } => {
                    let shift = self.get(b) & 63;
                    self.set(dst, self.get(a) << shift);
                }
                TcgOp::Shr { dst, a, b } => {
                    let shift = self.get(b) & 63;
                    self.set(dst, self.get(a) >> shift);
                }
                TcgOp::Sar { dst, a, b } => {
                    let shift = self.get(b) & 63;
                    self.set(dst, (self.get(a) as i64 >> shift) as u64);
                }

                // -- Memory --
                TcgOp::Load { dst, addr, size } => {
                    let a = self.get(addr);
                    let sz = *size as usize;
                    mem_accesses.push(MemAccess {
                        addr: a,
                        size: sz,
                        is_write: false,
                    });
                    let mut buf = [0u8; 8];
                    mem.read(a, &mut buf[..sz])?;
                    let val = match sz {
                        1 => buf[0] as u64,
                        2 => u16::from_le_bytes([buf[0], buf[1]]) as u64,
                        4 => u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64,
                        8 => u64::from_le_bytes(buf),
                        _ => 0,
                    };
                    self.set(dst, val);
                }
                TcgOp::Store { addr, val, size } => {
                    let a = self.get(addr);
                    let v = self.get(val);
                    let sz = *size as usize;
                    mem_accesses.push(MemAccess {
                        addr: a,
                        size: sz,
                        is_write: true,
                    });
                    let bytes = v.to_le_bytes();
                    mem.write(a, &bytes[..sz])?;
                }

                // -- Register access --
                TcgOp::ReadReg { dst, reg_id } => {
                    let val = regs[*reg_id as usize];
                    self.set(dst, val);
                }
                TcgOp::WriteReg { reg_id, src } => {
                    regs[*reg_id as usize] = self.get(src);
                }

                // -- Comparisons --
                TcgOp::SetEq { dst, a, b } => {
                    self.set(dst, if self.get(a) == self.get(b) { 1 } else { 0 });
                }
                TcgOp::SetNe { dst, a, b } => {
                    self.set(dst, if self.get(a) != self.get(b) { 1 } else { 0 });
                }
                TcgOp::SetLt { dst, a, b } => {
                    self.set(
                        dst,
                        if (self.get(a) as i64) < (self.get(b) as i64) {
                            1
                        } else {
                            0
                        },
                    );
                }
                TcgOp::SetGe { dst, a, b } => {
                    self.set(
                        dst,
                        if (self.get(a) as i64) >= (self.get(b) as i64) {
                            1
                        } else {
                            0
                        },
                    );
                }

                // -- Extensions --
                TcgOp::Sext {
                    dst,
                    src,
                    from_bits,
                } => {
                    let val = self.get(src);
                    let shift = 64 - *from_bits as u32;
                    self.set(dst, ((val << shift) as i64 >> shift) as u64);
                }
                TcgOp::Zext {
                    dst,
                    src,
                    from_bits,
                } => {
                    let val = self.get(src);
                    let mask = if *from_bits >= 64 {
                        u64::MAX
                    } else {
                        (1u64 << *from_bits) - 1
                    };
                    self.set(dst, val & mask);
                }

                // -- Control flow --
                TcgOp::Label { .. } => {
                    // no-op marker
                }
                TcgOp::Br { label } => {
                    if let Some((_, idx)) = labels.iter().find(|(id, _)| id == label) {
                        ip = *idx;
                        continue;
                    }
                }
                TcgOp::BrCond { cond, label } => {
                    if self.get(cond) != 0 {
                        if let Some((_, idx)) = labels.iter().find(|(id, _)| id == label) {
                            ip = *idx;
                            continue;
                        }
                    }
                }

                // -- System --
                TcgOp::GotoTb { target_pc } => {
                    return Ok(InterpResult {
                        insns_executed: block.insn_count,
                        exit: InterpExit::Chain {
                            target_pc: *target_pc,
                        },
                        mem_accesses,
                    });
                }
                TcgOp::Syscall { nr } => {
                    let nr_val = self.get(nr);
                    return Ok(InterpResult {
                        insns_executed: block.insn_count,
                        exit: InterpExit::Syscall { nr: nr_val },
                        mem_accesses,
                    });
                }
                TcgOp::ExitTb => {
                    return Ok(InterpResult {
                        insns_executed: block.insn_count,
                        exit: InterpExit::Exit,
                        mem_accesses,
                    });
                }
            }
            ip += 1;
        }

        // Fell off the end of the op list
        let next_pc = regs[REG_PC as usize];
        Ok(InterpResult {
            insns_executed: block.insn_count,
            exit: InterpExit::EndOfBlock { next_pc },
            mem_accesses,
        })
    }

    #[inline]
    fn get(&self, t: &TcgTemp) -> u64 {
        self.temps[t.0 as usize]
    }

    #[inline]
    fn set(&mut self, t: &TcgTemp, val: u64) {
        self.temps[t.0 as usize] = val;
    }
}

impl Default for TcgInterp {
    fn default() -> Self {
        Self::new()
    }
}

/// Find the maximum temp index referenced in an op.
fn max_temp_in_op(op: &TcgOp) -> u32 {
    match op {
        TcgOp::Movi { dst, .. } => dst.0,
        TcgOp::Mov { dst, src } => dst.0.max(src.0),
        TcgOp::Add { dst, a, b }
        | TcgOp::Sub { dst, a, b }
        | TcgOp::Mul { dst, a, b }
        | TcgOp::Div { dst, a, b }
        | TcgOp::And { dst, a, b }
        | TcgOp::Or { dst, a, b }
        | TcgOp::Xor { dst, a, b }
        | TcgOp::Shl { dst, a, b }
        | TcgOp::Shr { dst, a, b }
        | TcgOp::Sar { dst, a, b }
        | TcgOp::SetEq { dst, a, b }
        | TcgOp::SetNe { dst, a, b }
        | TcgOp::SetLt { dst, a, b }
        | TcgOp::SetGe { dst, a, b } => dst.0.max(a.0).max(b.0),
        TcgOp::Addi { dst, a, .. } => dst.0.max(a.0),
        TcgOp::Not { dst, src } | TcgOp::Sext { dst, src, .. } | TcgOp::Zext { dst, src, .. } => {
            dst.0.max(src.0)
        }
        TcgOp::Load { dst, addr, .. } => dst.0.max(addr.0),
        TcgOp::Store { addr, val, .. } => addr.0.max(val.0),
        TcgOp::ReadReg { dst, .. } => dst.0,
        TcgOp::WriteReg { src, .. } => src.0,
        TcgOp::BrCond { cond, .. } => cond.0,
        TcgOp::Syscall { nr } => nr.0,
        TcgOp::Br { .. } | TcgOp::Label { .. } | TcgOp::ExitTb | TcgOp::GotoTb { .. } => 0,
    }
}
