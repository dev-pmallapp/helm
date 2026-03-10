//! JitTranslator implementation for AArch64.
//!
//! Wraps the existing `A64TcgEmitter` to consume `DecodedInsn` instead of
//! raw bytes fetched from memory. The `DecodedInsn::encoding_bytes` field
//! carries the raw A64 instruction word.

use crate::a64_emitter::A64TcgEmitter;
use crate::block::TcgBlock;
use crate::context::TcgContext;
use crate::target::TranslateAction;
use helm_core::insn::DecodedInsn;
use helm_core::jit::{JitBlock, JitTranslator};
use helm_core::types::Addr;

/// AArch64 JIT translator consuming `DecodedInsn`.
///
/// Extracts the raw instruction word from `DecodedInsn::encoding_bytes`
/// and feeds it to the existing `A64TcgEmitter` for TcgOp emission.
pub struct A64JitTranslator {
    ctx: TcgContext,
}

impl A64JitTranslator {
    pub fn new() -> Self {
        Self {
            ctx: TcgContext::new(),
        }
    }

    /// Translate a single decoded instruction into TcgOps.
    /// Returns true if the instruction ends the block.
    pub fn translate_one(&mut self, insn: &DecodedInsn) -> bool {
        let word = u32::from_le_bytes([
            insn.encoding_bytes[0],
            insn.encoding_bytes[1],
            insn.encoding_bytes[2],
            insn.encoding_bytes[3],
        ]);

        let mut emitter = A64TcgEmitter::new(&mut self.ctx, insn.pc);
        let action = emitter.translate_insn(word);
        let ends_block = emitter.end_block;

        match action {
            TranslateAction::Continue => ends_block,
            TranslateAction::EndBlock => true,
            TranslateAction::Unhandled => true, // fall back to interpreter
        }
    }

    /// Get the accumulated TcgOp context.
    pub fn context(&self) -> &TcgContext {
        &self.ctx
    }

    /// Take the context, resetting the translator for the next block.
    pub fn take_context(&mut self) -> TcgContext {
        std::mem::replace(&mut self.ctx, TcgContext::new())
    }
}

impl Default for A64JitTranslator {
    fn default() -> Self {
        Self::new()
    }
}

impl JitTranslator for A64JitTranslator {
    fn translate_block(&mut self, insns: &[DecodedInsn], base_pc: Addr) -> JitBlock {
        self.ctx = TcgContext::new();

        let mut count = 0u32;
        let mut end_pc = base_pc;

        for insn in insns {
            count += 1;
            end_pc = insn.pc + insn.len as u64;

            let ends = self.translate_one(insn);
            if ends {
                break;
            }
        }

        JitBlock {
            pc: base_pc,
            insn_count: count,
            end_pc,
        }
    }
}
