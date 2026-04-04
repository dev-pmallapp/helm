# RMSB JIT — Phase 1 & Phase 2 Implementation Plan

> **Register-Mapped Superblock JIT (RMSB)**
> Detailed step-by-step implementation guide to reach 30–60 MIPS (Phase 1)
> and 80–150 MIPS (Phase 2), producing ~5 000 lines of Rust/C code across
> 8 weeks with zero new heavyweight dependencies.
>
> Source of truth: `docs/research/jit-acceleration-no-llvm.md`
> Worktree: `worktree-jit-rmsb-phase1-phase2`

---

## Quick Reference

| Phase | Weeks | New Lines | Predicted MIPS | Status |
|-------|-------|-----------|----------------|--------|
| 1-A — Register Pinning | 1 | ~650 | 15–25 | TODO |
| 1-B — Lazy NZCV | 2 | ~350 | 25–40 | TODO |
| 1-C — Inline TLB (SE) | 2 | ~250 | 40–60 | TODO |
| 1-D — Instruction Fusion | 3 | ~450 | 50–70 | TODO |
| 1-E — Direct Threading | 4 | ~300 | interp 5–6 | TODO |
| 1-F — Benchmark Gate | 4 | ~200 | — | TODO |
| 2-A — W^X Code Arena | 5 | ~450 | — | TODO |
| 2-B — Block Chaining | 5–6 | ~650 | 60–100 | TODO |
| 2-C — Adaptive Reg Bind | 6 | ~250 | +10–30% | TODO |
| 2-D — LuaJIT Tracing | 7 | ~1 000 | 80–150 | TODO |
| 2-E — Speculative IC | 8 | ~350 | +2x SE mem | TODO |
| 2-F — Benchmark Gate | 8 | ~200 | — | TODO |
| **Total** | **8** | **~5 100** | **80–150** | |

---

## Baseline Measurements (record before any change)

Run the following and fill in the blanks before Week 1:

```bash
# Build current main
cargo build --release --workspace

# Baseline MIPS (SE mode, AArch64)
cargo run --release --bin helm-aarch64 -- \
  --mode syscall --jit off \
  examples/se/hello_static_aarch64 2>&1 | grep MIPS
# Expected: ~3 MIPS   Actual: ___

cargo run --release --bin helm-aarch64 -- \
  --mode syscall --jit tiered \
  examples/se/hello_static_aarch64 2>&1 | grep MIPS
# Expected: ~5 MIPS   Actual: ___

# Correctness baseline — all JIT vs interp tests must pass
cargo test --package helm-jit --features backend-dynasm,backend-stencil
# Expected: all pass   Actual: ___
```

---

## Phase 1-A — Register Pinning (Week 1)

### Goal

Replace all `mov rax, [rdi + N*8]` guest register accesses with direct x86-64
hardware registers for the 8 most-used guest registers:

| x86-64 | Guest | Offset | Rationale |
|--------|-------|--------|-----------|
| `r8`   | X0    | 0      | Return value / arg 0 |
| `r9`   | X1    | 8      | Arg 1 |
| `r10`  | X2    | 16     | Arg 2 |
| `r11`  | X3    | 24     | Arg 3 |
| `r12`  | X19   | 152    | Callee-saved: loop counter |
| `r13`  | X20   | 160    | Callee-saved: loop base |
| `r14`  | X30 (LR) | 240 | Link register |
| `r15`  | SP    | 248    | Stack pointer |
| `rbx`  | X4    | 32     | Arg 4 |
| `rbp`  | NZCV  | 264    | Flags (packed u32) |

`rdi` = flat array ptr (preserved). `rsi` = mem ptr (preserved).
Remaining X5–X18, X21–X29 spill to `[rdi + slot*8]`.

### Step 1-A-1: Extend `regs.rs` — `RegBinding` struct (~100 lines)

**File:** `framework/helm-jit/src/regs.rs`

Add after the existing constants:

```rust
/// Which x86-64 hardware register holds a given guest register slot.
/// `None` means the slot lives in the flat array at `[rdi + slot*8]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostReg {
    R8, R9, R10, R11, R12, R13, R14, R15, Rbx, Rbp,
}

/// Static default binding: top-8 most-used ARM registers pinned to host.
pub static DEFAULT_BINDING: [(usize, HostReg); 10] = [
    (REG_X0,      HostReg::R8),
    (REG_X0 + 1,  HostReg::R9),    // X1
    (REG_X0 + 2,  HostReg::R10),   // X2
    (REG_X0 + 3,  HostReg::R11),   // X3
    (REG_X0 + 4,  HostReg::Rbx),   // X4
    (REG_X0 + 19, HostReg::R12),   // X19
    (REG_X0 + 20, HostReg::R13),   // X20
    (REG_X0 + 30, HostReg::R14),   // X30 (LR)
    (REG_SP,      HostReg::R15),   // SP
    (REG_NZCV,    HostReg::Rbp),   // NZCV
];

/// Lookup: guest slot → host register (if pinned).
pub fn pinned_host_reg(slot: usize) -> Option<HostReg> {
    DEFAULT_BINDING.iter()
        .find(|(s, _)| *s == slot)
        .map(|(_, r)| *r)
}
```

Add `arch_to_flat_pinned` and `flat_to_arch_pinned` variants that skip
pinned slots (they are already live in host regs on entry/exit of a
compiled block). These are called at JIT→interpreter boundary only.

```rust
/// Sync only the NON-PINNED guest registers from arch state.
/// Pinned regs (X0–X4, X19, X20, LR, SP, NZCV) are live in host regs.
pub fn arch_to_flat_nonpinned(a64: &Aarch64ArchState, flat: &mut [u64; REG_COUNT]) {
    for i in 0..31 {
        if pinned_host_reg(i).is_none() {
            flat[i] = a64.x[i];
        }
    }
    // SP and NZCV are pinned — skip.
    flat[REG_PC] = a64.pc;   // PC is never pinned (updated by branch emitters)
}
```

**Lines added:** ~100

### Step 1-A-2: Emit helpers for pinned vs spilled regs (~150 lines)

**New file:** `framework/helm-jit/src/dynasm/pinned.rs`

```rust
//! Dynasm register references for pinned vs spilled guest regs.
//!
//! Use these instead of bare `[rdi + off]` in emitters.

use crate::regs::{pinned_host_reg, reg_offset, HostReg};
use dynasm::dynasm;
use dynasmrt::{x64::Assembler, DynasmApi};

/// Load a guest register into `rax`. If pinned: `mov rax, <host_reg>`.
/// If spilled: `mov rax, [rdi + off]`.
pub fn load_guest_to_rax(ops: &mut Assembler, slot: usize) {
    match pinned_host_reg(slot) {
        Some(HostReg::R8)  => dynasm!(ops ; mov rax, r8),
        Some(HostReg::R9)  => dynasm!(ops ; mov rax, r9),
        Some(HostReg::R10) => dynasm!(ops ; mov rax, r10),
        Some(HostReg::R11) => dynasm!(ops ; mov rax, r11),
        Some(HostReg::R12) => dynasm!(ops ; mov rax, r12),
        Some(HostReg::R13) => dynasm!(ops ; mov rax, r13),
        Some(HostReg::R14) => dynasm!(ops ; mov rax, r14),
        Some(HostReg::R15) => dynasm!(ops ; mov rax, r15),
        Some(HostReg::Rbx) => dynasm!(ops ; mov rax, rbx),
        Some(HostReg::Rbp) => dynasm!(ops ; mov rax, rbp),
        None => {
            let off = reg_offset(slot);
            dynasm!(ops ; mov rax, QWORD [rdi + off]);
        }
    }
}

