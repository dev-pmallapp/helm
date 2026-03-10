# Crate Restructuring — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Big-bang switchover from 19 tightly-coupled crates to a trait-boundary architecture where every inter-crate API is a trait, enabling multi-ISA support and swappable timing backends.

**Status:** **SUBSTANTIALLY COMPLETE** — 24 commits, 2800+ tests passing, Linux boots, inflate E2E via GenericSession.

**Architecture:** Move `crates/` → `crates-old/` (frozen reference). Build new `crates/` bottom-up: helm-core (traits+types) → leaf crates (decode, object, memory, device, pipeline, stats) → mid crates (timing, isa, jit, syscall) → top crates (engine, plugin, python, cli). See `docs/architecture/restructuring-plan.md` for full design.

**Tech Stack:** Rust 2021 edition, bitflags 2.x, thiserror 2, serde 1, Cranelift 0.116, PyO3 0.24

**Reference:** `docs/architecture/restructuring-plan.md` — the authoritative design document. This plan maps that design to concrete tasks.

## Completion Summary

All planned tasks completed except Phase 4b (helm-isa→helm-memory dep cut — deferred, needs MMU/TLB refactor).

Key additions beyond original plan:
- **ExecMem trait** in exec.rs — enables both AddressSpace and dyn MemoryAccess
- **Aarch64TraitDecoder** — A64 instruction classifier (branch/ldst/dp/simd)
- **Aarch64TraitExecutor** — wraps existing step() via reg sync + TraitMemBridge
- **OwnedFlatMemory** — Box-friendly MemoryAccess wrapper
- **A64JitTranslator** — JitTranslator impl consuming DecodedInsn
- **Rv64CpuState/Decoder/Executor** — full RV64I base integer ISA
- **Cross-ISA test** — same generic function tests both AArch64 and RV64

---

## Task 1: Move old crates and set up workspace skeleton

**Files:**
- Rename: `crates/` → `crates-old/`
- Create: `crates/helm-core/Cargo.toml`, `crates/helm-core/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)

**Step 1: Move old crates**

```bash
mv crates crates-old
mkdir crates
```

**Step 2: Create helm-core Cargo.toml**

Create `crates/helm-core/Cargo.toml`:
```toml
[package]
name = "helm-core"
version.workspace = true
edition.workspace = true
description = "Core types, traits, and unified instruction representation for HELM"

[dependencies]
bitflags = "2"
thiserror = { workspace = true }
serde = { workspace = true }
log = { workspace = true }

[dev-dependencies]
serde_json = { workspace = true }
```

**Step 3: Create minimal lib.rs**

Create `crates/helm-core/src/lib.rs`:
```rust
//! # helm-core
//!
//! Foundation crate for HELM. Defines unified instruction types, execution
//! traits, error types, and common data structures used across all crates.

pub mod types;
pub mod error;

pub use error::{HelmError, HelmResult};
pub use types::{Addr, RegId, Word};
```

**Step 4: Copy types.rs and error.rs from old**

```bash
cp crates-old/helm-core/src/types.rs crates/helm-core/src/types.rs
cp crates-old/helm-core/src/error.rs crates/helm-core/src/error.rs
```

**Step 5: Update workspace Cargo.toml**

Comment out all members except helm-core. Update `[workspace.dependencies]` to add `bitflags = "2"`. Only keep helm-core in members for now — we'll add crates as we build them.

**Step 6: Verify it compiles**

```bash
cargo check -p helm-core
```

**Step 7: Commit**

```bash
git add crates/ crates-old/ Cargo.toml Cargo.lock
git commit -m "refactor: move crates to crates-old, bootstrap new helm-core"
```

---

## Task 2: Add DecodedInsn, InsnClass, InsnFlags to helm-core

**Files:**
- Create: `crates/helm-core/src/insn.rs`
- Create: `crates/helm-core/src/tests/mod.rs`
- Create: `crates/helm-core/src/tests/insn.rs`
- Modify: `crates/helm-core/src/lib.rs`

**Step 1: Write the failing test**

Create `crates/helm-core/src/tests/mod.rs`:
```rust
mod insn;
```

Create `crates/helm-core/src/tests/insn.rs`:
```rust
use crate::insn::*;
use crate::types::Addr;
use std::mem;

