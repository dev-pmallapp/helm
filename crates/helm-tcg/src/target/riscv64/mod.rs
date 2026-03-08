//! RISC-V 64-bit (RV64GC) TCG target — **stub**.
//!
//! Register map is defined; emitter is not yet implemented.
//! Using this target will produce a compile-time warning.

pub mod regs;

pub use regs::*;

use crate::context::TcgContext;
use crate::target::TranslateAction;

/// RISC-V 64 TCG emitter — **stub**.
///
/// All instructions return [`TranslateAction::Unhandled`], forcing
/// a fallback to the interpretive path.
pub struct Rv64TcgEmitter<'a> {
    pub ctx: &'a mut TcgContext,
    pub pc: u64,
    warned: bool,
}

impl<'a> Rv64TcgEmitter<'a> {
    pub fn new(ctx: &'a mut TcgContext, pc: u64) -> Self {
        Self { ctx, pc, warned: false }
    }

    pub fn translate_insn(&mut self, _insn_word: u32) -> TranslateAction {
        if !self.warned {
            log::warn!("RV64 TCG emitter is a stub — all instructions fall back to interpreter");
            self.warned = true;
        }
        TranslateAction::Unhandled
    }
}
