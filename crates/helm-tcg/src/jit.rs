//! Cranelift JIT compiler — compiles TcgBlocks to native machine code.
//!
//! Each TcgBlock becomes a native function that manipulates the guest
//! register array and calls helper functions for memory/sysreg access.
//!
//! ```text
//! TcgBlock  →  Cranelift IR  →  native x86-64 / AArch64
//!                                  ↓
//!                fn(regs, mem_ctx) → exit_code
//!                    │     │
//!                    │     └── passed to helper calls for Load/Store
//!                    └── [u64; NUM_REGS] direct register access
//! ```

use crate::block::TcgBlock;
use crate::interp::{InterpExit, InterpResult, MemAccess, NUM_REGS};
use crate::ir::TcgOp;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::*;
use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlags, UserFuncName};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use std::collections::HashMap;
use crate::interp::{sysreg_idx, SYSREG_FILE_SIZE};

// ── Exit codes ──────────────────────────────────────────────────────

const EXIT_END_OF_BLOCK: i64 = 0;
const EXIT_CHAIN: i64 = 1;          // target_pc in regs[PC]
const EXIT_SYSCALL: i64 = 2;
const EXIT_WFI: i64 = 3;
const EXIT_ERET: i64 = 4;
const EXIT_EXCEPTION: i64 = 5;      // class/iss in regs
const EXIT_FALLBACK: i64 = -1;      // can't JIT this block

// ── Helper function signatures ─────────────────────────────────────
//
// These extern "C" functions are called from JIT'd code via Cranelift
// `call` instructions.  They provide memory access and sysreg I/O.

/// Callback type for VA→PA translation. Called from JIT helpers.
/// `(cpu_ctx, mem_ctx, va, is_write) -> Option<pa>`
pub type TranslateVaFn = unsafe extern "C" fn(*mut u8, *mut u8, u64, u64) -> u64;

/// Sentinel value indicating translation failure.
const TRANSLATE_FAIL: u64 = u64::MAX;

/// Global VA→PA translation callback, set by the engine before JIT execution.
static mut TRANSLATE_VA: Option<TranslateVaFn> = None;

/// Set the VA→PA translation callback for JIT helpers.
/// # Safety
/// Must be called before any JIT execution and not concurrently.
pub unsafe fn set_translate_va(f: TranslateVaFn) {
    TRANSLATE_VA = Some(f);
}

#[inline]
unsafe fn translate(cpu_ctx: *mut u8, mem_ctx: *mut u8, va: u64, is_write: bool) -> u64 {
    match TRANSLATE_VA {
        Some(f) => f(cpu_ctx, mem_ctx, va, is_write as u64),
        None => va, // no translation = identity (MMU off)
    }
}

/// Read `size` bytes from guest virtual address `addr`. Returns the value.
/// `cpu_ctx` is an opaque pointer to CPU (for VA→PA translation).
/// `mem_ctx` is an opaque pointer to the AddressSpace.
extern "C" fn helm_mem_read(cpu_ctx: *mut u8, mem_ctx: *mut u8, addr: u64, size: u64) -> u64 {
    let mem = unsafe { &mut *(mem_ctx as *mut helm_memory::address_space::AddressSpace) };
    let pa = unsafe { translate(cpu_ctx, mem_ctx, addr, false) };
    if pa == TRANSLATE_FAIL { return 0; }
    let sz = size as usize;
    let mut buf = [0u8; 8];
    if mem.read(pa, &mut buf[..sz]).is_ok() {
        match sz {
            1 => buf[0] as u64,
            2 => u16::from_le_bytes([buf[0], buf[1]]) as u64,
            4 => u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64,
            8 => u64::from_le_bytes(buf),
            _ => 0,
        }
    } else {
        0
    }
}

/// Write `size` bytes to guest virtual address `addr`.
extern "C" fn helm_mem_write(cpu_ctx: *mut u8, mem_ctx: *mut u8, addr: u64, value: u64, size: u64) {
    let mem = unsafe { &mut *(mem_ctx as *mut helm_memory::address_space::AddressSpace) };
    let pa = unsafe { translate(cpu_ctx, mem_ctx, addr, true) };
    if pa == TRANSLATE_FAIL { return; }
    let sz = size as usize;
    let bytes = value.to_le_bytes();
    let _ = mem.write(pa, &bytes[..sz]);
}

