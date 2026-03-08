//! MicroOp conversion from LLVM IR
//!
//! This module converts LLVM IR instructions into MicroOps that can be
//! executed by the unified pipeline.

use crate::functional_units::FunctionalUnitType;
use crate::ir::{LLVMInstruction, LLVMType, LLVMValue};

/// Physical register identifier
pub type PhysReg = usize;

/// Memory operation size
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemSize {
    Byte,
    HalfWord,
    Word,
    DoubleWord,
}

impl MemSize {
    pub fn from_bits(bits: usize) -> Self {
        match bits {
            8 => Self::Byte,
            16 => Self::HalfWord,
            32 => Self::Word,
            64 => Self::DoubleWord,
            _ => Self::Word, // Default to word
        }
    }
}

/// MicroOp - unified execution representation
///
/// Both TCG and LLVM IR map to these MicroOps, which are then
/// executed by the unified pipeline.
#[derive(Debug, Clone)]
pub enum MicroOp {
    // Integer arithmetic
    IntAdd {
        dest: PhysReg,
        src1: PhysReg,
        src2: PhysReg,
    },
    IntSub {
        dest: PhysReg,
        src1: PhysReg,
        src2: PhysReg,
    },
    IntMul {
        dest: PhysReg,
        src1: PhysReg,
        src2: PhysReg,
    },
    IntDiv {
        dest: PhysReg,
        src1: PhysReg,
        src2: PhysReg,
    },

    // Floating-point arithmetic
    FPAdd {
        dest: PhysReg,
        src1: PhysReg,
        src2: PhysReg,
        double_precision: bool,
    },
    FPMul {
        dest: PhysReg,
        src1: PhysReg,
        src2: PhysReg,
        double_precision: bool,
    },

    // Memory operations
    Load {
        dest: PhysReg,
        addr: PhysReg,
        size: MemSize,
    },
    Store {
        src: PhysReg,
        addr: PhysReg,
        size: MemSize,
    },

    // Bitwise operations
    And {
        dest: PhysReg,
        src1: PhysReg,
        src2: PhysReg,
    },
    Or {
        dest: PhysReg,
        src1: PhysReg,
        src2: PhysReg,
    },
    Xor {
        dest: PhysReg,
        src1: PhysReg,
        src2: PhysReg,
    },
    Shl {
        dest: PhysReg,
        src1: PhysReg,
        amount: PhysReg,
    },

    // Comparison
    Compare {
        dest: PhysReg,
        src1: PhysReg,
        src2: PhysReg,
        op: CompareOp,
    },

    // Control flow
    Branch {
        target_bb: usize,
    },
    CondBranch {
        condition: PhysReg,
        true_bb: usize,
        false_bb: usize,
    },

    // Move/Copy
    Move {
        dest: PhysReg,
        src: PhysReg,
    },

    // Load immediate
    LoadImm {
        dest: PhysReg,
        value: i64,
    },

    // No-op
    Nop,
}

impl MicroOp {
    /// Get the functional unit type required for this operation
    pub fn functional_unit(&self) -> FunctionalUnitType {
        match self {
            Self::IntAdd { .. } | Self::IntSub { .. } => FunctionalUnitType::IntAdder,
            Self::IntMul { .. } => FunctionalUnitType::IntMultiplier,
            Self::IntDiv { .. } => FunctionalUnitType::IntDivider,
            Self::FPAdd {
                double_precision, ..
            } => {
                if *double_precision {
                    FunctionalUnitType::FPDPAdder
                } else {
                    FunctionalUnitType::FPSPAdder
                }
            }
            Self::FPMul {
                double_precision, ..
            } => {
                if *double_precision {
                    FunctionalUnitType::FPDPMultiplier
                } else {
                    FunctionalUnitType::FPSPMultiplier
                }
            }
            Self::Load { .. } | Self::Store { .. } => FunctionalUnitType::LoadStore,
            Self::And { .. } | Self::Or { .. } | Self::Xor { .. } | Self::Shl { .. } => {
                FunctionalUnitType::IntBit
            }
            Self::Compare { .. } => FunctionalUnitType::Compare,
            Self::Branch { .. } | Self::CondBranch { .. } => FunctionalUnitType::Branch,
            Self::Move { .. } | Self::LoadImm { .. } | Self::Nop => {
                FunctionalUnitType::IntAdder // Simple ops use adder
            }
        }
    }

