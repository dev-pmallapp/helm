//! Pluggable JIT backend trait.
//!
//! Backends compile sequences of decoded AArch64 instructions into native
//! x86-64 machine code blocks. The compiled code follows a fixed calling
//! convention:
//!
//! - `rdi` = pointer to flat register array (`[u64; 48]`)
//! - `rsi` = pointer to `FlatMem` (passed through to memory helpers)
//! - Returns exit code in `rax` (`EXIT_*` constants from `block.rs`)

use helm_arch::aarch64::insn::Instruction;

use crate::block::CompiledBlock;

/// Trait for JIT compilation backends.
///
/// Implementors translate a slice of decoded AArch64 instructions starting
/// at a given guest PC into a [`CompiledBlock`] of native x86-64 machine code.
pub trait JitBackend: Send {
    /// Compile a block of decoded AArch64 instructions into native code.
    ///
    /// # Arguments
    /// - `pc`: guest PC at the start of the block
    /// - `insns`: slice of pre-decoded instructions starting at `pc`
    ///
    /// # Returns
    /// - `Some(CompiledBlock)` on success (at least one instruction compiled)
    /// - `None` if the first instruction is unsupported
    fn compile_block(&mut self, pc: u64, insns: &[Instruction]) -> Option<CompiledBlock>;

    /// Human-readable name of this backend (e.g. `"dynasm"`, `"cranelift"`).
    fn name(&self) -> &str;
}
