//! Cranelift JIT compiler — compiles TcgBlocks to native machine code.
//!
//! Each TcgBlock becomes a native function that directly manipulates
//! the guest register array, achieving near-native execution speed.
//!
//! ```text
//! TcgBlock (IR)  →  Cranelift IR  →  Native x86-64/AArch64 code
//!                                     ↓
//!                              fn(regs, helpers) → exit_code
//! ```

use crate::block::TcgBlock;
use crate::interp::{InterpExit, InterpResult, MemAccess};
use crate::ir::TcgOp;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::*;
use cranelift_codegen::ir::{
    AbiParam, Function, InstBuilder, MemFlags, Signature, UserFuncName,
};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use std::collections::HashMap;

// Exit codes encoded in the return value
const EXIT_END_OF_BLOCK: i64 = 0;
const EXIT_CHAIN_BASE: i64 = 0x1_0000_0000; // | target_pc
const EXIT_SYSCALL_BASE: i64 = 0x2_0000_0000;
const EXIT_EXCEPTION_BASE: i64 = 0x3_0000_0000; // | (class << 16) | iss
const EXIT_WFI: i64 = 0x4_0000_0000;
const EXIT_ERET: i64 = 0x5_0000_0000;

/// A JIT-compiled block — holds the native function pointer.
pub struct JitBlock {
    /// The compiled native function.
    /// Signature: fn(regs: *mut u64) -> i64
    func_ptr: *const u8,
    pub guest_pc: u64,
    pub insn_count: usize,
}

// SAFETY: The function pointer is produced by Cranelift and is valid
// for the lifetime of the JITModule.
unsafe impl Send for JitBlock {}
unsafe impl Sync for JitBlock {}

/// The Cranelift JIT engine — compiles and caches native blocks.
pub struct JitEngine {
    module: JITModule,
    ctx: Context,
    func_ctx: FunctionBuilderContext,
    /// Counter for unique function names.
    func_counter: usize,
}

impl JitEngine {
    /// Create a new JIT engine for the host architecture.
    pub fn new() -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("opt_level", "speed").unwrap();
        flag_builder.set("is_pic", "false").unwrap();

        let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::Triple::host())
            .expect("host ISA not supported by Cranelift");
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();

        let builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        let module = JITModule::new(builder);

        Self {
            ctx: module.make_context(),
            module,
            func_ctx: FunctionBuilderContext::new(),
            func_counter: 0,
        }
    }

    /// Compile a TcgBlock into native code.
    pub fn compile(&mut self, block: &TcgBlock) -> Option<JitBlock> {
        let name = format!("tcg_block_{}", self.func_counter);
        self.func_counter += 1;

        // Function signature: fn(regs: *mut u64) -> i64
        let ptr_type = self.module.target_config().pointer_type();
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(ptr_type)); // regs pointer
        sig.returns.push(AbiParam::new(I64));       // exit code

        let func_id = self
            .module
            .declare_function(&name, Linkage::Local, &sig)
            .ok()?;

        self.ctx.func.signature = sig;
        self.ctx.func.name = UserFuncName::user(0, func_id.as_u32());

        {
            let mut builder =
                FunctionBuilder::new(&mut self.ctx.func, &mut self.func_ctx);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);

            let regs_ptr = builder.block_params(entry)[0];

            // Compile the TcgOps into Cranelift IR
            let exit_val = emit_ops(&mut builder, &block.ops, regs_ptr, ptr_type);

            builder.ins().return_(&[exit_val]);
            builder.finalize();
        }

        // Compile to native code
        self.module
            .define_function(func_id, &mut self.ctx)
            .ok()?;
        self.module.clear_context(&mut self.ctx);
        self.module.finalize_definitions().ok()?;

        let code_ptr = self.module.get_finalized_function(func_id);

        Some(JitBlock {
            func_ptr: code_ptr,
            guest_pc: block.guest_pc,
            insn_count: block.insn_count,
        })
    }

}