#[test]
fn decoded_insn_fits_two_cache_lines() {
    assert!(mem::size_of::<DecodedInsn>() <= 128, "DecodedInsn too large: {} bytes", mem::size_of::<DecodedInsn>());
}

#[test]
fn exec_outcome_fits_one_cache_line() {
    assert!(mem::size_of::<ExecOutcome>() <= 64, "ExecOutcome too large: {} bytes", mem::size_of::<ExecOutcome>());
}

#[test]
fn insn_flags_orthogonality() {
    // LOAD | STORE should not accidentally equal LOAD_STORE
    let combined = InsnFlags::LOAD | InsnFlags::STORE;
    assert!(combined.contains(InsnFlags::LOAD));
    assert!(combined.contains(InsnFlags::STORE));
    // LOAD_STORE is a separate bit, not the combination
    assert!(!combined.contains(InsnFlags::LOAD_STORE));
}

#[test]
fn insn_class_default_debug() {
    let class = InsnClass::IntAlu;
    assert_eq!(format!("{:?}", class), "IntAlu");
}

#[test]
fn decoded_insn_default_values() {
    let insn = DecodedInsn::default();
    assert_eq!(insn.pc, 0);
    assert_eq!(insn.len, 0);
    assert_eq!(insn.src_count, 0);
    assert_eq!(insn.dst_count, 0);
    assert_eq!(insn.imm, 0);
    assert!(insn.flags.is_empty());
}

#[test]
fn exec_outcome_no_mem_access() {
    let outcome = ExecOutcome::default();
    assert_eq!(outcome.mem_access_count, 0);
    assert!(!outcome.branch_taken);
    assert!(outcome.exception.is_none());
    assert!(!outcome.rep_ongoing);
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p helm-core
```
Expected: FAIL — `insn` module doesn't exist yet.

**Step 3: Write insn.rs**

Create `crates/helm-core/src/insn.rs` with `DecodedInsn`, `InsnClass`, `InsnFlags`, `ExecOutcome`, `MemAccessInfo`, `ExceptionInfo` — exactly as specified in `docs/architecture/restructuring-plan.md` §2.3 helm-core section.

```rust
//! Unified instruction types consumed by all backends.

use crate::types::{Addr, RegId};
use bitflags::bitflags;

/// ISA-independent decoded instruction. Single type consumed by all backends.
#[derive(Debug, Clone)]
pub struct DecodedInsn {
    pub pc: Addr,
    pub len: u8,
    pub encoding_bytes: [u8; 15],
    pub class: InsnClass,
    pub src_regs: [RegId; 6],
    pub dst_regs: [RegId; 4],
    pub src_count: u8,
    pub dst_count: u8,
    pub imm: i64,
    pub flags: InsnFlags,
    pub uop_count: u8,
    pub mem_count: u8,
}

impl Default for DecodedInsn {
    fn default() -> Self {
        Self {
            pc: 0,
            len: 0,
            encoding_bytes: [0; 15],
            class: InsnClass::Nop,
            src_regs: [0; 6],
            dst_regs: [0; 4],
            src_count: 0,
            dst_count: 0,
            imm: 0,
            flags: InsnFlags::empty(),
            uop_count: 1,
            mem_count: 0,
        }
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct InsnFlags: u32 {
        const BRANCH       = 1 << 0;
        const COND         = 1 << 1;
        const CALL         = 1 << 2;
        const RETURN       = 1 << 3;
        const LOAD         = 1 << 4;
        const STORE        = 1 << 5;
        const ATOMIC       = 1 << 6;
        const FENCE        = 1 << 7;
        const SYSCALL      = 1 << 8;
        const FLOAT        = 1 << 9;
        const SIMD         = 1 << 10;
        const SERIALIZE    = 1 << 11;
        const LOAD_STORE   = 1 << 12;
        const MULTI_MEM    = 1 << 13;
        const PAIR         = 1 << 14;
        const REP          = 1 << 15;
        const SEGMENT_OVR  = 1 << 16;
        const LOCK         = 1 << 17;
        const MICROCODE    = 1 << 18;
        const STRING_OP    = 1 << 19;
        const IO_PORT      = 1 << 20;
        const CRYPTO       = 1 << 21;
        const PRIVILEGED   = 1 << 22;
        const TRAP         = 1 << 23;
        const SYSREG       = 1 << 24;
        const COPROC       = 1 << 25;
        const HV_CALL      = 1 << 26;
        const PREFETCH     = 1 << 27;
        const CACHE_MAINT  = 1 << 28;
        const NOP          = 1 << 29;
        const SETS_FLAGS   = 1 << 30;
        const READS_FLAGS  = 1u32 << 31;
    }
}

/// Timing classification. One per instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InsnClass {
    IntAlu,
    IntMul,
    IntDiv,
    FpAlu,
    FpMul,
    FpDiv,
    FpCvt,
    SimdAlu,
    SimdMul,
    SimdFpAlu,
    SimdFpMul,
    SimdShuffle,
    Load,
    Store,
    LoadPair,
    StorePair,
    Atomic,
    Prefetch,
    Branch,
    CondBranch,
    IndBranch,
    Call,
    Return,
    Syscall,
    Fence,
    Nop,
    CacheMaint,
    SysRegAccess,
    Crypto,
    IoPort,
    Microcode,
    StringOp,
}

/// Result of functionally executing one instruction.
#[derive(Debug, Clone)]
pub struct ExecOutcome {
    pub next_pc: Addr,
    pub mem_accesses: [MemAccessInfo; 2],
    pub mem_access_count: u8,
    pub branch_taken: bool,
    pub exception: Option<ExceptionInfo>,
    pub rep_ongoing: bool,
}

impl Default for ExecOutcome {
    fn default() -> Self {
        Self {
            next_pc: 0,
            mem_accesses: [MemAccessInfo::default(); 2],
            mem_access_count: 0,
            branch_taken: false,
            exception: None,
            rep_ongoing: false,
        }
    }
}

/// A single memory access performed by an instruction.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemAccessInfo {
    pub addr: Addr,
    pub size: u8,
    pub is_write: bool,
}

