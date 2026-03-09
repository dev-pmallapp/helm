//! TCG interpreter — walks a `TcgBlock`'s op sequence and executes it.
//!
//! The interpreter maintains a temporary register file (`Vec<u64>`) and
//! reads/writes guest architectural state via a flat register array.
//! Memory accesses go through `AddressSpace`.

use crate::block::TcgBlock;
use crate::ir::{TcgOp, TcgTemp};
use helm_core::HelmResult;
use helm_memory::address_space::AddressSpace;

// AArch64 register constants — re-exported from target::aarch64::regs
// for backward compatibility. New code should use target::aarch64::regs directly.
pub use crate::target::aarch64::regs::SP as REG_SP;
pub use crate::target::aarch64::regs::PC as REG_PC;
pub use crate::target::aarch64::regs::NZCV as REG_NZCV;
pub use crate::target::aarch64::regs::DAIF as REG_DAIF;
pub use crate::target::aarch64::regs::ELR_EL1 as REG_ELR_EL1;
pub use crate::target::aarch64::regs::SPSR_EL1 as REG_SPSR_EL1;
pub use crate::target::aarch64::regs::ESR_EL1 as REG_ESR_EL1;
pub use crate::target::aarch64::regs::VBAR_EL1 as REG_VBAR_EL1;
pub use crate::target::aarch64::regs::CURRENT_EL as REG_CURRENT_EL;
pub use crate::target::aarch64::regs::SPSEL as REG_SPSEL;
pub use crate::target::aarch64::regs::SP_EL1 as REG_SP_EL1;
pub use crate::target::aarch64::regs::NUM_REGS;

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
    /// `Wfi` — CPU should halt until an interrupt is pending.
    Wfi,
    /// `SvcExc` — SVC in FS mode; outer loop must route the exception.
    Exception { class: u32, iss: u32 },
    /// `Eret` — exception return; outer loop restores PSTATE/EL.
    ExceptionReturn,
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
    /// System register file — flat array indexed directly by the
    /// 16-bit sysreg encoding.  All sysreg IDs are ≥ 0x8000 (op0=2|3),
    /// so we subtract 0x8000 and index into a 32768-entry array (256 KB).
    /// Only the ~30 hot entries occupy cache lines; the rest are cold.
    pub sysregs: Vec<u64>,
}

/// Offset subtracted from every sysreg ID before indexing.
/// All AArch64 sysregs have op0 ∈ {2, 3}, so the minimum encoded value
/// is `2 << 14 = 0x8000`.
pub const SYSREG_BASE: u32 = 0x8000;

/// Number of entries in the flat sysreg array (covers op0 = 2..3).
pub const SYSREG_FILE_SIZE: usize = 0x8000; // 32768 entries = 256 KB

/// Sentinel index used as an MMU-dirty flag.
///
/// Set to a non-zero value by `helm_sysreg_write` whenever a
/// page-table register (SCTLR_EL1, TCR_EL1, TTBR0_EL1, TTBR1_EL1)
/// is modified.  The session run-loop reads this index before each
/// JIT block and calls `sync_mmu_to_cpu` only when it is non-zero,
/// then clears it.  This avoids the 7-sysreg read + comparison on
/// every block boundary when the MMU configuration is stable.
///
/// **Index = 24323**, which sits in the **same 64-byte cache line**
/// as `CNTVCT_EL0` (index 24322, written every block).  Because
/// CNTVCT is written on every JIT block dispatch, its cache line is
/// always in L1 cache, making the dirty-flag check a guaranteed L1
/// hit rather than an L3 miss.
///
/// Sysreg ID `0xDF03` (op0=3, op1=3, CRn=14, CRm=0, op2=3) is
/// unallocated in AArch64 and never written by real guest code.
pub const MMU_DIRTY_IDX: usize = 24323; // same cache line as CNTVCT_EL0 (24322)

/// Convert a 16-bit sysreg ID to an array index.
#[inline(always)]
pub fn sysreg_idx(id: u32) -> usize {
    (id.wrapping_sub(SYSREG_BASE)) as usize & (SYSREG_FILE_SIZE - 1)
}

impl TcgInterp {
    pub fn new() -> Self {
        Self {
            temps: Vec::new(),
            sysregs: vec![0u64; SYSREG_FILE_SIZE],
        }
    }

    /// Pre-load a system register value before execution.
    pub fn set_sysreg(&mut self, id: u32, val: u64) {
        self.sysregs[sysreg_idx(id)] = val;
    }