/// Read a system register by ID. `sysreg_ctx` points to HashMap<u32,u64>.
/// Read a system register by ID. `sysreg_ctx` points to the flat sysreg array.
extern "C" fn helm_sysreg_read(sysreg_ctx: *mut u8, id: u64) -> u64 {
    let arr = unsafe { std::slice::from_raw_parts(sysreg_ctx as *const u64, SYSREG_FILE_SIZE) };
    arr[sysreg_idx(id as u32)]
}

/// Write a system register by ID.
extern "C" fn helm_sysreg_write(sysreg_ctx: *mut u8, id: u64, value: u64) {
    let arr = unsafe { std::slice::from_raw_parts_mut(sysreg_ctx as *mut u64, SYSREG_FILE_SIZE) };
    arr[sysreg_idx(id as u32)] = value;
}

// ── JIT types ───────────────────────────────────────────────────────

/// A JIT-compiled block.
pub struct JitBlock {
    func_ptr: *const u8,
    pub guest_pc: u64,
    pub insn_count: usize,
}

unsafe impl Send for JitBlock {}
unsafe impl Sync for JitBlock {}

/// The Cranelift JIT engine.
pub struct JitEngine {
    module: JITModule,
    ctx: Context,
    func_ctx: FunctionBuilderContext,
    func_counter: usize,
    // Imported helper function IDs
    fn_mem_read: FuncId,
    fn_mem_write: FuncId,
    fn_sysreg_read: FuncId,
    fn_sysreg_write: FuncId,
}

impl JitEngine {
    pub fn new() -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("opt_level", "speed").unwrap();
        flag_builder.set("is_pic", "false").unwrap();