/// Exception/fault information.
#[derive(Debug, Clone)]
pub struct ExceptionInfo {
    pub class: u32,
    pub iss: u32,
    pub vaddr: Addr,
    pub target_el: u8,
}
```

**Step 4: Wire up in lib.rs**

Add to `crates/helm-core/src/lib.rs`:
```rust
pub mod insn;

#[cfg(test)]
mod tests;
```

**Step 5: Run tests**

```bash
cargo test -p helm-core
```
Expected: PASS

**Step 6: Commit**

```bash
git add crates/helm-core/
git commit -m "feat(core): add DecodedInsn, InsnClass, InsnFlags unified instruction types"
```

---

## Task 3: Add CpuState and MemoryAccess traits to helm-core

**Files:**
- Create: `crates/helm-core/src/cpu.rs`
- Create: `crates/helm-core/src/mem.rs`
- Create: `crates/helm-core/src/tests/cpu.rs`
- Create: `crates/helm-core/src/tests/mem.rs`
- Modify: `crates/helm-core/src/tests/mod.rs`
- Modify: `crates/helm-core/src/lib.rs`

**Step 1: Write failing tests for CpuState**

Create `crates/helm-core/src/tests/cpu.rs`:
```rust
use crate::cpu::CpuState;
use crate::types::Addr;
use std::collections::HashMap;

struct MockCpu {
    pc: Addr,
    gprs: [u64; 32],
    vregs: [[u8; 32]; 32],
    flags_val: u64,
    sysregs: HashMap<u32, u64>,
}

impl MockCpu {
    fn new() -> Self {
        Self {
            pc: 0,
            gprs: [0; 32],
            vregs: [[0; 32]; 32],
            flags_val: 0,
            sysregs: HashMap::new(),
        }
    }
}

impl CpuState for MockCpu {
    fn pc(&self) -> Addr { self.pc }
    fn set_pc(&mut self, pc: Addr) { self.pc = pc; }
    fn gpr(&self, id: u16) -> u64 { self.gprs[id as usize] }
    fn set_gpr(&mut self, id: u16, val: u64) { self.gprs[id as usize] = val; }
    fn sysreg(&self, enc: u32) -> u64 { self.sysregs.get(&enc).copied().unwrap_or(0) }
    fn set_sysreg(&mut self, enc: u32, val: u64) { self.sysregs.insert(enc, val); }
    fn flags(&self) -> u64 { self.flags_val }
    fn set_flags(&mut self, f: u64) { self.flags_val = f; }
    fn privilege_level(&self) -> u8 { 0 }
    fn gpr_wide(&self, id: u16, dst: &mut [u8]) -> usize {
        let src = &self.vregs[id as usize];
        let n = dst.len().min(32);
        dst[..n].copy_from_slice(&src[..n]);
        n
    }
    fn set_gpr_wide(&mut self, id: u16, src: &[u8]) {
        let n = src.len().min(32);
        self.vregs[id as usize][..n].copy_from_slice(&src[..n]);
    }
}

