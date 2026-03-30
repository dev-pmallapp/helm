//! RISC-V64 stencil lookup — dispatches instruction names to generated stencil data.

#![allow(missing_docs)]
#![allow(unused_imports)]

use crate::stencil::types::{HoleKind, HelperFn, RegField, RelocKind, Stencil, StencilReloc};

// Include the build-time generated stencil data (byte arrays + reloc tables).
// The generated file does NOT contain `use` statements — it relies on the
// imports above.
include!(concat!(env!("OUT_DIR"), "/generated_rv64.rs"));

/// Look up a stencil for a RISC-V64 instruction by variant name.
pub fn lookup(name: &str) -> Option<&'static Stencil> {
    match name {
        // ALU immediate
        "ADDI" => Some(&STENCIL_RV_ADDI),
        "SLTI" => Some(&STENCIL_RV_SLTI),
        "SLTIU" => Some(&STENCIL_RV_SLTIU),
        "XORI" => Some(&STENCIL_RV_XORI),
        "ORI" => Some(&STENCIL_RV_ORI),
        "ANDI" => Some(&STENCIL_RV_ANDI),
        "SLLI" => Some(&STENCIL_RV_SLLI),
        "SRLI" => Some(&STENCIL_RV_SRLI),
        "SRAI" => Some(&STENCIL_RV_SRAI),

        // ALU register
        "ADD" => Some(&STENCIL_RV_ADD),
        "SUB" => Some(&STENCIL_RV_SUB),
        "SLL" => Some(&STENCIL_RV_SLL),
        "SLT" => Some(&STENCIL_RV_SLT),
        "SLTU" => Some(&STENCIL_RV_SLTU),
        "XOR" => Some(&STENCIL_RV_XOR),
        "SRL" => Some(&STENCIL_RV_SRL),
        "SRA" => Some(&STENCIL_RV_SRA),
        "OR" => Some(&STENCIL_RV_OR),
        "AND" => Some(&STENCIL_RV_AND),

        // Word ops
        "ADDIW" => Some(&STENCIL_RV_ADDIW),
        "ADDW" => Some(&STENCIL_RV_ADDW),
        "SUBW" => Some(&STENCIL_RV_SUBW),

        // Loads
        "LB" => Some(&STENCIL_RV_LB),
        "LH" => Some(&STENCIL_RV_LH),
        "LW" => Some(&STENCIL_RV_LW),
        "LD" => Some(&STENCIL_RV_LD),
        "LBU" => Some(&STENCIL_RV_LBU),
        "LHU" => Some(&STENCIL_RV_LHU),
        "LWU" => Some(&STENCIL_RV_LWU),

        // Stores
        "SB" => Some(&STENCIL_RV_SB),
        "SH" => Some(&STENCIL_RV_SH),
        "SW" => Some(&STENCIL_RV_SW),
        "SD" => Some(&STENCIL_RV_SD),

        // Branches
        "BEQ" => Some(&STENCIL_RV_BEQ),
        "BNE" => Some(&STENCIL_RV_BNE),
        "BLT" => Some(&STENCIL_RV_BLT),
        "BGE" => Some(&STENCIL_RV_BGE),
        "BLTU" => Some(&STENCIL_RV_BLTU),
        "BGEU" => Some(&STENCIL_RV_BGEU),
        "JAL" => Some(&STENCIL_RV_JAL),
        "JALR" => Some(&STENCIL_RV_JALR),

        // Upper immediate
        "LUI" => Some(&STENCIL_RV_LUI),
        "AUIPC" => Some(&STENCIL_RV_AUIPC),

        // Multiply/divide
        "MUL" => Some(&STENCIL_RV_MUL),
        "DIV" => Some(&STENCIL_RV_DIV),
        "DIVU" => Some(&STENCIL_RV_DIVU),
        "REM" => Some(&STENCIL_RV_REM),
        "REMU" => Some(&STENCIL_RV_REMU),

        // System
        "ECALL" => Some(&STENCIL_RV_ECALL),

        _ => None,
    }
}