        let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::Triple::host())
            .expect("host ISA not supported by Cranelift");
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();

        let ptr_type = isa.pointer_type();

        // Register helper functions with the JIT
        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        builder.symbol("helm_mem_read", helm_mem_read as *const u8);
        builder.symbol("helm_mem_write", helm_mem_write as *const u8);
        builder.symbol("helm_sysreg_read", helm_sysreg_read as *const u8);
        builder.symbol("helm_sysreg_write", helm_sysreg_write as *const u8);

        let mut module = JITModule::new(builder);

        // Declare helper function signatures
        // helm_mem_read(cpu_ctx: ptr, mem_ctx: ptr, addr: i64, size: i64) -> i64
        let mut sig_read = module.make_signature();
        sig_read.params.push(AbiParam::new(ptr_type)); // cpu_ctx
        sig_read.params.push(AbiParam::new(ptr_type)); // mem_ctx
        sig_read.params.push(AbiParam::new(I64));
        sig_read.params.push(AbiParam::new(I64));
        sig_read.returns.push(AbiParam::new(I64));
        let fn_mem_read = module
            .declare_function("helm_mem_read", Linkage::Import, &sig_read)
            .unwrap();

        // helm_mem_write(cpu_ctx: ptr, mem_ctx: ptr, addr: i64, value: i64, size: i64)
        let mut sig_write = module.make_signature();
        sig_write.params.push(AbiParam::new(ptr_type)); // cpu_ctx
        sig_write.params.push(AbiParam::new(ptr_type)); // mem_ctx
        sig_write.params.push(AbiParam::new(I64));
        sig_write.params.push(AbiParam::new(I64));
        sig_write.params.push(AbiParam::new(I64));
        let fn_mem_write = module
            .declare_function("helm_mem_write", Linkage::Import, &sig_write)
            .unwrap();

        // helm_sysreg_read(ctx: ptr, id: i64) -> i64
        let mut sig_sr_read = module.make_signature();
        sig_sr_read.params.push(AbiParam::new(ptr_type));
        sig_sr_read.params.push(AbiParam::new(I64));
        sig_sr_read.returns.push(AbiParam::new(I64));
        let fn_sysreg_read = module
            .declare_function("helm_sysreg_read", Linkage::Import, &sig_sr_read)
            .unwrap();

        // helm_sysreg_write(ctx: ptr, id: i64, value: i64)
        let mut sig_sr_write = module.make_signature();
        sig_sr_write.params.push(AbiParam::new(ptr_type));
        sig_sr_write.params.push(AbiParam::new(I64));
        sig_sr_write.params.push(AbiParam::new(I64));
        let fn_sysreg_write = module
            .declare_function("helm_sysreg_write", Linkage::Import, &sig_sr_write)
            .unwrap();

        Self {
            ctx: module.make_context(),
            module,
            func_ctx: FunctionBuilderContext::new(),
            func_counter: 0,
            fn_mem_read,
            fn_mem_write,
            fn_sysreg_read,
            fn_sysreg_write,
        }
    }

    /// Compile a TcgBlock into native code.
    pub fn compile(&mut self, block: &TcgBlock) -> Option<JitBlock> {
        let name = format!("tb_{}", self.func_counter);
        self.func_counter += 1;

        let ptr_type = self.module.target_config().pointer_type();

        // fn(regs: ptr, cpu_ctx: ptr, mem_ctx: ptr, sysreg_ctx: ptr) -> i64
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(ptr_type)); // regs
        sig.params.push(AbiParam::new(ptr_type)); // cpu_ctx
        sig.params.push(AbiParam::new(ptr_type)); // mem_ctx
        sig.params.push(AbiParam::new(ptr_type)); // sysreg_ctx
        sig.returns.push(AbiParam::new(I64));       // exit code

        let func_id = self.module.declare_function(&name, Linkage::Local, &sig).ok()?;
        self.ctx.func.signature = sig;
        self.ctx.func.name = UserFuncName::user(0, func_id.as_u32());

        // Import helper function references for this function
        let fn_mr = self.module.declare_func_in_func(self.fn_mem_read, &mut self.ctx.func);
        let fn_mw = self.module.declare_func_in_func(self.fn_mem_write, &mut self.ctx.func);
        let fn_sr = self.module.declare_func_in_func(self.fn_sysreg_read, &mut self.ctx.func);
        let fn_sw = self.module.declare_func_in_func(self.fn_sysreg_write, &mut self.ctx.func);

        let helpers = Helpers { fn_mr, fn_mw, fn_sr, fn_sw };

        {
            let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.func_ctx);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);

            let regs_ptr = builder.block_params(entry)[0];
            let cpu_ctx = builder.block_params(entry)[1];
            let mem_ctx = builder.block_params(entry)[2];
            let sysreg_ctx = builder.block_params(entry)[3];

            let exit_val = emit_ops(
                &mut builder, &block.ops, regs_ptr, cpu_ctx, mem_ctx, sysreg_ctx, &helpers,
            );

            builder.ins().return_(&[exit_val]);
            builder.finalize();
        }

        self.module.define_function(func_id, &mut self.ctx).ok()?;
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

impl Default for JitEngine {
    fn default() -> Self { Self::new() }
}

struct Helpers {
    fn_mr: cranelift_codegen::ir::FuncRef,
    fn_mw: cranelift_codegen::ir::FuncRef,
    fn_sr: cranelift_codegen::ir::FuncRef,
    fn_sw: cranelift_codegen::ir::FuncRef,
}

// ── IR emission ─────────────────────────────────────────────────────