    /// Get default latency for this operation
    pub fn default_latency(&self) -> u32 {
        match self {
            Self::IntAdd { .. } | Self::IntSub { .. } => 1,
            Self::IntMul { .. } => 3,
            Self::IntDiv { .. } => 10,
            Self::FPAdd { .. } => 4,
            Self::FPMul { .. } => 5,
            Self::Load { .. } => 2, // Cache hit
            Self::Store { .. } => 1,
            Self::And { .. } | Self::Or { .. } | Self::Xor { .. } | Self::Shl { .. } => 1,
            Self::Compare { .. } => 1,
            Self::Branch { .. } | Self::CondBranch { .. } => 1,
            Self::Move { .. } | Self::LoadImm { .. } => 1,
            Self::Nop => 0,
        }
    }

    /// Get source registers
    pub fn sources(&self) -> Vec<PhysReg> {
        match self {
            Self::IntAdd { src1, src2, .. }
            | Self::IntSub { src1, src2, .. }
            | Self::IntMul { src1, src2, .. }
            | Self::IntDiv { src1, src2, .. }
            | Self::FPAdd { src1, src2, .. }
            | Self::FPMul { src1, src2, .. }
            | Self::And { src1, src2, .. }
            | Self::Or { src1, src2, .. }
            | Self::Xor { src1, src2, .. }
            | Self::Compare { src1, src2, .. } => vec![*src1, *src2],
            Self::Shl { src1, amount, .. } => vec![*src1, *amount],
            Self::Load { addr, .. } => vec![*addr],
            Self::Store { src, addr, .. } => vec![*src, *addr],
            Self::CondBranch { condition, .. } => vec![*condition],
            Self::Move { src, .. } => vec![*src],
            _ => vec![],
        }
    }

    /// Get destination register
    pub fn dest(&self) -> Option<PhysReg> {
        match self {
            Self::IntAdd { dest, .. }
            | Self::IntSub { dest, .. }
            | Self::IntMul { dest, .. }
            | Self::IntDiv { dest, .. }
            | Self::FPAdd { dest, .. }
            | Self::FPMul { dest, .. }
            | Self::Load { dest, .. }
            | Self::And { dest, .. }
            | Self::Or { dest, .. }
            | Self::Xor { dest, .. }
            | Self::Shl { dest, .. }
            | Self::Compare { dest, .. }
            | Self::Move { dest, .. }
            | Self::LoadImm { dest, .. } => Some(*dest),
            _ => None,
        }
    }
}

/// Comparison operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    EQ, // equal
    NE, // not equal
    LT, // less than
    LE, // less or equal
    GT, // greater than
    GE, // greater or equal
}

/// Context for LLVM → MicroOp conversion
pub struct ConversionContext {
    /// Mapping from LLVM values to physical registers
    value_to_reg: std::collections::HashMap<LLVMValue, PhysReg>,
    /// Next available physical register
    next_reg: PhysReg,
    /// Basic block label to ID mapping
    bb_labels: std::collections::HashMap<String, usize>,
}

impl ConversionContext {
    pub fn new() -> Self {
        Self {
            value_to_reg: std::collections::HashMap::new(),
            next_reg: 0,
            bb_labels: std::collections::HashMap::new(),
        }
    }

    /// Get or allocate a physical register for an LLVM value
    pub fn get_or_alloc_reg(&mut self, value: &LLVMValue) -> PhysReg {
        if let Some(&reg) = self.value_to_reg.get(value) {
            return reg;
        }

        let reg = self.next_reg;
        self.next_reg += 1;
        self.value_to_reg.insert(value.clone(), reg);
        reg
    }

    /// Map basic block label to ID
    pub fn map_bb_label(&mut self, label: String, id: usize) {
        self.bb_labels.insert(label, id);
    }

    /// Get basic block ID from label
    pub fn get_bb_id(&self, label: &str) -> Option<usize> {
        self.bb_labels.get(label).copied()
    }
}