#[test]
fn gpr_round_trip() {
    let mut cpu = MockCpu::new();
    for i in 0..32u16 {
        cpu.set_gpr(i, (i as u64) * 1000 + 42);
        assert_eq!(cpu.gpr(i), (i as u64) * 1000 + 42);
    }
}

#[test]
fn pc_round_trip() {
    let mut cpu = MockCpu::new();
    cpu.set_pc(0xDEAD_BEEF_CAFE);
    assert_eq!(cpu.pc(), 0xDEAD_BEEF_CAFE);
}

#[test]
fn sysreg_round_trip() {
    let mut cpu = MockCpu::new();
    cpu.set_sysreg(0xC200, 0x1234);
    assert_eq!(cpu.sysreg(0xC200), 0x1234);
    assert_eq!(cpu.sysreg(0x9999), 0); // unset returns 0
}

#[test]
fn flags_round_trip() {
    let mut cpu = MockCpu::new();
    cpu.set_flags(0xF000_0000);
    assert_eq!(cpu.flags(), 0xF000_0000);
}

#[test]
fn wide_reg_round_trip() {
    let mut cpu = MockCpu::new();
    let data = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    cpu.set_gpr_wide(0, &data);
    let mut out = [0u8; 16];
    let n = cpu.gpr_wide(0, &mut out);
    assert_eq!(n, 16);
    assert_eq!(out, data);
}
```

**Step 2: Write failing tests for MemoryAccess**

Create `crates/helm-core/src/tests/mem.rs`:
```rust
use crate::mem::{MemoryAccess, MemFault, MemFaultKind};
use crate::types::Addr;
use std::collections::HashMap;

struct MockMemory {
    data: HashMap<Addr, u8>,
}

impl MockMemory {
    fn new() -> Self { Self { data: HashMap::new() } }
}

impl MemoryAccess for MockMemory {
    fn read(&mut self, addr: Addr, size: usize) -> Result<u64, MemFault> {
        let mut val = 0u64;
        for i in 0..size {
            let byte = self.data.get(&(addr + i as u64)).copied().unwrap_or(0);
            val |= (byte as u64) << (i * 8);
        }
        Ok(val)
    }

    fn write(&mut self, addr: Addr, size: usize, val: u64) -> Result<(), MemFault> {
        for i in 0..size {
            self.data.insert(addr + i as u64, (val >> (i * 8)) as u8);
        }
        Ok(())
    }

    fn fetch(&mut self, addr: Addr, buf: &mut [u8]) -> Result<(), MemFault> {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.data.get(&(addr + i as u64)).copied().unwrap_or(0);
        }
        Ok(())
    }
}

#[test]
fn read_write_all_sizes() {
    let mut mem = MockMemory::new();
    for size in [1, 2, 4, 8] {
        let val: u64 = 0xDEAD_BEEF_CAFE_BABE >> (64 - size * 8);
        let addr = size as u64 * 0x100;
        mem.write(addr, size, val).unwrap();
        assert_eq!(mem.read(addr, size).unwrap(), val, "size={size}");
    }
}

#[test]
fn fetch_bytes() {
    let mut mem = MockMemory::new();
    mem.write(0x1000, 4, 0x11223344).unwrap();
    let mut buf = [0u8; 4];
    mem.fetch(0x1000, &mut buf).unwrap();
    assert_eq!(buf, [0x44, 0x33, 0x22, 0x11]);
}

#[test]
fn wide_read_write_default() {
    let mut mem = MockMemory::new();
    let data = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    mem.write_wide(0x2000, &data).unwrap();
    let mut out = [0u8; 16];
    mem.read_wide(0x2000, &mut out).unwrap();
    assert_eq!(out, data);
}

#[test]
fn copy_bulk_default() {
    let mut mem = MockMemory::new();
    for i in 0..16u8 {
        mem.write(0x1000 + i as u64, 1, i as u64).unwrap();
    }
    mem.copy_bulk(0x2000, 0x1000, 16).unwrap();
    for i in 0..16u8 {
        assert_eq!(mem.read(0x2000 + i as u64, 1).unwrap(), i as u64);
    }
}