fn emit_ops(
    builder: &mut FunctionBuilder,
    ops: &[TcgOp],
    regs_ptr: cranelift_codegen::ir::Value,
    cpu_ctx: cranelift_codegen::ir::Value,
    mem_ctx: cranelift_codegen::ir::Value,
    sysreg_ctx: cranelift_codegen::ir::Value,
    helpers: &Helpers,
) -> cranelift_codegen::ir::Value {
    let mut temps: HashMap<u32, cranelift_codegen::ir::Value> = HashMap::new();
    let mut label_blocks: HashMap<u32, cranelift_codegen::ir::Block> = HashMap::new();
    let flags = MemFlags::new();

    // Pre-create blocks for labels
    for op in ops {
        if let TcgOp::Label { id } = op {
            label_blocks.insert(*id, builder.create_block());
        }
    }

    // Pre-populate zero constant
    let zero_val = builder.ins().iconst(I64, 0);

    macro_rules! t {
        ($id:expr) => {
            temps.get(&$id).copied().unwrap_or(zero_val)
        };
    }

    for op in ops {
        match op {
            TcgOp::Movi { dst, value } => {
                temps.insert(dst.0, builder.ins().iconst(I64, *value as i64));
            }
            TcgOp::Mov { dst, src } => {
                let v = t!(src.0);
                temps.insert(dst.0, v);
            }

            // Arithmetic
            TcgOp::Add { dst, a, b } => {
                let r = builder.ins().iadd(t!(a.0), t!(b.0));
                temps.insert(dst.0, r);
            }
            TcgOp::Sub { dst, a, b } => {
                let r = builder.ins().isub(t!(a.0), t!(b.0));
                temps.insert(dst.0, r);
            }
            TcgOp::Mul { dst, a, b } => {
                let r = builder.ins().imul(t!(a.0), t!(b.0));
                temps.insert(dst.0, r);
            }
            TcgOp::Div { dst, a, b } => {
                // Division by zero returns 0
                let bv = t!(b.0);
                let one = builder.ins().iconst(I64, 1);
                let is_zero = builder.ins().icmp(IntCC::Equal, bv, zero_val);
                let safe_b = builder.ins().select(is_zero, one, bv);
                let r = builder.ins().udiv(t!(a.0), safe_b);
                let result = builder.ins().select(is_zero, zero_val, r);
                temps.insert(dst.0, result);
            }
            TcgOp::Addi { dst, a, imm } => {
                let imm_v = builder.ins().iconst(I64, *imm);
                let r = builder.ins().iadd(t!(a.0), imm_v);
                temps.insert(dst.0, r);
            }

            // Bitwise
            TcgOp::And { dst, a, b } => {
                temps.insert(dst.0, builder.ins().band(t!(a.0), t!(b.0)));
            }
            TcgOp::Or { dst, a, b } => {
                temps.insert(dst.0, builder.ins().bor(t!(a.0), t!(b.0)));
            }
            TcgOp::Xor { dst, a, b } => {
                temps.insert(dst.0, builder.ins().bxor(t!(a.0), t!(b.0)));
            }
            TcgOp::Not { dst, src } => {
                temps.insert(dst.0, builder.ins().bnot(t!(src.0)));
            }
            TcgOp::Shl { dst, a, b } => {
                temps.insert(dst.0, builder.ins().ishl(t!(a.0), t!(b.0)));
            }
            TcgOp::Shr { dst, a, b } => {
                temps.insert(dst.0, builder.ins().ushr(t!(a.0), t!(b.0)));
            }
            TcgOp::Sar { dst, a, b } => {
                temps.insert(dst.0, builder.ins().sshr(t!(a.0), t!(b.0)));
            }

            // Extensions
            TcgOp::Sext { dst, src, from_bits } => {
                let shift = builder.ins().iconst(I64, (64 - *from_bits) as i64);
                let shifted = builder.ins().ishl(t!(src.0), shift);
                temps.insert(dst.0, builder.ins().sshr(shifted, shift));
            }
            TcgOp::Zext { dst, src, from_bits } => {
                let mask = if *from_bits >= 64 { u64::MAX } else { (1u64 << *from_bits) - 1 };
                let mask_v = builder.ins().iconst(I64, mask as i64);
                temps.insert(dst.0, builder.ins().band(t!(src.0), mask_v));
            }

            // Register access — direct load/store from regs array
            TcgOp::ReadReg { dst, reg_id } => {
                let v = builder.ins().load(I64, flags, regs_ptr, (*reg_id as i32) * 8);
                temps.insert(dst.0, v);
            }
            TcgOp::WriteReg { reg_id, src } => {
                builder.ins().store(flags, t!(src.0), regs_ptr, (*reg_id as i32) * 8);
            }

            // ── Memory access via helper calls ────────────────────
            TcgOp::Load { dst, addr, size } => {
                let addr_v = t!(addr.0);
                let size_v = builder.ins().iconst(I64, *size as i64);
                let inst = builder.ins().call(helpers.fn_mr, &[cpu_ctx, mem_ctx, addr_v, size_v]);
                let result = builder.inst_results(inst)[0];
                temps.insert(dst.0, result);
            }
            TcgOp::Store { addr, val, size } => {
                let addr_v = t!(addr.0);
                let val_v = t!(val.0);
                let size_v = builder.ins().iconst(I64, *size as i64);
                builder.ins().call(helpers.fn_mw, &[cpu_ctx, mem_ctx, addr_v, val_v, size_v]);
            }

            // ── System registers via helper calls ─────────────────
            TcgOp::ReadSysReg { dst, sysreg_id } => {
                let id_v = builder.ins().iconst(I64, *sysreg_id as i64);
                let inst = builder.ins().call(helpers.fn_sr, &[sysreg_ctx, id_v]);
                temps.insert(dst.0, builder.inst_results(inst)[0]);
            }
            TcgOp::WriteSysReg { sysreg_id, src } => {
                let id_v = builder.ins().iconst(I64, *sysreg_id as i64);
                builder.ins().call(helpers.fn_sw, &[sysreg_ctx, id_v, t!(src.0)]);
            }

            // Comparisons
            TcgOp::SetEq { dst, a, b } => {
                let c = builder.ins().icmp(IntCC::Equal, t!(a.0), t!(b.0));
                temps.insert(dst.0, builder.ins().uextend(I64, c));
            }
            TcgOp::SetNe { dst, a, b } => {
                let c = builder.ins().icmp(IntCC::NotEqual, t!(a.0), t!(b.0));
                temps.insert(dst.0, builder.ins().uextend(I64, c));
            }
            TcgOp::SetLt { dst, a, b } => {
                let c = builder.ins().icmp(IntCC::SignedLessThan, t!(a.0), t!(b.0));
                temps.insert(dst.0, builder.ins().uextend(I64, c));
            }
            TcgOp::SetGe { dst, a, b } => {
                let c = builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, t!(a.0), t!(b.0));
                temps.insert(dst.0, builder.ins().uextend(I64, c));
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
                    let n = builder.create_block();
                    builder.switch_to_block(n);
                    builder.seal_block(n);
                }
            }
            TcgOp::BrCond { cond, label } => {
                if let Some(&target) = label_blocks.get(label) {
                    let cv = t!(cond.0);
                    let zero = builder.ins().iconst(I64, 0);
                    let cmp = builder.ins().icmp(IntCC::NotEqual, cv, zero);
                    let fall = builder.create_block();
                    builder.ins().brif(cmp, target, &[], fall, &[]);
                    builder.switch_to_block(fall);
                    builder.seal_block(fall);
                }
            }

            // Block exits
            TcgOp::GotoTb { target_pc } => {
                // Write target PC to regs array, return EXIT_CHAIN
                let pc_slot = crate::interp::REG_PC as i32 * 8;
                let pc_v = builder.ins().iconst(I64, *target_pc as i64);
                builder.ins().store(flags, pc_v, regs_ptr, pc_slot);
                let code = builder.ins().iconst(I64, EXIT_CHAIN);
                builder.ins().return_(&[code]);
                let n = builder.create_block();
                builder.switch_to_block(n);
                builder.seal_block(n);
            }
            TcgOp::ExitTb => {
                let code = builder.ins().iconst(I64, EXIT_END_OF_BLOCK);
                builder.ins().return_(&[code]);
                let n = builder.create_block();
                builder.switch_to_block(n);
                builder.seal_block(n);
            }
            TcgOp::Wfi => {
                let code = builder.ins().iconst(I64, EXIT_WFI);
                builder.ins().return_(&[code]);
                let n = builder.create_block();
                builder.switch_to_block(n);
                builder.seal_block(n);
            }
            TcgOp::Eret => {
                let code = builder.ins().iconst(I64, EXIT_ERET);
                builder.ins().return_(&[code]);
                let n = builder.create_block();
                builder.switch_to_block(n);
                builder.seal_block(n);
            }
            TcgOp::Syscall { .. } => {
                let code = builder.ins().iconst(I64, EXIT_SYSCALL);
                builder.ins().return_(&[code]);
                let n = builder.create_block();
                builder.switch_to_block(n);
                builder.seal_block(n);
            }

            // PSTATE manipulation — direct register array access
            TcgOp::DaifSet { imm } => {
                let daif_slot = crate::interp::REG_DAIF as i32 * 8;
                let old = builder.ins().load(I64, flags, regs_ptr, daif_slot);
                let bits = builder.ins().iconst(I64, ((*imm & 0xF) as i64) << 6);
                let new = builder.ins().bor(old, bits);
                builder.ins().store(flags, new, regs_ptr, daif_slot);
            }
            TcgOp::DaifClr { imm } => {
                let daif_slot = crate::interp::REG_DAIF as i32 * 8;
                let old = builder.ins().load(I64, flags, regs_ptr, daif_slot);
                let bits = builder.ins().iconst(I64, ((*imm & 0xF) as i64) << 6);
                let mask = builder.ins().bnot(bits);
                let new = builder.ins().band(old, mask);
                builder.ins().store(flags, new, regs_ptr, daif_slot);
            }
            TcgOp::SetSpSel { imm } => {
                let slot = crate::interp::REG_SPSEL as i32 * 8;
                let v = builder.ins().iconst(I64, (*imm & 1) as i64);
                builder.ins().store(flags, v, regs_ptr, slot);
            }
            TcgOp::Cfinv => {
                let nzcv_slot = crate::interp::REG_NZCV as i32 * 8;
                let old = builder.ins().load(I64, flags, regs_ptr, nzcv_slot);
                let bit = builder.ins().iconst(I64, 1 << 29);
                let new = builder.ins().bxor(old, bit);
                builder.ins().store(flags, new, regs_ptr, nzcv_slot);
            }

            // Cache/TLB/barrier — no-ops in JIT, just continue
            TcgOp::Barrier { .. } | TcgOp::Clrex | TcgOp::Tlbi { .. } => {}
            TcgOp::DcZva { addr } => {
                // Zero 64 bytes at aligned address via helper
                let av = t!(addr.0);
                let mask = builder.ins().iconst(I64, !63i64);
                let aligned = builder.ins().band(av, mask);
                // Write 8 zero u64s
                for i in 0..8 {
                    let zero = builder.ins().iconst(I64, 0);
                    let off = builder.ins().iconst(I64, i * 8);
                    let a = builder.ins().iadd(aligned, off);
                    let sz = builder.ins().iconst(I64, 8);
                    builder.ins().call(helpers.fn_mw, &[mem_ctx, a, zero, sz]);
                }
            }

            // Exception ops — return to dispatcher
            TcgOp::SvcExc { .. } | TcgOp::HvcExc { .. } | TcgOp::SmcExc { .. }
            | TcgOp::BrkExc { .. } | TcgOp::HltExc { .. } => {
                let code = builder.ins().iconst(I64, EXIT_EXCEPTION);
                builder.ins().return_(&[code]);
                let n = builder.create_block();
                builder.switch_to_block(n);
                builder.seal_block(n);
            }

            TcgOp::At { .. } => {} // Address translation — no-op stub
        }
    }

    // Default: end of block
    builder.ins().iconst(I64, EXIT_END_OF_BLOCK)
}