    /// Read a system register value (returns 0 for unknown registers).
    pub fn get_sysreg(&self, id: u32) -> u64 {
        self.sysregs[sysreg_idx(id)]
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

                // -- System registers --
                TcgOp::ReadSysReg { dst, sysreg_id } => {
                    let val = self.sysregs[sysreg_idx(*sysreg_id)];
                    self.set(dst, val);
                }
                TcgOp::WriteSysReg { sysreg_id, src } => {
                    let val = self.get(src);
                    self.sysregs[sysreg_idx(*sysreg_id)] = val;
                }

                // -- PSTATE immediates --
                TcgOp::DaifSet { imm } => {
                    regs[REG_DAIF as usize] |= ((*imm & 0xF) as u64) << 6;
                }
                TcgOp::DaifClr { imm } => {
                    regs[REG_DAIF as usize] &= !(((*imm & 0xF) as u64) << 6);
                }
                TcgOp::SetSpSel { imm } => {
                    regs[REG_SPSEL as usize] = (*imm & 1) as u64;
                }

                // -- Exception generation --
                TcgOp::SvcExc { imm16 } => {
                    let pc = regs[REG_PC as usize];
                    regs[REG_ELR_EL1 as usize] = pc.wrapping_add(4);
                    let nzcv = regs[REG_NZCV as usize] as u32;
                    let daif = regs[REG_DAIF as usize] as u32;
                    let el = (regs[REG_CURRENT_EL as usize] >> 2) as u32;
                    let sp_sel = regs[REG_SPSEL as usize] as u32;
                    let spsr =
                        (nzcv & 0xF000_0000) | (daif & 0x3C0) | ((el & 3) << 2) | (sp_sel & 1);
                    regs[REG_SPSR_EL1 as usize] = spsr as u64;
                    regs[REG_ESR_EL1 as usize] =
                        (0x15u64 << 26) | (1u64 << 25) | (*imm16 as u64 & 0xFFFF);
                    regs[REG_DAIF as usize] |= 0x3C0;
                    let vbar = regs[REG_VBAR_EL1 as usize];
                    let offset: u64 = if el == 0 { 0x400 } else { 0x200 };
                    regs[REG_PC as usize] = vbar.wrapping_add(offset);
                    regs[REG_CURRENT_EL as usize] = 1 << 2;
                    return Ok(InterpResult {
                        insns_executed: block.insn_count,
                        exit: InterpExit::Exception {
                            class: 0x15,
                            iss: *imm16,
                        },
                        mem_accesses,
                    });
                }

                // -- Exception return --
                TcgOp::Eret => {
                    let elr = regs[REG_ELR_EL1 as usize];
                    let spsr = regs[REG_SPSR_EL1 as usize] as u32;
                    regs[REG_PC as usize] = elr;
                    regs[REG_NZCV as usize] = (spsr & 0xF000_0000) as u64;
                    regs[REG_DAIF as usize] = (spsr & 0x3C0) as u64;
                    regs[REG_CURRENT_EL as usize] = (((spsr >> 2) & 3) as u64) << 2;
                    regs[REG_SPSEL as usize] = (spsr & 1) as u64;
                    return Ok(InterpResult {
                        insns_executed: block.insn_count,
                        exit: InterpExit::ExceptionReturn,
                        mem_accesses,
                    });
                }

                // -- WFI --
                TcgOp::Wfi => {
                    return Ok(InterpResult {
                        insns_executed: block.insn_count,
                        exit: InterpExit::Wfi,
                        mem_accesses,
                    });
                }

                // -- Phase 5: cache/TLB/barriers --
                TcgOp::DcZva { addr } => {
                    let va = self.get(addr);
                    let block_size = 64u64;
                    let aligned = va & !(block_size - 1);
                    let zeros = [0u8; 64];
                    mem_accesses.push(MemAccess {
                        addr: aligned,
                        size: block_size as usize,
                        is_write: true,
                    });
                    let _ = mem.write(aligned, &zeros);
                }
                TcgOp::Tlbi { .. } => {
                    // TLB invalidation — the interpreter has no TLB cache,
                    // so this is a no-op.  The outer engine must flush its
                    // block cache after this block completes.
                }
                TcgOp::At { op: _, addr } => {
                    let va = self.get(addr);
                    let par = va & !0xFFF;
                    self.sysregs[sysreg_idx(0xC3A0)] = par;
                }
                TcgOp::Barrier { .. } => {
                    // Barriers are architectural ordering points.
                    // In a single-threaded interpreter they are no-ops;
                    // the outer engine flushes its block cache on ISB.
                }
                TcgOp::Clrex => {
                    // Clear the local exclusive monitor.  Our simplified
                    // LDXR/STXR model always succeeds, so this is a no-op.
                }

                // -- Phase 6: exception generation --
                TcgOp::HvcExc { imm16 } => {
                    return Ok(InterpResult {
                        insns_executed: block.insn_count,
                        exit: InterpExit::Exception {
                            class: 0x16,
                            iss: *imm16,
                        },
                        mem_accesses,
                    });
                }
                TcgOp::SmcExc { imm16 } => {
                    return Ok(InterpResult {
                        insns_executed: block.insn_count,
                        exit: InterpExit::Exception {
                            class: 0x17,
                            iss: *imm16,
                        },
                        mem_accesses,
                    });
                }
                TcgOp::BrkExc { imm16 } => {
                    return Ok(InterpResult {
                        insns_executed: block.insn_count,
                        exit: InterpExit::Exception {
                            class: 0x3C,
                            iss: *imm16,
                        },
                        mem_accesses,
                    });
                }
                TcgOp::HltExc { imm16 } => {
                    return Ok(InterpResult {
                        insns_executed: block.insn_count,
                        exit: InterpExit::Exception {
                            class: 0x3E,
                            iss: *imm16,
                        },
                        mem_accesses,
                    });
                }

                // -- Phase 8: flag manipulation --
                TcgOp::Cfinv => {
                    regs[REG_NZCV as usize] ^= 1 << 29; // toggle C flag
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
        TcgOp::ReadSysReg { dst, .. } => dst.0,
        TcgOp::WriteSysReg { src, .. } => src.0,
        TcgOp::DcZva { addr } => addr.0,
        TcgOp::Tlbi { addr, .. } => addr.0,
        TcgOp::At { addr, .. } => addr.0,
        TcgOp::Br { .. }
        | TcgOp::Label { .. }
        | TcgOp::ExitTb
        | TcgOp::GotoTb { .. }
        | TcgOp::DaifSet { .. }
        | TcgOp::DaifClr { .. }
        | TcgOp::SetSpSel { .. }
        | TcgOp::SvcExc { .. }
        | TcgOp::Eret
        | TcgOp::Wfi
        | TcgOp::Barrier { .. }
        | TcgOp::Clrex
        | TcgOp::HvcExc { .. }
        | TcgOp::SmcExc { .. }
        | TcgOp::BrkExc { .. }
        | TcgOp::HltExc { .. }
        | TcgOp::Cfinv => 0,
    }
}