#[test]
fn fill_bulk_default() {
    let mut mem = MockMemory::new();
    mem.fill_bulk(0x3000, 0xAB, 8).unwrap();
    for i in 0..8 {
        assert_eq!(mem.read(0x3000 + i, 1).unwrap(), 0xAB);
    }
}

#[test]
fn compare_exchange_success() {
    let mut mem = MockMemory::new();
    mem.write(0x4000, 8, 42).unwrap();
    let old = mem.compare_exchange(0x4000, 8, 42, 99).unwrap();
    assert_eq!(old, 42);
    assert_eq!(mem.read(0x4000, 8).unwrap(), 99);
}

#[test]
fn compare_exchange_failure() {
    let mut mem = MockMemory::new();
    mem.write(0x4000, 8, 42).unwrap();
    let old = mem.compare_exchange(0x4000, 8, 100, 99).unwrap();
    assert_eq!(old, 42);
    assert_eq!(mem.read(0x4000, 8).unwrap(), 42); // unchanged
}
```

**Step 3: Run tests to verify they fail**

```bash
cargo test -p helm-core
```
Expected: FAIL — modules don't exist.

**Step 4: Write cpu.rs**

Create `crates/helm-core/src/cpu.rs` — the CpuState trait exactly as in restructuring-plan.md.

**Step 5: Write mem.rs**

Create `crates/helm-core/src/mem.rs` — MemoryAccess trait + MemFault + MemFaultKind exactly as in restructuring-plan.md.

**Step 6: Wire up in lib.rs and tests/mod.rs**

Add `pub mod cpu; pub mod mem;` to lib.rs. Add `mod cpu; mod mem;` to tests/mod.rs.

**Step 7: Run tests**

```bash
cargo test -p helm-core
```
Expected: PASS

**Step 8: Commit**

```bash
git add crates/helm-core/
git commit -m "feat(core): add CpuState and MemoryAccess traits"
```

---

## Task 4: Add Decoder, Executor, TimingBackend, SyscallHandler traits to helm-core

**Files:**
- Create: `crates/helm-core/src/decode.rs`
- Create: `crates/helm-core/src/exec.rs`
- Create: `crates/helm-core/src/timing.rs`
- Create: `crates/helm-core/src/syscall.rs`
- Copy: `crates/helm-core/src/config.rs` (from old)
- Copy: `crates/helm-core/src/event.rs` (from old)
- Create: `crates/helm-core/src/tests/traits.rs`
- Modify: `crates/helm-core/src/lib.rs`
- Modify: `crates/helm-core/src/tests/mod.rs`

**Step 1: Write trait definition files**

`decode.rs`:
```rust
use crate::insn::DecodedInsn;
use crate::types::Addr;
use crate::error::HelmError;

pub trait Decoder: Send + Sync {
    fn decode(&self, pc: Addr, bytes: &[u8]) -> Result<DecodedInsn, HelmError>;
    fn min_insn_size(&self) -> usize;
}
```

`exec.rs`:
```rust
use crate::insn::{DecodedInsn, ExecOutcome};
use crate::cpu::CpuState;
use crate::mem::MemoryAccess;

pub trait Executor: Send {
    fn execute(
        &mut self,
        insn: &DecodedInsn,
        cpu: &mut dyn CpuState,
        mem: &mut dyn MemoryAccess,
    ) -> ExecOutcome;
}
```

`timing.rs`:
```rust
use crate::insn::{DecodedInsn, ExecOutcome};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccuracyLevel { FE, ITE, CAE }

pub trait TimingBackend: Send + Sync {
    fn accuracy(&self) -> AccuracyLevel;
    fn account(&mut self, insn: &DecodedInsn, outcome: &ExecOutcome) -> u64;
    fn end_of_quantum(&mut self) {}
    fn reset(&mut self) {}
}
```

`syscall.rs`:
```rust
use crate::cpu::CpuState;
use crate::mem::MemoryAccess;

pub enum SyscallAction {
    Handled(u64),
    Exit { code: u64 },
    // Additional variants added later as needed
}

