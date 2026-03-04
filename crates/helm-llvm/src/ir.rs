//! LLVM IR representation and parsing
//!
//! This module provides a simplified representation of LLVM IR suitable for
//! hardware accelerator simulation. It doesn't need the full complexity of LLVM IR,
//! just enough to model dataflow and dependencies.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::path::Path;

/// Simplified LLVM IR module
#[derive(Debug, Clone)]
pub struct LLVMModule {
    pub name: String,
    pub functions: Vec<LLVMFunction>,
    pub globals: HashMap<String, LLVMValue>,
}

impl LLVMModule {
    /// Parse LLVM IR from a file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_string(&content)
    }

    /// Parse LLVM IR from a string
    pub fn from_string(ir: &str) -> Result<Self> {
        // Use simple parser for basic LLVM IR
        let mut parser = crate::parser::LLVMParser::new(ir.to_string());
        parser.parse()
    }

    /// Get function by name
    pub fn get_function(&self, name: &str) -> Option<&LLVMFunction> {
        self.functions.iter().find(|f| f.name == name)
    }
}

/// LLVM function
#[derive(Debug, Clone)]
pub struct LLVMFunction {
    pub name: String,
    pub arguments: Vec<LLVMValue>,
    pub basic_blocks: Vec<LLVMBasicBlock>,
    pub return_type: LLVMType,
}

impl LLVMFunction {
    /// Get entry basic block
    pub fn entry_block(&self) -> Option<&LLVMBasicBlock> {
        self.basic_blocks.first()
    }
}

/// LLVM basic block
#[derive(Debug, Clone)]
pub struct LLVMBasicBlock {
    pub label: String,
    pub instructions: Vec<LLVMInstruction>,
    pub terminator: Option<Box<LLVMInstruction>>,
}

impl LLVMBasicBlock {
    /// Create a new basic block
    pub fn new(label: String) -> Self {
        Self {
            label,
            instructions: Vec::new(),
            terminator: None,
        }
    }

    /// Add an instruction
    pub fn add_instruction(&mut self, inst: LLVMInstruction) {
        self.instructions.push(inst);
    }

    /// Set terminator instruction
    pub fn set_terminator(&mut self, inst: LLVMInstruction) {
        self.terminator = Some(Box::new(inst));
    }
}

/// Simplified LLVM instruction representation
#[derive(Debug, Clone)]
pub enum LLVMInstruction {
    // Arithmetic operations
    Add {
        dest: LLVMValue,
        lhs: LLVMValue,
        rhs: LLVMValue,
        ty: LLVMType,
    },
    Sub {
        dest: LLVMValue,
        lhs: LLVMValue,
        rhs: LLVMValue,
        ty: LLVMType,
    },
    Mul {
        dest: LLVMValue,
        lhs: LLVMValue,
        rhs: LLVMValue,
        ty: LLVMType,
    },
    Div {
        dest: LLVMValue,
        lhs: LLVMValue,
        rhs: LLVMValue,
        ty: LLVMType,
    },

    // Floating-point operations
    FAdd {
        dest: LLVMValue,
        lhs: LLVMValue,
        rhs: LLVMValue,
        ty: LLVMType,
    },
    FMul {
        dest: LLVMValue,
        lhs: LLVMValue,
        rhs: LLVMValue,
        ty: LLVMType,
    },

    // Memory operations
    Load {
        dest: LLVMValue,
        ptr: LLVMValue,
        ty: LLVMType,
    },
    Store {
        value: LLVMValue,
        ptr: LLVMValue,
    },

    // Comparison
    ICmp {
        dest: LLVMValue,
        predicate: ICmpPredicate,
        lhs: LLVMValue,
        rhs: LLVMValue,
    },

    // Control flow
    Br {
        target: String,
    },
    CondBr {
        condition: LLVMValue,
        true_target: String,
        false_target: String,
    },
    Ret {
        value: Option<LLVMValue>,
    },

    // Bitwise operations
    And {
        dest: LLVMValue,
        lhs: LLVMValue,
        rhs: LLVMValue,
    },
    Or {
        dest: LLVMValue,
        lhs: LLVMValue,
        rhs: LLVMValue,
    },
    Xor {
        dest: LLVMValue,
        lhs: LLVMValue,
        rhs: LLVMValue,
    },
    Shl {
        dest: LLVMValue,
        lhs: LLVMValue,
        rhs: LLVMValue,
    },

    // Address calculation
    GetElementPtr {
        dest: LLVMValue,
        base: LLVMValue,
        indices: Vec<LLVMValue>,
    },

    // Phi node (SSA)
    Phi {
        dest: LLVMValue,
        incoming: Vec<(LLVMValue, String)>, // (value, block_label)
    },

    // Type conversion
    Trunc {
        dest: LLVMValue,
        value: LLVMValue,
        ty: LLVMType,
    },
    ZExt {
        dest: LLVMValue,
        value: LLVMValue,
        ty: LLVMType,
    },
    SExt {
        dest: LLVMValue,
        value: LLVMValue,
        ty: LLVMType,
    },