/// Store `rax` into a guest register slot.
pub fn store_rax_to_guest(ops: &mut Assembler, slot: usize) {
    match pinned_host_reg(slot) {
        Some(HostReg::R8)  => dynasm!(ops ; mov r8, rax),
        Some(HostReg::R9)  => dynasm!(ops ; mov r9, rax),
        // ... (same pattern for all 10 host regs)
        None => {
            let off = reg_offset(slot);
            dynasm!(ops ; mov QWORD [rdi + off], rax);
        }
    }
}

/// Emit block prologue: push callee-saved host regs, load pinned guest regs.
/// Called at entry of every compiled block.
pub fn emit_pinned_prologue(ops: &mut Assembler) {
    // Push callee-saved regs we're using as guest pinned registers.
    // RBX, RBP, R12–R15 are callee-saved in x86-64 ABI.
    dynasm!(ops
        ; push rbx
        ; push rbp
        ; push r12
        ; push r13
        ; push r14
        ; push r15
    );
    // Load pinned guest registers from flat array into host regs.
    // (flat array ptr is in rdi on entry)
    for (slot, hreg) in &DEFAULT_BINDING {
        let off = reg_offset(*slot) as i32;
        match hreg {
            HostReg::R8  => dynasm!(ops ; mov r8,  QWORD [rdi + off]),
            // ... all 10
            _ => {}
        }
    }
}

/// Emit block epilogue: flush pinned regs back, pop callee-saved.
pub fn emit_pinned_epilogue(ops: &mut Assembler) {
    // Write pinned host regs back to flat array.
    for (slot, hreg) in &DEFAULT_BINDING {
        let off = reg_offset(*slot) as i32;
        match hreg {
            HostReg::R8 => dynasm!(ops ; mov QWORD [rdi + off], r8),
            // ...
            _ => {}
        }
    }
    dynasm!(ops
        ; pop r15
        ; pop r14
        ; pop r13
        ; pop r12
        ; pop rbp
        ; pop rbx
    );
}
```

**Lines added:** ~150

### Step 1-A-3: Update `emit/dp.rs` to use pinned helpers (~200 lines changed)

**File:** `framework/helm-jit/src/dynasm/emit/dp.rs`

Replace every `mov rax, QWORD [rdi + rn_off]` / `mov QWORD [rdi + rd_off], rax`
call with `load_guest_to_rax(ops, slot)` / `store_rax_to_guest(ops, slot)`.

For fully-pinned pairs (rd and rn both pinned), the emitter can use the
host regs directly without going through `rax`:

```rust
// ADD X0, X1, #imm — both X0 and X1 are pinned
// Before:  mov rax,[rdi+8]; add rax,imm; mov [rdi+0],rax
// After:   lea r8, [r9 + imm]    (1 instruction!)
pub fn emit_add_sub_imm(ops: &mut Assembler, insn: &Instruction) {
    let rd_pin = pinned_host_reg(insn.rd as usize);
    let rn_pin = pinned_host_reg(src_slot_sp(insn.rn));
    match (rd_pin, rn_pin) {
        (Some(rd_h), Some(rn_h)) if insn.sf && !is_sub => {
            // Pure register-to-register with immediate
            emit_lea_add_imm(ops, rd_h, rn_h, insn.imm);
        }
        _ => {
            // Fallback: load to rax, operate, store
            load_guest_to_rax(ops, src_slot_sp(insn.rn));
            // ... existing logic
            store_rax_to_guest(ops, dst_slot_sp(insn.rd));
        }
    }
}
```

**Lines changed:** ~200 (not net-new — restructuring existing ~600 line file)

### Step 1-A-4: Update `dynasm/mod.rs` — prologue/epilogue injection (~50 lines)

**File:** `framework/helm-jit/src/dynasm/mod.rs`

At the start of `compile_block()`, call `emit_pinned_prologue(ops)`.
Before `ops.finalize()`, call `emit_pinned_epilogue(ops)`.
Remove the now-redundant `mov rax, QWORD EXIT_END_OF_BLOCK; ret` epilogue
(it moves inside `emit_pinned_epilogue`).

**Lines changed:** ~50

### Step 1-A-5: Update `jit.rs` — sync only non-pinned on boundary (~100 lines)

**File:** `runtime/helm-engine/src/jit.rs`

Replace `arch_to_flat` with `arch_to_flat_pinned_prologue` (loads pinned regs
into host regs at the start of `run_jit`). Replace `flat_to_arch` with
`flat_to_arch_pinned_epilogue` (reads pinned regs back from host regs on exit).

The loop body no longer calls `flat_to_arch` between blocks — pinned regs
stay live in host registers across the entire `run_jit` call.

**Lines changed:** ~100

### Step 1-A Tests

```bash
# Run existing JIT correctness suite — must still pass 100%
cargo test --package helm-jit --features backend-dynasm -- --test-threads=1

# New: pinned register round-trip test
# Add to regs.rs tests:
#   1. arch_to_flat_nonpinned skips pinned slots
#   2. pinned register values survive prologue+epilogue

# Run full binary (correctness):
cargo run --release --bin helm-aarch64 -- --mode syscall --jit dynasm \
  examples/se/hello_static_aarch64
# Expected: correct output   Actual: ___

# MIPS measurement:
# Expected: 15–25 MIPS   Actual: ___
```

**Predicted MIPS after 1-A:** 15–25 MIPS (3–5x over baseline)

---

## Phase 1-B — Lazy NZCV (~350 lines, Week 2)

### Goal

Defer NZCV flag computation. Instead of computing all 4 flags after every
ADDS/SUBS/ANDS, store `{op, lhs, rhs}` in 3 scratch slots in the flat array.
B.cond/CBZ emitters read the pending record and compute only the needed flag.

If SUBS and B.cond are adjacent in the same block, the compiler fuses them
into a direct `cmp; jcc` with no pending-flags storage at all.

### Step 1-B-1: Define `PendingFlags` and flat array slots (~50 lines)

**File:** `framework/helm-jit/src/regs.rs`

```rust
/// Pending-flags record layout in flat array (slots 38–40).
///
/// slot 38: FlagOp (0=none, 1=ADD64, 2=SUB64, 3=AND64, 4=ADD32, 5=SUB32, 6=AND32)
/// slot 39: lhs operand (u64)
/// slot 40: rhs operand (u64)
pub const REG_FLAG_OP:  usize = 38;
pub const REG_FLAG_LHS: usize = 39;
pub const REG_FLAG_RHS: usize = 40;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FlagOp {
    None   = 0,
    Add64  = 1, Sub64  = 2, And64  = 3,
    Add32  = 4, Sub32  = 5, And32  = 6,
}
```

### Step 1-B-2: Write `emit_nzcv_store_pending` and `emit_nzcv_materialize` (~150 lines)

**New file:** `framework/helm-jit/src/dynasm/emit/nzcv.rs`

```rust
/// Emit code to store a pending flag computation (for ADDS/SUBS/ANDS).
/// Instead of computing NZCV, stores {FlagOp, lhs, rhs} into slots 38–40.
pub fn emit_store_pending(ops: &mut Assembler, op: FlagOp, lhs_slot: usize, rhs_slot: usize) {
    let op_off  = reg_offset(REG_FLAG_OP);
    let lhs_off = reg_offset(REG_FLAG_LHS);
    let rhs_off = reg_offset(REG_FLAG_RHS);

    // Store op code
    dynasm!(ops ; mov BYTE [rdi + op_off], op as i8);
    // Store lhs
    load_guest_to_rax(ops, lhs_slot);
    dynasm!(ops ; mov QWORD [rdi + lhs_off], rax);
    // Store rhs (immediate or register)
    load_guest_to_rax(ops, rhs_slot);
    dynasm!(ops ; mov QWORD [rdi + rhs_off], rax);
}

