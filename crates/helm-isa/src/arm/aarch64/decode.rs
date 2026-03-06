//! AArch64 A64 instruction decoding → MicroOp production.
//!
//! Dispatches through the top-level op0 bits, then uses the generated
//! name decoders to identify mnemonics and map to MicroOp opcodes.

use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};
use helm_core::types::Addr;
use helm_core::HelmResult;

// Include generated name decoders (returns &'static str mnemonic).
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_dp_imm.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_dp_reg.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_branch.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_ldst.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_fp.rs"));
include!(concat!(env!("OUT_DIR"), "/decode_aarch64_simd.rs"));

/// Decodes a single A64 instruction (32-bit fixed width) into MicroOps.
pub struct Aarch64Decoder;

impl Aarch64Decoder {
    pub fn new() -> Self { Self }

    /// Decode the 32-bit instruction word at `pc`.
    pub fn decode_insn(&self, pc: Addr, insn: u32) -> HelmResult<Vec<MicroOp>> {
        let op0 = (insn >> 25) & 0xF;

        let (opcode, flags) = match op0 {
            0b1000 | 0b1001 => decode_dp_imm_to_opcode(insn),
            0b1010 | 0b1011 => decode_branch_to_opcode(insn),
            0b0100 | 0b0110 | 0b1100 | 0b1110 => decode_ldst_to_opcode(insn),
            0b0101 | 0b1101 => decode_dp_reg_to_opcode(insn),
            0b0111 | 0b1111 => decode_simd_fp_to_opcode(insn),
            _ => (Opcode::Nop, MicroOpFlags::default()),
        };

        let (sources, dest, imm) = extract_operands(insn, op0, opcode);

        Ok(vec![MicroOp {
            guest_pc: pc,
            opcode,
            sources,
            dest,
            immediate: imm,
            flags,
        }])
    }
}

