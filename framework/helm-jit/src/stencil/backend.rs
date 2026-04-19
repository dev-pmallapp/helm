//! Stencil JIT backend — implements `JitBackend` for AArch64, and provides
//! a separate `StencilBackendRv64` for RISC-V64.

#![allow(missing_docs)]

use helm_arch::aarch64::insn::Instruction;

use super::compiler;
use super::data;
use super::fields;
use crate::backend::JitBackend;
use crate::block::CompiledBlock;

/// Which guest ISA this stencil backend is configured for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StencilIsa {
    Aarch64,
    Riscv64,
}

/// Stencil (copy-and-patch) JIT backend.
///
/// Compiles guest instructions by copying pre-compiled x86-64 code templates
/// and patching relocation holes. Much faster than dynasm at the cost of less
/// per-instruction optimization.
pub struct StencilBackend {
    isa: StencilIsa,
    /// Runtime-selected memory read helper (0 = use default SE helper).
    mem_read_fn: u64,
    /// Runtime-selected memory write helper (0 = use default SE helper).
    mem_write_fn: u64,
}

impl StencilBackend {
    /// Create a new stencil backend for the given ISA.
    pub fn new(isa: StencilIsa) -> Self {
        Self {
            isa,
            mem_read_fn: 0,
            mem_write_fn: 0,
        }
    }

    /// Create a new stencil backend for AArch64.
    pub fn new_aarch64() -> Self {
        Self::new(StencilIsa::Aarch64)
    }

    /// Create a new stencil backend for RISC-V64.
    pub fn new_riscv64() -> Self {
        Self::new(StencilIsa::Riscv64)
    }
}

/// Check if a 64-bit value fits in a signed 32-bit relocation.
/// Values in [0, 0x7FFFFFFF] and [0xFFFFFFFF_80000000, 0xFFFFFFFF_FFFFFFFF]
/// are representable (the latter sign-extend correctly).
#[inline]
fn fits_i32(val: u64) -> bool {
    let signed = val as i64;
    i32::try_from(signed).is_ok()
}

impl JitBackend for StencilBackend {
    fn compile_block(&mut self, pc: u64, insns: &[Instruction]) -> Option<CompiledBlock> {
        if self.isa != StencilIsa::Aarch64 {
            // RISC-V64 uses a separate trait (different Instruction type).
            // This JitBackend impl is AArch64-only.
            return None;
        }

        // Look up stencils and extract fields for each instruction.
        let mut entries = Vec::new();
        for (i, insn) in insns.iter().enumerate() {
            let stencil = data::lookup_stencil_a64(insn)?;

            // If first instruction has no stencil, return None.
            // If a later instruction has no stencil, compile what we have.
            if i == 0 && stencil.is_none() {
                return None;
            }
            let stencil = match stencil {
                Some(s) => s,
                None => break,
            };

            let mut fields = fields::extract_fields_a64(insn, insn.pc);
            fields.mem_read_fn = self.mem_read_fn;
            fields.mem_write_fn = self.mem_write_fn;

            // Terminator stencils use 32-bit holes for target/next_pc.
            // Skip if any address exceeds signed 32-bit range (kernel addresses).
            if stencil.is_terminator && !fits_i32(fields.branch_target) {
                break;
            }
            // Non-terminator stencils use 32-bit holes for immediates.
            // Adr/Adrp pre-compute addresses in imm — skip if > 32 bits.
            if !stencil.is_terminator && !fits_i32(fields.imm as u64) {
                if i == 0 {
                    return None;
                }
                break;
            }

            entries.push((stencil, fields));

            // Stop at terminators and non-leaf stencils (can't chain past them).
            // Unlike the dynasm backend, conditional-branch fall-through is not
            // yet split into a separate continuation path here: the current
            // stencil corpus still models CBZ/CBNZ/TBZ/TBNZ/B.cond as full
            // terminators. Phase 4 continuity gains are therefore dynasm-only
            // until the stencil templates are regenerated with chainable
            // taken-edge exits and non-terminating fall-through bodies.
            if stencil.is_terminator || !stencil.is_leaf {
                break;
            }
        }

        if entries.is_empty() {
            return None;
        }

        compiler::compile_block(pc, &entries)
    }

    fn name(&self) -> &str {
        "stencil"
    }

    fn set_mem_helpers(&mut self, read_fn: u64, write_fn: u64) {
        self.mem_read_fn = read_fn;
        self.mem_write_fn = write_fn;
    }
}

// ── RISC-V64 stencil backend ────────────────────────────────────────────────

use helm_arch::riscv::insn::Instruction as RvInstruction;

/// Stencil JIT backend for RISC-V64.
///
/// Separate from `StencilBackend` because the `Instruction` type differs
/// (AArch64 vs RISC-V), and the stencil lookup uses string-keyed dispatch.
pub struct StencilBackendRv64;

impl StencilBackendRv64 {
    pub fn new() -> Self {
        Self
    }

    /// Compile a block of decoded RISC-V64 instructions into native x86-64 code.
    pub fn compile_block_rv64(
        &mut self,
        pc: u64,
        insns: &[RvInstruction],
    ) -> Option<CompiledBlock> {
        let mut entries = Vec::new();
        for (i, insn) in insns.iter().enumerate() {
            let name = insn.stencil_name()?;
            if i == 0 && name.is_empty() {
                return None;
            }

            let stencil = if let Some(s) = data::lookup_stencil_rv64(name) {
                s
            } else {
                if i == 0 {
                    return None;
                }
                break;
            };

            let fields = fields::extract_fields_rv64(insn, pc + (i as u64) * 4);

            // Range check for 32-bit relocation holes.
            if stencil.is_terminator && !fits_i32(fields.branch_target) {
                break;
            }
            if !stencil.is_terminator && !fits_i32(fields.imm as u64) {
                if i == 0 {
                    return None;
                }
                break;
            }

            entries.push((stencil, fields));

            if stencil.is_terminator || !stencil.is_leaf {
                break;
            }
        }

        if entries.is_empty() {
            return None;
        }

        compiler::compile_block(pc, &entries)
    }

    pub fn name(&self) -> &str {
        "stencil-rv64"
    }
}

impl Default for StencilBackendRv64 {
    fn default() -> Self {
        Self::new()
    }
}
