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
use crate::interp::{sysreg_idx, SYSREG_FILE_SIZE};
use crate::interp::{InterpExit, InterpResult, NUM_REGS};
use crate::ir::TcgOp;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::*;
use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlags, UserFuncName};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use helm_memory::tlb::FAST_TLB_MASK;
use std::collections::HashMap;

// ── Exit codes (pub for direct fn-pointer chaining in session.rs) ───

pub const EXIT_END_OF_BLOCK: i64 = 0;
pub const EXIT_CHAIN: i64 = 1; // target_pc in regs[PC]
pub const EXIT_SYSCALL: i64 = 2;
pub const EXIT_WFI: i64 = 3;
pub const EXIT_ERET: i64 = 4;
pub const EXIT_EXCEPTION: i64 = 5; // class/iss in regs
pub const EXIT_ISB: i64 = 6; // instruction sync barrier — may need cache flush

// ── Translation failure tracking ────────────────────────────────────
//
// Counts how many times the JIT's memory helpers silently drop reads/writes
// due to TRANSLATE_FAIL.  In the interpreter, these would raise a Data Abort
// exception.  Non-zero counts indicate the JIT is hiding page faults.

use std::sync::atomic::{AtomicU64, Ordering};

static TRANSLATE_FAIL_READS: AtomicU64 = AtomicU64::new(0);
static TRANSLATE_FAIL_WRITES: AtomicU64 = AtomicU64::new(0);

/// Set by JIT memory helpers when a translation fault occurs.
/// When true, cpu.regs contains the fault handler state (PC = VBAR+offset,
/// ELR/SPSR/ESR/FAR set) because translate_va called raise_translation_fault.
/// The session must NOT overwrite cpu.regs with array_to_regs; instead it
/// should rebuild the regs array from cpu.regs and continue from the fault
/// handler.
///
/// # Safety
/// Only accessed from the single-threaded JIT execution path.
static mut DATA_ABORT_PENDING: bool = false;

/// Check and clear the data-abort-pending flag.
///
/// # Safety
/// Must only be called from the session's JIT execution loop (single-threaded).
pub unsafe fn take_data_abort_pending() -> bool {
    let was = DATA_ABORT_PENDING;
    DATA_ABORT_PENDING = false;
    was
}

/// Set the data-abort-pending flag.
///
/// # Safety
/// Must only be called from JIT helper functions (single-threaded).
pub unsafe fn set_data_abort_pending() {
    DATA_ABORT_PENDING = true;
}

/// Set by JIT write helpers when a write targets a kernel VA that could be
/// code (text section).  Checked on ISB to decide whether to flush caches.
static mut CODE_WRITE_PENDING: bool = false;

/// Check and clear the code-write-pending flag.
pub unsafe fn take_code_write_pending() -> bool {
    let was = CODE_WRITE_PENDING;
    CODE_WRITE_PENDING = false;
    was
}

/// Set the code-write-pending flag.  Called from the interpreter when
/// it executes IC IVAU (instruction cache invalidation by VA).
pub unsafe fn set_code_write_pending() {
    CODE_WRITE_PENDING = true;
}

/// Return (read_fails, write_fails) since last reset.
pub fn translate_fail_counts() -> (u64, u64) {
    (
        TRANSLATE_FAIL_READS.load(Ordering::Relaxed),
        TRANSLATE_FAIL_WRITES.load(Ordering::Relaxed),
    )
}

/// Reset failure counters to zero.
pub fn reset_translate_fail_counts() {
    TRANSLATE_FAIL_READS.store(0, Ordering::Relaxed);
    TRANSLATE_FAIL_WRITES.store(0, Ordering::Relaxed);
}

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

/// Callback type for TLB invalidation.  Called from JIT helpers.
/// `(cpu_ctx, op, addr_value)` where `op` is `(op1 << 8) | (crm << 4) | op2`.
pub type TlbiFn = unsafe extern "C" fn(*mut u8, u64, u64);

/// Global TLBI callback, set by the engine before JIT execution.
static mut TLBI_CB: Option<TlbiFn> = None;