/// Emit code to materialize a specific flag from the pending record.
/// Used by B.cond emitters when the pending flag is needed.
///
/// For the Z flag (B.EQ / B.NE), emits:
///   mov rax, [lhs]; mov rcx, [rhs]; cmp rax, rcx; setz cl; test cl, cl
pub fn emit_materialize_z(ops: &mut Assembler) {
    let op_off  = reg_offset(REG_FLAG_OP);
    let lhs_off = reg_offset(REG_FLAG_LHS);
    let rhs_off = reg_offset(REG_FLAG_RHS);
    dynasm!(ops
        ; movzx ecx, BYTE [rdi + op_off]
        ; test ecx, ecx
        ; jz >already_live           // FlagOp::None → NZCV already in rbp
        ; mov rax, QWORD [rdi + lhs_off]
        ; mov rcx, QWORD [rdi + rhs_off]
        ; cmp rax, rcx               // This sets x86 ZF, CF, SF, OF
        // Pack result into rbp (NZCV slot)
        ; setz al  ; shl eax, 30     // Z flag at bit 30
        ; mov ebp, eax
        ; mov BYTE [rdi + op_off], 0 // clear pending
        ; already_live:
    );
}
```

### Step 1-B-3: Update `emit/dp.rs` — ADDS/SUBS emit pending instead of NZCV (~80 lines changed)

For ADDS Xd, Xn, Xm: call `emit_store_pending(ops, FlagOp::Add64, rn_slot, rm_slot)`.
Skip the existing 12-instruction `setCC` sequence entirely.

### Step 1-B-4: Update `emit/branch.rs` — B.cond calls `emit_materialize_*` (~70 lines changed)

For B.EQ: call `emit_materialize_z(ops)` then `jnz >not_taken`.
For B.NE: call `emit_materialize_z(ops)` then `jz >not_taken`.
For B.LT/B.GE/B.GT/B.LE/B.CC/B.CS: equivalent N, C, V materializers.

**Adjacent-pair fusion:** In `dynasm/mod.rs`'s emit loop, detect when insns[i]
is a flag-setting instruction and insns[i+1] is a B.cond using that flag.
Emit them as a single `cmp; jcc` pair — no pending storage at all.

```rust
// In compile_block():
if let (Some(flag_insn), Some(branch_insn)) = peek_flag_branch_pair(&insns[i..]) {
    emit_fused_flag_branch(ops, flag_insn, branch_insn);
    i += 2;
    break; // terminates block
}
```

### Step 1-B Tests

```bash
# Existing NZCV tests in dynasm/mod.rs — must all still pass
cargo test --package helm-jit --features backend-dynasm \
  jit_vs_interp_subs_sweep
cargo test --package helm-jit -- jit_vs_interp

# New property-based tests (add to helm-jit/tests/nzcv_lazy.rs):
#   For every (FlagOp, u64 lhs, u64 rhs) triple:
#   - lazy path NZCV == eager path NZCV
#   Using proptest with 10_000 cases

cargo test --package helm-jit --features backend-dynasm -- nzcv_lazy

# MIPS:
# Expected: 25–40 MIPS   Actual: ___
```

**Predicted MIPS after 1-B:** 25–40 MIPS (+1.5–2x over 1-A)

---

## Phase 1-C — Inline TLB Fast Path, SE Mode (~250 lines, Week 2)

### Goal

Replace the `call jit_mem_read` / `call jit_mem_write` helper call sequences
(~20 x86 instructions per memory access) with an inline TLB lookup (~8
x86 instructions, 99% hit rate).

### New TLB data structures (~60 lines)

**File:** `framework/helm-jit/src/helpers.rs`

```rust
/// One TLB entry: 16 bytes, cache-line friendly.
#[repr(C, align(16))]
pub struct JitTlbEntry {
    /// Guest virtual page number (guest_addr >> 12).
    /// 0xFFFF_FFFF_FFFF_FFFF = invalid.
    pub va_tag:   u64,
    /// Corresponding host page base pointer.
    pub host_ptr: u64,  // *mut u8 as u64
}

/// Flat 256-entry direct-mapped TLB for SE-mode JIT.
/// Indexed by (guest_addr >> 12) & 0xFF.
/// Lives in HelmEngine alongside FlatMem.
pub struct JitSeTlb {
    pub entries: Box<[JitTlbEntry; 256]>,
}

impl JitSeTlb {
    pub fn new() -> Self {
        Self {
            entries: Box::new(
                std::array::from_fn(|_| JitTlbEntry { va_tag: u64::MAX, host_ptr: 0 })
            ),
        }
    }
    pub fn flush(&mut self) {
        for e in self.entries.iter_mut() { e.va_tag = u64::MAX; }
    }
}
```

Store a `JitSeTlb` in `HelmEngine`. Pass `tlb.entries.as_ptr() as u64`
through flat array slot 41 at the start of `run_jit`.

### Inline TLB emitter (~130 lines)

**File:** `framework/helm-jit/src/dynasm/emit/ldst.rs`

Replace `emit_mem_read_call` with:

```rust
/// Emit inline TLB load. On TLB miss, falls back to `jit_mem_read`.
/// Inputs: rax = guest effective address.
/// Output: rax = loaded value (size bytes, zero-extended).
pub fn emit_tlb_load(ops: &mut Assembler, size_bytes: u32) {
    let tlb_ptr_off = reg_offset(41) as i32; // slot 41 = TLB base
    dynasm!(ops
        ; mov rcx, rax                   // rcx = guest addr
        ; shr rcx, 12                    // page number
        ; and ecx, 0xFF                  // TLB index (256 entries)
        ; mov rdx, QWORD [rdi + tlb_ptr_off]  // rdx = TLB base
        ; shl rcx, 4                     // entry size = 16 bytes
        ; add rdx, rcx                   // rdx = &tlb[idx]
        ; mov rcx, QWORD [rdx]           // rcx = va_tag
        ; mov r9, rax
        ; shr r9, 12
        ; cmp rcx, r9                    // TLB hit?
        ; jne >miss
        ; mov rcx, QWORD [rdx + 8]       // rcx = host_ptr
        ; and eax, 0xFFF                 // page offset
        ; add rcx, rax                   // host effective address
    );
    match size_bytes {
        1 => dynasm!(ops ; movzx eax, BYTE [rcx]),
        2 => dynasm!(ops ; movzx eax, WORD [rcx]),
        4 => dynasm!(ops ; mov eax, DWORD [rcx]),
        8 => dynasm!(ops ; mov rax, QWORD [rcx]),
        _ => unreachable!(),
    }
    dynasm!(ops
        ; jmp >done
        ; miss:
    );
    // Fall back to helper call (existing code path)
    emit_mem_read_call_slow(ops, size_bytes);
    dynasm!(ops ; done:);
}
```

Also add `emit_tlb_store` for writes, and `emit_tlb_fill` (the slow path
that fills a TLB entry on miss from FlatMem's page table).

**Lines added:** ~130

### Step 1-C Tests

```bash
# Existing load/store JIT tests
cargo test --package helm-jit --features backend-dynasm -- ldst