pub trait SyscallHandler: Send {
    fn handle(
        &mut self,
        nr: u64,
        cpu: &mut dyn CpuState,
        mem: &mut dyn MemoryAccess,
    ) -> SyscallAction;
}
```

**Step 2: Copy config.rs and event.rs from old**

```bash
cp crates-old/helm-core/src/config.rs crates/helm-core/src/config.rs
cp crates-old/helm-core/src/event.rs crates/helm-core/src/event.rs
```

**Step 3: Copy IrqSignal to lib.rs, wire up all modules**

Update lib.rs to include all modules and re-exports.

**Step 4: Write trait tests**

Create `crates/helm-core/src/tests/traits.rs` with mock impls verifying Decoder/Executor/TimingBackend/SyscallHandler traits compile and work.

**Step 5: Run tests**

```bash
cargo test -p helm-core
```
Expected: PASS

**Step 6: Commit**

```bash
git add crates/helm-core/
git commit -m "feat(core): add Decoder, Executor, TimingBackend, SyscallHandler traits"
```

---

## Task 5: Bring back leaf crates unchanged (decode, object, pipeline, stats)

These crates need no structural changes — copy them from crates-old and verify they compile.

**Files:**
- Copy: `crates-old/helm-decode/` → `crates/helm-decode/`
- Copy: `crates-old/helm-object/` → `crates/helm-object/`
- Copy: `crates-old/helm-pipeline/` → `crates/helm-pipeline/`
- Copy: `crates-old/helm-stats/` → `crates/helm-stats/`
- Modify: `Cargo.toml` (add members)

**Step 1: Copy crates**

```bash
cp -r crates-old/helm-decode crates/helm-decode
cp -r crates-old/helm-object crates/helm-object
cp -r crates-old/helm-pipeline crates/helm-pipeline
cp -r crates-old/helm-stats crates/helm-stats
```

**Step 2: Add to workspace members**

Add `"crates/helm-decode"`, `"crates/helm-object"`, `"crates/helm-pipeline"`, `"crates/helm-stats"` to `[workspace] members` in root Cargo.toml.

**Step 3: Verify compilation and tests**

```bash
cargo test -p helm-decode -p helm-object -p helm-pipeline -p helm-stats
```
Expected: PASS

**Step 4: Commit**

```bash
git add crates/helm-decode crates/helm-object crates/helm-pipeline crates/helm-stats Cargo.toml
git commit -m "refactor: bring back helm-decode, helm-object, helm-pipeline, helm-stats unchanged"
```

---

## Task 6: Bring back helm-memory with MemoryAccess impls

**Files:**
- Copy: `crates-old/helm-memory/` → `crates/helm-memory/`
- Create: `crates/helm-memory/src/flat.rs`
- Create: `crates/helm-memory/src/tests/flat.rs`
- Modify: `crates/helm-memory/src/lib.rs`
- Modify: `Cargo.toml` (add member)

**Step 1: Copy crate**

```bash
cp -r crates-old/helm-memory crates/helm-memory
```

**Step 2: Write failing test for FlatMemoryAccess**

Create `crates/helm-memory/src/tests/flat.rs`:
```rust
use crate::flat::FlatMemoryAccess;
use crate::address_space::AddressSpace;
use helm_core::mem::MemoryAccess;

#[test]
fn flat_read_write_round_trip() {
    let mut space = AddressSpace::new();
    space.add_region(0x0, 0x10000, true, true, true);
    let mut mem = FlatMemoryAccess { space: &mut space };
    mem.write(0x100, 8, 0xDEAD_BEEF).unwrap();
    assert_eq!(mem.read(0x100, 8).unwrap(), 0xDEAD_BEEF);
}

#[test]
fn flat_fetch() {
    let mut space = AddressSpace::new();
    space.add_region(0x0, 0x10000, true, true, true);
    let mut mem = FlatMemoryAccess { space: &mut space };
    mem.write(0x200, 4, 0x11223344).unwrap();
    let mut buf = [0u8; 4];
    mem.fetch(0x200, &mut buf).unwrap();
    assert_eq!(buf, [0x44, 0x33, 0x22, 0x11]);
}
```

**Step 3: Run tests — should fail**

```bash
cargo test -p helm-memory
```
Expected: FAIL — `flat` module doesn't exist.

**Step 4: Implement FlatMemoryAccess**

Create `crates/helm-memory/src/flat.rs`:
```rust
use crate::address_space::AddressSpace;
use helm_core::mem::{MemoryAccess, MemFault, MemFaultKind};
use helm_core::types::Addr;

pub struct FlatMemoryAccess<'a> {
    pub space: &'a mut AddressSpace,
}