/// Set the TLBI callback for JIT helpers.
/// # Safety
/// Must be called before any JIT execution and not concurrently.
pub unsafe fn set_tlbi_cb(f: TlbiFn) {
    TLBI_CB = Some(f);
}

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

/// Read `size` bytes from a host pointer (unaligned).
#[inline(always)]
unsafe fn read_host(host: *const u8, size: usize) -> u64 {
    match size {
        1 => *host as u64,
        2 => (host as *const u16).read_unaligned() as u64,
        4 => (host as *const u32).read_unaligned() as u64,
        8 => (host as *const u64).read_unaligned(),
        _ => 0,
    }
}

/// Write `size` bytes to a host pointer (unaligned).
#[inline(always)]
unsafe fn write_host(host: *mut u8, value: u64, size: usize) {
    match size {
        1 => *host = value as u8,
        2 => (host as *mut u16).write_unaligned(value as u16),
        4 => (host as *mut u32).write_unaligned(value as u32),
        8 => (host as *mut u64).write_unaligned(value),
        _ => {}
    }
}

/// Read `size` bytes from guest virtual address `addr`. Returns the value.
/// `cpu_ctx` is a pointer to Aarch64Cpu (for VA→PA translation + fast TLB).
/// `mem_ctx` is a pointer to the AddressSpace.
extern "C" fn helm_mem_read(cpu_ctx: *mut u8, mem_ctx: *mut u8, addr: u64, size: u64) -> u64 {
    // Fast path: inline fast TLB lookup with addend → direct host read
    if !cpu_ctx.is_null() {
        let cpu = unsafe { &*(cpu_ctx as *const helm_isa::arm::aarch64::exec::Aarch64Cpu) };
        let va_tag = addr >> 12;
        let idx = (va_tag as usize) & FAST_TLB_MASK;
        let entry = unsafe { cpu.tlb.fast_entries.get_unchecked(idx) };
        if entry.va_tag == va_tag
            && entry.perm_read
            && entry.has_addend
            && (entry.global || entry.asid == cpu.current_asid())
        {
            let host = (addr as isize).wrapping_add(entry.addend) as *const u8;
            return unsafe { read_host(host, size as usize) };
        }
    }

    // Slow path: full translate + AddressSpace read
    helm_mem_read_slow(cpu_ctx, mem_ctx, addr, size)
}