# New: TLB correctness test
# Add helm-jit/tests/tlb_inline.rs:
#   1. Set up a JitSeTlb pointing at a mock page
#   2. Compile a block with LDR X0, [X1, #0]
#   3. Execute — verify load hits TLB, returns correct value
#   4. Flush TLB — verify miss path invokes slow helper

# MIPS (memory-heavy workload):
# Expected: 40–60 MIPS   Actual: ___
```

**Predicted MIPS after 1-C:** 40–60 MIPS (compound: pinning + lazy NZCV + inline TLB)

---

## Phase 1-D — Instruction Fusion (~450 lines, Week 3)

### Goal

Detect common 2-instruction patterns in the decode window before stencil
or dynasm compilation. Emit them as single fused units, eliminating
intermediate register writes and NZCV materialization.

### Fusion patterns (priority order)

| ID | Pattern | Frequency | Savings |
|----|---------|-----------|---------|
| F1 | `CMP Xn, #imm` + `B.cond` | ~12% | Eliminates NZCV store, 1 block exit |
| F2 | `SUBS Xd, Xn, #1` + `B.NE` | ~5% | Loop decrement + branch in 1 unit |
| F3 | `LDR Xd, [Xn, #off]` + `ADD Xd, Xd, Xm` | ~4% | Load-use fuse, no intermediate reg |
| F4 | `MOV Xd, #imm` + `STR Xd, [Xn]` | ~3% | Constant store |
| F5 | `ADD Xd, Xn, Xm` + `STR Xd, [Xk]` | ~3% | Compute-store |

### Step 1-D-1: Fusion detector (~150 lines)

**New file:** `framework/helm-jit/src/dynasm/fusion.rs`

```rust
use helm_arch::aarch64::insn::{Instruction, Opcode};

#[derive(Debug)]
pub enum FusedPair<'a> {
    /// CMP Xn,#imm immediately followed by B.cond.
    CmpBranch { cmp: &'a Instruction, branch: &'a Instruction },
    /// SUBS Xd,Xn,#1 immediately followed by B.NE (loop decrement).
    SubsBne { subs: &'a Instruction, bne: &'a Instruction },
    /// LDR Xd,[Xn] immediately followed by ALU using Xd.
    LoadUse { load: &'a Instruction, alu: &'a Instruction },
}

/// Try to detect a fusable pair starting at `insns[0]`.
/// Returns `Some((pair, insns_consumed))` if a fusion is possible.
pub fn try_fuse<'a>(insns: &'a [Instruction]) -> Option<(FusedPair<'a>, usize)> {
    let (a, b) = (insns.get(0)?, insns.get(1)?);
    // F1: CMP + B.cond
    if is_cmp(a) && is_bcond(b) {
        return Some((FusedPair::CmpBranch { cmp: a, branch: b }, 2));
    }
    // F2: SUBS *,*,#1 + B.NE
    if a.opcode == Opcode::SubsImm && a.imm == 1 && b.opcode == Opcode::BCond && b.cond == 1 {
        return Some((FusedPair::SubsBne { subs: a, bne: b }, 2));
    }
    // F3: LDR Xd + ALU using Xd as source
    if is_ldr(a) && is_alu_reg(b) && b.rn == a.rd {
        return Some((FusedPair::LoadUse { load: a, alu: b }, 2));
    }
    None
}

fn is_cmp(i: &Instruction) -> bool {
    (i.opcode == Opcode::SubsImm || i.opcode == Opcode::SubsReg) && i.rd == 31
}
fn is_bcond(i: &Instruction) -> bool { i.opcode == Opcode::BCond }
fn is_ldr(i: &Instruction) -> bool {
    matches!(i.opcode, Opcode::Ldr | Opcode::Ldrb | Opcode::Ldrh | Opcode::Ldrsw)
}
fn is_alu_reg(i: &Instruction) -> bool {
    matches!(i.opcode, Opcode::AddReg | Opcode::SubReg | Opcode::OrrReg | Opcode::AndReg)
}
```

### Step 1-D-2: Fused emitters (~200 lines)

**New file:** `framework/helm-jit/src/dynasm/emit/fused.rs`

```rust
/// Emit F1: CMP Xn,#imm + B.cond as a single fused unit.
/// Result: no intermediate NZCV write; direct jcc to target.
pub fn emit_cmp_branch(ops: &mut Assembler, pair: &FusedPair) {
    if let FusedPair::CmpBranch { cmp, branch } = pair {
        load_guest_to_rax(ops, cmp.rn as usize); // load Xn
        let imm = cmp.imm;
        let target = branch.pc.wrapping_add(branch.imm as u64);
        let fallthrough = branch.pc.wrapping_add(4);
        dynasm!(ops
            ; cmp rax, QWORD imm        // direct comparison
        );
        // Emit jcc based on branch.cond
        emit_jcc_for_cond(ops, branch.cond, target, fallthrough);
    }
}

/// Emit F2: SUBS Xd,Xn,#1 + B.NE (tight loop decrement).
pub fn emit_subs_bne(ops: &mut Assembler, pair: &FusedPair) {
    if let FusedPair::SubsBne { subs, bne } = pair {
        let rd_slot = subs.rd as usize;
        let rn_slot = subs.rn as usize;
        load_guest_to_rax(ops, rn_slot);
        dynasm!(ops ; sub rax, 1);
        store_rax_to_guest(ops, rd_slot);
        let target = bne.pc.wrapping_add(bne.imm as u64);
        let fallthrough = bne.pc.wrapping_add(4);
        dynasm!(ops ; jnz QWORD =>target_label); // loop back if non-zero
        // (handle label resolution via dynasm DynamicLabel)
    }
}
```

### Step 1-D-3: Hook into `compile_block` (~100 lines changed)

**File:** `framework/helm-jit/src/dynasm/mod.rs`

In the instruction emit loop, before calling `emit::emit_insn`, try `try_fuse`:

```rust
let mut i = 0;
while i < insns.len().min(MAX_BLOCK_INSNS) {
    if let Some((pair, consumed)) = fusion::try_fuse(&insns[i..]) {
        if let Some(is_term) = emit::fused::emit_fused_pair(&mut ops, &pair) {
            insn_count += consumed as u32;
            if is_term { break; }
            i += consumed;
            continue;
        }
    }
    // Normal single-instruction path
    match emit::emit_insn(&mut ops, &insns[i]) { ... }
    i += 1;
}
```

### Step 1-D Tests

```bash
# Correctness: fusion must produce same result as two separate instructions
# Add helm-jit/tests/fusion.rs:
#   - For each fusion pattern: JIT fused == interpreter sequential
#   - Test with proptest 10_000 random (Xn, imm) pairs per pattern

cargo test --package helm-jit --features backend-dynasm -- fusion

# MIPS:
# Expected: 50–70 MIPS   Actual: ___
```

**Predicted MIPS after 1-D:** 50–70 MIPS