impl Default for Aarch64Decoder {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Mnemonic → Opcode mapping using the generated name decoders
// ---------------------------------------------------------------------------

fn decode_dp_imm_to_opcode(insn: u32) -> (Opcode, MicroOpFlags) {
    let name = decode_aarch64_dp_imm(insn);
    let opcode = match name {
        "UNKNOWN" => Opcode::Nop,
        _ => Opcode::IntAlu,
    };
    (opcode, MicroOpFlags::default())
}

fn decode_branch_to_opcode(insn: u32) -> (Opcode, MicroOpFlags) {
    let name = decode_aarch64_branch(insn);
    match name {
        "B" => (Opcode::Branch, MicroOpFlags { is_branch: true, ..Default::default() }),
        "BL" => (Opcode::Branch, MicroOpFlags { is_branch: true, is_call: true, ..Default::default() }),
        "BR" => (Opcode::Branch, MicroOpFlags { is_branch: true, ..Default::default() }),
        "BLR" => (Opcode::Branch, MicroOpFlags { is_branch: true, is_call: true, ..Default::default() }),
        "RET" => (Opcode::Branch, MicroOpFlags { is_branch: true, is_return: true, ..Default::default() }),
        "B_cond" | "CBZ" | "CBNZ" | "TBZ" | "TBNZ" => (Opcode::CondBranch, MicroOpFlags { is_branch: true, ..Default::default() }),
        "SVC" => (Opcode::Syscall, MicroOpFlags::default()),
        "NOP" | "YIELD" | "WFE" | "WFI" | "SEV" | "SEVL" => (Opcode::Nop, MicroOpFlags::default()),
        "DSB" | "DMB" => (Opcode::Fence, MicroOpFlags { is_memory_barrier: true, ..Default::default() }),
        "ISB" => (Opcode::Fence, MicroOpFlags { is_serialising: true, ..Default::default() }),
        "CLREX" => (Opcode::Fence, MicroOpFlags::default()),
        "MRS" | "MSR" | "CCMP_reg" | "CCMN_reg" | "CCMP_imm" | "CCMN_imm" => (Opcode::IntAlu, MicroOpFlags::default()),
        _ => (Opcode::Nop, MicroOpFlags::default()),
    }
}

fn decode_ldst_to_opcode(insn: u32) -> (Opcode, MicroOpFlags) {
    let name = decode_aarch64_ldst(insn);
    if name == "UNKNOWN" {
        return (Opcode::Nop, MicroOpFlags::default());
    }
    // Loads: name starts with "LD" or "LDR" or "LDAR" or "CAS"
    let is_load = name.starts_with("LD") || name.starts_with("CAS") || name.starts_with("SWP");
    let opcode = if is_load { Opcode::Load } else { Opcode::Store };
    (opcode, MicroOpFlags::default())
}

fn decode_dp_reg_to_opcode(insn: u32) -> (Opcode, MicroOpFlags) {
    let name = decode_aarch64_dp_reg(insn);
    let opcode = match name {
        "MADD" | "MSUB" | "SMADDL" | "UMADDL" | "SMSUBL" | "UMSUBL" | "SMULH" | "UMULH" => Opcode::IntMul,
        "UDIV" | "SDIV" => Opcode::IntDiv,
        "UNKNOWN" => Opcode::Nop,
        _ => Opcode::IntAlu,
    };
    (opcode, MicroOpFlags::default())
}

fn decode_simd_fp_to_opcode(insn: u32) -> (Opcode, MicroOpFlags) {
    // Try scalar FP decoder first
    let fp_name = decode_aarch64_fp(insn);
    if fp_name != "UNKNOWN" {
        let opcode = match fp_name {
            "FMUL_s" | "FMADD" | "FMSUB" | "FNMADD" | "FNMSUB" | "FNMUL_s"
            | "FMUL_si" | "FMLA_si" | "FMLS_si" | "FMULX_si" => Opcode::FpMul,
            "FDIV_s" | "FSQRT_s" => Opcode::FpDiv,
            _ => Opcode::FpAlu,
        };
        return (opcode, MicroOpFlags::default());
    }

    // Try SIMD decoder
    let simd_name = decode_aarch64_simd(insn);
    if simd_name != "UNKNOWN" {
        let opcode = match simd_name {
            // FP multiply variants
            n if n.starts_with("FMUL") || n.starts_with("FMLA") || n.starts_with("FMLS")
                || n.starts_with("FMULX") => Opcode::FpMul,
            // FP divide/sqrt
            n if n.starts_with("FDIV") || n.starts_with("FSQRT") => Opcode::FpDiv,
            // FP other → FpAlu
            n if n.starts_with('F') => Opcode::FpAlu,
            // Integer multiply
            n if n.starts_with("MUL") || n.starts_with("MLA") || n.starts_with("MLS")
                || n.starts_with("PMUL") || n.starts_with("SQD") || n.starts_with("SQRD")
                || n.starts_with("SMULL") || n.starts_with("UMULL")
                || n.starts_with("SMLAL") || n.starts_with("UMLAL")
                || n.starts_with("SMLSL") || n.starts_with("UMLSL") => Opcode::IntMul,
            // Load/store variants for SIMD
            n if n.starts_with("LD") => Opcode::Load,
            n if n.starts_with("ST") => Opcode::Store,
            // Everything else → IntAlu (SIMD integer ops)
            _ => Opcode::IntAlu,
        };
        return (opcode, MicroOpFlags::default());
    }

    (Opcode::Nop, MicroOpFlags::default())
}

/// Extract register operands and immediate from the instruction.
fn extract_operands(insn: u32, op0: u32, opcode: Opcode) -> (Vec<u16>, Option<u16>, Option<u64>) {
    let rd = (insn & 0x1F) as u16;
    let rn = ((insn >> 5) & 0x1F) as u16;
    let rm = ((insn >> 16) & 0x1F) as u16;

    match op0 {
        // DP-immediate: Rd, Rn, imm
        0b1000 | 0b1001 => {
            let op_hi = (insn >> 23) & 0x7;
            let imm = match op_hi {
                0b010 | 0b011 => Some(((insn >> 10) & 0xFFF) as u64), // ADD/SUB imm12
                0b100 | 0b101 => Some(((insn >> 5) & 0xFFFF) as u64), // MOV wide imm16
                _ => None,
            };
            (vec![rn], Some(rd), imm)
        }
        // DP-register: Rd, Rn, Rm
        0b0101 | 0b1101 => {
            match opcode {
                Opcode::IntMul => (vec![rn, rm], Some(rd), None),
                Opcode::IntDiv => (vec![rn, rm], Some(rd), None),
                _ => (vec![rn, rm], Some(rd), None),
            }
        }
        // Branches
        0b1010 | 0b1011 => {
            match opcode {
                Opcode::Branch | Opcode::CondBranch => (vec![], None, None),
                _ => (vec![], None, None),
            }
        }
        // Load/store: Rt, [Rn]
        0b0100 | 0b0110 | 0b1100 | 0b1110 => {
            match opcode {
                Opcode::Load => (vec![rn], Some(rd), None),
                Opcode::Store => (vec![rn, rd], None, None), // Rt is source for stores
                _ => (vec![], None, None),
            }
        }
        // SIMD/FP: Rd, Rn (or Rd, Rn, Rm)
        0b0111 | 0b1111 => {
            (vec![rn, rm], Some(rd), None)
        }
        _ => (vec![], None, None),
    }
}