impl Default for ConversionContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert LLVM IR instruction to MicroOps
pub fn llvm_to_micro_ops(inst: &LLVMInstruction, ctx: &mut ConversionContext) -> Vec<MicroOp> {
    match inst {
        LLVMInstruction::Add { dest, lhs, rhs, ty } => {
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let lhs_reg = ctx.get_or_alloc_reg(lhs);
            let rhs_reg = ctx.get_or_alloc_reg(rhs);
            log::trace!("Add ({ty:?}): r{dest_reg} = r{lhs_reg} + r{rhs_reg}");
            vec![MicroOp::IntAdd {
                dest: dest_reg,
                src1: lhs_reg,
                src2: rhs_reg,
            }]
        }

        LLVMInstruction::FAdd { dest, lhs, rhs, ty } => {
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let lhs_reg = ctx.get_or_alloc_reg(lhs);
            let rhs_reg = ctx.get_or_alloc_reg(rhs);
            vec![MicroOp::FPAdd {
                dest: dest_reg,
                src1: lhs_reg,
                src2: rhs_reg,
                double_precision: ty.is_fp() && ty == &LLVMType::Double,
            }]
        }

        LLVMInstruction::Load { dest, ptr, ty } => {
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let ptr_reg = ctx.get_or_alloc_reg(ptr);
            vec![MicroOp::Load {
                dest: dest_reg,
                addr: ptr_reg,
                size: MemSize::from_bits(ty.size_bits()),
            }]
        }

        LLVMInstruction::Store { value, ptr } => {
            let src_reg = ctx.get_or_alloc_reg(value);
            let ptr_reg = ctx.get_or_alloc_reg(ptr);
            vec![MicroOp::Store {
                src: src_reg,
                addr: ptr_reg,
                size: MemSize::Word, // TODO: Get actual size from value type
            }]
        }

        LLVMInstruction::Sub { dest, lhs, rhs, .. } => {
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let lhs_reg = ctx.get_or_alloc_reg(lhs);
            let rhs_reg = ctx.get_or_alloc_reg(rhs);
            vec![MicroOp::IntSub {
                dest: dest_reg,
                src1: lhs_reg,
                src2: rhs_reg,
            }]
        }

        LLVMInstruction::Mul { dest, lhs, rhs, .. } => {
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let lhs_reg = ctx.get_or_alloc_reg(lhs);
            let rhs_reg = ctx.get_or_alloc_reg(rhs);
            vec![MicroOp::IntMul {
                dest: dest_reg,
                src1: lhs_reg,
                src2: rhs_reg,
            }]
        }

        LLVMInstruction::Div { dest, lhs, rhs, .. } => {
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let lhs_reg = ctx.get_or_alloc_reg(lhs);
            let rhs_reg = ctx.get_or_alloc_reg(rhs);
            vec![MicroOp::IntDiv {
                dest: dest_reg,
                src1: lhs_reg,
                src2: rhs_reg,
            }]
        }

        LLVMInstruction::FMul { dest, lhs, rhs, ty } => {
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let lhs_reg = ctx.get_or_alloc_reg(lhs);
            let rhs_reg = ctx.get_or_alloc_reg(rhs);
            vec![MicroOp::FPMul {
                dest: dest_reg,
                src1: lhs_reg,
                src2: rhs_reg,
                double_precision: ty == &LLVMType::Double,
            }]
        }

        LLVMInstruction::And { dest, lhs, rhs } => {
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let lhs_reg = ctx.get_or_alloc_reg(lhs);
            let rhs_reg = ctx.get_or_alloc_reg(rhs);
            vec![MicroOp::And {
                dest: dest_reg,
                src1: lhs_reg,
                src2: rhs_reg,
            }]
        }

        LLVMInstruction::Or { dest, lhs, rhs } => {
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let lhs_reg = ctx.get_or_alloc_reg(lhs);
            let rhs_reg = ctx.get_or_alloc_reg(rhs);
            vec![MicroOp::Or {
                dest: dest_reg,
                src1: lhs_reg,
                src2: rhs_reg,
            }]
        }

        LLVMInstruction::Xor { dest, lhs, rhs } => {
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let lhs_reg = ctx.get_or_alloc_reg(lhs);
            let rhs_reg = ctx.get_or_alloc_reg(rhs);
            vec![MicroOp::Xor {
                dest: dest_reg,
                src1: lhs_reg,
                src2: rhs_reg,
            }]
        }

        LLVMInstruction::Shl { dest, lhs, rhs } => {
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let lhs_reg = ctx.get_or_alloc_reg(lhs);
            let rhs_reg = ctx.get_or_alloc_reg(rhs);
            vec![MicroOp::Shl {
                dest: dest_reg,
                src1: lhs_reg,
                amount: rhs_reg,
            }]
        }

        LLVMInstruction::ICmp {
            dest,
            predicate,
            lhs,
            rhs,
        } => {
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let lhs_reg = ctx.get_or_alloc_reg(lhs);
            let rhs_reg = ctx.get_or_alloc_reg(rhs);

            use crate::ir::ICmpPredicate;
            let op = match predicate {
                ICmpPredicate::EQ => CompareOp::EQ,
                ICmpPredicate::NE => CompareOp::NE,
                ICmpPredicate::ULT | ICmpPredicate::SLT => CompareOp::LT,
                ICmpPredicate::ULE | ICmpPredicate::SLE => CompareOp::LE,
                ICmpPredicate::UGT | ICmpPredicate::SGT => CompareOp::GT,
                ICmpPredicate::UGE | ICmpPredicate::SGE => CompareOp::GE,
            };

            vec![MicroOp::Compare {
                dest: dest_reg,
                src1: lhs_reg,
                src2: rhs_reg,
                op,
            }]
        }

        LLVMInstruction::Br { target } => {
            let bb_id = ctx.get_bb_id(target).unwrap_or(0);
            vec![MicroOp::Branch { target_bb: bb_id }]
        }

        LLVMInstruction::CondBr {
            condition,
            true_target,
            false_target,
        } => {
            let cond_reg = ctx.get_or_alloc_reg(condition);
            let true_bb = ctx.get_bb_id(true_target).unwrap_or(0);
            let false_bb = ctx.get_bb_id(false_target).unwrap_or(0);
            vec![MicroOp::CondBranch {
                condition: cond_reg,
                true_bb,
                false_bb,
            }]
        }

        LLVMInstruction::Ret { value } => {
            if let Some(val) = value {
                let src = ctx.get_or_alloc_reg(val);
                vec![MicroOp::Move { dest: 0, src }]
            } else {
                vec![MicroOp::Nop]
            }
        }

        LLVMInstruction::GetElementPtr {
            dest,
            base,
            indices,
        } => {
            // GEP expands to address calculation MicroOps
            // Simplified: base + sum(indices * scale)
            let mut ops = Vec::new();
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let base_reg = ctx.get_or_alloc_reg(base);

            if indices.is_empty() {
                // Just move base to dest
                ops.push(MicroOp::Move {
                    dest: dest_reg,
                    src: base_reg,
                });
            } else {
                // Start with base
                let mut current_reg = base_reg;

                // Add each index (simplified - assumes byte offsets)
                for (i, index) in indices.iter().enumerate() {
                    let index_reg = ctx.get_or_alloc_reg(index);
                    let result_reg = if i == indices.len() - 1 {
                        dest_reg
                    } else {
                        ctx.next_reg
                    };
                    ctx.next_reg += 1;

                    ops.push(MicroOp::IntAdd {
                        dest: result_reg,
                        src1: current_reg,
                        src2: index_reg,
                    });

                    current_reg = result_reg;
                }
            }

            ops
        }

        LLVMInstruction::Phi { dest, incoming } => {
            // Phi nodes are resolved at runtime based on predecessor block
            // For now, we'll use a move from the first incoming value
            // Proper implementation needs control flow tracking
            let dest_reg = ctx.get_or_alloc_reg(dest);
            if let Some((value, _)) = incoming.first() {
                let src_reg = ctx.get_or_alloc_reg(value);
                vec![MicroOp::Move {
                    dest: dest_reg,
                    src: src_reg,
                }]
            } else {
                vec![MicroOp::Nop]
            }
        }

        LLVMInstruction::Trunc { dest, value, .. }
        | LLVMInstruction::ZExt { dest, value, .. }
        | LLVMInstruction::SExt { dest, value, .. } => {
            // Type conversions are modeled as moves
            // Actual conversion logic would be in execution stage
            let dest_reg = ctx.get_or_alloc_reg(dest);
            let src_reg = ctx.get_or_alloc_reg(value);
            vec![MicroOp::Move {
                dest: dest_reg,
                src: src_reg,
            }]
        }

        LLVMInstruction::Call {
            dest,
            function,
            args,
        } => {
            // Function calls are complex - simplified for now
            // Real implementation would need to inline or model call overhead
            log::warn!("Function call to {} not fully implemented", function);
            log::trace!("  args: {args:?}");

            if let Some(dest) = dest {
                let dest_reg = ctx.get_or_alloc_reg(dest);
                vec![MicroOp::LoadImm {
                    dest: dest_reg,
                    value: 0, // Placeholder
                }]
            } else {
                vec![MicroOp::Nop]
            }
        }
    }
}