---

## Phase 1-E — Direct Threading for Interpreter (~300 lines, Week 4)

### Goal

Replace the `match insn.opcode { ... }` dispatch in `step_aarch64()` with a
flat function-pointer dispatch table. Raises the interpreter floor from
~3 MIPS to ~5–6 MIPS — important for the fallback path.

### Step 1-E-1: Define `ExecFn` and dispatch table (~200 lines)

**New file:** `runtime/helm-engine/src/dispatch.rs`

```rust
use helm_arch::aarch64::{arch_state::Aarch64ArchState, insn::Instruction};
use helm_core::{ExecError, MemInterface};

pub type ExecFn = fn(
    &mut Aarch64ArchState,
    &Instruction,
    &mut dyn MemInterface,
) -> Result<bool, ExecError>;

/// Dispatch table: indexed by `insn.opcode as u8`.
/// Returns Ok(pc_written) on success; Err on fault/unimplemented.
pub static EXEC_TABLE: [ExecFn; 256] = build_table();

const fn build_table() -> [ExecFn; 256] {
    let mut t = [exec_unimpl as ExecFn; 256];
    // Fill in known opcodes:
    t[Opcode::AddImm as usize]  = exec_add_imm;
    t[Opcode::SubImm as usize]  = exec_sub_imm;
    t[Opcode::AddReg as usize]  = exec_add_reg;
    // ... all ~120 implemented opcodes
    t
}

fn exec_unimpl(
    _s: &mut Aarch64ArchState,
    i: &Instruction,
    _m: &mut dyn MemInterface,
) -> Result<bool, ExecError> {
    Err(ExecError::Unimplemented(i.opcode))
}
```

### Step 1-E-2: Update hot loop in `lib.rs` (~100 lines changed)

Replace:
```rust
match execute(&insn, state, mem)? { ... }
```
With:
```rust
let f = dispatch::EXEC_TABLE[insn.opcode as u8 as usize];
f(state, &insn, mem)?;
```

### Step 1-E Tests

```bash
cargo test --workspace  # All existing tests pass
# Benchmark interpreter-only path:
# Expected: ~5–6 MIPS   Actual: ___
```

---

## Phase 1-F — Benchmark Gate (Week 4)

### Benchmark suite (~200 lines)

**New file:** `benches/jit_mips.rs` (criterion benchmark)

```rust
use criterion::{criterion_group, criterion_main, Criterion, Throughput};

fn bench_jit_se(c: &mut Criterion) {
    let binary = include_bytes!("../examples/se/dhrystone_aarch64");
    // Run 10M instructions, measure wall time → compute MIPS
    let mut g = c.benchmark_group("jit-se");
    g.throughput(Throughput::Elements(10_000_000));
    for mode in &["interp", "stencil", "dynasm", "tiered"] {
        g.bench_with_input(BenchmarkId::from_parameter(mode), mode, |b, m| {
            b.iter(|| run_simulation(binary, m, 10_000_000));
        });
    }
}

criterion_group!(benches, bench_jit_se);
criterion_main!(benches);
```

### Phase 1 Pass/Fail Gate

| Check | Expected | Actual | Pass? |
|-------|----------|--------|-------|
| `cargo test --workspace` | all pass | ___ | ___ |
| JIT vs interp correctness | 0 diff | ___ | ___ |
| MIPS (tiered, Dhrystone) | 40–70 | ___ | ___ |
| MIPS (interpreter fallback) | 5–6 | ___ | ___ |
| Binary size delta | < +50KB | ___ | ___ |
| Build time delta | < +5s | ___ | ___ |

If MIPS < 30 after Phase 1: diagnose with `perf stat` before proceeding to Phase 2.

---

## Phase 2-A — W^X Double-Mapped Code Arena (~450 lines, Week 5)

### Goal

Replace ad-hoc `mmap(PROT_READ|PROT_EXEC)` for compiled blocks with a
double-mapped arena: one RW view for patching, one RX view for execution.
Enables block chaining (§2-B) and IC patching (§2-E) without mprotect syscalls.

### New file: `framework/helm-jit/src/arena.rs` (~350 lines)

```rust
//! Double-mapped W^X code arena.
//!
//! Physical pages are shared between two virtual mappings:
//! - `rw`: PROT_READ | PROT_WRITE  — for writing/patching compiled code
//! - `rx`: PROT_READ | PROT_EXEC   — for executing compiled code
//!
//! No mprotect() calls needed for patching. Any write through `rw`
//! is immediately visible at the same offset in `rx`.
//!
//! Implementation uses memfd_create (Linux ≥3.17) + two mmap calls.

use std::os::unix::io::RawFd;

pub struct CodeArena {
    rw_base:  *mut u8,   // RW view start
    rx_base:  *mut u8,   // RX view start (same physical pages)
    capacity: usize,
    cursor:   usize,     // next allocation offset
    fd:       RawFd,     // memfd backing
}

impl CodeArena {
    pub fn new(capacity: usize) -> Result<Self, ArenaError> {
        // 1. Create anonymous memory file
        let fd = unsafe { libc::memfd_create(b"helm_jit_code\0".as_ptr() as _, 0) };
        // 2. Set size
        unsafe { libc::ftruncate(fd, capacity as i64) };
        // 3. Map RW view
        let rw = unsafe { libc::mmap(
            std::ptr::null_mut(), capacity,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED, fd, 0,
        ) as *mut u8 };
        // 4. Map RX view
        let rx = unsafe { libc::mmap(
            std::ptr::null_mut(), capacity,
            libc::PROT_READ | libc::PROT_EXEC,
            libc::MAP_SHARED, fd, 0,
        ) as *mut u8 };
        Ok(Self { rw_base: rw, rx_base: rx, capacity, cursor: 0, fd })
    }

    /// Allocate `size` bytes. Returns (rw_ptr, rx_ptr) pair.
    pub fn alloc(&mut self, size: usize) -> Option<(*mut u8, *const u8)> {
        let aligned = (self.cursor + 15) & !15; // 16-byte align
        if aligned + size > self.capacity { return None; }
        self.cursor = aligned + size;
        Some((
            unsafe { self.rw_base.add(aligned) },
            unsafe { self.rx_base.add(aligned) },
        ))
    }

    /// Offset of an RX pointer back to its RW mirror.
    pub fn rx_to_rw(&self, rx_ptr: *const u8) -> *mut u8 {
        let off = rx_ptr as usize - self.rx_base as usize;
        unsafe { self.rw_base.add(off) }
    }
}
```

### Update `CompiledBlock` to use arena (~100 lines changed)

**File:** `framework/helm-jit/src/block.rs`

Add `rw_ptr: *mut u8` alongside `entry: JitBlockFn`. The RW pointer is used
by the block chaining patcher (§2-B) to overwrite the `ret` epilogue in-place.

---

## Phase 2-B — Block Chaining (~650 lines, Weeks 5–6)

### Goal

After compiling block A, look up block B in the JIT cache. If found, overwrite
block A's 5-byte `ret + nop` epilogue with a `jmp rel32` pointing directly
at block B's RX entry. Zero dispatch overhead for block-to-block transitions.

### Patch site protocol

Every compiled block reserves 5 bytes at the end for the exit:

```x86
; Unlinked exit (default):
ret                           ; 1 byte: 0xC3
nop nop nop nop               ; 4 bytes: 0x90 0x90 0x90 0x90

; Linked exit (patched by block chainer):
jmp rel32                     ; 5 bytes: 0xE9 <rel32>
```

The 5-byte slot is always written through the RW view of the CodeArena.

### New struct `PatchSite` and `BlockLinkage` (~150 lines)

**File:** `framework/helm-jit/src/block.rs`

```rust
/// One patchable exit point in a compiled block.
pub struct PatchSite {
    /// Offset within the block's allocation where the 5-byte exit lives.
    pub byte_offset: u32,
    /// Target guest PC this exit should jump to when linked.
    pub target_pc: u64,
    /// Whether this site is currently linked (jmp) or unlinked (ret+nop).
    pub linked: bool,
}

/// Back-reference for invalidation: which blocks jump to a given block.
pub type BlockKey = u64; // = guest PC

pub struct CompiledBlock {
    pub entry:        JitBlockFn,
    pub rw_ptr:       *mut u8,       // RW mirror for patching
    pub guest_pc:     u64,
    pub insn_count:   u32,
    pub patch_sites:  Vec<PatchSite>, // exits to be linked
    pub back_refs:    Vec<BlockKey>,  // blocks that jmp into this one
}
```

### Block linker in `JitCache` (~200 lines)

**File:** `framework/helm-jit/src/cache.rs`

```rust
impl JitCache {
    /// After inserting block B, scan for blocks that were waiting to
    /// link to B's guest PC and patch them.
    pub fn link_waiters(&mut self, arena: &CodeArena, new_block_pc: u64) {
        for entry in self.entries.iter_mut().flatten() {
            for site in entry.block.patch_sites.iter_mut() {
                if site.target_pc == new_block_pc && !site.linked {
                    // Compute rel32 from site to new block entry
                    let site_rw = unsafe {
                        (Arc::as_ptr(&entry.block) as *mut CompiledBlock)
                            .as_mut().unwrap()
                            .rw_ptr.add(site.byte_offset as usize)
                    };
                    let from = site_rw as i64 + 5; // after the jmp insn
                    let to   = entry.block.entry as i64;
                    let rel  = (to - from) as i32;
                    unsafe {
                        *site_rw = 0xE9;  // JMP rel32 opcode
                        *(site_rw.add(1) as *mut i32) = rel;
                    }
                    site.linked = true;
                }
            }
        }
    }

    /// On eviction, unlink all blocks that jump to the evicted block.
    pub fn unlink_block(&mut self, evicted_pc: u64) {
        for entry in self.entries.iter_mut().flatten() {
            for site in entry.block.patch_sites.iter_mut() {
                if site.target_pc == evicted_pc && site.linked {
                    // Restore ret + nop
                    let site_rw = ...; // same as above
                    unsafe {
                        *site_rw = 0xC3;                   // ret
                        *site_rw.add(1) = 0x90;            // nop ×4
                        *site_rw.add(2) = 0x90;
                        *site_rw.add(3) = 0x90;
                        *site_rw.add(4) = 0x90;
                    }
                    site.linked = false;
                }
            }
        }
    }
}
```

### `run_jit` update (~150 lines changed in `jit.rs`)

Remove `arch_to_flat` / `flat_to_arch` between blocks. Pinned regs are live
in host regs. The dispatch loop becomes a single `while` that calls the cached
entry point and handles non-zero exit codes only:

```rust
// After block chaining: the JIT hot loop is near-empty
while retired < max_insns {
    let pc = /* read from pinned r-register via inline asm */ ...;
    let entry = cache.lookup(pc)?;
    let exit = unsafe { (entry)(flat_regs.as_mut_ptr(), mem_ptr) };
    retired += entry.insn_count;
    if exit != EXIT_END_OF_BLOCK { handle_exit(exit); break; }
    // For chained blocks: entry() already patched → cpu just runs the chain
    // without returning to Rust at all
}
```

### Step 2-B Tests

```bash
# Chain correctness: compile A→B→C, verify execution visits all three
# without returning to Rust between them
cargo test --package helm-jit --features backend-dynasm -- chaining

# Invalidation: evict B, verify A's exit returns to Rust
cargo test --package helm-jit -- chain_invalidation

# MIPS:
# Expected: 60–100 MIPS   Actual: ___
```

**Predicted MIPS after 2-B:** 60–100 MIPS

---

## Phase 2-C — Adaptive Register Binding (~250 lines, Week 6)

### Goal

Profile per-register access frequency during execution. After 500K instructions,
re-rank the top-10 most-used guest registers. If the ranking differs from
the current `DEFAULT_BINDING`, flush the JIT cache and recompile with the new
binding. Adapts to actual workload rather than static ARM ABI assumptions.

### `RegHeatMap` struct (~100 lines)

**File:** `framework/helm-jit/src/regs.rs`

```rust
pub struct RegHeatMap {
    /// Count of how many times each guest register slot was accessed.
    pub access_count: [u32; 32], // X0–X31
    pub total_insns:  u64,
    pub generation:   u32,       // bumped each time binding changes
}

impl RegHeatMap {
    /// Record a register access. Called from dynasm emitters (at compile time,
    /// not execution time — we count based on instruction frequency in compiled blocks).
    pub fn record_access(&mut self, slot: usize) {
        if slot < 32 { self.access_count[slot] = self.access_count[slot].saturating_add(1); }
    }

    /// After `total_insns` reaches threshold, compute optimal top-10 binding.
    pub fn compute_optimal_binding(&self) -> [(usize, HostReg); 10] {
        let mut ranked: Vec<(u32, usize)> = self.access_count.iter()
            .enumerate().map(|(i, &c)| (c, i)).collect();
        ranked.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        let host_regs = [HostReg::R8, HostReg::R9, HostReg::R10, HostReg::R11,
                         HostReg::R12, HostReg::R13, HostReg::R14, HostReg::R15,
                         HostReg::Rbx, HostReg::Rbp];
        std::array::from_fn(|i| (ranked[i].1, host_regs[i]))
    }
}
```

### Integration in `jit.rs` (~100 lines)

Every 500K retired instructions (checked in the `run_jit` loop), call
`heat_map.compute_optimal_binding()`. If the result differs from the current
binding, update `CURRENT_BINDING` (an `AtomicPtr` or `RwLock`), flush the
JIT cache, and log the change.

### Step 2-C Tests

```bash
# Verify binding adaptation: run a workload that uses X8–X15 heavily
# (not the default X0–X4 assumption). Verify after 500K insns the
# binding shifts to include X8–X15.
cargo test --package helm-engine -- adaptive_reg_binding

# MIPS: adaptive binding should give +10–30% over static
# Expected: previous + 10%   Actual: ___
```

---

## Phase 2-D — LuaJIT-Style Trace Compilation (~1000 lines, Week 7)

### Goal

Record the actual execution path through multiple basic blocks. Compile the
hot path as a single x86-64 function with internal backward jmp for loops.
Guard exits handle mispredictions.

### New module: `framework/helm-jit/src/trace/` (~1000 lines total)

#### `trace/mod.rs` (~150 lines)