/// Emit Cranelift IR for a sequence of TcgOps (free function to avoid borrow issues).
fn emit_ops(
    builder: &mut FunctionBuilder,
    ops: &[TcgOp],
    regs_ptr: cranelift_codegen::ir::Value,
    ptr_type: cranelift_codegen::ir::Type,
) -> cranelift_codegen::ir::Value {
        // Map TcgTemp → Cranelift Value
        let mut temps: HashMap<u32, cranelift_codegen::ir::Value> = HashMap::new();
        // Label → Block mapping
        let mut label_blocks: HashMap<u32, cranelift_codegen::ir::Block> = HashMap::new();

        // Pre-create blocks for all labels
        for op in ops {
            if let TcgOp::Label { id } = op {
                let blk = builder.create_block();
                label_blocks.insert(*id, blk);
            }
        }

        let flags = MemFlags::new();

        for op in ops {
            match op {
                TcgOp::Movi { dst, value } => {
                    let v = builder.ins().iconst(I64, *value as i64);
                    temps.insert(dst.0, v);
                }
                TcgOp::Mov { dst, src } => {
                    if let Some(&v) = temps.get(&src.0) {
                        temps.insert(dst.0, v);
                    }
                }
                TcgOp::Add { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let r = builder.ins().iadd(av, bv);
                    temps.insert(dst.0, r);
                }
                TcgOp::Sub { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let r = builder.ins().isub(av, bv);
                    temps.insert(dst.0, r);
                }
                TcgOp::Mul { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let r = builder.ins().imul(av, bv);
                    temps.insert(dst.0, r);
                }
                TcgOp::Addi { dst, a, imm } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let imm_v = builder.ins().iconst(I64, *imm);
                    let r = builder.ins().iadd(av, imm_v);
                    temps.insert(dst.0, r);
                }
                TcgOp::And { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let r = builder.ins().band(av, bv);
                    temps.insert(dst.0, r);
                }
                TcgOp::Or { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let r = builder.ins().bor(av, bv);
                    temps.insert(dst.0, r);
                }
                TcgOp::Xor { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let r = builder.ins().bxor(av, bv);
                    temps.insert(dst.0, r);
                }
                TcgOp::Not { dst, src } => {
                    let sv = temps.get(&src.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let r = builder.ins().bnot(sv);
                    temps.insert(dst.0, r);
                }
                TcgOp::Shl { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let r = builder.ins().ishl(av, bv);
                    temps.insert(dst.0, r);
                }
                TcgOp::Shr { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let r = builder.ins().ushr(av, bv);
                    temps.insert(dst.0, r);
                }
                TcgOp::Sar { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let r = builder.ins().sshr(av, bv);
                    temps.insert(dst.0, r);
                }
                TcgOp::Sext { dst, src, from_bits } => {
                    let sv = temps.get(&src.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let shift = builder.ins().iconst(I64, (64 - *from_bits) as i64);
                    let shifted = builder.ins().ishl(sv, shift);
                    let r = builder.ins().sshr(shifted, shift);
                    temps.insert(dst.0, r);
                }
                TcgOp::Zext { dst, src, from_bits } => {
                    let sv = temps.get(&src.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let mask = if *from_bits >= 64 { u64::MAX } else { (1u64 << *from_bits) - 1 };
                    let mask_v = builder.ins().iconst(I64, mask as i64);
                    let r = builder.ins().band(sv, mask_v);
                    temps.insert(dst.0, r);
                }

                // Register access — load/store from regs array
                TcgOp::ReadReg { dst, reg_id } => {
                    let offset = (*reg_id as i32) * 8;
                    let v = builder.ins().load(I64, flags, regs_ptr, offset);
                    temps.insert(dst.0, v);
                }
                TcgOp::WriteReg { reg_id, src } => {
                    let sv = temps.get(&src.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let offset = (*reg_id as i32) * 8;
                    builder.ins().store(flags, sv, regs_ptr, offset);
                }

                // Comparisons
                TcgOp::SetEq { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let cmp = builder.ins().icmp(IntCC::Equal, av, bv);
                    let r = builder.ins().uextend(I64, cmp);
                    temps.insert(dst.0, r);
                }
                TcgOp::SetNe { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let cmp = builder.ins().icmp(IntCC::NotEqual, av, bv);
                    let r = builder.ins().uextend(I64, cmp);
                    temps.insert(dst.0, r);
                }
                TcgOp::SetLt { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let cmp = builder.ins().icmp(IntCC::SignedLessThan, av, bv);
                    let r = builder.ins().uextend(I64, cmp);
                    temps.insert(dst.0, r);
                }
                TcgOp::SetGe { dst, a, b } => {
                    let av = temps.get(&a.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let bv = temps.get(&b.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                    let cmp = builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, av, bv);
                    let r = builder.ins().uextend(I64, cmp);
                    temps.insert(dst.0, r);
                }

                // Control flow
                TcgOp::Label { id } => {
                    if let Some(&blk) = label_blocks.get(id) {
                        builder.ins().jump(blk, &[]);
                        builder.switch_to_block(blk);
                        builder.seal_block(blk);
                    }
                }
                TcgOp::Br { label } => {
                    if let Some(&blk) = label_blocks.get(label) {
                        builder.ins().jump(blk, &[]);
                        // Create a new unreachable block to continue emitting
                        let next = builder.create_block();
                        builder.switch_to_block(next);
                        builder.seal_block(next);
                    }
                }
                TcgOp::BrCond { cond, label } => {
                    if let Some(&target_blk) = label_blocks.get(label) {
                        let cv = temps.get(&cond.0).copied().unwrap_or_else(|| builder.ins().iconst(I64, 0));
                        let zero = builder.ins().iconst(I64, 0);
                        let cmp = builder.ins().icmp(IntCC::NotEqual, cv, zero);
                        let fallthrough = builder.create_block();
                        builder.ins().brif(cmp, target_blk, &[], fallthrough, &[]);
                        builder.switch_to_block(fallthrough);
                        builder.seal_block(fallthrough);
                    }
                }

                // Block exits — return encoded exit reason
                TcgOp::GotoTb { target_pc } => {
                    let code = builder.ins().iconst(I64, EXIT_CHAIN_BASE | (*target_pc as i64 & 0xFFFF_FFFF));
                    builder.ins().return_(&[code]);
                    let next = builder.create_block();
                    builder.switch_to_block(next);
                    builder.seal_block(next);
                }
                TcgOp::ExitTb => {
                    let code = builder.ins().iconst(I64, EXIT_END_OF_BLOCK);
                    builder.ins().return_(&[code]);
                    let next = builder.create_block();
                    builder.switch_to_block(next);
                    builder.seal_block(next);
                }
                TcgOp::Wfi => {
                    let code = builder.ins().iconst(I64, EXIT_WFI);
                    builder.ins().return_(&[code]);
                    let next = builder.create_block();
                    builder.switch_to_block(next);
                    builder.seal_block(next);
                }
                TcgOp::Eret => {
                    let code = builder.ins().iconst(I64, EXIT_ERET);
                    builder.ins().return_(&[code]);
                    let next = builder.create_block();
                    builder.switch_to_block(next);
                    builder.seal_block(next);
                }

                // Ops we can't JIT — skip (handled at fallback level)
                TcgOp::Load { .. } | TcgOp::Store { .. } => {
                    // Memory ops need helper calls — return to dispatcher
                    let code = builder.ins().iconst(I64, EXIT_END_OF_BLOCK);
                    builder.ins().return_(&[code]);
                    let next = builder.create_block();
                    builder.switch_to_block(next);
                    builder.seal_block(next);
                }

                // System ops — return to dispatcher
                _ => {
                    let code = builder.ins().iconst(I64, EXIT_END_OF_BLOCK);
                    builder.ins().return_(&[code]);
                    let next = builder.create_block();
                    builder.switch_to_block(next);
                    builder.seal_block(next);
                }
            }
        }

        // Default: end of block
        builder.ins().iconst(I64, EXIT_END_OF_BLOCK)
    }

impl Default for JitEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Execute a JIT-compiled block.
///
/// # Safety
/// The caller must ensure `regs` has at least `NUM_REGS` elements
/// and the JitBlock was compiled by a still-valid JitEngine.
pub unsafe fn exec_jit(
    block: &JitBlock,
    regs: &mut [u64; crate::interp::NUM_REGS],
) -> InterpResult {
    type JitFn = unsafe extern "C" fn(*mut u64) -> i64;
    let func: JitFn = std::mem::transmute(block.func_ptr);

    let exit_code = func(regs.as_mut_ptr());

    let exit = decode_exit(exit_code);

    InterpResult {
        insns_executed: block.insn_count,
        exit,
        mem_accesses: Vec::new(), // JIT doesn't track individual accesses
    }
}

fn decode_exit(code: i64) -> InterpExit {
    if code == EXIT_END_OF_BLOCK {
        InterpExit::Exit
    } else if code == EXIT_WFI {
        InterpExit::Wfi
    } else if code == EXIT_ERET {
        InterpExit::ExceptionReturn
    } else if code & EXIT_CHAIN_BASE != 0 && code & EXIT_SYSCALL_BASE == 0 {
        InterpExit::Chain {
            target_pc: (code & 0xFFFF_FFFF) as u64,
        }
    } else if code & EXIT_SYSCALL_BASE != 0 {
        InterpExit::Syscall {
            nr: (code & 0xFFFF_FFFF) as u64,
        }
    } else if code & EXIT_EXCEPTION_BASE != 0 {
        let payload = (code & 0xFFFF_FFFF) as u32;
        InterpExit::Exception {
            class: payload >> 16,
            iss: payload & 0xFFFF,
        }
    } else {
        InterpExit::Exit
    }
}