/// Slow path for helm_mem_read: translate VA→PA then read from AddressSpace.
#[inline(never)]
fn helm_mem_read_slow(cpu_ctx: *mut u8, mem_ctx: *mut u8, addr: u64, size: u64) -> u64 {
    let mem = unsafe { &mut *(mem_ctx as *mut helm_memory::address_space::AddressSpace) };
    let pa = unsafe { translate(cpu_ctx, mem_ctx, addr, false) };
    if pa == TRANSLATE_FAIL {
        TRANSLATE_FAIL_READS.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
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
    // Detect writes to kernel text VA range (code patching).
    // runtime_const_fixup writes instructions directly + ISB (no IC IVAU).
    if size == 4 && !cpu_ctx.is_null() {
        let cpu = unsafe { &mut *(cpu_ctx as *mut helm_isa::arm::aarch64::exec::Aarch64Cpu) };
        if cpu.text_va_end > cpu.text_va_start
            && addr >= cpu.text_va_start
            && addr < cpu.text_va_end
        {
            cpu.ic_ivau_pending = true;
        }
    }

    // Fast path: inline fast TLB lookup with addend → direct host write
    if !cpu_ctx.is_null() {
        let cpu = unsafe { &*(cpu_ctx as *const helm_isa::arm::aarch64::exec::Aarch64Cpu) };
        let va_tag = addr >> 12;
        let idx = (va_tag as usize) & FAST_TLB_MASK;
        let entry = unsafe { cpu.tlb.fast_entries.get_unchecked(idx) };
        if entry.va_tag == va_tag
            && entry.perm_write
            && entry.has_addend
            && (entry.global || entry.asid == cpu.current_asid())
        {
            let host = (addr as isize).wrapping_add(entry.addend) as *mut u8;
            unsafe { write_host(host, value, size as usize) };
            return;
        }
    }

    // Slow path
    helm_mem_write_slow(cpu_ctx, mem_ctx, addr, value, size);
}

/// Slow path for helm_mem_write: translate VA→PA then write to AddressSpace.
#[inline(never)]
fn helm_mem_write_slow(cpu_ctx: *mut u8, mem_ctx: *mut u8, addr: u64, value: u64, size: u64) {
    let mem = unsafe { &mut *(mem_ctx as *mut helm_memory::address_space::AddressSpace) };
    let pa = unsafe { translate(cpu_ctx, mem_ctx, addr, true) };
    if pa == TRANSLATE_FAIL {
        TRANSLATE_FAIL_WRITES.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let sz = size as usize;
    let bytes = value.to_le_bytes();
    let _ = mem.write(pa, &bytes[..sz]);
}

/// Read a system register by ID. `sysreg_ctx` points to the flat sysreg array.
///
/// Dynamically computes the ISTATUS bit (bit 2) for `CNTV_CTL_EL0` and
/// `CNTP_CTL_EL0`, matching the interpreter's MRS handler.  Without this
/// the kernel's `arch_timer_handler` sees ISTATUS=0 on every read, returns
/// `IRQ_NONE`, and never re-arms the timer.
extern "C" fn helm_sysreg_read(sysreg_ctx: *mut u8, id: u64) -> u64 {
    use helm_isa::arm::aarch64::sysreg;
    let arr = unsafe { std::slice::from_raw_parts(sysreg_ctx as *const u64, SYSREG_FILE_SIZE) };
    let id32 = id as u32;
    let val = arr[sysreg_idx(id32)];

    if id32 == sysreg::CNTV_CTL_EL0 {
        let cntvct = arr[sysreg_idx(sysreg::CNTVCT_EL0)];
        let cval = arr[sysreg_idx(sysreg::CNTV_CVAL_EL0)];
        return if val & 1 != 0 && cntvct >= cval {
            val | (1 << 2)
        } else {
            val & !(1u64 << 2)
        };
    }
    if id32 == sysreg::CNTP_CTL_EL0 {
        let cntvct = arr[sysreg_idx(sysreg::CNTVCT_EL0)];
        let cval = arr[sysreg_idx(sysreg::CNTP_CVAL_EL0)];
        return if val & 1 != 0 && cntvct >= cval {
            val | (1 << 2)
        } else {
            val & !(1u64 << 2)
        };
    }
    val
}

/// Write a system register by ID.
///
/// Special-cases TVAL writes: writing CNTV_TVAL_EL0 or CNTP_TVAL_EL0
/// must update the corresponding CVAL register (CVAL = CNTVCT + sext(TVAL)).
extern "C" fn helm_sysreg_write(sysreg_ctx: *mut u8, id: u64, value: u64) {
    use helm_isa::arm::aarch64::sysreg;
    let arr = unsafe { std::slice::from_raw_parts_mut(sysreg_ctx as *mut u64, SYSREG_FILE_SIZE) };
    let id32 = id as u32;
    let widx = sysreg_idx(id32);
    arr[widx] = value;

    // Mark the MMU config dirty so the session run-loop re-syncs cpu.regs
    // before the next block.  Indices: SCTLR_EL1=16512, TTBR0_EL1=16640,
    // TTBR1_EL1=16641, TCR_EL1=16642, MAIR_EL1=17680, VBAR_EL1=17920.
    // MMU_DIRTY_IDX=24323 shares a cache line with CNTVCT (24322) → L1 hit.
    if matches!(widx, 16512 | 16640 | 16641 | 16642 | 17680 | 17920) {
        arr[MMU_DIRTY_IDX] = 1;
    }

    // TVAL write → update CVAL = CNTVCT + sign_extend(TVAL)
    if id32 == sysreg::CNTV_TVAL_EL0 {
        let cntvct = arr[sysreg_idx(sysreg::CNTVCT_EL0)];
        arr[sysreg_idx(sysreg::CNTV_CVAL_EL0)] = cntvct.wrapping_add(value as i32 as i64 as u64);
    } else if id32 == sysreg::CNTP_TVAL_EL0 {
        let cntvct = arr[sysreg_idx(sysreg::CNTVCT_EL0)];
        arr[sysreg_idx(sysreg::CNTP_CVAL_EL0)] = cntvct.wrapping_add(value as i32 as i64 as u64);
    }
}

/// TLB invalidation helper.  `cpu_ctx` points to an `Aarch64Cpu`.
/// `op` encodes `(op1 << 8) | (crm << 4) | op2`.
/// `addr_value` is the Xt register value (VA for VA-based TLBI variants).
///
/// Special: op == 0xFFFF signals IC IVAU (instruction cache invalidation).
/// This sets CODE_WRITE_PENDING so the next ISB flushes JIT caches.
extern "C" fn helm_tlbi(cpu_ctx: *mut u8, op: u64, addr_value: u64) {
    if op == 0xFFFF {
        // IC IVAU sentinel — set per-CPU flag via cpu_ctx
        let cpu = unsafe { &mut *(cpu_ctx as *mut helm_isa::arm::aarch64::exec::Aarch64Cpu) };
        cpu.ic_ivau_pending = true;
        return;
    }
    unsafe {
        if let Some(f) = TLBI_CB {
            f(cpu_ctx, op, addr_value);
        }
    }
}

// ── JIT types ───────────────────────────────────────────────────────

/// A JIT-compiled block.
pub struct JitBlock {
    pub func_ptr: *const u8,
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
    fn_tlbi: FuncId,
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
        builder.symbol("helm_tlbi", helm_tlbi as *const u8);

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

        // helm_tlbi(cpu_ctx: ptr, op: i64, addr_value: i64)
        let mut sig_tlbi = module.make_signature();
        sig_tlbi.params.push(AbiParam::new(ptr_type)); // cpu_ctx
        sig_tlbi.params.push(AbiParam::new(I64)); // op
        sig_tlbi.params.push(AbiParam::new(I64)); // addr_value
        let fn_tlbi = module
            .declare_function("helm_tlbi", Linkage::Import, &sig_tlbi)
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
            fn_tlbi,
        }
    }

    /// Compile a TcgBlock into native code.
    pub fn compile(&mut self, block: &TcgBlock) -> Option<JitBlock> {
        let name = format!("tb_{}", self.func_counter);
        self.func_counter += 1;

        let ptr_type = self.module.target_config().pointer_type();

        // fn(regs: ptr, cpu_ctx: ptr, mem_ctx: ptr, sysreg_ctx: ptr, tlb_ptr: ptr) -> i64
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(ptr_type)); // regs
        sig.params.push(AbiParam::new(ptr_type)); // cpu_ctx
        sig.params.push(AbiParam::new(ptr_type)); // mem_ctx
        sig.params.push(AbiParam::new(ptr_type)); // sysreg_ctx
        sig.params.push(AbiParam::new(ptr_type)); // tlb_ptr (InlineTlbEntry array)
        sig.returns.push(AbiParam::new(I64)); // exit code

        let func_id = self
            .module
            .declare_function(&name, Linkage::Local, &sig)
            .ok()?;
        self.ctx.func.signature = sig;
        self.ctx.func.name = UserFuncName::user(0, func_id.as_u32());

        // Import helper function references for this function
        let fn_mr = self
            .module
            .declare_func_in_func(self.fn_mem_read, &mut self.ctx.func);
        let fn_mw = self
            .module
            .declare_func_in_func(self.fn_mem_write, &mut self.ctx.func);
        let fn_sr = self
            .module
            .declare_func_in_func(self.fn_sysreg_read, &mut self.ctx.func);
        let fn_sw = self
            .module
            .declare_func_in_func(self.fn_sysreg_write, &mut self.ctx.func);
        let fn_ti = self
            .module
            .declare_func_in_func(self.fn_tlbi, &mut self.ctx.func);

        let helpers = Helpers {
            fn_mr,
            fn_mw,
            fn_sr,
            fn_sw,
            fn_ti,
        };

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
            let tlb_ptr = builder.block_params(entry)[4];

            let exit_val = emit_ops(
                &mut builder,
                &block.ops,
                regs_ptr,
                cpu_ctx,
                mem_ctx,
                sysreg_ctx,
                tlb_ptr,
                &helpers,
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
    fn default() -> Self {
        Self::new()
    }
}

struct Helpers {
    fn_mr: cranelift_codegen::ir::FuncRef,
    fn_mw: cranelift_codegen::ir::FuncRef,
    fn_sr: cranelift_codegen::ir::FuncRef,
    fn_sw: cranelift_codegen::ir::FuncRef,
    fn_ti: cranelift_codegen::ir::FuncRef,
}

// ── PSTATE mirror ──────────────────────────────────────────────────
//
// PSTATE fields live in both the regs array (for DaifSet/DaifClr/ERET)
// and the sysreg array (for MSR/MRS).  This helper maps sysreg IDs to
// the corresponding regs-array slot so ReadSysReg/WriteSysReg can keep
// them in sync.

fn pstate_mirror_slot(sysreg_id: u32) -> Option<i32> {
    use crate::target::aarch64::regs;
    use helm_isa::arm::aarch64::sysreg;
    match sysreg_id {
        sysreg::DAIF => Some(crate::interp::REG_DAIF as i32 * 8),
        sysreg::NZCV => Some(crate::interp::REG_NZCV as i32 * 8),
        sysreg::CURRENT_EL => Some(crate::interp::REG_CURRENT_EL as i32 * 8),
        sysreg::SPSEL => Some(crate::interp::REG_SPSEL as i32 * 8),
        // TPIDR_EL0 is the TLS pointer — hot in user-space code.
        // Mirror in regs[42] so MRS avoids the sysreg call.
        sysreg::TPIDR_EL0 => Some(regs::TPIDR_EL0 as i32 * 8),
        _ => None,
    }
}

// ── IR emission ─────────────────────────────────────────────────────

fn emit_ops(
    builder: &mut FunctionBuilder,
    ops: &[TcgOp],
    regs_ptr: cranelift_codegen::ir::Value,
    cpu_ctx: cranelift_codegen::ir::Value,
    mem_ctx: cranelift_codegen::ir::Value,
    sysreg_ctx: cranelift_codegen::ir::Value,
    tlb_ptr: cranelift_codegen::ir::Value,
    helpers: &Helpers,
) -> cranelift_codegen::ir::Value {
    let mut temps: HashMap<u32, cranelift_codegen::ir::Value> = HashMap::new();
    let mut label_blocks: HashMap<u32, cranelift_codegen::ir::Block> = HashMap::new();
    let flags = MemFlags::new();
    let mut saw_isb = false;

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
            TcgOp::SDiv { dst, a, b } => {
                // Signed division: div-by-zero → 0, MIN/-1 → MIN
                let bv = t!(b.0);
                let one = builder.ins().iconst(I64, 1);
                let is_zero = builder.ins().icmp(IntCC::Equal, bv, zero_val);
                let safe_b = builder.ins().select(is_zero, one, bv);
                let r = builder.ins().sdiv(t!(a.0), safe_b);
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
            TcgOp::Sext {
                dst,
                src,
                from_bits,
            } => {
                let shift = builder.ins().iconst(I64, (64 - *from_bits) as i64);
                let shifted = builder.ins().ishl(t!(src.0), shift);
                temps.insert(dst.0, builder.ins().sshr(shifted, shift));
            }
            TcgOp::Zext {
                dst,
                src,
                from_bits,
            } => {
                let mask = if *from_bits >= 64 {
                    u64::MAX
                } else {
                    (1u64 << *from_bits) - 1
                };
                let mask_v = builder.ins().iconst(I64, mask as i64);
                temps.insert(dst.0, builder.ins().band(t!(src.0), mask_v));
            }

            // Register access — direct load/store from regs array
            TcgOp::ReadReg { dst, reg_id } => {
                let v = builder
                    .ins()
                    .load(I64, flags, regs_ptr, (*reg_id as i32) * 8);
                temps.insert(dst.0, v);
            }
            TcgOp::WriteReg { reg_id, src } => {
                builder
                    .ins()
                    .store(flags, t!(src.0), regs_ptr, (*reg_id as i32) * 8);
            }

            // Memory access — helper call with built-in fast TLB check.
            // The helper (helm_mem_read/write) has an inline fast TLB path
            // that checks FastTlbEntry and uses the addend for direct host
            // access on hit.  Only falls to the slow translate path on miss.
            TcgOp::Load { dst, addr, size } => {
                let addr_v = t!(addr.0);
                let size_v = builder.ins().iconst(I64, *size as i64);
                let inst = builder
                    .ins()
                    .call(helpers.fn_mr, &[cpu_ctx, mem_ctx, addr_v, size_v]);
                temps.insert(dst.0, builder.inst_results(inst)[0]);
            }
            TcgOp::Store { addr, val, size } => {
                let addr_v = t!(addr.0);
                let val_v = t!(val.0);
                let size_v = builder.ins().iconst(I64, *size as i64);
                builder
                    .ins()
                    .call(helpers.fn_mw, &[cpu_ctx, mem_ctx, addr_v, val_v, size_v]);
            }

            // ── System registers via helper calls ─────────────────
            TcgOp::ReadSysReg { dst, sysreg_id } => {
                // PSTATE fields (DAIF, NZCV, CURRENT_EL, SPSEL) are mirrored
                // in the regs array by DaifSet/DaifClr/ERET.  Read from regs
                // so MRS sees values written by those ops within the same block.
                if let Some(slot) = pstate_mirror_slot(*sysreg_id) {
                    let v = builder.ins().load(I64, flags, regs_ptr, slot);
                    temps.insert(dst.0, v);
                } else {
                    let id_v = builder.ins().iconst(I64, *sysreg_id as i64);
                    let inst = builder.ins().call(helpers.fn_sr, &[sysreg_ctx, id_v]);
                    temps.insert(dst.0, builder.inst_results(inst)[0]);
                }
            }
            TcgOp::WriteSysReg { sysreg_id, src } => {
                let id_v = builder.ins().iconst(I64, *sysreg_id as i64);
                let sv = t!(src.0);
                builder.ins().call(helpers.fn_sw, &[sysreg_ctx, id_v, sv]);
                // Mirror to regs array so DaifSet/DaifClr and the periodic
                // IRQ check see the updated value.
                if let Some(slot) = pstate_mirror_slot(*sysreg_id) {
                    builder.ins().store(flags, sv, regs_ptr, slot);
                }
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
                let c = builder
                    .ins()
                    .icmp(IntCC::SignedGreaterThanOrEqual, t!(a.0), t!(b.0));
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
                // ERET: read ELR/SPSR from sysreg array (where MSR writes them),
                // NOT from the flat regs array (which may be stale).
                use crate::interp::{REG_CURRENT_EL, REG_DAIF, REG_NZCV, REG_PC, REG_SPSEL};
                let pc_off = (REG_PC as i32) * 8;
                let nzcv_off = (REG_NZCV as i32) * 8;
                let daif_off = (REG_DAIF as i32) * 8;
                let cel_off = (REG_CURRENT_EL as i32) * 8;
                let spsel_off = (REG_SPSEL as i32) * 8;

                // Read ELR_EL1 and SPSR_EL1 from sysreg array via helper
                let elr_id = builder.ins().iconst(I64, 0xC201); // ELR_EL1
                let elr_inst = builder.ins().call(helpers.fn_sr, &[sysreg_ctx, elr_id]);
                let elr = builder.inst_results(elr_inst)[0];

                let spsr_id = builder.ins().iconst(I64, 0xC200); // SPSR_EL1
                let spsr_inst = builder.ins().call(helpers.fn_sr, &[sysreg_ctx, spsr_id]);
                let spsr = builder.inst_results(spsr_inst)[0];

                // PC = ELR_EL1
                builder.ins().store(flags, elr, regs_ptr, pc_off);
                // NZCV = SPSR & 0xF0000000
                let nzcv_mask = builder.ins().iconst(I64, 0xF000_0000u64 as i64);
                let nzcv = builder.ins().band(spsr, nzcv_mask);
                builder.ins().store(flags, nzcv, regs_ptr, nzcv_off);
                // DAIF = SPSR & 0x3C0
                let daif_mask = builder.ins().iconst(I64, 0x3C0);
                let daif = builder.ins().band(spsr, daif_mask);
                builder.ins().store(flags, daif, regs_ptr, daif_off);
                // CurrentEL = ((SPSR >> 2) & 3) << 2
                let two = builder.ins().iconst(I64, 2);
                let shifted = builder.ins().ushr(spsr, two);
                let three = builder.ins().iconst(I64, 3);
                let el = builder.ins().band(shifted, three);
                let el_shifted = builder.ins().ishl(el, two);
                builder.ins().store(flags, el_shifted, regs_ptr, cel_off);
                // SPSel = SPSR & 1
                let one = builder.ins().iconst(I64, 1);
                let spsel = builder.ins().band(spsr, one);
                builder.ins().store(flags, spsel, regs_ptr, spsel_off);

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

            // Cache/barrier — no-ops in JIT, but track ISB
            TcgOp::Barrier { kind } if *kind == 2 => {
                saw_isb = true; // ISB — session may need to flush caches
            }
            TcgOp::Barrier { .. } | TcgOp::Clrex => {}
            // TLBI — flush CPU TLB via helper
            TcgOp::Tlbi { op, addr } => {
                let op_v = builder.ins().iconst(I64, *op as i64);
                let addr_v = t!(addr.0);
                builder.ins().call(helpers.fn_ti, &[cpu_ctx, op_v, addr_v]);
            }
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
                    builder
                        .ins()
                        .call(helpers.fn_mw, &[cpu_ctx, mem_ctx, a, zero, sz]);
                }
            }

            // Exception ops — return to dispatcher
            TcgOp::SvcExc { .. }
            | TcgOp::HvcExc { .. }
            | TcgOp::SmcExc { .. }
            | TcgOp::BrkExc { .. }
            | TcgOp::HltExc { .. } => {
                let code = builder.ins().iconst(I64, EXIT_EXCEPTION);
                builder.ins().return_(&[code]);
                let n = builder.create_block();
                builder.switch_to_block(n);
                builder.seal_block(n);
            }

            TcgOp::At { .. } => {} // Address translation — no-op stub
        }
    }

    // Default: end of block (EXIT_ISB if ISB was in this block)
    let exit = if saw_isb { EXIT_ISB } else { EXIT_END_OF_BLOCK };
    builder.ins().iconst(I64, exit)
}

// ── Execution ───────────────────────────────────────────────────────

/// Execute a JIT-compiled block.
///
/// # Safety
/// Caller must ensure `regs` has NUM_REGS elements, `tlb_ptr` points to
/// a valid `[InlineTlbEntry; FAST_TLB_SIZE]`, and the JitBlock was compiled
/// by a still-valid JitEngine.
pub unsafe fn exec_jit(
    block: &JitBlock,
    regs: &mut [u64; NUM_REGS],
    cpu_ctx: *mut u8,
    mem: &mut helm_memory::address_space::AddressSpace,
    sysregs: &mut [u64],
    tlb_ptr: *mut u8,
) -> InterpResult {
    type JitFn = unsafe extern "C" fn(*mut u64, *mut u8, *mut u8, *mut u8, *mut u8) -> i64;
    let func: JitFn = std::mem::transmute(block.func_ptr);

    let exit_code = func(
        regs.as_mut_ptr(),
        cpu_ctx,
        mem as *mut _ as *mut u8,
        sysregs as *mut _ as *mut u8,
        tlb_ptr,
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
use crate::interp::MMU_DIRTY_IDX;