```rust
//! LuaJIT-style trace recorder and compiler.

pub mod compiler;
pub mod exit;
pub mod recorder;

/// State machine for a single recording session.
pub enum RecordState {
    /// Not currently recording.
    Idle,
    /// Recording from `start_pc`, have seen `depth` blocks so far.
    Recording {
        start_pc: u64,
        insns:    Vec<Instruction>,
        pcs:      Vec<u64>,
        depth:    u32,
    },
    /// Recording complete — ready to compile.
    Complete(Vec<Instruction>),
}

/// Hot counter: when a back-edge PC is executed N times, start recording.
pub const TRACE_THRESHOLD: u32 = 64;
pub const TRACE_MAX_INSNS: usize = 512;
pub const TRACE_MAX_DEPTH: u32 = 8;  // max blocks to inline
```

#### `trace/recorder.rs` (~200 lines)

```rust
//! Detects hot back-edges and transitions to recording state.

pub struct TraceRecorder {
    /// Per-PC back-edge execution counters.
    counters: HashMap<u64, u32>,
    pub state: RecordState,
}

impl TraceRecorder {
    /// Called at every backward branch target. Returns true if recording started.
    pub fn on_backward_branch(&mut self, target_pc: u64) -> bool {
        let cnt = self.counters.entry(target_pc).or_insert(0);
        *cnt += 1;
        if *cnt == TRACE_THRESHOLD {
            self.state = RecordState::Recording {
                start_pc: target_pc,
                insns: Vec::with_capacity(TRACE_MAX_INSNS),
                pcs: Vec::new(),
                depth: 0,
            };
            true
        } else { false }
    }

    /// Feed a decoded instruction into the active recording.
    /// Returns Some(trace) when the recording loop is complete.
    pub fn record(&mut self, pc: u64, insns: &[Instruction]) -> Option<Vec<Instruction>> {
        if let RecordState::Recording { start_pc, insns: ref mut rec, depth, .. } = self.state {
            for insn in insns {
                rec.push(insn.clone());
                // Detect loop closure: back to start_pc
                if insn.is_branch() {
                    let target = branch_target(insn);
                    if target == *start_pc {
                        // Loop found — compile the trace
                        return Some(std::mem::take(rec));
                    }
                    *depth += 1;
                    if *depth >= TRACE_MAX_DEPTH || rec.len() >= TRACE_MAX_INSNS {
                        self.state = RecordState::Idle;
                        return None;
                    }
                }
            }
        }
        None
    }
}
```

#### `trace/compiler.rs` (~450 lines)

```rust
//! Compiles a recorded trace into a single x86-64 function.
//!
//! Key differences from block compilation:
//! - All conditional branches emit guards (jcc to side exit) not full exits
//! - The loop back-edge emits a direct `jmp trace_start`
//! - All pinned registers are live throughout (no sync between blocks)
//! - Memory accesses use the inline TLB fast path

pub fn compile_trace(
    arena: &mut CodeArena,
    insns: &[Instruction],
    start_pc: u64,
) -> Option<CompiledTrace> {
    let mut ops = Assembler::new().ok()?;
    let mut guard_exits: Vec<GuardExit> = Vec::new();

    // Emit prologue (same as block prologue: push callee-saved, load pinned regs)
    emit_pinned_prologue(&mut ops);

    // Dynamic label for back-edge
    let trace_start = ops.new_dynamic_label();
    dynasm!(ops ; =>trace_start);

    let mut i = 0;
    while i < insns.len() {
        let insn = &insns[i];

        // Try fusion first
        if let Some((pair, consumed)) = try_fuse(&insns[i..]) {
            emit_trace_fused(&mut ops, &pair, start_pc, &mut guard_exits);
            i += consumed;
            continue;
        }

        match insn.opcode {
            // Conditional branch: emit guard exit
            Opcode::BCond => {
                let taken_target = insn.pc.wrapping_add(insn.imm as u64);
                if taken_target == start_pc {
                    // Back-edge: emit jcc back to trace_start (loop!)
                    emit_loop_back_edge(&mut ops, insn, &trace_start);
                } else {
                    // Forward branch: emit guard, continue with fall-through
                    let guard_id = guard_exits.len();
                    emit_guard_exit(&mut ops, insn, guard_id, &mut guard_exits);
                    // Fall-through continues inlined in this trace
                }
            }
            // Normal instructions: emit same as block compiler
            _ => { emit_trace_insn(&mut ops, insn); }
        }
        i += 1;
    }

    // Epilogue: flush pinned regs, return EXIT_END_OF_BLOCK
    emit_pinned_epilogue(&mut ops);

    let buf = ops.finalize().ok()?;
    let entry = /* rx pointer */ ...;
    Some(CompiledTrace { entry, guard_exits, insn_count: insns.len() as u32, start_pc })
}

/// Emit a guard exit: `jcc <side_exit_N>` + side exit stub that syncs
/// registers and returns EXIT_GUARD(guard_id).
fn emit_guard_exit(ops: &mut Assembler, insn: &Instruction, id: usize, exits: &mut Vec<GuardExit>) {
    let exit_label = ops.new_dynamic_label();
    // Conditional jump to exit (taken = not the hot path)
    dynasm!(ops ; jcc =>exit_label);
    exits.push(GuardExit { label: exit_label, guard_id: id, exit_pc: branch_target(insn) });
    // Side exit stub (cold path)
    dynasm!(ops
        ; =>exit_label
        // sync pinned regs to flat array
        ; ... // emit_pinned_epilogue
        ; mov rax, QWORD EXIT_GUARD(id) as i64
        ; ret
    );
}
```

#### `trace/exit.rs` (~200 lines)

Guard exit handling in `run_jit`: when `EXIT_GUARD(id)` is returned, read
the exit PC from the `CompiledTrace`'s `guard_exits[id]`. Execute from there
using block-level JIT or interpreter. Track per-guard miss counts; retire the
trace if any guard misses >16 times.

### Integration in `jit.rs` (~100 lines changed)

In the `run_jit` loop, after a block executes N times (N = TRACE_THRESHOLD),
if it ends with a backward branch, call `recorder.on_backward_branch()`. If
recording is active, feed instructions to `recorder.record()`. On completion,
call `trace_compiler::compile_trace()` and insert into `TraceCache`.

On loop entry, check `TraceCache` before `JitCache`. If a trace hits, execute
the entire loop body with zero Rust-level dispatch per iteration.

### Step 2-D Tests

```bash
# Trace correctness: tight loop, verify N iterations produce correct register state
cargo test --package helm-jit --features backend-dynasm -- trace_basic_loop

# Guard exit: verify side exit produces same state as interpreter
cargo test --package helm-jit -- trace_guard_exit_correctness

# Trace invalidation: verify retiring a bad trace falls back cleanly
cargo test --package helm-jit -- trace_retire

# MIPS:
# Expected: 80–150 MIPS for loop-heavy workloads   Actual: ___
```

**Predicted MIPS after 2-D:** 80–150 MIPS

---

## Phase 2-E — Speculative Inline Cache for Memory (~350 lines, Week 8)

### Goal

In SE mode, 99%+ of guest memory accesses go to the same physical pages
across millions of executions (no MMU, no ASLR). Patch compiled blocks to
embed the host pointer directly after the first TLB hit:

```x86
; Before first hit (TLB inline fast path from §1-C):
; TLB lookup → host_ptr → deref

; After IC specialization (patched through RW arena view):
mov rax, [0x7F3A00001234]   ; DIRECT host address embedded in code
```

### `IcPatch` struct (~80 lines)

**File:** `framework/helm-jit/src/block.rs`

