# helm-core — LLD: Architectural State

> Version: 0.1.0
> Status: Draft
> Cross-references: [HLD.md](HLD.md) · [LLD-interfaces.md](LLD-interfaces.md)

---

## Table of Contents

1. [ArchState Trait](#1-archstate-trait)
2. [RiscvArchState](#2-riscvarchstate)
   - [IntRegs](#21-intregs)
   - [FloatRegs](#22-floatregs)
   - [CsrFile](#23-csrfile)
   - [PC Representation](#24-pc-representation)
3. [Aarch64ArchState](#3-aarch64archstate)
   - [GprFile (X registers)](#31-gprfile-x-registers)
   - [VRegFile (SIMD/FP V registers)](#32-vregfile-simdvp-v-registers)
   - [SysRegFile](#33-sysregfile)
   - [PSTATE / NZCV](#34-pstate--nzcv)
   - [PC Representation](#35-pc-representation)
4. [Checkpoint and Restore via HelmAttr](#4-checkpoint-and-restore-via-helmattr)
5. [Memory Layout and Cache Behavior](#5-memory-layout-and-cache-behavior)
6. [Invariants and Enforcement](#6-invariants-and-enforcement)

---

## 1. ArchState Trait

The `ArchState` trait is the minimal common interface over all ISA-specific state structures. It is used by the checkpoint coordinator, the GDB stub, and the Python attribute layer. It is **not** used by the ISA execute functions — those receive the concrete type directly for zero-overhead access.

```rust
// helm-core/src/arch_state/mod.rs

use crate::attr::AttrRegistry;

/// The common interface over all ISA-specific architectural state types.
///
/// This trait is NOT used in the hot path. ISA execute functions receive
/// the concrete type (e.g., &mut RiscvArchState) directly. This trait is
/// used only on cold paths: checkpoint, GDB, Python inspection.
pub trait ArchState: Send + 'static {
    /// Register all architectural state fields as HelmAttr entries.
    ///
    /// Called once during construction. The checkpoint coordinator and
    /// GDB stub use the registered attributes for serialization and
    /// register read/write respectively.
    fn register_attrs(&self, registry: &mut AttrRegistry);

    /// The ISA this state structure represents.
    fn isa(&self) -> Isa;

    /// Reset all state to architectural power-on defaults.
    ///
    /// Called by HelmEngine::reset(). Must be idempotent.
    fn reset(&mut self);
}

/// ISA selector — identifies which ArchState implementation is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Isa {
    RiscV,
    AArch64,
}
```

The two concrete types are `RiscvArchState` and `Aarch64ArchState`, defined in their respective submodules. Neither type is parameterized — they are plain structs.

---

## 2. RiscvArchState

```rust
// helm-core/src/arch_state/riscv.rs

use super::{ArchState, Isa};
use crate::attr::AttrRegistry;

/// Complete architectural state for one RISC-V RV64GC hardware thread.
///
/// Field layout is chosen for hot-path cache locality:
/// int_regs and pc are accessed every instruction and are placed first.
/// float_regs are accessed by FP instructions; placed second.
/// csr is accessed infrequently (OS entry/exit, timer); placed last.
pub struct RiscvArchState {
    /// Integer register file: x0–x31.
    /// x0 is always 0; writes to x0 are silently discarded.
    pub(crate) int_regs: IntRegs,

    /// Program counter.
    pub(crate) pc: u64,

    /// Floating-point register file: f0–f31.
    /// Stored as u64 (NaN-boxed); bit-cast at point of use.
    pub(crate) float_regs: FloatRegs,

    /// Control and Status Register file.
    pub(crate) csr: CsrFile,
}

impl RiscvArchState {
    pub fn new() -> Self {
        Self {
            int_regs:   IntRegs::new(),
            pc:         0x8000_0000, // conventional RISC-V boot address
            float_regs: FloatRegs::new(),
            csr:        CsrFile::new(),
        }
    }

    // ── Integer register accessors (hot path) ──────────────────────────────

    /// Read integer register. x0 always returns 0.
    #[inline(always)]
    pub fn read_x(&self, idx: u8) -> u64 {
        debug_assert!(idx < 32, "int reg index out of range: {}", idx);
        self.int_regs.read(idx)
    }

    /// Write integer register. Writes to x0 are silently discarded.
    #[inline(always)]
    pub fn write_x(&mut self, idx: u8, val: u64) {
        debug_assert!(idx < 32, "int reg index out of range: {}", idx);
        self.int_regs.write(idx, val);
    }

    // ── Float register accessors (hot path) ────────────────────────────────

    /// Read float register as raw u64 bits (NaN-boxed storage).
    #[inline(always)]
    pub fn read_f_bits(&self, idx: u8) -> u64 {
        debug_assert!(idx < 32, "float reg index out of range: {}", idx);
        self.float_regs.read(idx)
    }

    /// Write float register as raw u64 bits.
    #[inline(always)]
    pub fn write_f_bits(&mut self, idx: u8, val: u64) {
        debug_assert!(idx < 32, "float reg index out of range: {}", idx);
        self.float_regs.write(idx, val);
    }

    /// Read float register as f32 (NaN-unboxed from upper 32 bits).
    ///
    /// For RISC-V: a 32-bit float stored in a 64-bit FP register has
    /// the upper 32 bits set to all-ones (NaN-boxing). This accessor
    /// validates the NaN-box and returns the f32 value.
    #[inline(always)]
    pub fn read_f32(&self, idx: u8) -> f32 {
        let bits = self.float_regs.read(idx);
        // NaN-box check: upper 32 bits must be all-ones
        debug_assert!(
            bits >> 32 == 0xFFFF_FFFF,
            "NaN-box violation in f{}: {:#018x}", idx, bits
        );
        f32::from_bits(bits as u32)
    }

    /// Write float register as f32 (NaN-boxed into upper 32 bits).
    #[inline(always)]
    pub fn write_f32(&mut self, idx: u8, val: f32) {
        // NaN-box: upper 32 bits = all-ones
        let bits = 0xFFFF_FFFF_0000_0000u64 | val.to_bits() as u64;
        self.float_regs.write(idx, bits);
    }

    /// Read float register as f64.
    #[inline(always)]
    pub fn read_f64(&self, idx: u8) -> f64 {
        f64::from_bits(self.float_regs.read(idx))
    }

    /// Write float register as f64.
    #[inline(always)]
    pub fn write_f64(&mut self, idx: u8, val: f64) {
        self.float_regs.write(idx, val.to_bits());
    }

    // ── PC accessors ───────────────────────────────────────────────────────

    #[inline(always)]
    pub fn read_pc(&self) -> u64 { self.pc }

    #[inline(always)]
    pub fn write_pc(&mut self, val: u64) { self.pc = val; }

    // ── CSR accessors ──────────────────────────────────────────────────────

    /// Read a CSR. Returns 0 for undefined CSRs (not an exception —
    /// the ISA layer raises the exception after checking access rights).
    #[inline(always)]
    pub fn read_csr_raw(&self, addr: u16) -> u64 {
        self.csr.read(addr)
    }

    /// Write a CSR. Side effects are invoked by the ISA layer after
    /// calling this method; this only updates the raw value.
    #[inline(always)]
    pub fn write_csr_raw(&mut self, addr: u16, val: u64) {
        self.csr.write(addr, val);
    }
}

impl ArchState for RiscvArchState {
    fn register_attrs(&self, registry: &mut AttrRegistry) {
        // x0–x31
        for i in 0u8..32 {
            let ptr = &self.int_regs.regs[i as usize] as *const u64;
            registry.add_u64(
                Box::leak(format!("x{}", i).into_boxed_str()),
                move || unsafe { *ptr },
                move |v| unsafe { *(ptr as *mut u64) = v },
            );
        }
        // f0–f31 (raw bits)
        for i in 0u8..32 {
            let ptr = &self.float_regs.regs[i as usize] as *const u64;
            registry.add_u64(
                Box::leak(format!("f{}", i).into_boxed_str()),
                move || unsafe { *ptr },
                move |v| unsafe { *(ptr as *mut u64) = v },
            );
        }
        // pc
        let pc_ptr = &self.pc as *const u64;
        registry.add_u64(
            "pc",
            move || unsafe { *pc_ptr },
            move |v| unsafe { *(pc_ptr as *mut u64) = v },
        );
        // All CSRs
        self.csr.register_attrs(registry);
    }

    fn isa(&self) -> Isa { Isa::RiscV }

    fn reset(&mut self) {
        self.int_regs = IntRegs::new();
        self.pc = 0x8000_0000;
        self.float_regs = FloatRegs::new();
        self.csr = CsrFile::new();
    }
}
```

### 2.1 IntRegs

```rust
/// RISC-V integer register file: x0–x31.
///
/// x0 is hardwired to zero. The backing array stores 32 u64 values.
/// x0's slot is initialized to 0 and writes are masked away by the
/// write() method, so the array index is always valid (no branch needed
/// on read — x0's slot is always 0).
pub struct IntRegs {
    regs: [u64; 32],
}

impl IntRegs {
    pub fn new() -> Self {
        Self { regs: [0u64; 32] }
    }

    /// Read register. x0 returns 0.
    #[inline(always)]
    pub fn read(&self, idx: u8) -> u64 {
        // SAFETY: idx is validated by the caller (debug_assert).
        // x0's slot is always 0; no branch needed.
        unsafe { *self.regs.get_unchecked(idx as usize) }
    }

    /// Write register. Writes to x0 are discarded.
    #[inline(always)]
    pub fn write(&mut self, idx: u8, val: u64) {
        // Branch-free: mask the write index. If idx == 0, write to slot 0,
        // then immediately overwrite with 0. Two stores vs. one branch;
        // the branch-free version is preferable on modern CPUs.
        // Alternative: single conditional store (profile to choose).
        if idx != 0 {
            unsafe { *self.regs.get_unchecked_mut(idx as usize) = val; }
        }
    }
}
```

**Layout:** `[u64; 32]` = 256 bytes. Fits in 4 cache lines (64 bytes each). A typical instruction accesses at most 3 registers, so 3 cache line touches maximum per instruction for the integer file.

**The x0 branch:** A conditional write avoids polluting x0's slot. On an out-of-order CPU, the branch is predictable (almost always `idx != 0`), so the branch cost is near-zero. An alternative branch-free approach (write then overwrite slot 0 with 0) avoids the branch but causes an unconditional store — not obviously better. Profiling should decide if this becomes a bottleneck.

### 2.2 FloatRegs

```rust
/// RISC-V floating-point register file: f0–f31.
///
/// Stored as raw u64 (NaN-boxed). All 32-bit float values occupy the
/// lower 32 bits with the upper 32 bits set to all-ones per the RISC-V
/// NaN-boxing spec. 64-bit floats use all 64 bits.
pub struct FloatRegs {
    regs: [u64; 32],
}

impl FloatRegs {
    pub fn new() -> Self {
        // Initialize to canonical NaN-boxed NaN (upper 32 all-ones,
        // lower 32 = quiet NaN pattern for f32).
        let canonical_nan = 0xFFFF_FFFF_7FC0_0000u64;
        Self { regs: [canonical_nan; 32] }
    }

    #[inline(always)]
    pub fn read(&self, idx: u8) -> u64 {
        unsafe { *self.regs.get_unchecked(idx as usize) }
    }

    #[inline(always)]
    pub fn write(&mut self, idx: u8, val: u64) {
        unsafe { *self.regs.get_unchecked_mut(idx as usize) = val; }
    }
}
```

**NaN-boxing:** RISC-V spec §11.2: "Floating-point operations that produce a result in a 32-bit format must NaN-box the result, extending the value to FLEN bits by setting bits [FLEN-1:32] to all-ones." This simulator enforces the invariant on write (via `write_f32`) and validates it on read (via `read_f32` debug assert). The raw `read_f_bits` / `write_f_bits` methods bypass the check for use by internal state initialization code.

### 2.3 CsrFile

```rust
// helm-core/src/arch_state/riscv.rs (continued)

/// RISC-V Control and Status Register file.
///
/// The CSR address space is 12 bits wide (u12, stored as u16), giving
/// 4096 possible addresses. A flat array provides O(1) read/write with
/// no hashing and excellent cache locality for the subset of CSRs
/// accessed in a typical run.
///
/// Side effects (e.g., writing satp flushes TLB) are NOT handled here.
/// This struct stores only raw values. The ISA layer (helm-arch) is
/// responsible for triggering side effects after calling write_csr_raw().
pub struct CsrFile {
    /// Raw CSR values, indexed by CSR address (0x000–0xFFF).
    values: Box<[u64; 4096]>,

    /// Handler table: what type of register is at each address?
    handlers: Box<[CsrKind; 4096]>,
}

/// The access semantics for a CSR address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsrKind {
    /// Normal read/write register. No side effects tracked here.
    ReadWrite,
    /// Read-only register. Write raises illegal instruction (caller's
    /// responsibility to check before writing).
    ReadOnly,
    /// Undefined CSR. Access raises illegal instruction.
    Undefined,
    /// WARL (Write Any Read Legal) register. Writes are masked by the
    /// ISA layer; raw storage accepts any value.
    Warl,
}

impl CsrFile {
    pub fn new() -> Self {
        let mut handlers = Box::new([CsrKind::Undefined; 4096]);

        // Initialize known CSRs from the RISC-V privileged spec.
        // This initialization covers the standard set; extensions add more.
        for addr in Self::standard_csr_list() {
            handlers[addr as usize] = addr.default_kind();
        }

        Self {
            values:   Box::new([0u64; 4096]),
            handlers: handlers,
        }
    }

    /// Read raw CSR value. Returns 0 for undefined CSRs.
    /// Caller is responsible for checking CsrKind before using the value.
    #[inline(always)]
    pub fn read(&self, addr: u16) -> u64 {
        unsafe { *self.values.get_unchecked(addr as usize) }
    }

    /// Write raw CSR value. No side effects. Caller handles side effects.
    #[inline(always)]
    pub fn write(&mut self, addr: u16, val: u64) {
        unsafe { *self.values.get_unchecked_mut(addr as usize) = val; }
    }

    /// Query the access kind for a CSR address.
    #[inline(always)]
    pub fn kind(&self, addr: u16) -> CsrKind {
        unsafe { *self.handlers.get_unchecked(addr as usize) }
    }

    /// Register all non-Undefined CSRs as HelmAttr entries.
    pub fn register_attrs(&self, registry: &mut AttrRegistry) {
        for (addr, kind) in self.handlers.iter().enumerate() {
            if *kind == CsrKind::Undefined { continue; }
            let name = csr_name(addr as u16).unwrap_or("csr_unknown");
            let ptr = &self.values[addr] as *const u64;
            registry.add_u64(
                name,
                move || unsafe { *ptr },
                move |v| unsafe { *(ptr as *mut u64) = v },
            );
        }
    }

    fn standard_csr_list() -> &'static [u16] {
        // Selected standard CSR addresses (abridged — full list in impl)
        &[
            0x001, // fflags
            0x002, // frm
            0x003, // fcsr
            0xC00, // cycle
            0xC01, // time
            0xC02, // instret
            0x100, // sstatus
            0x104, // sie
            0x105, // stvec
            0x106, // scounteren
            0x140, // sscratch
            0x141, // sepc
            0x142, // scause
            0x143, // stval
            0x144, // sip
            0x180, // satp
            0x300, // mstatus
            0x301, // misa
            0x302, // medeleg
            0x303, // mideleg
            0x304, // mie
            0x305, // mtvec
            0x306, // mcounteren
            0x340, // mscratch
            0x341, // mepc
            0x342, // mcause
            0x343, // mtval
            0x344, // mip
            0xF11, // mvendorid
            0xF12, // marchid
            0xF13, // mimpid
            0xF14, // mhartid
        ]
    }
}

/// Map a 12-bit CSR address to its canonical name.
/// Returns None for non-standard / implementation-defined CSRs.
pub fn csr_name(addr: u16) -> Option<&'static str> {
    match addr {
        0x001 => Some("fflags"),
        0x002 => Some("frm"),
        0x003 => Some("fcsr"),
        0xC00 => Some("cycle"),
        0xC01 => Some("time"),
        0xC02 => Some("instret"),
        0x100 => Some("sstatus"),
        0x104 => Some("sie"),
        0x105 => Some("stvec"),
        0x140 => Some("sscratch"),
        0x141 => Some("sepc"),
        0x142 => Some("scause"),
        0x143 => Some("stval"),
        0x144 => Some("sip"),
        0x180 => Some("satp"),
        0x300 => Some("mstatus"),
        0x301 => Some("misa"),
        0x302 => Some("medeleg"),
        0x303 => Some("mideleg"),
        0x304 => Some("mie"),
        0x305 => Some("mtvec"),
        0x340 => Some("mscratch"),
        0x341 => Some("mepc"),
        0x342 => Some("mcause"),
        0x343 => Some("mtval"),
        0x344 => Some("mip"),
        0xF11 => Some("mvendorid"),
        0xF12 => Some("marchid"),
        0xF13 => Some("mimpid"),
        0xF14 => Some("mhartid"),
        _     => None,
    }
}
```

**Memory cost:** Two `Box<[_; 4096]>` arrays. `values` = 32 KiB, `handlers` = 4 KiB (1 byte per entry). Total: 36 KiB per hart. At typical 1–8 cores, 36–288 KiB for all CSR storage, which fits comfortably in L2. The hot-path CSR access pattern is highly local: `cycle`, `mstatus`, `mepc`, `mcause` are accessed far more than most others.

**Side effects:** The RISC-V privileged spec defines numerous CSRs with side effects — `satp` triggers TLB invalidation, `mstatus.MIE` controls interrupt delivery, `mip`/`mie` affect interrupt pending state. These side effects are implemented in `helm-arch`, not here. The protocol is:

1. ISA layer checks `CsrKind` for the target CSR.
2. ISA layer calls `write_csr_raw(addr, val)` to update storage.
3. ISA layer invokes the side-effect handler registered for that CSR.

This keeps `helm-core` free of ISA-specific behavior while allowing `helm-arch` to implement the full spec.

### 2.4 PC Representation

```rust
// In RiscvArchState: pc: u64
```

RISC-V uses a 64-bit PC for RV64. The PC always points to the current instruction's address. After execution, the ISA layer writes the next PC (either `pc + 4` for normal flow, `pc + 2` for compressed instructions, or the branch/jump target).

The PC is stored as a raw `u64` field. No `PCState` wrapper struct — RISC-V does not have Thumb/ITSTATE bits, predication state, or A/T mode that would require a compound PC type. AArch64 has a similarly simple PC.

---

## 3. Aarch64ArchState

```rust
// helm-core/src/arch_state/aarch64.rs

use super::{ArchState, Isa};
use crate::attr::AttrRegistry;

/// Complete architectural state for one AArch64 Processing Element (PE).
///
/// Covers: EL0–EL3 general-purpose registers (banked), SIMD/FP V registers,
/// system registers, PSTATE, and the program counter.
pub struct Aarch64ArchState {
    /// General-purpose registers: X0–X30 + SP (per exception level).
    pub(crate) gpr: GprFile,

    /// SIMD/FP register file: V0–V31, 128 bits each.
    pub(crate) vreg: VRegFile,

    /// Program counter.
    pub(crate) pc: u64,

    /// PSTATE (Process State), including NZCV flags.
    pub(crate) pstate: Pstate,

    /// System register file: all EL-indexed system registers.
    pub(crate) sysreg: SysRegFile,
}

impl Aarch64ArchState {
    pub fn new() -> Self {
        Self {
            gpr:    GprFile::new(),
            vreg:   VRegFile::new(),
            pc:     0x4000_0000, // conventional AArch64 Linux load address
            pstate: Pstate::new(),
            sysreg: SysRegFile::new(),
        }
    }

    // Hot-path accessors — used by ISA execute functions

    #[inline(always)]
    pub fn read_x(&self, idx: u8) -> u64 { self.gpr.read_x(idx) }

    #[inline(always)]
    pub fn write_x(&mut self, idx: u8, val: u64) { self.gpr.write_x(idx, val); }

    /// Read W register (lower 32 bits of X register).
    /// Write to W register zero-extends into X register (AArch64 §C.1.2.4).
    #[inline(always)]
    pub fn read_w(&self, idx: u8) -> u32 { self.gpr.read_x(idx) as u32 }

    #[inline(always)]
    pub fn write_w(&mut self, idx: u8, val: u32) {
        // Zero-extend: upper 32 bits are always cleared on W write
        self.gpr.write_x(idx, val as u64);
    }

    #[inline(always)]
    pub fn read_pc(&self) -> u64 { self.pc }

    #[inline(always)]
    pub fn write_pc(&mut self, val: u64) { self.pc = val; }

    #[inline(always)]
    pub fn pstate(&self) -> &Pstate { &self.pstate }

    #[inline(always)]
    pub fn pstate_mut(&mut self) -> &mut Pstate { &mut self.pstate }
}

impl ArchState for Aarch64ArchState {
    fn register_attrs(&self, registry: &mut AttrRegistry) {
        // X0–X30 + XZR (X31)
        for i in 0u8..31 {
            let ptr = &self.gpr.regs[i as usize] as *const u64;
            registry.add_u64(
                Box::leak(format!("x{}", i).into_boxed_str()),
                move || unsafe { *ptr },
                move |v| unsafe { *(ptr as *mut u64) = v },
            );
        }
        // SP (per EL — simplified: expose EL0 SP)
        let sp_ptr = &self.gpr.sp_el[0] as *const u64;
        registry.add_u64("sp",
            move || unsafe { *sp_ptr },
            move |v| unsafe { *(sp_ptr as *mut u64) = v },
        );
        // pc
        let pc_ptr = &self.pc as *const u64;
        registry.add_u64("pc",
            move || unsafe { *pc_ptr },
            move |v| unsafe { *(pc_ptr as *mut u64) = v },
        );
        // NZCV as a single u64 (bits 31:28)
        let pstate_ptr = &self.pstate as *const Pstate;
        registry.add_u64("nzcv",
            move || unsafe { (*pstate_ptr).nzcv_bits() as u64 },
            move |v| unsafe { (*(pstate_ptr as *mut Pstate)).set_nzcv_bits(v as u32) },
        );
        // V0–V31 (lower 64 bits as u64 for GDB compatibility)
        for i in 0u8..32 {
            let ptr = &self.vreg.regs[i as usize] as *const u128;
            registry.add_u64(
                Box::leak(format!("v{}_lo", i).into_boxed_str()),
                move || unsafe { *ptr as u64 },
                move |v| unsafe { *(ptr as *mut u128) = (*ptr & !0xFFFF_FFFF_FFFF_FFFFu128) | v as u128 },
            );
        }
        // System registers
        self.sysreg.register_attrs(registry);
    }

    fn isa(&self) -> Isa { Isa::AArch64 }

    fn reset(&mut self) {
        self.gpr    = GprFile::new();
        self.vreg   = VRegFile::new();
        self.pc     = 0x4000_0000;
        self.pstate = Pstate::new();
        self.sysreg = SysRegFile::new();
    }
}
```

### 3.1 GprFile (X registers)

```rust
/// AArch64 General-Purpose Register file.
///
/// 31 general-purpose 64-bit registers X0–X30. X31 is context-dependent:
/// - As a source/destination: XZR (always reads 0, writes discarded)
/// - As a base address: SP (stack pointer, per exception level)
///
/// SP is banked per exception level (EL0–EL3). The current EL determines
/// which SP is active.
pub struct GprFile {
    /// X0–X30. X31 (XZR/SP) handled separately.
    regs: [u64; 31],

    /// Stack pointer per exception level: [EL0, EL1, EL2, EL3].
    sp_el: [u64; 4],
}

impl GprFile {
    pub fn new() -> Self {
        Self { regs: [0u64; 31], sp_el: [0u64; 4] }
    }

    /// Read X register. idx 31 reads XZR (zero).
    #[inline(always)]
    pub fn read_x(&self, idx: u8) -> u64 {
        debug_assert!(idx < 32);
        if idx == 31 { return 0; } // XZR
        unsafe { *self.regs.get_unchecked(idx as usize) }
    }

    /// Write X register. Writes to idx 31 are discarded (XZR).
    #[inline(always)]
    pub fn write_x(&mut self, idx: u8, val: u64) {
        debug_assert!(idx < 32);
        if idx == 31 { return; } // XZR
        unsafe { *self.regs.get_unchecked_mut(idx as usize) = val; }
    }

    /// Read the active SP for the given exception level (0–3).
    #[inline(always)]
    pub fn read_sp(&self, el: u8) -> u64 {
        debug_assert!(el < 4);
        self.sp_el[el as usize]
    }

    /// Write the active SP for the given exception level.
    #[inline(always)]
    pub fn write_sp(&mut self, el: u8, val: u64) {
        debug_assert!(el < 4);
        self.sp_el[el as usize] = val;
    }
}
```

**AArch64 SP architecture:** Per the ARMv8-A architecture reference, when the current EL is EL0 with SPsel=0, reads/writes of SP use `SP_EL0`. When EL1, EL2, or EL3, reads/writes use the corresponding `SP_ELn`. The `SPsel` system register controls SP banking. Tracking current EL is a PSTATE concern.

### 3.2 VRegFile (SIMD/FP V registers)

```rust
/// AArch64 SIMD/FP register file: V0–V31.
///
/// Each register is 128 bits. The same storage is accessed at different
/// widths depending on the instruction:
///   B = 8-bit  (byte)    — Bn
///   H = 16-bit (halfword) — Hn
///   S = 32-bit (word)    — Sn
///   D = 64-bit (doubleword) — Dn
///   Q = 128-bit (quadword) — Qn / Vn
///   SIMD vector views: 8B, 16B, 4H, 8H, 2S, 4S, 2D
///
/// Storage is u128 (little-endian element 0 in the LSBs, matching
/// AArch64's SIMD element ordering).
pub struct VRegFile {
    regs: [u128; 32],
}

impl VRegFile {
    pub fn new() -> Self { Self { regs: [0u128; 32] } }

    /// Read full 128-bit Q register.
    #[inline(always)]
    pub fn read_q(&self, idx: u8) -> u128 {
        unsafe { *self.regs.get_unchecked(idx as usize) }
    }

    /// Write full 128-bit Q register.
    #[inline(always)]
    pub fn write_q(&mut self, idx: u8, val: u128) {
        unsafe { *self.regs.get_unchecked_mut(idx as usize) = val; }
    }

    /// Read lower 64 bits (D register view).
    #[inline(always)]
    pub fn read_d(&self, idx: u8) -> u64 {
        self.read_q(idx) as u64
    }

    /// Write D register. Upper 64 bits are zeroed (AArch64 §C.7.1).
    #[inline(always)]
    pub fn write_d(&mut self, idx: u8, val: u64) {
        // AArch64: writing a D register zeroes the upper half of the Q register.
        unsafe { *self.regs.get_unchecked_mut(idx as usize) = val as u128; }
    }

    /// Read lower 32 bits (S register view).
    #[inline(always)]
    pub fn read_s(&self, idx: u8) -> u32 {
        self.read_q(idx) as u32
    }

    /// Write S register. Upper 96 bits are zeroed.
    #[inline(always)]
    pub fn write_s(&mut self, idx: u8, val: u32) {
        unsafe { *self.regs.get_unchecked_mut(idx as usize) = val as u128; }
    }

    /// Read lower 16 bits (H register view).
    #[inline(always)]
    pub fn read_h(&self, idx: u8) -> u16 {
        self.read_q(idx) as u16
    }

    /// Read lower 8 bits (B register view).
    #[inline(always)]
    pub fn read_b(&self, idx: u8) -> u8 {
        self.read_q(idx) as u8
    }
}
```

**Memory cost:** `[u128; 32]` = 512 bytes = 8 cache lines per hart. This is acceptable — SIMD instructions are less frequent than integer instructions in typical workloads, so the SIMD register file will not always be hot.

### 3.3 SysRegFile

```rust
/// AArch64 System Register file.
///
/// System registers use a 20-bit encoded key:
///   key = (op0 << 14) | (op1 << 11) | (CRn << 7) | (CRm << 3) | op2
///
/// The space is large and sparse — hundreds of defined registers scattered
/// across a 20-bit key space. A HashMap provides O(1) average access with
/// low memory overhead.
///
/// Access frequency is low: system registers are read/written only in
/// OS entry/exit, context switch, and hardware configuration paths.
/// HashMap overhead is acceptable here.
pub struct SysRegFile {
    regs: std::collections::HashMap<u32, u64>,
}

/// Encode a system register key from its components.
#[inline(always)]
pub fn sysreg_key(op0: u8, op1: u8, crn: u8, crm: u8, op2: u8) -> u32 {
    ((op0 as u32) << 14)
        | ((op1 as u32) << 11)
        | ((crn as u32) << 7)
        | ((crm as u32) << 3)
        | op2 as u32
}

impl SysRegFile {
    pub fn new() -> Self {
        let mut regs = std::collections::HashMap::with_capacity(256);
        // Pre-insert commonly used system registers at their reset values
        Self::insert_defaults(&mut regs);
        Self { regs }
    }

    /// Read system register by encoded key. Returns 0 if not present.
    /// The ISA layer checks access rights before calling this.
    #[inline]
    pub fn read(&self, key: u32) -> u64 {
        self.regs.get(&key).copied().unwrap_or(0)
    }

    /// Write system register by encoded key.
    #[inline]
    pub fn write(&mut self, key: u32, val: u64) {
        self.regs.insert(key, val);
    }

    /// Check if a system register exists (is defined) at this key.
    pub fn is_defined(&self, key: u32) -> bool {
        self.regs.contains_key(&key)
    }

    pub fn register_attrs(&self, registry: &mut AttrRegistry) {
        for (&key, val_ref) in &self.regs {
            let name = sysreg_name(key)
                .unwrap_or_else(|| Box::leak(format!("sysreg_{:#07x}", key).into_boxed_str()));
            let ptr = val_ref as *const u64;
            registry.add_u64(
                name,
                move || unsafe { *ptr },
                move |v| unsafe { *(ptr as *mut u64) = v },
            );
        }
    }

    fn insert_defaults(regs: &mut std::collections::HashMap<u32, u64>) {
        // MIDR_EL1: Main ID register
        regs.insert(sysreg_key(3, 0, 0, 0, 0), 0x410FD034);
        // MPIDR_EL1: Multiprocessor Affinity Register (hart 0)
        regs.insert(sysreg_key(3, 0, 0, 0, 5), 0x8000_0000);
        // CurrentEL: EL1
        regs.insert(sysreg_key(3, 0, 4, 0, 2), 0x4);
        // SCTLR_EL1: MMU disabled, caches disabled (reset value)
        regs.insert(sysreg_key(3, 0, 1, 0, 0), 0x0000_0000_00C5_0838);
        // Additional standard registers initialized as needed
    }
}

/// Map an encoded system register key to its canonical name.
/// Returns a static str for standard registers, None for unknown.
pub fn sysreg_name(key: u32) -> Option<&'static str> {
    match key {
        k if k == sysreg_key(3, 0, 0, 0, 0) => Some("MIDR_EL1"),
        k if k == sysreg_key(3, 0, 0, 0, 5) => Some("MPIDR_EL1"),
        k if k == sysreg_key(3, 0, 1, 0, 0) => Some("SCTLR_EL1"),
        k if k == sysreg_key(3, 0, 2, 0, 0) => Some("TTBR0_EL1"),
        k if k == sysreg_key(3, 0, 2, 0, 1) => Some("TTBR1_EL1"),
        k if k == sysreg_key(3, 0, 2, 0, 2) => Some("TCR_EL1"),
        k if k == sysreg_key(3, 0, 4, 0, 0) => Some("SPSR_EL1"),
        k if k == sysreg_key(3, 0, 4, 0, 1) => Some("ELR_EL1"),
        k if k == sysreg_key(3, 0, 4, 0, 2) => Some("CurrentEL"),
        k if k == sysreg_key(3, 0, 5, 1, 0) => Some("ESR_EL1"),
        k if k == sysreg_key(3, 0, 5, 1, 1) => Some("ESR_EL2"),
        k if k == sysreg_key(3, 0, 6, 0, 0) => Some("FAR_EL1"),
        k if k == sysreg_key(3, 0, 10, 2, 0) => Some("MAIR_EL1"),
        k if k == sysreg_key(3, 0, 12, 0, 0) => Some("VBAR_EL1"),
        _ => None,
    }
}
```

### 3.4 PSTATE / NZCV

```rust
/// AArch64 PSTATE (Process State).
///
/// PSTATE is not a single register in the ISA; it is a collection of
/// fields maintained by the processor. The fields most relevant to
/// functional simulation are NZCV, EL, SP, and daif masks.
///
/// This struct stores the logical PSTATE fields. Concrete bit packing
/// matches the `SPSR_ELn` layout so checkpointing is straightforward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pstate {
    /// Condition flags: N (bit 31), Z (bit 30), C (bit 29), V (bit 28).
    /// Stored in bits [31:28] of this u32, matching SPSR layout.
    nzcv: u32,

    /// Current exception level (0–3).
    el: u8,

    /// Stack pointer selection: 0 = use SP_EL0, 1 = use SP_ELn.
    spsel: bool,

    /// Debug mask bit (D), SError mask bit (A),
    /// IRQ mask bit (I), FIQ mask bit (F).
    daif: u8,

    /// Software Step (SS) bit — used by debug single-step.
    ss: bool,

    /// IL (Illegal Execution State) bit.
    il: bool,
}

impl Pstate {
    pub fn new() -> Self {
        Self {
            nzcv:  0,
            el:    1,     // Start at EL1 (kernel mode for SE simulation)
            spsel: false, // Use SP_EL0 at EL0
            daif:  0xF,   // All interrupts masked at reset
            ss:    false,
            il:    false,
        }
    }

    // ── NZCV flag accessors ────────────────────────────────────────────────

    /// Get NZCV as bits [31:28] (matching SPSR_ELn layout).
    #[inline(always)]
    pub fn nzcv_bits(&self) -> u32 { self.nzcv }

    /// Set NZCV from bits [31:28].
    #[inline(always)]
    pub fn set_nzcv_bits(&mut self, bits: u32) {
        self.nzcv = bits & 0xF000_0000;
    }

    #[inline(always)] pub fn n(&self) -> bool { self.nzcv & (1 << 31) != 0 }
    #[inline(always)] pub fn z(&self) -> bool { self.nzcv & (1 << 30) != 0 }
    #[inline(always)] pub fn c(&self) -> bool { self.nzcv & (1 << 29) != 0 }
    #[inline(always)] pub fn v(&self) -> bool { self.nzcv & (1 << 28) != 0 }

    /// Set all four NZCV flags at once from boolean values.
    #[inline(always)]
    pub fn set_nzcv(&mut self, n: bool, z: bool, c: bool, v: bool) {
        self.nzcv = ((n as u32) << 31)
            | ((z as u32) << 30)
            | ((c as u32) << 29)
            | ((v as u32) << 28);
    }

    // ── EL / SP selection ──────────────────────────────────────────────────

    #[inline(always)] pub fn el(&self)    -> u8   { self.el }
    #[inline(always)] pub fn spsel(&self) -> bool  { self.spsel }
    #[inline(always)] pub fn daif(&self)  -> u8    { self.daif }

    /// Produce a SPSR_ELn value from current PSTATE fields.
    pub fn to_spsr(&self) -> u64 {
        let mut spsr = 0u64;
        spsr |= (self.nzcv as u64) << 32; // bits [63:32] — adjust if needed
        spsr |= ((self.daif as u64) & 0xF) << 6;
        spsr |= (self.el as u64 & 0x3) << 2;
        spsr |= self.spsel as u64;
        spsr
    }

    /// Restore PSTATE from a saved SPSR value.
    pub fn from_spsr(&mut self, spsr: u64) {
        self.nzcv  = ((spsr >> 32) as u32) & 0xF000_0000;
        self.daif  = ((spsr >> 6) & 0xF) as u8;
        self.el    = ((spsr >> 2) & 0x3) as u8;
        self.spsel = spsr & 1 != 0;
    }
}
```

### 3.5 PC Representation

```rust
// In Aarch64ArchState: pc: u64
```

AArch64 has a 64-bit PC. Unlike AArch32, there is no Thumb bit in the PC — AArch64 always uses 32-bit aligned instruction addresses. Bit 0 of the PC is architecturally always 0.

AArch64 does not have a mode-switch indicator in the PC itself (unlike AArch32 where the Thumb bit lives in CPSR.T). All mode and privilege information is in PSTATE. This makes the AArch64 PC representation exactly as simple as RISC-V's.

---

## 4. Checkpoint and Restore via HelmAttr

Both `RiscvArchState` and `Aarch64ArchState` expose all state through the `HelmAttr` attribute system. The checkpoint coordinator in `helm-debug` iterates all registered attributes and serializes the values:

```rust
// In helm-debug: CheckpointCoordinator::save_arch_state()
pub fn save_arch_state(registry: &AttrRegistry) -> Vec<(String, AttrValue)> {
    registry.iter().map(|attr| {
        let name = attr.name.to_owned();
        let value = (attr.get)();
        (name, value)
    }).collect()
}

pub fn restore_arch_state(registry: &AttrRegistry, saved: &[(String, AttrValue)]) {
    for (name, value) in saved {
        if let Some(attr) = registry.get(name) {
            (attr.set)(value.clone());
        }
    }
}
```

This approach requires no custom serialization code per `ArchState` implementation. The `HelmAttr` attribute registration fully describes the state surface.

**Checkpoint format for `ArchState`:** A sequence of `(name: String, value: AttrValue)` pairs encoded as a length-prefixed binary blob. The coordinator prepends a 4-byte version tag before saving.

---

## 5. Memory Layout and Cache Behavior

Hot-path memory access patterns for a typical RISC-V instruction:

1. `pc` read — 8 bytes at offset 0 of `RiscvArchState`
2. `int_regs.read(rs1)` — 8 bytes in the first 256 bytes
3. `int_regs.read(rs2)` — 8 bytes in the first 256 bytes
4. `int_regs.write(rd)` — 8 bytes in the first 256 bytes
5. `pc` write — 8 bytes at offset 0

The ordering of fields in `RiscvArchState` (pc, then int_regs) puts the most-accessed fields in the first cache line. The compiler will respect field order for struct layout (no reordering without `#[repr(C)]` or `#[repr(packed)]`). On modern 64-byte cache lines, `pc` + `int_regs[0..7]` fit in one cache line.

`float_regs` and `csr` are accessed by a subset of instructions (FP and privileged instructions respectively). Placing them after `int_regs` keeps them out of the hot-path cache lines.

---

## 6. Invariants and Enforcement

| Invariant | Enforcement |
|---|---|
| RISC-V x0 always reads 0 | `IntRegs::read` returns `self.regs[0]` which is always 0 (writes to idx 0 skip the store) |
| RISC-V f-reg NaN-boxing | `write_f32` sets upper 32 bits to all-ones; `read_f32` has a `debug_assert` |
| AArch64 W-reg write zeroes upper half | `write_w` passes `val as u64` (zero-extended) to `write_x` |
| AArch64 D-reg write zeroes upper half | `write_d` stores `val as u128` (zero-extended) |
| AArch64 X31 reads as zero (XZR) | `GprFile::read_x(31)` returns 0 unconditionally |
| PC alignment (RISC-V: 4 or 2 bytes) | Checked by ISA fetch, not enforced in `RiscvArchState` itself |
| PSTATE EL in [0..3] | `Pstate::el` is a `u8`; ISA layer enforces range on write |
| CSR handler table size = 4096 | Compile-time constant in `[CsrKind; 4096]` |

Debug builds use `debug_assert!` for range checks on register indices. Release builds skip the assertions for maximum throughput. The ISA decode layer is responsible for ensuring indices are in range before calling state accessors.