impl MemoryAccess for FlatMemoryAccess<'_> {
    fn read(&mut self, addr: Addr, size: usize) -> Result<u64, MemFault> {
        self.space.read(addr, size).map_err(|_| MemFault {
            addr, is_write: false, kind: MemFaultKind::Unmapped,
        })
    }

    fn write(&mut self, addr: Addr, size: usize, val: u64) -> Result<(), MemFault> {
        self.space.write(addr, size, val).map_err(|_| MemFault {
            addr, is_write: true, kind: MemFaultKind::Unmapped,
        })
    }

    fn fetch(&mut self, addr: Addr, buf: &mut [u8]) -> Result<(), MemFault> {
        self.space.read_bytes(addr, buf).map_err(|_| MemFault {
            addr, is_write: false, kind: MemFaultKind::Unmapped,
        })
    }
}
```

**Step 5: Wire up in lib.rs, add test module**

**Step 6: Run tests**

```bash
cargo test -p helm-memory
```
Expected: PASS

**Step 7: Commit**

```bash
git add crates/helm-memory Cargo.toml
git commit -m "feat(memory): add FlatMemoryAccess implementing MemoryAccess trait"
```

---

## Task 7: Bring back helm-device unchanged

**Files:**
- Copy: `crates-old/helm-device/` → `crates/helm-device/`
- Modify: `Cargo.toml` (add member)

**Step 1: Copy and add to workspace**

```bash
cp -r crates-old/helm-device crates/helm-device
```

**Step 2: Verify**

```bash
cargo test -p helm-device
```

**Step 3: Commit**

```bash
git add crates/helm-device Cargo.toml
git commit -m "refactor: bring back helm-device unchanged"
```

---

## Task 8: Create helm-timing with TimingBackend implementations

**Files:**
- Create: `crates/helm-timing/` (new, from scratch — not a copy)
- Keep old sampling, event_queue, temporal as-is where useful

**Step 1: Write failing tests**

Tests for NullBackend, IntervalBackend — see restructuring-plan.md §8.2 T5.

```rust
// NullBackend: account() always returns 0
// IntervalBackend: per-class latencies match config
// IntervalBackend: branch misprediction returns penalty cycles
```

**Step 2: Implement**

- `NullBackend` — returns 0 always, `#[inline(always)]`
- `IntervalBackend` — per-InsnClass latencies + probabilistic cache model
- `PipelineBackend` — wraps helm-pipeline (can be stubbed initially)
- Copy `AccuracyLevel` re-export from helm-core
- Copy `event_queue.rs`, `sampling.rs`, `temporal.rs` from old

**Step 3: Run tests, commit**

```bash
cargo test -p helm-timing
git commit -m "feat(timing): add NullBackend and IntervalBackend implementing TimingBackend"
```

---

## Task 9: Create helm-isa with Decoder, Executor, CpuState implementations

This is the hardest task. Split into sub-tasks.

### Task 9a: Aarch64CpuState implementing CpuState

**Files:**
- Create: `crates/helm-isa/src/arm/aarch64/cpu_state.rs`
- Copy: `crates-old/helm-isa/src/arm/regs.rs` → `crates/helm-isa/src/arm/regs.rs`

**Step 1: Write test** — CpuState round-trip for all 31 GPRs + SP + PC + NZCV + sysregs.

**Step 2: Implement** — Wrap existing `Aarch64Regs` struct with CpuState trait impl.

**Step 3: Run tests, commit.**

### Task 9b: Aarch64Decoder implementing Decoder

**Files:**
- Repurpose existing `decode.rs` to produce `DecodedInsn` instead of `MicroOp`
- Use `helm-decode` build dependency for decode tree generation

**Step 1: Write test** — Table-driven: known instruction words → expected InsnClass, flags, src_count, dst_count.

**Step 2: Implement** — New decoder producing DecodedInsn.

**Step 3: Run tests, commit.**

### Task 9c: Aarch64Executor implementing Executor

**Step 1: Write test** — Single-instruction parity: ADD, SUB, LDR, STR, B, B.cc, SVC.

**Step 2: Implement** — Thin wrapper around exec.rs functions, using CpuState+MemoryAccess traits.

**Step 3: Run tests, commit.**

### Task 9d: RISC-V / x86 stubs

Create minimal stub modules (Decoder returns DecodeError for everything, Executor panics). Proves trait design compiles for multiple ISAs.