```rust
/// An inline-cache site in a compiled block.
/// After the first TLB hit for this instruction, the IC is patched to embed
/// the host pointer directly, bypassing TLB lookup entirely.
pub struct IcPatch {
    /// Offset in the block's RW allocation where the mov-imm64 target lives.
    pub imm64_offset: u32,
    /// Whether this IC has been specialized.
    pub specialized: bool,
    /// The guest address this IC is specialized for (for invalidation).
    pub guest_page: u64,
}
```

### IC patching in TLB slow path (~150 lines)

**File:** `framework/helm-jit/src/helpers.rs`

When `jit_mem_read` (the TLB slow path) fills a new TLB entry, it checks
if the calling block has an `IcPatch` for this instruction's PC. If so, it
patches the `mov rax, imm64` instruction in the block's RW view:

```rust
pub extern "C" fn jit_mem_read_ic(
    ctx: *mut u8, addr: u64, size: u32, out: *mut u64,
    block_rw: *mut u8, ic_offset: u32,
) -> u64 {
    // ... normal TLB fill + memory read ...
    // Patch the IC site:
    let host_ptr = /* resolved host address */;
    unsafe {
        let patch_addr = block_rw.add(ic_offset as usize) as *mut u64;
        *patch_addr = host_ptr as u64;
    }
    0
}
```

### IC invalidation on mmap/brk/munmap (~120 lines)

**File:** `runtime/helm-engine/src/se/linux_aarch64.rs` (and `linux_riscv64.rs`)

Hook the `brk` and `mmap` syscall handlers: on any memory layout change,
flush all ICs in the JIT cache whose `guest_page` overlaps the affected
range. Since this happens <10 times per program execution, the cost is
negligible.

### Step 2-E Tests

```bash
# IC correctness: verify direct host-pointer load == TLB-mediated load
cargo test --package helm-jit -- ic_correctness

# IC invalidation: after brk(), verify IC is flushed and refilled correctly
cargo test --package helm-engine -- ic_invalidation_on_brk

# MIPS (memory-heavy SE workload):
# Expected: +50–100% over Phase 2-D for pointer-chasing workloads   Actual: ___
```

---

## Phase 2-F — Final Benchmark Gate (Week 8)

### Full benchmark suite

```bash
# Build release
cargo build --release --workspace

# Correctness: all tests
cargo test --workspace --release

# JIT differential: 0 mismatches across all opcode combinations
cargo test --package helm-jit --features backend-dynasm -- \
  jit_vs_interp --release

# MIPS table (fill in all rows):
workload="examples/se/dhrystone_aarch64 examples/se/coremark_aarch64"
for w in $workload; do
  for mode in interp stencil dynasm tiered; do
    MIPS=$(helm-aarch64 --jit $mode $w | grep MIPS)
    echo "$w $mode: $MIPS"
  done
done
```

### Phase 2 Pass/Fail Gate

| Check | Expected | Actual | Pass? |
|-------|----------|--------|-------|
| `cargo test --workspace --release` | all pass | ___ | ___ |
| JIT vs interp correctness | 0 diff | ___ | ___ |
| MIPS (Dhrystone, tiered) | 80–150 | ___ | ___ |
| MIPS (Dhrystone, trace) | 100–200 | ___ | ___ |
| MIPS (interpreter fallback) | 5–6 | ___ | ___ |
| Chain link rate | >90% of exits linked | ___ | ___ |
| Trace guard miss rate | <5% | ___ | ___ |
| IC hit rate (SE memory) | >99% | ___ | ___ |
| Binary size delta total | <200KB | ___ | ___ |
| Build time delta total | <15s | ___ | ___ |

---

## File-Level Change Summary

| File | Phase | Action | Est. Lines |
|------|-------|--------|------------|
| `framework/helm-jit/src/regs.rs` | 1-A, 2-C | Extend | +200 |
| `framework/helm-jit/src/dynasm/pinned.rs` | 1-A | New | +150 |
| `framework/helm-jit/src/dynasm/emit/dp.rs` | 1-A, 1-B | Rewrite | ±200 |
| `framework/helm-jit/src/dynasm/emit/ldst.rs` | 1-C | Extend | +130 |
| `framework/helm-jit/src/dynasm/emit/branch.rs` | 1-B | Extend | +70 |
| `framework/helm-jit/src/dynasm/emit/nzcv.rs` | 1-B | New | +150 |
| `framework/helm-jit/src/dynasm/emit/fused.rs` | 1-D | New | +200 |
| `framework/helm-jit/src/dynasm/fusion.rs` | 1-D | New | +150 |
| `framework/helm-jit/src/dynasm/mod.rs` | 1-A, 1-D | Extend | +100 |
| `framework/helm-jit/src/helpers.rs` | 1-C, 2-E | Extend | +230 |
| `framework/helm-jit/src/arena.rs` | 2-A | New | +350 |
| `framework/helm-jit/src/block.rs` | 2-A, 2-B, 2-E | Extend | +200 |
| `framework/helm-jit/src/cache.rs` | 2-B, 2-C | Extend | +250 |
| `framework/helm-jit/src/trace/mod.rs` | 2-D | New | +150 |
| `framework/helm-jit/src/trace/recorder.rs` | 2-D | New | +200 |
| `framework/helm-jit/src/trace/compiler.rs` | 2-D | New | +450 |
| `framework/helm-jit/src/trace/exit.rs` | 2-D | New | +200 |
| `runtime/helm-engine/src/jit.rs` | 1-A, 2-B, 2-D | Extend | +250 |
| `runtime/helm-engine/src/dispatch.rs` | 1-E | New | +200 |
| `runtime/helm-engine/src/se/linux_aarch64.rs` | 2-E | Extend | +120 |
| `benches/jit_mips.rs` | 1-F, 2-F | New | +200 |
| **Tests** (various) | all | New | +600 |
| **Total** | | | **~5 100** |

---

## Risk Register

| Risk | Phase | Mitigation |
|------|-------|------------|
| dynasm callee-save clobber | 1-A | Emit push/pop in prologue/epilogue; test with `valgrind --tool=callgrind` |
| Lazy NZCV stale across block boundary | 1-B | Assert `FLAG_OP == None` at block entry in debug mode |
| TLB aliasing between SE and FS mode | 1-C | Separate `JitSeTlb` and `JitFsTlb` types; never share |
| memfd_create unavailable (old kernel) | 2-A | Fallback: mprotect-based patching (slower but correct) |
| Block chain storm on hot eviction | 2-B | Cap back-ref list at 64; log eviction rate |
| Trace explosion (too many recordings) | 2-D | Max 256 live traces; LRU eviction |
| IC stale after mmap without flush | 2-E | Assert guest_page in IC matches current FlatMem on debug builds |

---

## Glossary

- **RMSB** — Register-Mapped Superblock JIT (the architecture of this plan)
- **Pinned reg** — guest register held in a fixed x86-64 hardware register
- **Spilled reg** — guest register held in flat array `[rdi + slot*8]`
- **PatchSite** — 5-byte exit slot in a compiled block, switchable between `ret+nop` and `jmp rel32`
- **W^X** — Write XOR Execute: memory is either writable or executable, never both; our double-map workaround gives both views without mprotect
- **Guard exit** — conditional jump to a side-exit stub in a trace, triggered when the hot path prediction is wrong
- **IC** — Inline Cache: specialization of a memory access site for a specific host address
