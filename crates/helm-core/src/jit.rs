//! JIT translation trait — translates decoded instructions into JIT IR.

use crate::insn::DecodedInsn;
use crate::types::Addr;

/// A compiled native-code block ready for execution.
pub struct JitBlock {
    /// Guest PC this block starts at.
    pub pc: Addr,
    /// Number of guest instructions in this block.
    pub insn_count: u32,
    /// Guest PC at end of block (next PC after last instruction).
    pub end_pc: Addr,
}

/// Translates a sequence of DecodedInsn into JIT-compilable IR.
///
/// ISA-specific translators (e.g. A64JitTranslator) implement this trait.
/// The JIT compiler then compiles the IR to native code.
pub trait JitTranslator: Send {
    /// Translate a block of decoded instructions into JIT IR.
    ///
    /// Returns a `JitBlock` descriptor. The actual compiled code is
    /// managed internally by the translator/compiler.
    fn translate_block(
        &mut self,
        insns: &[DecodedInsn],
        base_pc: Addr,
    ) -> JitBlock;
}
