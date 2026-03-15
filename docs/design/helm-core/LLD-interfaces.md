# helm-core — LLD: Interfaces

> Version: 0.1.0
> Status: Draft
> Cross-references: [HLD.md](HLD.md) · [LLD-arch-state.md](LLD-arch-state.md) · [TEST.md](TEST.md)

---

## Table of Contents

1. [ExecContext Trait](#1-execcontext-trait)
2. [ThreadContext Trait](#2-threadcontext-trait)
3. [ExecContext vs ThreadContext — Split Rationale](#3-execcontext-vs-threadcontext--split-rationale)
4. [MemInterface Trait](#4-meminterface-trait)
5. [Memory Types](#5-memory-types)
   - [MemRequest](#51-memrequest)
   - [MemResponse](#52-memresponse)
   - [MemOp](#53-memop)
   - [AccessType](#54-accesstype)
6. [MemFault Enum](#6-memfault-enum)
7. [HartException Enum](#7-hartexception-enum)

---

## 1. ExecContext Trait

`ExecContext` is the **hot-path** interface provided to ISA execute functions. It is called at least once per simulated instruction and potentially multiple times (for multi-operand instructions or memory accesses). All dispatch must be static.

The trait is defined in `helm-core`. The concrete implementing struct (`HelmExecContext`) lives in `helm-engine`. ISA execute functions in `helm-arch` receive `&mut impl ExecContext` — the compiler monomorphizes all calls.

```rust
// helm-core/src/exec_context.rs

use crate::arch_state::Isa;
use crate::mem::{MemFault, AccessType};
use crate::error::HartException;

/// Hot-path interface between the ISA execute layer and the hart's
/// architectural state + memory system.
///
/// Every method is called at least once per instruction.
/// Implementations MUST NOT allocate. MUST NOT block. MUST be `#[inline]`.
///
/// Dispatch: static only. ISA execute functions are:
///   fn execute_add<C: ExecContext>(ctx: &mut C, rd: u8, rs1: u8, rs2: u8)
/// The compiler monomorphizes C = HelmExecContext, inlining all calls.
pub trait ExecContext {

    // ── Integer registers ─────────────────────────────────────────────────

    /// Read an integer register (x0–x31 for RISC-V, X0–X30 for AArch64).
    ///
    /// - RISC-V: x0 always returns 0.
    /// - AArch64: X31 in a register-as-source context returns XZR (0).
    /// - Index is ISA-defined. Callers must ensure idx is in range;
    ///   behavior on out-of-range index is undefined in release builds.
    #[must_use]
    fn read_int_reg(&self, idx: u8) -> u64;

    /// Write an integer register.
    ///
    /// - RISC-V: writes to x0 are silently discarded.
    /// - AArch64 (W register): the caller zero-extends to 64 bits before
    ///   passing the value here; the context stores the full 64-bit value.
    fn write_int_reg(&mut self, idx: u8, val: u64);

    // ── Float registers ───────────────────────────────────────────────────

    /// Read a float register as raw u64 bits.
    ///
    /// - RISC-V: value is NaN-boxed. 32-bit floats occupy the lower 32
    ///   bits with the upper 32 bits set to all-ones.
    /// - AArch64: lower 64 bits of the 128-bit V register (D-register view).
    #[must_use]
    fn read_float_reg(&self, idx: u8) -> u64;

    /// Write a float register as raw u64 bits.
    ///
    /// - RISC-V: value must be NaN-boxed by the caller for 32-bit floats.
    /// - AArch64: stored in the lower 64 bits; upper 64 bits are zeroed
    ///   (D-register write semantics).
    fn write_float_reg(&mut self, idx: u8, val: u64);

    // ── Program counter ───────────────────────────────────────────────────

    /// Read the current PC.
    #[must_use]
    fn read_pc(&self) -> u64;

    /// Write the PC (for branch/jump/exception targets).
    fn write_pc(&mut self, val: u64);

    // ── CSR / system registers ────────────────────────────────────────────

    /// Read a control/status register (RISC-V) or system register (AArch64).
    ///
    /// - RISC-V: addr is the 12-bit CSR address (0x000–0xFFF).
    /// - AArch64: addr is the 20-bit encoded system register key
    ///   computed by sysreg_key(op0, op1, crn, crm, op2).
    ///
    /// Returns the raw stored value. The ISA layer is responsible for:
    /// - Checking access privilege before calling this.
    /// - Applying WARL masks to the returned value if needed.
    /// - Triggering side effects (TLB flush, interrupt mask change, etc.).
    #[must_use]
    fn read_csr(&self, addr: u32) -> u64;

    /// Write a control/status register.
    ///
    /// Stores the raw value. The ISA layer handles:
    /// - Access privilege checking.
    /// - WARL masking of the value before passing it here.
    /// - Side effects after this call returns.
    fn write_csr(&mut self, addr: u32, val: u64);

    // ── Memory ────────────────────────────────────────────────────────────

    /// Read `size` bytes from `addr` using the specified access type.
    ///
    /// Returns the value zero-extended to u64.
    ///
    /// `size` must be 1, 2, 4, or 8. Other values are undefined behavior
    /// in release builds; debug builds assert.
    ///
    /// `access_type` distinguishes: Load (data read), Fetch (instruction
    /// fetch), and Atomic (load-reserved / CAS source read). The memory
    /// system uses this for cache policies and access control.
    ///
    /// On error, returns `Err(MemFault)`. The caller (ISA layer) decides
    /// whether to call `raise_exception`, propagate with `?`, or handle
    /// ISA-specific soft-fail behavior.
    fn read_mem(
        &mut self,
        addr:        u64,
        access_type: AccessType,
        size:        usize,
    ) -> Result<u64, MemFault>;

    /// Write `size` bytes of `val` to `addr`.
    ///
    /// `val` is the value to write, stored in the lower `size * 8` bits.
    /// Upper bits are ignored.
    ///
    /// On error, returns `Err(MemFault)`.
    fn write_mem(
        &mut self,
        addr:        u64,
        access_type: AccessType,
        size:        usize,
        val:         u64,
    ) -> Result<(), MemFault>;

    // ── Exception raising ─────────────────────────────────────────────────

    /// Raise a hart exception, interrupting normal instruction flow.
    ///
    /// Implementations update the ISA-defined exception state (e.g.,
    /// RISC-V `mcause`, `mtval`, `mepc`; AArch64 `ESR_EL1`, `ELR_EL1`,
    /// `FAR_EL1`) and redirect the PC to the exception vector.
    ///
    /// Returns `Err(HartException)` so callers can propagate with `?`
    /// to break out of the execute loop. The `Err` value mirrors the
    /// exception that was raised (for the engine's scheduler to act on).
    ///
    /// Callers in the ISA layer typically write:
    ///   ctx.raise_exception(HartException::IllegalInstruction { pc })?;
    /// and the `?` propagates up to `Hart::step()`, which handles recovery.
    fn raise_exception(&mut self, exc: HartException) -> Result<!, HartException>;

    // ── ISA identification (used by dispatch code) ─────────────────────────

    /// Which ISA this context executes.
    ///
    /// Called once by the decode dispatch, not per-instruction. Inlined.
    fn isa(&self) -> Isa;
}
```

### Usage Pattern

ISA execute functions follow this pattern:

```rust
// In helm-arch/src/riscv/execute.rs

use helm_core::exec_context::ExecContext;
use helm_core::mem::AccessType;
use helm_core::error::HartException;

/// Execute: ADD rd, rs1, rs2
pub fn exec_add<C: ExecContext>(ctx: &mut C, rd: u8, rs1: u8, rs2: u8) {
    let a = ctx.read_int_reg(rs1);
    let b = ctx.read_int_reg(rs2);
    ctx.write_int_reg(rd, a.wrapping_add(b));
}

/// Execute: LW rd, imm(rs1) — load 4 bytes, sign-extend to 64 bits
pub fn exec_lw<C: ExecContext>(ctx: &mut C, rd: u8, rs1: u8, imm: i32)
    -> Result<(), HartException>
{
    let addr = ctx.read_int_reg(rs1).wrapping_add(imm as i64 as u64);
    let val = ctx.read_mem(addr, AccessType::Load, 4)
        .map_err(|f| HartException::from_mem_fault_load(f, addr))?;
    // Sign-extend: bits [31] extend to [63:32]
    let sign_extended = (val as i32) as i64 as u64;
    ctx.write_int_reg(rd, sign_extended);
    Ok(())
}

/// Execute: CSRRW rd, csr, rs1 — atomic CSR read/write
pub fn exec_csrrw<C: ExecContext>(ctx: &mut C, rd: u8, csr: u16, rs1: u8)
    -> Result<(), HartException>
{
    // The ISA layer checks privilege before this point.
    let old = ctx.read_csr(csr as u32);
    let new = ctx.read_int_reg(rs1);
    ctx.write_int_reg(rd, old);
    ctx.write_csr(csr as u32, new);
    // Side effects (e.g., satp flush) triggered by the caller after this returns.
    Ok(())
}
```

---

## 2. ThreadContext Trait

`ThreadContext` is the **cold-path** interface for external agents. It is `dyn`-safe and uses dynamic dispatch (`&mut dyn ThreadContext`). Called by:

- GDB RSP stub (read/write registers, single-step)
- Linux syscall handler (read arguments, write return value)
- Python inspection API (attribute read/write)
- Checkpoint coordinator (full state dump/restore)

```rust
// helm-core/src/thread_context.rs

use crate::exec_context::ExecContext;
use crate::arch_state::Isa;
use crate::error::HartException;

/// Cold-path interface for external agents interacting with a hart.
///
/// Dispatch: dynamic (`&mut dyn ThreadContext`). Overhead is acceptable
/// because calls originate from GDB, Python, or syscall handlers —
/// never from the per-instruction hot loop.
///
/// ThreadContext is a supertrait of ExecContext. A `&mut dyn ThreadContext`
/// can be coerced to `&mut dyn ExecContext` for passing to ISA helpers
/// that require ExecContext but need to be called from cold paths.
pub trait ThreadContext: ExecContext {

    // ── Hart identification ────────────────────────────────────────────────

    /// The hart ID. Corresponds to RISC-V mhartid / AArch64 MPIDR_EL1.
    fn hart_id(&self) -> u64;

    /// The ISA implemented by this hart.
    ///
    /// (Also available via ExecContext::isa(), repeated here for convenience
    /// when working with only a ThreadContext reference.)
    fn isa_id(&self) -> Isa;

    /// The execution mode: Functional, Syscall, or FullSystem.
    fn exec_mode(&self) -> ExecMode;

    // ── Bulk register access (for GDB register dump/restore) ───────────────

    /// Read all architectural integer registers into `out`.
    ///
    /// `out` must be pre-allocated to hold `num_int_regs()` u64 values.
    /// Values are written in ISA-defined register order (x0–x31 for
    /// RISC-V, X0–X30, SP, PC for AArch64 per the GDB target description).
    fn read_all_int_regs(&self, out: &mut Vec<u64>);

    /// Write all architectural integer registers from `values`.
    ///
    /// `values` must have exactly `num_int_regs()` entries in ISA order.
    fn write_all_int_regs(&mut self, values: &[u64]);

    /// Read all float registers as raw u64 bits into `out`.
    fn read_all_float_regs(&self, out: &mut Vec<u64>);

    /// Write all float registers.
    fn write_all_float_regs(&mut self, values: &[u64]);

    /// Number of integer registers this ISA exposes.
    fn num_int_regs(&self) -> usize;

    /// Number of float registers this ISA exposes.
    fn num_float_regs(&self) -> usize;

    // ── Execution control ──────────────────────────────────────────────────

    /// Pause execution at the end of the current instruction.
    ///
    /// The hart completes the current instruction and halts before
    /// fetching the next. Does not immediately suspend the thread.
    fn request_pause(&mut self);

    /// Resume execution from a paused state.
    fn resume(&mut self);

    /// True if the hart is currently paused (not executing).
    fn is_paused(&self) -> bool;

    // ── Architectural state access (for checkpoint coordinator) ────────────

    /// Read the full program counter.
    ///
    /// (Also available via ExecContext::read_pc(), repeated here for
    /// completeness in cold-path code that may not have an ExecContext ref.)
    fn get_pc(&self) -> u64;

    /// Write the program counter directly (used by GDB 'P' packet and
    /// checkpoint restore).
    fn set_pc(&mut self, val: u64);

    // ── Attribute system access ────────────────────────────────────────────

    /// Read a named attribute by name (e.g., "x0", "pc", "mstatus").
    ///
    /// Returns None if the attribute name is not registered.
    fn get_attr(&self, name: &str) -> Option<crate::attr::AttrValue>;

    /// Write a named attribute by name.
    ///
    /// Returns Err if the attribute name is not registered or if the
    /// value type is incompatible.
    fn set_attr(&mut self, name: &str, val: crate::attr::AttrValue)
        -> Result<(), crate::attr::AttrError>;

    /// List all registered attribute names for this hart.
    fn list_attrs(&self) -> Vec<&'static str>;

    // ── Memory (functional reads — used by GDB 'm' packet) ─────────────────

    /// Read `len` bytes from `addr` using functional (side-effect-free) access.
    ///
    /// Returns the raw bytes. Never updates caches or TLBs.
    /// Used by the GDB stub and Python memory inspection.
    fn read_mem_functional(&self, addr: u64, len: usize)
        -> Result<Vec<u8>, crate::mem::MemFault>;

    /// Write `data` bytes to `addr` using functional access.
    ///
    /// Used by GDB 'M' packet to inject breakpoint instructions.
    fn write_mem_functional(&mut self, addr: u64, data: &[u8])
        -> Result<(), crate::mem::MemFault>;
}

/// Execution mode for this hart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// Functional emulation: no OS, no timing. Pure ISA correctness.
    Functional,
    /// Syscall emulation: userspace only, syscalls handled by host OS shim.
    Syscall,
    /// Full system: boot a kernel, model all hardware.
    FullSystem,
}
```

### Usage by the GDB Stub

```rust
// In helm-debug/src/gdb.rs (simplified)

fn handle_read_registers(tc: &dyn ThreadContext) -> Vec<u8> {
    let mut regs = Vec::new();
    tc.read_all_int_regs(&mut regs);
    // Encode as GDB hex-encoded register dump
    encode_gdb_registers(&regs)
}

fn handle_write_register(tc: &mut dyn ThreadContext, reg_idx: usize, val: u64) {
    if reg_idx == gdb_pc_index(tc.isa_id()) {
        tc.set_pc(val);
    } else {
        tc.write_int_reg(reg_idx as u8, val);
    }
}

fn handle_read_memory(tc: &dyn ThreadContext, addr: u64, len: usize)
    -> Result<Vec<u8>, String>
{
    tc.read_mem_functional(addr, len)
        .map_err(|f| format!("MemFault: {:?}", f))
}
```

### Usage by the Syscall Handler

```rust
// In helm-engine/se/src/handler.rs (simplified)

use helm_core::thread_context::ThreadContext;

pub fn handle_write_syscall(tc: &mut dyn ThreadContext) -> i64 {
    // RISC-V: args in a0 (fd), a1 (buf_addr), a2 (count)
    let fd    = tc.read_int_reg(10) as i32;
    let buf   = tc.read_int_reg(11);
    let count = tc.read_int_reg(12) as usize;

    let data = tc.read_mem_functional(buf, count)
        .expect("syscall read_mem_functional failed");

    let result = host_write(fd, &data);

    // Return value in a0
    tc.write_int_reg(10, result as u64);
    result
}
```

---

### Design Decision: ExecContext/ThreadContext Relationship (Q6)

Q6 (answered in DESIGN-QUESTIONS.md) specifies that `ExecContext` and `ThreadContext` are **separate, independent traits with no supertrait relationship**. The `ThreadContext: ExecContext` supertrait shown above is the pre-Q6 design; the final answer resolves this differently:

- `ExecContext` and `ThreadContext` are separate traits.
- `RiscvHart` / `Aarch64Hart` implement **both** on the same struct.
- The engine holds `hart: H where H: ExecContext` for the hot path.
- Cold-path callers obtain `&mut dyn ThreadContext` via `hart.as_thread_context()`.
- The syscall handler receives **only** `&mut dyn ThreadContext`, never `ExecContext`. This matches gem5's pattern where `syscall_emul.hh` handlers receive `ThreadContext *tc` only.

The **exact method allocation** from Q6:

```rust
// ExecContext — hot path only, called per instruction
pub trait ExecContext {
    fn read_ireg(&self, reg: IReg) -> u64;
    fn write_ireg(&mut self, reg: IReg, val: u64);
    fn read_freg(&self, reg: FReg) -> u64;
    fn write_freg(&mut self, reg: FReg, val: u64);
    fn read_pc(&self) -> u64;
    fn write_next_pc(&mut self, val: u64);
    fn read_csr(&self, csr: u16) -> Result<u64, CsrFault>;
    fn write_csr(&mut self, csr: u16, val: u64) -> Result<(), CsrFault>;
    fn privilege_level(&self) -> PrivilegeLevel;
    fn raise_exception(&mut self, cause: ExceptionCause) -> !;
    fn read_sc_failures(&self) -> u32;
    fn write_sc_failures(&mut self, n: u32);
}

// ThreadContext — cold path only, always &mut dyn ThreadContext
pub trait ThreadContext {
    fn hart_id(&self) -> u32;
    fn isa(&self) -> Isa;
    fn read_ireg_raw(&self, idx: usize) -> u64;
    fn write_ireg_raw(&mut self, idx: usize, val: u64);
    fn read_freg_raw(&self, idx: usize) -> u64;
    fn write_freg_raw(&mut self, idx: usize, val: u64);
    fn read_pc(&self) -> u64;
    fn set_pc(&mut self, val: u64);
    fn privilege_level(&self) -> PrivilegeLevel;
    fn set_privilege_level(&mut self, pl: PrivilegeLevel);
    fn read_csr_raw(&self, csr: u16) -> u64;
    fn write_csr_raw(&mut self, csr: u16, val: u64);
    fn syscall_args(&self) -> SyscallArgs;
    fn set_syscall_return(&mut self, val: i64);
    fn status(&self) -> HartStatus;
    fn activate(&mut self);
    fn suspend(&mut self);
    fn halt(&mut self);
    fn save_attrs(&self, store: &mut AttrStore);
    fn restore_attrs(&mut self, store: &AttrStore);
}
```

**Note:** The remaining sections of this document reflect the earlier supertrait design and require reconciliation with the Q6 final answer during implementation.

---

## 3. ExecContext vs ThreadContext — Split Rationale

The table below documents which methods appear in each interface and why:

| Method | ExecContext | ThreadContext | Rationale |
|---|---|---|---|
| `read_int_reg` | Yes | (inherited) | Hot path: called every instruction |
| `write_int_reg` | Yes | (inherited) | Hot path |
| `read_float_reg` | Yes | (inherited) | Hot path for FP instructions |
| `write_float_reg` | Yes | (inherited) | Hot path |
| `read_pc` | Yes | (inherited) | Hot path: needed for branch resolution |
| `write_pc` | Yes | (inherited) | Hot path: needed for every instruction |
| `read_csr` | Yes | (inherited) | Called for CSR instructions |
| `write_csr` | Yes | (inherited) | Called for CSR instructions |
| `read_mem` | Yes | (inherited) | Hot path: every load instruction |
| `write_mem` | Yes | (inherited) | Hot path: every store instruction |
| `raise_exception` | Yes | (inherited) | Called on instruction faults |
| `isa` | Yes | (inherited) | Called once per dispatch, inlined |
| `hart_id` | No | Yes | Cold: GDB thread identification |
| `isa_id` | No | Yes | Cold: convenience on cold path |
| `exec_mode` | No | Yes | Cold: set at config time, checked rarely |
| `read_all_int_regs` | No | Yes | Cold: GDB `g` packet, checkpoint |
| `write_all_int_regs` | No | Yes | Cold: GDB `G` packet, checkpoint restore |
| `read_all_float_regs` | No | Yes | Cold: GDB register dump |
| `write_all_float_regs` | No | Yes | Cold: GDB restore |
| `num_int_regs` | No | Yes | Cold: GDB introspection |
| `num_float_regs` | No | Yes | Cold: GDB introspection |
| `request_pause` | No | Yes | Cold: GDB `c` halt, Python pause |
| `resume` | No | Yes | Cold: GDB `vCont;c` |
| `is_paused` | No | Yes | Cold: GDB state query |
| `get_pc` | No | Yes | Cold: GDB `p PC`, Python inspection |
| `set_pc` | No | Yes | Cold: GDB `P PC=val`, checkpoint |
| `get_attr` | No | Yes | Cold: Python attribute read |
| `set_attr` | No | Yes | Cold: Python attribute write |
| `list_attrs` | No | Yes | Cold: Python introspection |
| `read_mem_functional` | No | Yes | Cold: GDB `m`, Python memory read |
| `write_mem_functional` | No | Yes | Cold: GDB `M`, breakpoint injection |

**Key principle:** if a method is needed by `helm-arch`'s execute functions, it belongs in `ExecContext`. If it is only needed by `helm-debug`, `helm-engine/se`, or `helm-python`, it belongs in `ThreadContext`.

**The supertrait relationship `ThreadContext: ExecContext`** allows the single-step GDB operation to be implemented as:
1. Set a step-one flag via `tc.request_pause()`.
2. Call the ISA step function with `tc as &mut dyn ExecContext` — the dynamic coercion is valid because `ThreadContext: ExecContext`.
3. One instruction executes.
4. The engine sees `is_paused()` and stops.

Without the supertrait, step 2 would require duplicating the step function signature or passing the full `ThreadContext` reference (which the hot-path ISA execute functions should not have access to).

---

## 4. MemInterface Trait

`MemInterface` abstracts the memory subsystem. Three access mode families:

- **Timing** — asynchronous, event-driven, used in Virtual/Interval/Accurate timing modes
- **Atomic** — synchronous with estimated latency, used for fast-forward
- **Functional** — synchronous, side-effect-free, used for debugging and binary loading

The concrete implementor lives in `helm-memory`. The trait is defined here so `helm-core` types can reference it.

```rust
// helm-core/src/mem/mod.rs

use super::types::{MemRequest, MemResponse, MemOp, AccessType};
use super::fault::MemFault;

/// The interface between a hart and the memory subsystem.
///
/// Three access mode families with distinct latency and side-effect contracts.
/// The implementing type (in helm-memory) routes requests through the
/// MemoryRegion tree, cache hierarchy, and MMIO handlers.
///
/// Dispatch: this trait is passed as `&mut impl MemInterface` to the
/// ExecContext implementation, which calls it for every load/store.
/// Static dispatch is used for the timing hot path; functional mode
/// may use dynamic dispatch since it is a cold path.
pub trait MemInterface {

    // ── Timing mode: asynchronous ──────────────────────────────────────────
    // Used when timing simulation is active (Virtual / Interval / Accurate).
    // The CPU fires the request and registers a callback. The call returns
    // immediately; the callback fires when the memory system delivers data.

    /// Issue an asynchronous read request.
    ///
    /// `on_complete` is called by the memory system when the request
    /// completes (possibly many simulated cycles later). The callback
    /// receives the `MemResponse` including data and timing metadata.
    ///
    /// The memory system owns the in-flight state for this request.
    /// The caller must not assume any ordering relative to other requests
    /// unless the memory model guarantees sequential consistency.
    fn read_timing(
        &mut self,
        req:         MemRequest,
        on_complete: Box<dyn FnOnce(MemResponse) + Send>,
    );

    /// Issue an asynchronous write request.
    ///
    /// `on_complete` is called when the write has been accepted by the
    /// memory system (not necessarily written to backing store). For
    /// write-back caches, this fires when the cache line is claimed.
    fn write_timing(
        &mut self,
        req:         MemRequest,
        on_complete: Box<dyn FnOnce(MemResponse) + Send>,
    );

    // ── Atomic mode: synchronous with latency ─────────────────────────────
    // Used for fast-forward (functional + latency estimate) and interval
    // timing. Returns immediately with the data and an estimated latency.

    /// Synchronous read that returns data and estimated latency.
    ///
    /// The memory system simulates the access through the cache hierarchy
    /// and returns the data along with the estimated cycle count. The
    /// caller adds this to the simulated cycle counter.
    ///
    /// May update cache state (unlike functional mode).
    fn read_atomic(
        &mut self,
        req: MemRequest,
    ) -> Result<MemResponse, MemFault>;

    /// Synchronous write with estimated latency.
    fn write_atomic(
        &mut self,
        req: MemRequest,
    ) -> Result<MemResponse, MemFault>;

    // ── Functional mode: synchronous, side-effect-free ─────────────────────
    // Used by debugger (GDB), binary loader, and checkpoint restore.
    // Must NOT update cache state, TLB, MSHR tables, or any timing state.

    /// Functional (side-effect-free) read.
    ///
    /// Reads directly from backing storage, bypassing caches and TLBs.
    /// May read from MMIO regions (device handlers receive `is_functional=true`).
    ///
    /// Returns raw bytes (not a zero-extended u64, to support multi-byte
    /// functional reads for binary loading).
    fn read_functional(
        &self,
        addr: u64,
        size: usize,
    ) -> Result<Vec<u8>, MemFault>;

    /// Functional (side-effect-free) write.
    ///
    /// Writes directly to backing storage, bypassing caches.
    /// Used by GDB to inject breakpoint instructions and by the binary
    /// loader to initialize guest memory.
    fn write_functional(
        &mut self,
        addr: u64,
        data: &[u8],
    ) -> Result<(), MemFault>;

    // ── Atomic memory operations (RISC-V A extension, AArch64 LDXR/STXR) ──

    /// Attempt an atomic compare-and-swap.
    ///
    /// If `*addr == expected`, writes `new_val` to `*addr` and returns
    /// `Ok(true)`. Otherwise returns `Ok(false)` and leaves `*addr` unchanged.
    ///
    /// Size must be 4 or 8 bytes. Other sizes are ISA-undefined.
    fn compare_and_swap(
        &mut self,
        addr:     u64,
        expected: u64,
        new_val:  u64,
        size:     usize,
    ) -> Result<bool, MemFault>;

    /// Load-reserved (RISC-V LR instruction / AArch64 LDXR).
    ///
    /// Reads `size` bytes from `addr` and reserves the address for
    /// a subsequent store-conditional. The reservation is hart-local.
    ///
    /// Returns the loaded value (zero-extended to u64).
    fn load_reserved(
        &mut self,
        addr: u64,
        size: usize,
    ) -> Result<u64, MemFault>;

    /// Store-conditional (RISC-V SC instruction / AArch64 STXR).
    ///
    /// If the load reservation set by `load_reserved` is still valid for
    /// `addr`, writes `val` and returns `Ok(true)`. Otherwise returns
    /// `Ok(false)` (store did not happen). Reservation is always cleared.
    fn store_conditional(
        &mut self,
        addr: u64,
        val:  u64,
        size: usize,
    ) -> Result<bool, MemFault>;
}
```

---

## 5. Memory Types

### 5.1 MemRequest

```rust
// helm-core/src/mem/types.rs

/// A memory access request: address, size, operation, and access type.
///
/// Used by timing and atomic mode. Functional mode uses simpler addr/size
/// parameters directly (see MemInterface::read_functional).
#[derive(Debug, Clone)]
pub struct MemRequest {
    /// Target physical address.
    pub addr:        u64,

    /// Access size in bytes. Must be 1, 2, 4, or 8 for most operations.
    /// Binary loader functional writes may use any size.
    pub size:        usize,

    /// The type of memory operation (read, write, fetch, atomic).
    pub op:          MemOp,

    /// The access type (load, store, fetch, functional).
    /// Determines cache policy and access control checks.
    pub access_type: AccessType,

    /// For write requests: the data to write, in the lower `size * 8` bits.
    /// Ignored for read requests.
    pub write_data:  u64,
}

impl MemRequest {
    /// Construct a load request.
    pub fn load(addr: u64, size: usize) -> Self {
        Self { addr, size, op: MemOp::Read, access_type: AccessType::Load, write_data: 0 }
    }

    /// Construct a store request.
    pub fn store(addr: u64, size: usize, val: u64) -> Self {
        Self { addr, size, op: MemOp::Write, access_type: AccessType::Store, write_data: val }
    }

    /// Construct an instruction fetch request.
    pub fn fetch(addr: u64, size: usize) -> Self {
        Self { addr, size, op: MemOp::Read, access_type: AccessType::Fetch, write_data: 0 }
    }
}
```

### 5.2 MemResponse

```rust
/// Response from a memory operation (timing and atomic modes).
#[derive(Debug, Clone)]
pub struct MemResponse {
    /// The original request that produced this response.
    pub request: MemRequest,

    /// For read operations: the loaded data, zero-extended to u64.
    pub data: u64,

    /// Estimated latency in simulated cycles. Zero for functional access.
    /// Informational in atomic mode; drives the event queue in timing mode.
    pub latency_cycles: u64,

    /// Whether the access hit in the L1 cache.
    pub l1_hit: bool,

    /// Whether the access was satisfied by the LLC (last-level cache).
    pub llc_hit: bool,
}
```

### 5.3 MemOp

```rust
/// The type of memory bus operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemOp {
    /// Read data from memory.
    Read,
    /// Write data to memory.
    Write,
    /// Read-Modify-Write (atomic operation like RISC-V AMO).
    ReadModifyWrite,
}
```

### 5.4 AccessType

```rust
/// The ISA-level purpose of a memory access.
///
/// The memory system uses this to:
/// - Select the appropriate cache policy (e.g., instruction caches
///   are separate from data caches in Harvard-style architectures).
/// - Apply access control (PMA/PMP in RISC-V, MPU/MMU attributes in AArch64).
/// - Decide whether to skip side effects (Functional type).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    /// Data load (e.g., LW, LD, LH).
    Load,
    /// Data store (e.g., SW, SD, STR).
    Store,
    /// Instruction fetch (PC-driven read for decode).
    Fetch,
    /// Functional/debug access. Side-effect-free. Never fills caches.
    /// Used by GDB, Python inspection, binary loading.
    Functional,
    /// Atomic load (LR in RISC-V, LDXR in AArch64).
    /// Reserves the address for a subsequent store-conditional.
    AtomicLoad,
    /// Atomic store (SC in RISC-V, STXR in AArch64).
    AtomicStore,
    /// Atomic read-modify-write (AMO instructions in RISC-V).
    AtomicRMW,
}
```

---

## 6. MemFault Enum

`MemFault` represents all reasons a memory operation can fail. It is returned as `Err(MemFault)` from memory access methods. The ISA layer converts it to a `HartException` using ISA-specific mapping.

```rust
// helm-core/src/mem/fault.rs

use thiserror::Error;

/// A memory access fault.
///
/// Returned as Err(MemFault) from ExecContext::read_mem, write_mem, and
/// the MemInterface accessor methods.
///
/// The ISA layer (helm-arch) converts MemFault to HartException using
/// ISA-specific rules (e.g., RISC-V uses mcause codes; AArch64 uses ESR_EL1).
///
/// MemFault variants are Copy + Clone (no allocation) for use in hot paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MemFault {
    /// Address is misaligned for the requested access size.
    ///
    /// RISC-V: may raise misaligned load/store exception (or trap if
    ///   RISC-V Misaligned extension not supported).
    /// AArch64: may raise alignment fault (DFSR).
    #[error("misaligned access at {addr:#018x}: size {size}")]
    Misaligned { addr: u64, size: usize },

    /// Address is not mapped in the physical address space.
    ///
    /// RISC-V: raises access fault (mcause = 5 for load, 7 for store).
    /// AArch64: raises translation fault.
    #[error("unmapped address {addr:#018x}")]
    Unmapped { addr: u64 },

    /// Access denied by the memory protection model.
    ///
    /// RISC-V: PMP (Physical Memory Protection) denied the access.
    /// AArch64: MPU or stage-1 translation attributes denied the access.
    #[error("access fault at {addr:#018x}: {reason}")]
    AccessFault { addr: u64, reason: AccessFaultReason },

    /// Page fault: virtual address translation failed.
    ///
    /// RISC-V: Sv39/Sv48 page table walk failed or protection mismatch.
    /// AArch64: stage-1 or stage-2 translation fault.
    ///
    /// `is_write`: true if this was a store or AMO access.
    #[error("page fault at {vaddr:#018x} (write={is_write})")]
    PageFault { vaddr: u64, is_write: bool },

    /// Instruction fetch fault: PC points to non-executable memory.
    #[error("instruction access fault at {pc:#018x}")]
    InstructionFault { pc: u64 },

    /// Store-conditional failure: reservation was lost.
    ///
    /// This is not an error — `Ok(false)` is returned from
    /// `store_conditional()` for this case. This variant is here for
    /// completeness and for RISC-V SC failure injection.
    #[error("store-conditional failed at {addr:#018x}")]
    StoreConditionalFail { addr: u64 },

    /// MMIO handler returned an error for a device register access.
    #[error("MMIO error at {addr:#018x}: {code}")]
    MmioError { addr: u64, code: u32 },

    /// Access size is not supported for this region.
    /// (e.g., 8-byte access to a device that only supports 4-byte reads)
    #[error("unsupported access size {size} at {addr:#018x}")]
    UnsupportedSize { addr: u64, size: usize },
}

/// Reason for an access fault (PMP / MPU violation details).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessFaultReason {
    /// Attempted write to a read-only region.
    WriteToReadOnly,
    /// Attempted execute from a non-executable region.
    ExecuteFromNonExecutable,
    /// Attempted user-mode access to a supervisor-only region.
    UserAccessSupervisorRegion,
    /// Attempted privileged access to a user-only region.
    PrivilegedAccessUserRegion,
    /// PMP (RISC-V): access not covered by any PMP entry.
    PmpUnmatched,
    /// PMP: access matched a PMP entry with incompatible permissions.
    PmpPermissionDenied,
}
```

### MemFault → HartException Mapping

The mapping is ISA-specific and lives in `helm-arch`. A convenience method:

```rust
// In helm-arch/src/riscv/exceptions.rs

use helm_core::mem::MemFault;
use helm_core::error::HartException;

impl HartException {
    /// Convert a MemFault from a load instruction to a RISC-V HartException.
    pub fn from_mem_fault_load(fault: MemFault, addr: u64) -> HartException {
        match fault {
            MemFault::Misaligned { addr, size: _ } =>
                HartException::LoadAddressMisaligned { addr },
            MemFault::Unmapped { addr } | MemFault::AccessFault { addr, .. } =>
                HartException::LoadAccessFault { addr },
            MemFault::PageFault { vaddr, .. } =>
                HartException::LoadPageFault { vaddr },
            _ => HartException::LoadAccessFault { addr },
        }
    }

    /// Convert a MemFault from a store instruction.
    pub fn from_mem_fault_store(fault: MemFault, addr: u64) -> HartException {
        match fault {
            MemFault::Misaligned { addr, .. } =>
                HartException::StoreAddressMisaligned { addr },
            MemFault::Unmapped { addr } | MemFault::AccessFault { addr, .. } =>
                HartException::StoreAccessFault { addr },
            MemFault::PageFault { vaddr, .. } =>
                HartException::StorePageFault { vaddr },
            _ => HartException::StoreAccessFault { addr },
        }
    }
}
```

---

## 7. HartException Enum

`HartException` represents all reasons a hart can interrupt normal instruction flow. It is returned as `Err(HartException)` from `Hart::step()` and from `ExecContext::raise_exception()`.

```rust
// helm-core/src/error.rs

use thiserror::Error;

/// A hart-level exception: synchronous trap or asynchronous interrupt
/// that interrupts normal instruction execution.
///
/// Returned as Err(HartException) from Hart::step() and used internally
/// by the ISA execute functions via raise_exception().
///
/// ISA-specific exceptions carry the ISA-defined exception cause codes
/// as inner values. Non-ISA exceptions (e.g., simulation halt) are also
/// represented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum HartException {
    // ── RISC-V synchronous exceptions (mcause codes) ──────────────────────

    /// Instruction address misaligned (mcause=0).
    #[error("instruction address misaligned at {pc:#018x}")]
    InstructionAddressMisaligned { pc: u64 },

    /// Instruction access fault (mcause=1).
    #[error("instruction access fault at {pc:#018x}")]
    InstructionAccessFault { pc: u64 },

    /// Illegal instruction (mcause=2).
    #[error("illegal instruction at {pc:#018x}: encoding {encoding:#010x}")]
    IllegalInstruction { pc: u64, encoding: u32 },

    /// Breakpoint (mcause=3). EBREAK instruction or hardware breakpoint.
    #[error("breakpoint at {pc:#018x}")]
    Breakpoint { pc: u64 },

    /// Load address misaligned (mcause=4).
    #[error("load address misaligned at {addr:#018x}")]
    LoadAddressMisaligned { addr: u64 },

    /// Load access fault (mcause=5).
    #[error("load access fault at {addr:#018x}")]
    LoadAccessFault { addr: u64 },

    /// Store/AMO address misaligned (mcause=6).
    #[error("store address misaligned at {addr:#018x}")]
    StoreAddressMisaligned { addr: u64 },

    /// Store/AMO access fault (mcause=7).
    #[error("store access fault at {addr:#018x}")]
    StoreAccessFault { addr: u64 },

    /// Environment call from U-mode (mcause=8). ECALL from user mode.
    #[error("ecall from U-mode at {pc:#018x}")]
    EcallFromUMode { pc: u64 },

    /// Environment call from S-mode (mcause=9).
    #[error("ecall from S-mode at {pc:#018x}")]
    EcallFromSMode { pc: u64 },

    /// Environment call from M-mode (mcause=11).
    #[error("ecall from M-mode at {pc:#018x}")]
    EcallFromMMode { pc: u64 },

    /// Instruction page fault (mcause=12).
    #[error("instruction page fault at {vaddr:#018x}")]
    InstructionPageFault { vaddr: u64 },

    /// Load page fault (mcause=13).
    #[error("load page fault at {vaddr:#018x}")]
    LoadPageFault { vaddr: u64 },

    /// Store/AMO page fault (mcause=15).
    #[error("store page fault at {vaddr:#018x}")]
    StorePageFault { vaddr: u64 },

    // ── AArch64 exceptions ─────────────────────────────────────────────────

    /// AArch64 SVC instruction (supervisor call from EL0 to EL1).
    #[error("SVC #{imm} at {pc:#018x}")]
    Svc { pc: u64, imm: u16 },

    /// AArch64 HVC instruction (hypervisor call from EL1 to EL2).
    #[error("HVC #{imm} at {pc:#018x}")]
    Hvc { pc: u64, imm: u16 },

    /// AArch64 SMC instruction (secure monitor call to EL3).
    #[error("SMC #{imm} at {pc:#018x}")]
    Smc { pc: u64, imm: u16 },

    /// AArch64 undefined instruction at current EL.
    #[error("undefined instruction at {pc:#018x}: encoding {encoding:#010x}")]
    UndefinedInstruction { pc: u64, encoding: u32 },

    /// AArch64 data abort (load/store fault).
    #[error("data abort at {pc:#018x}: fault addr {fault_addr:#018x}")]
    DataAbort { pc: u64, fault_addr: u64 },

    /// AArch64 instruction abort (instruction fetch fault).
    #[error("instruction abort at {pc:#018x}")]
    InstructionAbort { pc: u64 },

    // ── Simulation-level pseudo-exceptions ────────────────────────────────

    /// The hart encountered a magic instruction (SIMICS-style simulation
    /// control via a RISC-V HINT or AArch64 NOP-space encoding).
    /// The simulation engine intercepts this before the ISA layer.
    #[error("magic instruction at {pc:#018x}: value {value:#018x}")]
    MagicInstruction { pc: u64, value: u64 },

    /// The hart has been requested to halt (simulation end condition).
    #[error("halt requested")]
    SimulationHalt,

    /// Execution mode requires a syscall handler, but none is configured.
    #[error("unhandled ecall: syscall mode not configured")]
    UnhandledEcall,

    /// WFI (wait for interrupt) — hart has no pending interrupts.
    /// The engine may advance simulated time to the next interrupt.
    #[error("WFI: waiting for interrupt")]
    WaitForInterrupt,
}
```

### Usage in the Execute Loop

```rust
// In helm-engine/src/engine.rs (simplified)

fn step_one(&mut self) -> Result<(), HartException> {
    let pc = self.state.read_pc();
    let insn_bytes = self.mem.read_functional(pc, 4)
        .map_err(|f| HartException::InstructionAccessFault { pc })?;

    let insn = decode_riscv(u32::from_le_bytes(insn_bytes.try_into().unwrap()))
        .ok_or(HartException::IllegalInstruction {
            pc,
            encoding: u32::from_le_bytes(insn_bytes.try_into().unwrap()),
        })?;

    execute_riscv(&mut self.exec_ctx, insn)
}

pub fn run(&mut self, n: u64) {
    for _ in 0..n {
        match self.step_one() {
            Ok(()) => {}
            Err(HartException::EcallFromUMode { pc }) => {
                self.syscall_handler.handle(&mut self.thread_ctx);
            }
            Err(HartException::Breakpoint { pc }) => {
                self.pause();
                break;
            }
            Err(HartException::SimulationHalt) => break,
            Err(HartException::WaitForInterrupt) => {
                self.advance_to_next_interrupt();
            }
            Err(exc) => {
                // Deliver exception to ISA trap handler
                self.deliver_exception(exc);
            }
        }
    }
}
```

### HartException Properties

| Property | Value |
|---|---|
| `Copy` | Yes — no allocation, safe to pass by value |
| `Clone` | Yes |
| `PartialEq` / `Eq` | Yes — enables test assertions |
| `Debug` | Yes — human-readable in error messages |
| `Error` (thiserror) | Yes — chain into higher-level errors |
| Heap allocation | Never — all fields are `Copy` primitive types |

The combination of `Copy` and `Error` means `HartException` values can be propagated with `?` without cloning and can be used in test assertions without borrow issues.
