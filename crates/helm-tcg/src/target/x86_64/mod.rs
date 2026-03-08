//! x86-64 (AMD64) TCG target — **stub**.
//!
//! Register map is defined; emitter is not yet implemented.
//! Using this target will produce a compile-time warning.

pub mod regs;

pub use regs::*;

use crate::context::TcgContext;
use crate::target::TranslateAction;

/// x86-64 TCG emitter — **stub**.
///
/// All instructions return [`TranslateAction::Unhandled`], forcing
/// a fallback to the interpretive path. x86 instructions are variable-
/// length (1–15 bytes), so the emitter would need a length decoder.
pub struct X86TcgEmitter<'a> {
    pub ctx: &'a mut TcgContext,
    pub pc: u64,
    warned: bool,
}

impl<'a> X86TcgEmitter<'a> {
    pub fn new(ctx: &'a mut TcgContext, pc: u64) -> Self {
        Self { ctx, pc, warned: false }
    }

    pub fn translate_insn(&mut self, _insn_bytes: &[u8]) -> TranslateAction {
        if !self.warned {
            log::warn!("x86-64 TCG emitter is a stub — all instructions fall back to interpreter");
            self.warned = true;
        }
        TranslateAction::Unhandled
    }
}