    // Function call
    Call {
        dest: Option<LLVMValue>,
        function: String,
        args: Vec<LLVMValue>,
    },
}

impl LLVMInstruction {
    /// Get the destination register/value if this instruction produces one
    pub fn dest(&self) -> Option<&LLVMValue> {
        match self {
            Self::Add { dest, .. }
            | Self::Sub { dest, .. }
            | Self::Mul { dest, .. }
            | Self::Div { dest, .. }
            | Self::FAdd { dest, .. }
            | Self::FMul { dest, .. }
            | Self::Load { dest, .. }
            | Self::ICmp { dest, .. }
            | Self::And { dest, .. }
            | Self::Or { dest, .. }
            | Self::Xor { dest, .. }
            | Self::Shl { dest, .. }
            | Self::GetElementPtr { dest, .. }
            | Self::Phi { dest, .. }
            | Self::Trunc { dest, .. }
            | Self::ZExt { dest, .. }
            | Self::SExt { dest, .. } => Some(dest),
            Self::Call { dest, .. } => dest.as_ref(),
            _ => None,
        }
    }

    /// Get all source operands
    pub fn operands(&self) -> Vec<&LLVMValue> {
        match self {
            Self::Add { lhs, rhs, .. }
            | Self::Sub { lhs, rhs, .. }
            | Self::Mul { lhs, rhs, .. }
            | Self::Div { lhs, rhs, .. }
            | Self::FAdd { lhs, rhs, .. }
            | Self::FMul { lhs, rhs, .. }
            | Self::ICmp { lhs, rhs, .. }
            | Self::And { lhs, rhs, .. }
            | Self::Or { lhs, rhs, .. }
            | Self::Xor { lhs, rhs, .. }
            | Self::Shl { lhs, rhs, .. } => vec![lhs, rhs],
            Self::Load { ptr, .. } => vec![ptr],
            Self::Store { value, ptr } => vec![value, ptr],
            Self::CondBr { condition, .. } => vec![condition],
            Self::Ret { value: Some(v) } => vec![v],
            Self::GetElementPtr { base, indices, .. } => {
                let mut ops = vec![base];
                ops.extend(indices.iter());
                ops
            }
            Self::Phi { incoming, .. } => incoming.iter().map(|(v, _)| v).collect(),
            Self::Trunc { value, .. }
            | Self::ZExt { value, .. }
            | Self::SExt { value, .. } => vec![value],
            Self::Call { args, .. } => args.iter().collect(),
            _ => vec![],
        }
    }
}

/// Integer comparison predicates
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ICmpPredicate {
    EQ,  // equal
    NE,  // not equal
    UGT, // unsigned greater than
    UGE, // unsigned greater or equal
    ULT, // unsigned less than
    ULE, // unsigned less or equal
    SGT, // signed greater than
    SGE, // signed greater or equal
    SLT, // signed less than
    SLE, // signed less or equal
}

/// LLVM value representation
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LLVMValue {
    /// Virtual register (SSA value)
    Register { name: String, id: usize },
    /// Constant integer
    ConstInt { value: i64, bits: u32 },
    /// Constant float
    ConstFloat { value: String }, // Store as string to avoid float comparison issues
    /// Global variable
    Global { name: String },
    /// Function argument
    Argument { index: usize, name: String },
}

impl LLVMValue {
    /// Create a new register
    pub fn register(name: String, id: usize) -> Self {
        Self::Register { name, id }
    }

    /// Create a constant integer
    pub fn const_int(value: i64, bits: u32) -> Self {
        Self::ConstInt { value, bits }
    }

    /// Check if this is a constant
    pub fn is_constant(&self) -> bool {
        matches!(self, Self::ConstInt { .. } | Self::ConstFloat { .. })
    }
}

/// LLVM type representation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LLVMType {
    Void,
    Integer { bits: u32 },
    Float,
    Double,
    Pointer { pointee: Box<LLVMType> },
    Array { element: Box<LLVMType>, size: usize },
    Struct { fields: Vec<LLVMType> },
    Vector { element: Box<LLVMType>, count: usize },
}

impl LLVMType {
    /// Get the size in bits
    pub fn size_bits(&self) -> usize {
        match self {
            Self::Void => 0,
            Self::Integer { bits } => *bits as usize,
            Self::Float => 32,
            Self::Double => 64,
            Self::Pointer { .. } => 64, // Assume 64-bit pointers
            Self::Array { element, size } => element.size_bits() * size,
            Self::Struct { fields } => fields.iter().map(|f| f.size_bits()).sum(),
            Self::Vector { element, count } => element.size_bits() * count,
        }
    }

    /// Check if this is a floating-point type
    pub fn is_fp(&self) -> bool {
        matches!(self, Self::Float | Self::Double)
    }

    /// Check if this is an integer type
    pub fn is_int(&self) -> bool {
        matches!(self, Self::Integer { .. })
    }
}