// ── Execution ───────────────────────────────────────────────────────

/// Execute a JIT-compiled block.
///
/// # Safety
/// Caller must ensure `regs` has NUM_REGS elements and the JitBlock
/// was compiled by a still-valid JitEngine.
pub unsafe fn exec_jit(
    block: &JitBlock,
    regs: &mut [u64; NUM_REGS],
    cpu_ctx: *mut u8,
    mem: &mut helm_memory::address_space::AddressSpace,
    sysregs: &mut [u64],
) -> InterpResult {
    type JitFn = unsafe extern "C" fn(*mut u64, *mut u8, *mut u8, *mut u8) -> i64;
    let func: JitFn = std::mem::transmute(block.func_ptr);

    let exit_code = func(
        regs.as_mut_ptr(),
        cpu_ctx,
        mem as *mut _ as *mut u8,
        sysregs as *mut _ as *mut u8,
    );

    let exit = match exit_code {
        EXIT_END_OF_BLOCK => InterpExit::Exit,
        EXIT_CHAIN => InterpExit::Chain {
            target_pc: regs[crate::interp::REG_PC as usize],
        },
        EXIT_SYSCALL => InterpExit::Syscall { nr: 0 },
        EXIT_WFI => InterpExit::Wfi,
        EXIT_ERET => InterpExit::ExceptionReturn,
        EXIT_EXCEPTION => InterpExit::Exception { class: 0, iss: 0 },
        _ => InterpExit::Exit,
    };

    InterpResult {
        insns_executed: block.insn_count,
        exit,
        mem_accesses: Vec::new(),
    }
}