---

## Task 10: Create helm-jit (renamed from helm-tcg)

**Files:**
- Create: `crates/helm-jit/` (new name)
- Copy: IR, compiler, cache, interpreter, threaded from old helm-tcg
- Rework A64TcgEmitter → A64JitTranslator consuming DecodedInsn

**Step 1: Copy core JIT infrastructure** — ir.rs, jit.rs, interp.rs, threaded.rs, block.rs, context.rs.

**Step 2: Add JitTranslator trait to helm-core** — `core/src/jit.rs`.

**Step 3: Write A64JitTranslator test** — parity with old emitter for simple blocks.

**Step 4: Implement A64JitTranslator** — consumes DecodedInsn, emits TcgOp.

**Step 5: Run parity tests, commit.**

---

## Task 11: Create helm-syscall with SyscallHandler trait

**Files:**
- Copy: `crates-old/helm-syscall/` → `crates/helm-syscall/`
- Modify: handler to use `dyn CpuState` + `dyn MemoryAccess` instead of concrete types
- Remove dependency on `helm-memory`

**Step 1: Copy crate.**

**Step 2: Refactor handler signature** — `handle(&mut self, nr, &mut dyn CpuState, &mut dyn MemoryAccess)`.

**Step 3: Implement SyscallHandler trait for Aarch64SyscallHandler.**

**Step 4: Update Cargo.toml** — drop helm-memory dep.

**Step 5: Run tests, commit.**

---

## Task 12: Create helm-engine with generic Session

**Files:**
- Create: `crates/helm-engine/` (new structure)
- Copy: loader/, monitor.rs, symbols.rs from old
- Create: `session.rs` with `Session<D, E, C>`

**Step 1: Copy infrastructure** — loader, monitor, symbols.

**Step 2: Write Session test** — MockDecoder + MockExecutor + MockCpu + MockMemory → run 10 instructions, verify PC advances.

**Step 3: Implement generic Session<D, E, C>** with `run_interpreted()`.

**Step 4: Write inflate parity test** — Aarch64Decoder + Aarch64Executor + Aarch64CpuState → inflate binary exits with 0.

**Step 5: Add JIT path** — `run_jit()` using dyn JitTranslator.

**Step 6: Run all tests, commit.**

---

## Task 13: Bring back helm-plugin, helm-python, helm-cli

**Step 1: Copy and adapt helm-plugin** — change callbacks to use `&dyn CpuState`.

**Step 2: Copy and adapt helm-python** — wire concrete types into generic Session.

**Step 3: Copy and adapt helm-cli** — same wiring.

**Step 4: Run full workspace tests.**

```bash
cargo test --workspace
```

**Step 5: Commit.**

---

## Task 14: Bring back side crates (kvm, llvm, systemc)

Copy unchanged, add to workspace with feature gates.

---

## Task 15: Clean up and delete crates-old

**Step 1: Verify all tests pass.**

```bash
cargo test --workspace
```

**Step 2: Remove crates-old.**

```bash
rm -rf crates-old
```

**Step 3: Remove helm-translate from workspace deps.**

**Step 4: Final commit.**

```bash
git commit -m "refactor: complete crate restructuring, remove crates-old"
```

---

## Execution Order and Dependencies

```
Task 1 (skeleton)
  └─→ Task 2 (DecodedInsn)
       └─→ Task 3 (CpuState, MemoryAccess)
            └─→ Task 4 (Decoder, Executor, TimingBackend, SyscallHandler)
                 ├─→ Task 5 (leaf crates: decode, object, pipeline, stats)
                 ├─→ Task 6 (helm-memory + FlatMemoryAccess)
                 │    └─→ Task 7 (helm-device)
                 ├─→ Task 8 (helm-timing)
                 ├─→ Task 9 (helm-isa: CpuState, Decoder, Executor)
                 │    └─→ Task 10 (helm-jit)
                 └─→ Task 11 (helm-syscall)
                      └─→ Task 12 (helm-engine)
                           └─→ Task 13 (plugin, python, cli)
                                └─→ Task 14 (kvm, llvm, systemc)
                                     └─→ Task 15 (cleanup)
```

Tasks 5, 6, 8, 9, 11 can run in parallel after Task 4.
Tasks 10 depends on 9b (Decoder).
Task 12 depends on 6, 8, 9, 10, 11.
