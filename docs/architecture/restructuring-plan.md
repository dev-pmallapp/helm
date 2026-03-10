# HELM Crate Restructuring Plan

**Status**: Phase 0-5 Substantially Complete
**Date**: 2026-03-10 (updated 2026-03-10)

## Implementation Progress

| Phase | Status | Key Deliverables |
|-------|--------|-----------------|
| 0 — Preparatory | **Done** | DecodedInsn, InsnClass (32 variants), InsnFlags (32 bits), CpuState/MemoryAccess/Decoder/Executor/TimingBackend/SyscallHandler/JitTranslator traits |
| 1 — Implement traits | **Done** | Aarch64CpuState, Aarch64TraitDecoder, Aarch64TraitExecutor, FlatMemoryAccess, OwnedFlatMemory, NullBackend, IntervalBackend, TraitSyscallHandler, A64JitTranslator |
| 2 — Migrate engine | **Done** | GenericSession<D,E,C>, inflate binary passes through trait pipeline |
| 3 — Migrate JIT | **Done** | A64JitTranslator consuming DecodedInsn, helm-tcg→helm-jit rename |
| 4 — Cut dead deps | **Done** | helm-isa depends only on helm-core at runtime. helm-timing dep cut (InsnClass in core). helm-memory dep cut (MMU+TLB moved into helm-isa, ExecMem trait in core). |
| 5 — Polish | **Done** | Rv64CpuState, Rv64Decoder, Rv64Executor — full RV64I base integer. Cross-ISA test proves generic design. |

**Key metrics:**
- 2816 tests passing across 15 crates
- Linux boots at 46 MIPS via JIT (existing FsSession path)
- inflate binary runs through GenericSession (trait-based path)
- RV64 programs run through GenericSession (multi-ISA proven)

**Dependency graph achieved:**
```
helm-core (0 deps) ← ExecMem, CpuState, MemoryAccess, Decoder, Executor,
                      TimingBackend, SyscallHandler, JitTranslator, DecodedInsn
  ├── helm-isa      ← depends ONLY on helm-core (runtime)
  │                    helm-memory is dev-dependency only
  │                    MMU + TLB live here (ARM arch-specific)
  ├── helm-memory   ← depends on helm-core
  │                    impl ExecMem for AddressSpace lives here
  ├── helm-jit      ← depends on helm-core, helm-isa, helm-memory
  ├── helm-timing   ← depends on helm-core
  └── helm-engine   ← composition point
```
**Goal**: Redesign the workspace so the same crate graph serves both extremes —
maximum-speed functional emulation (500+ MIPS target) and cycle-accurate
architectural exploration (gem5-class) — without duplicated ISA semantics,
duplicated IRs, or tight coupling between layers.

**Design constraint**: Rust traits define every boundary.  No crate should name
a concrete type from another crate when a trait will do.  This makes every
component swappable at compile time (generics) or runtime (trait objects).

---

## 1  Current State — 19 Crates

### 1.1  Inventory

| Crate | Purpose | Deps (helm-*) |
|-------|---------|---------------|
| `helm-core` | Types, IR (`MicroOp`), config, error, events, `IrqSignal` | — |
| `helm-decode` | QEMU `.decode` file parser → decode tree | core |
| `helm-isa` | ISA frontends (`IsaFrontend` → `MicroOp`), Aarch64Cpu, exec.rs interpreter | core, memory, timing, decode |
| `helm-tcg` | TCG IR (`TcgOp`), A64 emitter, interpreter, threaded backend, Cranelift JIT | core, isa, memory, decode |
| `helm-translate` | `Translator` (guest bytes → `TranslatedBlock` of `MicroOp`s via `IsaFrontend`) | core, isa |
| `helm-memory` | AddressSpace, Cache, MMU, TLB, MemorySubsystem | core |
| `helm-device` | Device trait, bus, transactions, IRQs, DMA, platforms, VirtIO, PCI, ARM devices | core, object |
| `helm-object` | QOM-inspired object model: `HelmObject`, property introspection, type registry | core |
| `helm-pipeline` | OoO pipeline: ROB, rename, scheduler, branch predictor, `Stage` trait | core |
| `helm-timing` | `TimingModel` trait, FE/ITE/CAE models, event queue, sampling, temporal decoupling | core |
| `helm-stats` | `StatsCollector`, `SimResults`, event observer | core |
| `helm-engine` | Orchestrator: SE session, FS session, `Simulation`, `CoreSim`, loader, monitor | core, device, isa, pipeline, tcg, memory, timing, translate, syscall, stats, plugin |
| `helm-syscall` | Linux/FreeBSD syscall emulation | core, memory |
| `helm-kvm` | KVM backend (ioctl wrapper, GIC, vCPU) | core |
| `helm-llvm` | LLVM IR accelerator simulation (gem5-SALAM style) | core, device |
| `helm-systemc` | SystemC/TLM-2.0 co-simulation bridge | core, device, timing |
| `helm-plugin` | Plugin API (`HelmComponent`), runtime registry, builtins | core, object, device, timing |
| `helm-python` | PyO3 bindings for Python scripting | core, device, engine, llvm, plugin, stats, timing |
| `helm-cli` | Binary entry points (helm_arm, helm_system_aarch64) | core, device, engine, isa, memory, plugin, timing, python |

### 1.2  Problems

1. **Two IRs, no bridge** — `TcgOp` (for JIT speed) and `MicroOp` (for pipeline
   exploration) are unrelated types.  Every instruction pattern is implemented
   twice: once in `A64TcgEmitter` and once in `IsaFrontend::decode()`.

2. **`helm-engine` is a God crate** — depends on 11 other helm crates.  It
   hardcodes Aarch64-specific types (`Aarch64Cpu`, `A64TcgEmitter`) throughout
   `se/linux.rs` and `fs/session.rs`.  Adding RISC-V or x86 would require
   forking the entire engine.

3. **`helm-isa` depends on `helm-memory` and `helm-timing`** — the ISA crate
   (decode + execute) pulls in the memory subsystem and timing, creating a
   circular conceptual dependency (ISA → memory → timing → ??? → ISA).

4. **Pipeline is disconnected** — `CoreSim.tick()` runs the pipeline but never
   receives real instruction results.  `Pipeline` and `TimingModel` are
   independent paths that don't compose.

5. **`InsnClass` ≈ `Opcode`** — nearly identical enums in `helm-timing` and
   `helm-core::ir`, coupling the timing crate to the IR definition.

6. **Concrete types cross boundaries** — `SeSession` names `Aarch64Cpu`,
   `A64TcgEmitter`, `Aarch64SyscallHandler` directly.  `FsSession` hardcodes
   TCG block cache layout.  These are implementation details, not interfaces.

7. **`helm-translate` is redundant** — `Translator` is 60 lines wrapping
   `IsaFrontend::decode()` with a cache.  This functionality belongs in the
   block cache layer.

---

## 2  Proposed Architecture

### 2.1  Guiding Principles

- **Trait boundaries everywhere** — every inter-crate API is a trait.  Crates
  depend on traits, not concrete types.  Composition happens in the top-level
  crate (engine or cli).
- **One decode, multiple consumers** — a single `Decoder` trait produces a
  single `DecodedInsn` type.  The JIT emitter, the pipeline model, and the
  functional executor all consume it.
- **Timing is a strategy, not a layer** — the `TimingBackend` trait wraps FE
  (null), ITE (interval), and CAE (pipeline) behind one interface.  The
  execution loop calls it; it doesn't call the execution loop.
- **ISA is leaf-only** — ISA crates decode and execute.  They don't depend on
  memory subsystem, timing, or engine.  Memory access goes through a trait.
- **Feature flags, not crate proliferation** — optional capabilities (KVM,
  SystemC, LLVM accel) are feature-gated, not always-compiled.

### 2.2  Target Crate Graph

```
                            helm-core
                           /    |    \
                     helm-isa  helm-memory  helm-object
                    /   |        |     \
             helm-decode |   helm-device  helm-timing
                         |        |           |
                    helm-jit  helm-pipeline  helm-stats
                         \        |         /
                          \       |        /
                           helm-engine
                          /    |     \
                   helm-syscall |   helm-plugin
                                |
                          helm-python
                          helm-cli

Side crates (feature-gated):
  helm-kvm, helm-llvm, helm-systemc
```

### 2.3  Crate-by-Crate Specification

---

#### `helm-core` — Foundation Types and Traits

**No changes to role.**  Add the unified instruction type and execution traits.

**New types:**

```rust
// --- Unified decoded instruction ---

/// ISA-independent decoded instruction.  Single type consumed by all backends.
///
/// Designed to cover RISC (AArch64, RISC-V) and CISC (x86_64) alike.
///
/// CISC considerations:
/// - `len` is 1–15 for x86_64 (variable-length encoding), 2 or 4 for ARM/RV
/// - `encoding_bytes` holds full encoding (x86 needs up to 15 bytes)
/// - `uop_count` > 1 for complex CISC instructions (e.g. PUSH = store + SP--)
/// - `mem_count` > 1 for string ops (MOVSB), PUSH/POP, ENTER/LEAVE
/// - `InsnFlags` has CISC-specific bits (REP, SEGMENT, MICROCODE, etc.)
/// - `src_regs`/`dst_regs` include implicit operands (MUL→RAX/RDX, REP→RCX)
pub struct DecodedInsn {
    pub pc: Addr,
    pub len: u8,               // instruction length in bytes (1–15)
    pub encoding_bytes: [u8; 15], // raw encoding — x86 needs up to 15 bytes
    pub class: InsnClass,      // primary timing category
    pub src_regs: [RegId; 6],  // source operands incl. implicits (0 = unused)
    pub dst_regs: [RegId; 4],  // dest operands incl. implicits (0 = unused)
    pub src_count: u8,         // number of valid entries in src_regs
    pub dst_count: u8,         // number of valid entries in dst_regs
    pub imm: i64,              // immediate operand (0 if none)
    pub flags: InsnFlags,      // behavioural / classification flags
    pub uop_count: u8,         // micro-ops this insn decomposes into (1 for RISC)
    pub mem_count: u8,         // number of distinct memory accesses (0, 1, or 2)
}

bitflags! {
    pub struct InsnFlags: u32 {
        // ── Category bits (shared across all ISAs) ──────────────
        const BRANCH       = 1 << 0;   // unconditional branch
        const COND         = 1 << 1;   // conditional (branch or CMOV/CSEL)
        const CALL         = 1 << 2;   // function call (BL, CALL)
        const RETURN       = 1 << 3;   // function return (RET)
        const LOAD         = 1 << 4;   // memory read
        const STORE        = 1 << 5;   // memory write
        const ATOMIC       = 1 << 6;   // atomic RMW (LDADD, LOCK CMPXCHG, LR/SC)
        const FENCE        = 1 << 7;   // memory barrier (DMB, MFENCE, fence)
        const SYSCALL      = 1 << 8;   // supervisor call (SVC, SYSCALL, ECALL)
        const FLOAT        = 1 << 9;   // scalar floating-point
        const SIMD         = 1 << 10;  // SIMD/vector (NEON, SSE/AVX, RVV)
        const SERIALIZE    = 1 << 11;  // serialising (ISB, CPUID, ERET)

        // ── Memory-shape bits ───────────────────────────────────
        const LOAD_STORE   = 1 << 12;  // both load AND store (LDADD, XCHG, MOVSB)
        const MULTI_MEM    = 1 << 13;  // >1 distinct mem access (LDP/STP, PUSH, ENTER)
        const PAIR         = 1 << 14;  // register-pair load/store (LDP, STP)

        // ── CISC-specific bits ──────────────────────────────────
        const REP          = 1 << 15;  // x86 REP/REPZ/REPNZ prefix
        const SEGMENT_OVR  = 1 << 16;  // x86 segment override prefix
        const LOCK         = 1 << 17;  // x86 LOCK prefix (bus lock)
        const MICROCODE    = 1 << 18;  // microcode-sequenced (x86 ENTER, PUSHA, etc.)
        const STRING_OP    = 1 << 19;  // x86 string op (MOVS, STOS, CMPS, SCAS, LODS)
        const IO_PORT      = 1 << 20;  // x86 IN/OUT (port I/O, not MMIO)
        const CRYPTO       = 1 << 21;  // crypto extension (AES-NI, SHA, SM3/SM4)

        // ── Privileged / system ─────────────────────────────────
        const PRIVILEGED   = 1 << 22;  // requires EL1+/ring0 (MSR, MOV CR, LGDT)
        const TRAP         = 1 << 23;  // exception-generating (BRK, INT3, EBREAK)
        const SYSREG       = 1 << 24;  // system register access (MRS/MSR, RDMSR/WRMSR)
        const COPROC       = 1 << 25;  // coprocessor op (x87, legacy ARM CP15)
        const HV_CALL      = 1 << 26;  // hypervisor call (HVC, VMCALL)

        // ── Pipeline hints ──────────────────────────────────────
        const PREFETCH     = 1 << 27;  // prefetch hint (PRFM, PREFETCH*)
        const CACHE_MAINT  = 1 << 28;  // cache maintenance (DC ZVA, CLFLUSH, WBINVD)
        const NOP          = 1 << 29;  // no-op (NOP, HINT, YIELD)
        const SETS_FLAGS   = 1 << 30;  // updates condition flags (ADDS, CMP, TEST)
        const READS_FLAGS  = 1u32 << 31; // reads condition flags (CSEL, ADC, CMOV, Jcc)
    }
}

/// Result of functionally executing one instruction.
///
/// For CISC instructions with multiple memory accesses (LDP, PUSH, MOVSB),
/// `mem_accesses` holds all of them.  RISC instructions have at most one.
pub struct ExecOutcome {
    pub next_pc: Addr,
    pub mem_accesses: [MemAccess; 2],  // up to 2 mem ops (RISC: 0-1, CISC: 0-2)
    pub mem_access_count: u8,          // number of valid entries
    pub branch_taken: bool,            // meaningful only if InsnFlags::BRANCH
    pub exception: Option<ExceptionInfo>,
    pub rep_ongoing: bool,             // x86 REP: true if loop not finished
}

/// A single memory access performed by an instruction.
pub struct MemAccess {
    pub addr: Addr,
    pub size: u8,       // bytes (1, 2, 4, 8, 16 for SSE, 32 for AVX)
    pub is_write: bool,
}

pub struct ExceptionInfo {
    pub class: u32,     // ESR exception class (ARM), vector number (x86), cause (RV)
    pub iss: u32,       // syndrome / error code
    pub vaddr: Addr,    // fault address (0 if N/A)
    pub target_el: u8,  // target exception level (ARM) / ring (x86) / mode (RV)
}
```

**Unified `InsnClass`** (merge `helm_timing::InsnClass` and `helm_core::ir::Opcode`):

```rust
/// Timing classification.  One per instruction.
///
/// For CISC instructions that decompose into multiple uops (e.g. x86 ADD [mem], reg
/// = load + ALU + store), the class reflects the *dominant* operation — usually
/// the longest-latency one.  The `uop_count` field in `DecodedInsn` tells the
/// pipeline model how many scheduler slots it consumes.
pub enum InsnClass {
    // ── Integer ──────────────────
    IntAlu,         // simple ALU (ADD, AND, MOV, LEA, shifts)
    IntMul,         // integer multiply (MUL, MADD, SMULL)
    IntDiv,         // integer divide (SDIV, UDIV, DIV/IDIV)

    // ── Floating-point ───────────
    FpAlu,          // FP add/sub/compare (FADD, FCMP, UCOMISS)
    FpMul,          // FP multiply (FMUL, MULSD)
    FpDiv,          // FP divide / sqrt (FDIV, FSQRT, DIVSD, SQRTSD)
    FpCvt,          // FP convert (FCVT, CVTSI2SD, CVTTSD2SI)

    // ── SIMD / vector ────────────
    SimdAlu,        // SIMD integer ALU (ADDV, PADDB, VADD)
    SimdMul,        // SIMD integer multiply (PMULL, PMULLD)
    SimdFpAlu,      // SIMD FP ALU (FADDP, ADDPS, VADDPD)
    SimdFpMul,      // SIMD FP multiply (FMULX, MULPS, VMULPD)
    SimdShuffle,    // SIMD permute/shuffle (TBL, PSHUFB, VPERM)

    // ── Memory ───────────────────
    Load,           // scalar load
    Store,          // scalar store
    LoadPair,       // register-pair load (LDP, POP reg+reg, MOVDQU)
    StorePair,      // register-pair store (STP, PUSH reg+reg, MOVDQU)
    Atomic,         // atomic RMW (LDADD, LOCK CMPXCHG, AMO*)
    Prefetch,       // prefetch hint

    // ── Control flow ─────────────
    Branch,         // unconditional branch
    CondBranch,     // conditional branch (B.cc, Jcc)
    IndBranch,      // indirect branch (BR, JMP reg, JALR)
    Call,           // function call (BL, CALL)
    Return,         // function return (RET)

    // ── System / special ─────────
    Syscall,        // supervisor call (SVC, SYSCALL, ECALL)
    Fence,          // memory barrier (DMB, MFENCE, fence)
    Nop,            // no-op / hint
    CacheMaint,     // cache maintenance (DC ZVA, CLFLUSH)
    SysRegAccess,   // system register (MRS/MSR, RDMSR/WRMSR, CSR)
    Crypto,         // crypto extension (AESE, AESENC, SHA*)
    IoPort,         // x86 port I/O (IN/OUT)
    Microcode,      // x86 microcode-sequenced (ENTER, PUSHA, CPUID, XSAVE)
    StringOp,       // x86 string op (MOVSB, STOSB — single iteration)
}
```

**New traits:**

```rust
/// Architectural state that an executor reads/writes.
/// ISA crates implement this; engine crates provide the concrete struct.
///
/// The interface is intentionally minimal — read/write GPR, system reg, and
/// flags.  ISA-specific details (x87 stack, segment descriptors, SVE vector
/// length, RISC-V CSR) live in the concrete implementation; the `Executor`
/// downcasts or uses ISA-specific extension traits when needed.
///
/// `gpr_wide`/`set_gpr_wide` handle SIMD/vector registers that exceed 64 bits
/// (SSE = 128-bit XMM, AVX = 256-bit YMM, SVE = up to 2048-bit Z).  The
/// RegId namespace is ISA-defined: ARM uses 0-30 for X-regs, 32-63 for
/// V-regs; x86 uses 0-15 for GPRs, 16-31 for XMM/YMM; RISC-V uses 0-31
/// for X, 32-63 for F.
pub trait CpuState: Send {
    fn pc(&self) -> Addr;
    fn set_pc(&mut self, pc: Addr);
    fn gpr(&self, id: RegId) -> u64;
    fn set_gpr(&mut self, id: RegId, val: u64);
    fn sysreg(&self, enc: u32) -> u64;
    fn set_sysreg(&mut self, enc: u32, val: u64);

    /// Read processor status / flags register.
    /// ARM: PSTATE (NZCV + DAIF + EL + SPSel).
    /// x86: RFLAGS (CF, ZF, SF, OF, ...).
    /// RISC-V: mstatus/sstatus.
    fn flags(&self) -> u64;
    fn set_flags(&mut self, flags: u64);

    /// Current privilege level (ARM EL, x86 CPL, RISC-V priv mode).
    fn privilege_level(&self) -> u8;

    /// Read a SIMD/vector register (> 64 bits).  Writes into `dst`.
    /// Returns number of bytes written (16 for SSE, 32 for AVX, etc.).
    /// Default returns 0 (no wide regs).
    fn gpr_wide(&self, _id: RegId, _dst: &mut [u8]) -> usize { 0 }

    /// Write a SIMD/vector register from a byte slice.
    fn set_gpr_wide(&mut self, _id: RegId, _src: &[u8]) {}
}

/// Abstract memory interface for instruction execution.
/// Decouples ISA execute from AddressSpace/TLB/cache details.
///
/// Scalar `read`/`write` handle values up to 8 bytes (sufficient for most
/// instructions).  `read_wide`/`write_wide` handle 16/32/64-byte SIMD and
/// AVX operands.  `read_bulk`/`write_bulk` handle x86 string ops and
/// block transfers where the count may be large.
pub trait MemoryAccess: Send {
    fn read(&mut self, addr: Addr, size: usize) -> Result<u64, MemFault>;
    fn write(&mut self, addr: Addr, size: usize, val: u64) -> Result<(), MemFault>;
    fn fetch(&mut self, addr: Addr, buf: &mut [u8]) -> Result<(), MemFault>;

    /// Read 16/32/64-byte value (SSE/AVX/SVE).  Default builds from scalar reads.
    fn read_wide(&mut self, addr: Addr, buf: &mut [u8]) -> Result<(), MemFault> {
        for (i, chunk) in buf.chunks_mut(8).enumerate() {
            let v = self.read(addr + (i * 8) as u64, chunk.len())?;
            chunk.copy_from_slice(&v.to_le_bytes()[..chunk.len()]);
        }
        Ok(())
    }

    /// Write 16/32/64-byte value (SSE/AVX/SVE).  Default builds from scalar writes.
    fn write_wide(&mut self, addr: Addr, data: &[u8]) -> Result<(), MemFault> {
        for (i, chunk) in data.chunks(8).enumerate() {
            let mut buf = [0u8; 8];
            buf[..chunk.len()].copy_from_slice(chunk);
            self.write(addr + (i * 8) as u64, chunk.len(), u64::from_le_bytes(buf))?;
        }
        Ok(())
    }

    /// Bulk copy (x86 REP MOVSB, block transfer).  Default loops scalar reads/writes.
    fn copy_bulk(&mut self, dst: Addr, src: Addr, len: usize) -> Result<(), MemFault> {
        for i in 0..len {
            let v = self.read(src + i as u64, 1)?;
            self.write(dst + i as u64, 1, v)?;
        }
        Ok(())
    }

    /// Bulk fill (x86 REP STOSB).  Default loops scalar writes.
    fn fill_bulk(&mut self, dst: Addr, val: u8, len: usize) -> Result<(), MemFault> {
        for i in 0..len {
            self.write(dst + i as u64, 1, val as u64)?;
        }
        Ok(())
    }

    /// Compare-and-exchange (CMPXCHG, LDXR/STXR, LR/SC).
    /// Returns `Ok(old_value)`.  Writes `new` only if `*addr == expected`.
    fn compare_exchange(
        &mut self, addr: Addr, size: usize, expected: u64, new: u64,
    ) -> Result<u64, MemFault> {
        let old = self.read(addr, size)?;
        if old == expected {
            self.write(addr, size, new)?;
        }
        Ok(old)
    }
}

pub struct MemFault {
    pub addr: Addr,
    pub is_write: bool,
    pub kind: MemFaultKind,  // Permission, Unmapped, Alignment, PageFault
}
```

**Remove:** `ir::MicroOp`, `ir::Opcode`, `ir::MicroOpFlags` — replaced by
`DecodedInsn` + `InsnClass` + `InsnFlags`.

**Keep:** `config`, `error`, `event`, `types`, `IrqSignal`.

---

#### `helm-decode` — Decode-Tree Engine

**No changes.**  Continues to parse `.decode` files and build `DecodeTree`.
Still depends only on `helm-core`.

---

#### `helm-isa` — Decode + Execute (ISA-specific, memory-independent)

**Role change:** Pure ISA logic.  Decode bytes → `DecodedInsn`.  Execute
instruction against `dyn CpuState` + `dyn MemoryAccess`.  No dependency on
`helm-memory`, `helm-timing`, or `helm-tcg`.

**New traits:**

```rust
/// Decodes raw instruction bytes into DecodedInsn.
pub trait Decoder: Send + Sync {
    fn decode(&self, pc: Addr, bytes: &[u8]) -> Result<DecodedInsn, DecodeError>;
    fn min_insn_size(&self) -> usize;  // 2 for Thumb/RVC, 4 for A64
}

/// Functionally executes a decoded instruction, mutating CPU and memory state.
///
/// For CISC instructions with REP prefix (x86 MOVSB, STOSB, etc.), a single
/// `execute()` call performs ONE iteration.  The caller checks
/// `outcome.rep_ongoing` and re-calls `execute()` with the same `insn`
/// until it returns `false`.  This keeps the interface simple and lets the
/// engine insert timing/interrupt checks between iterations.
///
/// For micro-coded instructions that decompose into multiple µops (x86 ENTER,
/// PUSHA, CPUID), `execute()` performs the full operation in one call but
/// sets `insn.uop_count` so the timing backend can charge multiple slots.
pub trait Executor: Send {
    fn execute(
        &mut self,
        insn: &DecodedInsn,
        cpu: &mut dyn CpuState,
        mem: &mut dyn MemoryAccess,
    ) -> ExecOutcome;
}
```

**Per-ISA modules provide concrete implementations:**

```
helm-isa/
  src/
    lib.rs          — re-exports Decoder, Executor traits
    aarch64/
      decoder.rs    — Aarch64Decoder: Decoder
      executor.rs   — Aarch64Executor: Executor (replaces exec.rs + step_fast)
      cpu_state.rs  — Aarch64CpuState: CpuState (replaces Aarch64Cpu)
      sysreg.rs     — system register encodings
      hcr.rs        — HCR/SCR bit definitions
    riscv64/
      decoder.rs    — Rv64Decoder: Decoder
      executor.rs   — Rv64Executor: Executor
      cpu_state.rs  — Rv64CpuState: CpuState
    x86_64/
      decoder.rs    — X86Decoder: Decoder
      executor.rs   — X86Executor: Executor
      cpu_state.rs  — X86CpuState: CpuState
```

**Dependencies:** `helm-core`, `helm-decode` only.

**Removes dependency on:** `helm-memory` (replaced by `dyn MemoryAccess`),
`helm-timing` (no longer needed at ISA level).

---

#### `helm-memory` — Address Space, MMU, TLB, Cache Hierarchy

**No changes to role.**  Continues to own `AddressSpace`, `MemorySubsystem`,
`Cache`, `Mmu`, `Tlb`.

**New:** Implement `MemoryAccess` for its types:

```rust
/// Wraps AddressSpace as a MemoryAccess for SE mode (flat, no MMU).
pub struct FlatMemoryAccess<'a> {
    pub space: &'a mut AddressSpace,
}
impl MemoryAccess for FlatMemoryAccess<'_> { ... }

/// Wraps AddressSpace + MMU + TLB as a MemoryAccess for FS mode.
pub struct MmuMemoryAccess<'a> {
    pub space: &'a mut AddressSpace,
    pub mmu: &'a mut Mmu,
    pub tlb: &'a mut Tlb,
    pub current_el: u8,
}
impl MemoryAccess for MmuMemoryAccess<'_> { ... }
```

**Dependencies:** `helm-core` only (unchanged).

---

#### `helm-device` — Device Framework

**No changes.**  The `Device` trait, bus hierarchy, transactions, IRQ routing,
DMA, platforms, VirtIO, PCI — all remain.  Already well-structured with trait
boundaries.

**Dependencies:** `helm-core`, `helm-object` (unchanged).

---

#### `helm-object` — Object Model

**No changes.**  `HelmObject` trait, property system, type registry.

**Dependencies:** `helm-core` only (unchanged).

---

#### `helm-jit` — JIT Compilation (renamed from `helm-tcg`)

**Role change:** Pure compiler.  Consumes `DecodedInsn` (not raw bytes).
Owns `TcgOp` IR, Cranelift codegen, block cache.  Does not decode instructions.

**Key interfaces:**

```rust
/// Translates a sequence of DecodedInsn into TcgOp IR.
pub trait JitTranslator: Send {
    fn translate_block(
        &mut self,
        insns: &[DecodedInsn],
        base_pc: Addr,
    ) -> JitBlock;
}

/// A compiled native-code block ready for execution.
pub struct CompiledBlock { ... }

/// Compiles TcgOp IR to native code via Cranelift.
pub struct JitCompiler { ... }

/// Direct-mapped block cache keyed by guest PC.
pub struct BlockCache { ... }
```

**Per-ISA JIT translators** move here from current `A64TcgEmitter`:

```
helm-jit/
  src/
    lib.rs
    ir.rs           — TcgOp (internal IR, not exported widely)
    compiler.rs     — JitCompiler (Cranelift codegen)
    cache.rs        — BlockCache
    interp.rs       — TcgInterp (interpreter for TcgOp, testing oracle)
    threaded.rs     — threaded bytecode backend
    aarch64/
      translator.rs — A64JitTranslator: JitTranslator
                      (consumes DecodedInsn, emits TcgOp)
    riscv64/
      translator.rs — Rv64JitTranslator: JitTranslator
    tests/
      parity.rs     — interp vs threaded
      jit_parity.rs — interp vs Cranelift
```

**Dependencies:** `helm-core`, `helm-memory` (for TLB constants/inline TLB).

**Removes dependency on:** `helm-isa` (JIT translator consumes `DecodedInsn`
from `helm-core`, doesn't call the decoder itself), `helm-decode`.

---

#### `helm-timing` — Timing Backend Trait + Models

**Role change:** `TimingModel` becomes `TimingBackend` with richer interface.
The crate defines the trait and lightweight models (FE, ITE).  The heavy
pipeline model (`PipelineBackend`) lives here too, wrapping `helm-pipeline`.

**New trait:**

```rust
/// Pluggable timing strategy.  The execution loop calls this after
/// every instruction (or after every JIT block for FE mode).
pub trait TimingBackend: Send + Sync {
    fn accuracy(&self) -> AccuracyLevel;

    /// Called after functional execution.  Returns stall cycles.
    fn account(
        &mut self,
        insn: &DecodedInsn,
        outcome: &ExecOutcome,
    ) -> u64;

    /// End-of-quantum hook for temporal decoupling.
    fn end_of_quantum(&mut self) {}

    /// Reset internal state.
    fn reset(&mut self) {}
}
```

**Built-in implementations:**

```rust
/// FE — returns 0 always.  #[inline(always)] so it compiles away.
pub struct NullBackend;

/// ITE — per-class latencies + probabilistic cache model.
pub struct IntervalBackend { ... }

/// CAE — wraps helm-pipeline::Pipeline for full OoO modelling.
pub struct PipelineBackend {
    pipeline: Pipeline,
    // cache hierarchy queries go through here
}
```

**SamplingBackend** — wraps two backends for SimPoint/SMARTS:

```rust
pub struct SamplingBackend {
    fast: NullBackend,
    detailed: Box<dyn TimingBackend>,
    controller: SamplingController,
}
```

**Remove:** `InsnClass` from this crate — it now lives in `helm-core`.
**Remove:** `TimingModel` trait — replaced by `TimingBackend`.

**Dependencies:** `helm-core`, `helm-pipeline` (for `PipelineBackend`).

---

#### `helm-pipeline` — OoO Pipeline Model

**No changes to role.**  ROB, rename, scheduler, branch predictor, `Stage`
trait.  Now driven by `PipelineBackend` in `helm-timing` instead of standalone
`CoreSim.tick()`.

**Dependencies:** `helm-core` only (unchanged).

---

#### `helm-stats` — Statistics

**No changes.**  `StatsCollector`, `SimResults`, event observer.

**Dependencies:** `helm-core` only (unchanged).

---

#### `helm-engine` — Orchestrator

**Role change:** Composes traits, owns no ISA-specific logic.  Sessions are
generic over `Decoder`, `Executor`, `CpuState`, `TimingBackend`.

**Key restructuring:**

```rust
/// Generic session that works with any ISA.
pub struct Session<D: Decoder, E: Executor, C: CpuState> {
    decoder: D,
    executor: E,
    cpu: C,
    mem: Box<dyn MemoryAccess>,
    timing: Box<dyn TimingBackend>,
    stats: StatsCollector,
    // optional:
    jit: Option<JitEngine>,
    plugins: PluginRegistry,
}

impl<D: Decoder, E: Executor, C: CpuState> Session<D, E, C> {
    /// Interpreted execution loop — works at every accuracy level.
    pub fn run_interpreted(&mut self, budget: u64) -> StopReason {
        for _ in 0..budget {
            let bytes = self.mem.fetch(self.cpu.pc(), ...)?;
            let insn = self.decoder.decode(self.cpu.pc(), &bytes)?;
            let outcome = self.executor.execute(&insn, &mut self.cpu, &mut self.mem);
            let stall = self.timing.account(&insn, &outcome);
            self.stats.record(&insn, &outcome);
            self.cpu.set_pc(outcome.next_pc);
        }
    }

    /// JIT execution loop — FE mode only, maximum speed.
    pub fn run_jit(&mut self, budget: u64) -> StopReason {
        // block-level: decode → translate → compile → execute
        // timing.account() called per-block or not at all
    }
}
```

**SE/FS distinction** becomes a configuration choice on the same `Session`,
not a separate type:

```rust
pub enum SessionMode {
    /// Syscall emulation — FlatMemoryAccess, syscall handler
    SE { syscall: Box<dyn SyscallHandler> },
    /// Full system — MmuMemoryAccess, device bus, interrupt controller
    FS { bus: DeviceBus, irq: IrqRouter },
}
```

**`CoreSim` removed** — its job is now done by `PipelineBackend` inside
`TimingBackend::account()`.

**Dependencies:** `helm-core`, `helm-memory`, `helm-stats`, `helm-plugin`.

Engine does NOT depend on `helm-isa`, `helm-jit`, `helm-timing`, `helm-device`
*structurally* — it depends on traits from `helm-core`.  Concrete types are
injected by the binary crate (`helm-cli`) or Python bindings.

Wait — this requires careful design.  In practice, `helm-engine` will need to
reference `Decoder`, `Executor`, `CpuState` which are defined in `helm-isa`,
and `TimingBackend` from `helm-timing`.  But since both traits live in or are
re-exported through `helm-core`, the engine only depends on `helm-core`.

**The key insight:** traits in `helm-core`, implementations in leaf crates,
composition in `helm-engine`/`helm-cli`.

---

#### `helm-syscall` — Syscall Emulation

**Minor change:** Depend on `dyn CpuState` + `dyn MemoryAccess` traits
instead of concrete `Aarch64Cpu` / `AddressSpace`.

```rust
pub trait SyscallHandler: Send {
    fn handle(
        &mut self,
        nr: u64,
        cpu: &mut dyn CpuState,
        mem: &mut dyn MemoryAccess,
    ) -> SyscallAction;
}
```

**Dependencies:** `helm-core` only (drops `helm-memory`).

---

#### `helm-kvm` — KVM Backend

**No changes.**  Self-contained ioctl wrapper.

**Dependencies:** `helm-core` only (unchanged).

---

#### `helm-llvm` — LLVM IR Accelerator

**No changes.**  Self-contained accelerator simulation.

**Dependencies:** `helm-core`, `helm-device` (unchanged).

---

#### `helm-systemc` — SystemC Bridge

**No changes.**

**Dependencies:** `helm-core`, `helm-device`, `helm-timing`.

---

#### `helm-plugin` — Plugin System

**Minor change:** Plugin callbacks receive `&dyn CpuState` instead of
concrete CPU types.

**Dependencies:** `helm-core`, `helm-object` (drop `helm-device`, `helm-timing`
if possible — plugins should depend on traits only).

---

#### `helm-python` — PyO3 Bindings

**Role:** Thin wrapper.  Constructs concrete types and injects them into
generic `Session`.

**Dependencies:** `helm-core`, `helm-engine`, `helm-isa`, `helm-jit`,
`helm-memory`, `helm-timing`, `helm-device`, `helm-plugin`, `helm-stats`.

This is the composition point — it's OK for it to depend on many crates
because it wires concrete implementations together.

---

#### `helm-cli` — Binary Entry Points

**Role:** Same as `helm-python` — a composition point.

**Dependencies:** Same set as `helm-python`.

---

#### `helm-translate` — **DELETE**

Subsumed by `helm-jit::BlockCache` + `Decoder` trait.  The 60-line
`Translator` struct adds no value over a decoder + cache.

---

## 3  Dependency Graph Comparison

### 3.1  Current (problematic edges highlighted)

```
helm-engine ──→ helm-isa           (concrete Aarch64Cpu)
helm-engine ──→ helm-tcg           (concrete A64TcgEmitter, TcgInterp)
helm-engine ──→ helm-pipeline      (concrete Pipeline via CoreSim)
helm-engine ──→ helm-translate     (redundant)
helm-engine ──→ helm-syscall       (concrete Aarch64SyscallHandler)
helm-isa ──→ helm-memory           (concrete AddressSpace in step_fast)
helm-isa ──→ helm-timing           (InsnClass, but shouldn't need it)
helm-plugin ──→ helm-device        (too much coupling for plugin API)
helm-plugin ──→ helm-timing        (unnecessary)
```

### 3.2  Target (all inter-crate edges are via traits)

```
helm-core          (traits: CpuState, MemoryAccess, Decoder, Executor,
                    TimingBackend, SyscallHandler — all in one crate)
  ↑
  ├── helm-isa     (implements: Decoder, Executor, CpuState)
  ├── helm-memory  (implements: MemoryAccess)
  ├── helm-jit     (implements: JitTranslator; consumes DecodedInsn)
  ├── helm-timing  (implements: TimingBackend; wraps helm-pipeline)
  ├── helm-pipeline(standalone, no trait deps)
  ├── helm-syscall (implements: SyscallHandler)
  ├── helm-device  (standalone device framework)
  ├── helm-stats   (standalone observer)
  ├── helm-object  (standalone object model)
  ├── helm-kvm     (standalone KVM wrapper)
  ├── helm-llvm    (standalone accelerator)
  ├── helm-systemc (standalone bridge)
  ├── helm-plugin  (depends on helm-core traits only)
  │
  └── helm-engine  (depends on helm-core traits; composes everything)
        ↑
        ├── helm-python (composition point: wires concrete types)
        └── helm-cli    (composition point: wires concrete types)
```

---

## 4  Migration Phases

### Phase 0 — Preparatory (no breaking changes)

**Goal:** Add new types alongside old ones.  Nothing removed yet.

| Step | What | Where | Risk |
|------|------|-------|------|
| 0.1 | Add `DecodedInsn`, `InsnFlags`, `ExecOutcome`, `ExceptionInfo` to `helm-core` | `core/src/insn.rs` | None — additive |
| 0.2 | Merge `InsnClass` and `Opcode` into one enum in `helm-core` | `core/src/insn.rs` | Low — rename + re-export |
| 0.3 | Add `CpuState` trait to `helm-core` | `core/src/cpu.rs` | None — additive |
| 0.4 | Add `MemoryAccess` trait to `helm-core` | `core/src/mem.rs` | None — additive |
| 0.5 | Add `Decoder` trait to `helm-core` | `core/src/decode.rs` | None — additive |
| 0.6 | Add `Executor` trait to `helm-core` | `core/src/exec.rs` | None — additive |
| 0.7 | Add `TimingBackend` trait to `helm-core` | `core/src/timing.rs` | None — additive |
| 0.8 | Add `SyscallHandler` trait to `helm-core` | `core/src/syscall.rs` | None — additive |

**Tests:** All existing tests pass unchanged.  New traits have standalone
unit tests.

**Commit convention:** `feat(core): add DecodedInsn unified instruction type`

---

### Phase 1 — Implement Traits for Existing Types

**Goal:** Make existing concrete types implement the new traits, without
changing their callers yet.

| Step | What | Where | Risk |
|------|------|-------|------|
| 1.1 | `impl CpuState for Aarch64Cpu` | `helm-isa/src/aarch64/cpu_state.rs` | Low |
| 1.2 | `impl MemoryAccess for FlatMemoryAccess` (wraps AddressSpace) | `helm-memory/src/flat.rs` | Low |
| 1.3 | `impl MemoryAccess for MmuMemoryAccess` (wraps AddressSpace+MMU+TLB) | `helm-memory/src/mmu_access.rs` | Medium — must handle faults |
| 1.4 | `Aarch64Decoder: impl Decoder` producing `DecodedInsn` | `helm-isa/src/aarch64/decoder.rs` | Medium — new decoder alongside old one |
| 1.5 | `Aarch64Executor: impl Executor` consuming `DecodedInsn` | `helm-isa/src/aarch64/executor.rs` | Hard — factor out from `exec.rs` |
| 1.6 | `impl TimingBackend for NullBackend` | `helm-timing/src/null.rs` | Trivial |
| 1.7 | `impl TimingBackend for IntervalBackend` (wraps IteModelDetailed) | `helm-timing/src/interval.rs` | Low |
| 1.8 | `impl TimingBackend for PipelineBackend` (wraps Pipeline) | `helm-timing/src/pipeline.rs` | Medium — connects pipeline to real insns |
| 1.9 | `impl SyscallHandler for Aarch64SyscallHandler` | `helm-syscall/src/os/linux/handler.rs` | Low |

**The hardest step is 1.5** — extracting `Aarch64Executor` from the 2700-line
`exec.rs`.  Strategy:

1. Keep `exec.rs` as-is (used by `step_fast` legacy path).
2. Write `executor.rs` as a thin wrapper that calls the same per-instruction
   functions but through the `Executor` trait interface.
3. Both paths share the actual instruction implementations (as free functions
   or methods on a shared struct).
4. Once all callers migrate to `Executor`, remove the old `step_fast` entry.

**Tests:** Parity tests between old path and new trait-based path.

---

### Phase 2 — Migrate Engine to Traits

**Goal:** `helm-engine` uses `dyn Decoder`, `dyn Executor`, `dyn CpuState`,
`dyn TimingBackend` instead of concrete types.

| Step | What | Where | Risk |
|------|------|-------|------|
| 2.1 | Create generic `Session<D, E, C>` alongside existing `SeSession`/`FsSession` | `engine/src/session.rs` | Medium |
| 2.2 | Implement `run_interpreted()` on generic `Session` | Same | Medium |
| 2.3 | Implement `run_jit()` on generic `Session` (uses `dyn JitTranslator`) | Same | Medium |
| 2.4 | Migrate SE-mode callers (CLI, Python) to generic `Session` | `cli/`, `python/` | Low |
| 2.5 | Migrate FS-mode callers to generic `Session` | `cli/`, `python/` | Medium |
| 2.6 | Remove old `SeSession`, `FsSession`, `CoreSim` | `engine/src/` | High — final cutover |
| 2.7 | Remove `Simulation` struct (replaced by `Session` + config) | `engine/src/sim.rs` | Medium |

**Strategy:** Steps 2.1–2.3 run in parallel with old sessions.  Both paths
coexist.  Steps 2.4–2.5 are the switchover.  Step 2.6 is the cleanup.

**Tests:** The inflate parity test and all engine tests must pass against
the new `Session` before old code is removed.

---

### Phase 3 — Migrate JIT to Consume DecodedInsn

**Goal:** Rename `helm-tcg` → `helm-jit`.  JIT translator consumes
`DecodedInsn` instead of raw bytes.

| Step | What | Where | Risk |
|------|------|-------|------|
| 3.1 | Create `JitTranslator` trait in `helm-core` | `core/src/jit.rs` | None — additive |
| 3.2 | Implement `A64JitTranslator: JitTranslator` | `jit/src/aarch64/translator.rs` | Hard — rewrite A64TcgEmitter to consume DecodedInsn |
| 3.3 | Move block cache from `FsSession` to `helm-jit` | `jit/src/cache.rs` | Medium |
| 3.4 | Remove old `A64TcgEmitter` that decodes raw bytes | `jit/src/` | Medium |
| 3.5 | Rename crate `helm-tcg` → `helm-jit` | `Cargo.toml` everywhere | Low but tedious |

**Tests:** JIT parity tests rewritten to use `Decoder` → `JitTranslator`
pipeline.

---

### Phase 4 — Cut Dead Dependencies

**Goal:** Remove unnecessary crate edges.

| Step | What | Removes |
|------|------|---------|
| 4.1 | `helm-isa` no longer depends on `helm-memory` | Direct `AddressSpace` usage |
| 4.2 | `helm-isa` no longer depends on `helm-timing` | `InsnClass` import (now in core) |
| 4.3 | `helm-engine` no longer depends on `helm-isa` | Uses `dyn Decoder`/`Executor` |
| 4.4 | `helm-engine` no longer depends on `helm-jit` | Uses `dyn JitTranslator` |
| 4.5 | `helm-engine` no longer depends on `helm-pipeline` | Pipeline accessed via `TimingBackend` |
| 4.6 | `helm-plugin` drops `helm-device`, `helm-timing` | Plugin API uses traits only |
| 4.7 | `helm-syscall` drops `helm-memory` | Uses `dyn MemoryAccess` |
| 4.8 | Delete `helm-translate` | Redundant |

---

### Phase 5 — Polish

| Step | What |
|------|------|
| 5.1 | Add `Rv64Decoder` + `Rv64Executor` in `helm-isa/src/riscv64/` — proves the trait design works for a second ISA |
| 5.2 | Add `SamplingBackend` in `helm-timing` — proves timing composability |
| 5.3 | Feature-gate `helm-kvm`, `helm-llvm`, `helm-systemc` in workspace |
| 5.4 | Update Python bindings to use generic `Session` |
| 5.5 | Update all documentation |

---

## 5  Trait Summary Table

| Trait | Defined in | Implemented by | Consumed by |
|-------|-----------|---------------|-------------|
| `CpuState` | helm-core | helm-isa (Aarch64CpuState, Rv64CpuState) | helm-engine, helm-syscall, helm-plugin |
| `MemoryAccess` | helm-core | helm-memory (FlatMemoryAccess, MmuMemoryAccess) | helm-isa (Executor), helm-engine, helm-syscall |
| `Decoder` | helm-core | helm-isa (Aarch64Decoder, Rv64Decoder) | helm-engine, helm-jit |
| `Executor` | helm-core | helm-isa (Aarch64Executor, Rv64Executor) | helm-engine |
| `TimingBackend` | helm-core | helm-timing (NullBackend, IntervalBackend, PipelineBackend) | helm-engine |
| `JitTranslator` | helm-core | helm-jit (A64JitTranslator) | helm-engine |
| `SyscallHandler` | helm-core | helm-syscall (Aarch64SyscallHandler) | helm-engine |
| `HelmObject` | helm-object | helm-device, user plugins | helm-plugin, helm-engine |
| `Device` | helm-device | arm devices, VirtIO, user devices | helm-engine (FS mode) |
| `Stage` | helm-pipeline | pipeline stages | helm-timing (PipelineBackend) |

---

## 6  Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|-----------|
| `Executor` trait dispatch overhead (vtable) | ~2-5% IPC loss in interpreted mode | Use generics (`Session<D, E, C>`) not `dyn` for hot path; monomorphise |
| `MemoryAccess` trait call per load/store | Significant in interpreted mode | Inline via generics; JIT path still uses direct TLB probe |
| 2700-line exec.rs refactor breaks instructions | Silent correctness bugs | Inflate parity test + per-instruction JIT parity coverage |
| `DecodedInsn` is too fat (48+ bytes) | Cache pressure in decode loop | Keep it on the stack; decode-then-execute, don't store arrays of them |
| A64JitTranslator rewrite from DecodedInsn | Major effort, JIT regressions | Keep old A64TcgEmitter working during transition; parity tests gate cutover |
| Renaming `helm-tcg` → `helm-jit` | Churn in imports everywhere | Do as one atomic commit at the end |

---

## 7  What Stays the Same

These crates are well-designed and need no structural changes:

- **helm-decode** — clean, single-purpose
- **helm-device** — well-structured trait-based framework
- **helm-object** — clean QOM analogue
- **helm-memory** — clean (just adds `MemoryAccess` impls)
- **helm-pipeline** — clean (just gets driven by `PipelineBackend`)
- **helm-stats** — clean
- **helm-kvm** — self-contained
- **helm-llvm** — self-contained
- **helm-systemc** — self-contained

---

## 8  Testing Plan

Every phase has a testing gate that must pass before the next phase begins.
Tests are additive — nothing is deleted until the code it covers is deleted.

### 8.1  Testing Principles

1. **Interpreter is the oracle** — for any instruction, the interpreter result
   is ground truth.  JIT, threaded, and new trait-based executor must match it.
2. **Parity tests gate every transition** — old path vs new path, register-by-
   register, before the old path is removed.
3. **No silent failures** — every test asserts specific values, not just
   "didn't crash".  Flag registers, PC, SP, and memory are all checked.
4. **Edge cases from bug history** — every bug from MEMORY.md (BFM 32-bit
   rotation, C flag sign-flip, decode_bitmask, CNTVCT stale, etc.) becomes
   a permanent regression test.
5. **Multi-ISA tests use the same harness** — the test framework is
   ISA-parametric; adding RISC-V tests means adding input data, not test code.

### 8.2  Test Categories

#### T1 — Trait Contract Tests (Phase 0)

Unit tests for each new trait in `helm-core`.  These test the *contract*,
not any specific implementation.

| Test | What it verifies | Location |
|------|-----------------|----------|
| `CpuState` round-trip | `set_gpr(5, x); assert gpr(5) == x` for all reg ids | `core/src/tests/cpu.rs` |
| `CpuState` wide regs | `set_gpr_wide` / `gpr_wide` round-trip for 16/32-byte SIMD | Same |
| `CpuState` privilege level | Consistent with `flags()` | Same |
| `MemoryAccess` basic | read-after-write for all sizes (1,2,4,8) | `core/src/tests/mem.rs` |
| `MemoryAccess` wide | `write_wide` / `read_wide` round-trip for 16, 32 bytes | Same |
| `MemoryAccess` bulk | `fill_bulk` + verify, `copy_bulk` + verify | Same |
| `MemoryAccess` compare_exchange | success case (old==expected) and failure case | Same |
| `MemoryAccess` faults | unmapped addr → MemFault, permission violation → MemFault | Same |
| `DecodedInsn` sizing | `size_of::<DecodedInsn>()` assertion (must fit 2 cache lines) | `core/src/tests/insn.rs` |
| `InsnFlags` orthogonality | `LOAD \| STORE` = `LOAD_STORE`, exclusive bits don't overlap | Same |
| `ExecOutcome` defaults | `mem_access_count == 0` when no memory op | Same |

**Mock implementations** for contract tests:

```rust
/// Minimal CpuState with 32 GPRs and 32 vector regs, for testing.
struct MockCpu {
    pc: Addr,
    gprs: [u64; 32],
    vregs: [[u8; 32]; 32],  // 256-bit wide regs
    flags: u64,
    sysregs: HashMap<u32, u64>,
}
impl CpuState for MockCpu { ... }

/// Minimal MemoryAccess backed by a HashMap, for testing.
struct MockMemory {
    data: HashMap<Addr, u8>,
    size: usize,
}
impl MemoryAccess for MockMemory { ... }
```

#### T2 — Decoder Parity Tests (Phase 1)

Every `Decoder` implementation is tested against a reference.

| Test | What it verifies | Location |
|------|-----------------|----------|
| Aarch64 decode exhaustive | For all 651 decode-tree patterns: old `IsaFrontend::decode()` and new `Aarch64Decoder::decode()` produce equivalent `InsnClass`, `src_regs`, `dst_regs`, `flags` | `isa/src/tests/decoder_parity.rs` |
| Aarch64 decode fuzz | Random 32-bit words: new decoder must not panic, must return `DecodeError` or valid `DecodedInsn` | Same |
| Aarch64 decode round-trip | `encoding_bytes[0..4]` matches input bytes | Same |
| x86 decode length | Known instructions: verify `len` matches Intel manual | `isa/src/tests/x86_decoder.rs` |
| x86 decode prefixes | REP, LOCK, segment override correctly set `InsnFlags` | Same |
| x86 implicit operands | MUL → `src_regs` includes RAX, `dst_regs` includes RAX+RDX | Same |
| x86 multi-mem | PUSH → `STORE \| LOAD_STORE` cleared, MOVSB → `LOAD_STORE \| STRING_OP` | Same |
| RV64 decode | C-extension (16-bit) and standard (32-bit) lengths correct | `isa/src/tests/rv64_decoder.rs` |

**Table-driven approach:**

```rust
/// Each entry: (raw_bytes, expected_class, expected_flags, expected_src_count, ...)
const AARCH64_DECODE_VECTORS: &[(u32, InsnClass, InsnFlags, u8, u8)] = &[
    (0x8B020020, InsnClass::IntAlu,  InsnFlags::empty(),           2, 1), // ADD X0,X1,X2
    (0xF9400020, InsnClass::Load,    InsnFlags::LOAD,              1, 1), // LDR X0,[X1]
    (0xA9BF7BFD, InsnClass::StorePair, InsnFlags::STORE|MULTI_MEM, 3, 0), // STP X29,X30,[SP,#-16]!
    (0xD4000001, InsnClass::Syscall, InsnFlags::SYSCALL|TRAP,      0, 0), // SVC #0
    // ... hundreds more, one per decode-tree pattern
];

#[test]
fn decoder_matches_vectors() {
    let dec = Aarch64Decoder::new();
    for (enc, exp_class, exp_flags, exp_src, exp_dst) in AARCH64_DECODE_VECTORS {
        let insn = dec.decode(0x1000, &enc.to_le_bytes()).unwrap();
        assert_eq!(insn.class, *exp_class, "class mismatch for {enc:#010x}");
        assert!(insn.flags.contains(*exp_flags), "flags mismatch for {enc:#010x}");
        assert_eq!(insn.src_count, *exp_src, "src_count mismatch for {enc:#010x}");
        assert_eq!(insn.dst_count, *exp_dst, "dst_count mismatch for {enc:#010x}");
    }
}
```

#### T3 — Executor Parity Tests (Phase 1)

The new `Executor` trait implementation must match the old `exec.rs` /
`step_fast()` for every instruction, on every edge case.

| Test | What it verifies | Location |
|------|-----------------|----------|
| Single-insn parity | For each decode vector: run through both old `step_fast` and new `Executor::execute()`, compare all 31 GPRs + SP + PC + NZCV + DAIF | `isa/src/tests/executor_parity.rs` |
| Flag edge cases | SUBS: `0-1` (C=0), `x-x` (Z=1,C=1), `MIN-1` (V=1); ADDS: `MAX+1` (C=1,Z=1), `MAX_SIGNED+1` (V=1); ANDS: result=0 (Z=1) | Same |
| 32-bit truncation | All W-register ops: high 32 bits of Xn must not leak into result or flags | Same |
| Memory ops | LDR/STR: all sizes (1,2,4,8), all offsets, pre/post-index, writeback to base | Same |
| Pair ops | LDP/STP: aligned, offset variants, SP-relative | Same |
| System ops | MRS/MSR, DC ZVA, barriers, WFI, SVC, HVC, ERET | Same |
| BFM/BFI/BFXIL | 32-bit rotation truncation (regression for BFM bug) | Same |
| decode_bitmask | All logical immediates: verify against precomputed table | Same |
| Exclusive access | LDXR/STXR: success, failure (monitor cleared), nested | Same |
| CRC32/CRC32C | Known input/output pairs from kernel | Same |
| x86 REP MOVSB | ECX=100, verify 100 bytes copied, ECX=0 after | `isa/src/tests/x86_executor.rs` |
| x86 PUSH/POP | RSP adjusted, value on stack, paired round-trip | Same |
| x86 CMPXCHG | Success (ZF=1, store) and failure (ZF=0, load old) paths | Same |
| x86 MUL/IMUL | RAX * operand → RDX:RAX, flags correct | Same |
| RV64 AMO | AMOSWAP, AMOADD: memory and register values correct | `isa/src/tests/rv64_executor.rs` |

**Harness pattern:**

```rust
/// Run one instruction through both old and new paths, compare everything.
fn executor_parity(
    enc: u32,
    init_regs: &[u64; 32],
    init_flags: u64,
    mem_init: &[(Addr, &[u8])],  // pre-populate memory
) {
    // Old path: Aarch64Cpu + step_fast
    let mut old_cpu = Aarch64Cpu::new();
    /* set regs, flags, write memory */
    old_cpu.step_fast(&mut old_mem);

    // New path: Decoder + Executor trait
    let dec = Aarch64Decoder::new();
    let mut exec = Aarch64Executor::new();
    let insn = dec.decode(pc, &enc.to_le_bytes()).unwrap();
    let outcome = exec.execute(&insn, &mut new_cpu, &mut new_mem);

    // Compare
    for i in 0..31 {
        assert_eq!(old_cpu.gpr(i), new_cpu.gpr(i), "X{i} mismatch for {enc:#010x}");
    }
    assert_eq!(old_cpu.flags(), new_cpu.flags(), "flags mismatch for {enc:#010x}");
    assert_eq!(old_cpu.pc(), outcome.next_pc, "PC mismatch for {enc:#010x}");
    /* compare memory contents at all touched addresses */
}
```

#### T4 — MemoryAccess Implementation Tests (Phase 1)

| Test | What it verifies | Location |
|------|-----------------|----------|
| FlatMemoryAccess basic | read/write round-trip, unmapped fault, permission fault | `memory/src/tests/flat.rs` |
| FlatMemoryAccess wide | 16-byte SSE / 32-byte AVX read/write | Same |
| FlatMemoryAccess bulk | `copy_bulk` 4KB, `fill_bulk` 4KB, verify contents | Same |
| FlatMemoryAccess CAS | `compare_exchange` success and failure | Same |
| MmuMemoryAccess translate | VA→PA through page table, TLB hit, TLB miss+fill | `memory/src/tests/mmu_access.rs` |
| MmuMemoryAccess fault | Stage-1 fault, stage-2 fault (with correct FAR) | Same |
| MmuMemoryAccess alignment | Unaligned access (allowed/trapped depending on SCTLR.A) | Same |

#### T5 — TimingBackend Tests (Phase 1)

| Test | What it verifies | Location |
|------|-----------------|----------|
| NullBackend always zero | `account()` returns 0 for every InsnClass | `timing/src/tests/null.rs` |
| IntervalBackend latencies | Each InsnClass returns configured latency | `timing/src/tests/interval.rs` |
| IntervalBackend cache | L1/L2/L3/DRAM hit distribution matches config | Same |
| IntervalBackend branch | Misprediction returns `branch_penalty` cycles | Same |
| PipelineBackend flow | Insert insn → ROB allocated → complete → commit → stall = 0 | `timing/src/tests/pipeline.rs` |
| PipelineBackend stall | Data dependency → stall > 0 | Same |
| PipelineBackend full ROB | ROB full → stall until commit frees entry | Same |
| SamplingBackend switch | Phase transition switches between NullBackend and detailed | `timing/src/tests/sampling.rs` |

#### T6 — JIT Translator Parity Tests (Phase 3)

The new `JitTranslator` (consuming `DecodedInsn`) must produce identical
TcgOp sequences to the old `A64TcgEmitter` (consuming raw bytes).

| Test | What it verifies | Location |
|------|-----------------|----------|
| TcgOp sequence parity | For each decode vector: old emitter and new translator produce same TcgOp list (or semantically equivalent) | `jit/src/tests/translator_parity.rs` |
| JIT parity (interp vs Cranelift) | Existing `jit_parity.rs` tests, adapted to use `Decoder` → `JitTranslator` pipeline | `jit/src/tests/jit_parity.rs` |
| JIT parity NZCV | All flag edge cases (SUBS, ADDS, CCMP, ANDS — both 32 and 64-bit) | Same |
| JIT parity emitter-level | `jit_parity_one_insn()` adapted: decode → translate → compile → execute → compare with interpreter | Same |

#### T7 — Engine Session Tests (Phase 2)

| Test | What it verifies | Location |
|------|-----------------|----------|
| Generic Session SE inflate | `Session<Aarch64Decoder, Aarch64Executor, ...>` with interpreted backend runs inflate to exit(0) | `engine/src/tests/session_inflate.rs` |
| Generic Session SE JIT inflate | Same with JIT backend — exit(0) | Same |
| Session interp/JIT parity | Both backends produce same exit code and instruction count (within 1%) | Same |
| Session FE timing | NullBackend: `cycles == instructions_committed` (IPC=1) | `engine/src/tests/session_timing.rs` |
| Session ITE timing | IntervalBackend: `cycles > instructions_committed` (stalls observed) | Same |
| Session CAE timing | PipelineBackend: IPC < pipeline width, stalls from dependencies | Same |
| Session mode switch | Start FE, switch to ITE mid-run, verify timing changes | Same |
| Session SE syscalls | `SyscallHandler` called for SVC, exit code propagated | `engine/src/tests/session_syscall.rs` |
| Session FS boot | FsSession-equivalent boots kernel to UART output | `engine/src/tests/session_fs.rs` |
| Session FS IRQ | Timer interrupt delivered, handler runs | Same |
| Old SeSession parity | Old `SeSession.run()` and new `Session.run_interpreted()` produce identical results for inflate binary | `engine/src/tests/session_migration.rs` |
| Old FsSession parity | Same for FS mode (kernel boot to same UART output) | Same |

#### T8 — Cross-ISA Conformance Tests (Phase 5)

Prove the trait design works for multiple ISAs.

| Test | What it verifies | Location |
|------|-----------------|----------|
| RV64 decode | All RV64I/M/A/F/D base instructions decode correctly | `isa/src/tests/rv64_decode.rs` |
| RV64 execute | ADD, SUB, LW, SW, BEQ, JAL, ECALL, AMO* | `isa/src/tests/rv64_execute.rs` |
| RV64 SE inflate | Same inflate binary (cross-compiled for RV64) runs to exit(0) | `engine/src/tests/rv64_inflate.rs` |
| RV64 JIT parity | Interpreter vs JIT for RV64 basic blocks | `jit/src/tests/rv64_jit_parity.rs` |
| x86 decode | Variable-length, prefix handling, ModR/M, SIB parsing | `isa/src/tests/x86_decode.rs` |
| x86 REP/string | REP MOVSB, REP STOSB: count, direction flag, RSI/RDI update | `isa/src/tests/x86_execute.rs` |
| x86 implicit regs | MUL RAX*operand→RDX:RAX, DIV, CPUID | Same |
| x86 segment/IO | IN/OUT flagged as IO_PORT, FS/GS override flagged | `isa/src/tests/x86_decode.rs` |

#### T9 — Regression Tests (Permanent)

Every bug from the project history becomes a named regression test.

| Test name | Bug it prevents | Origin |
|-----------|----------------|--------|
| `regr_bfm_32bit_rotation` | BFM src must be truncated to Wn for sf=0 | 2026-03-07 |
| `regr_c_flag_32bit_signflip` | C flag computation: flip at bit 63, truncate for sf=0 | 2026-03-08 |
| `regr_decode_bitmask_len` | `31 - leading_zeros()`, not `leading_zeros() - 26` | 2026-03-08 |
| `regr_decode_bitmask_rotation` | esize-bit rotation, not 64-bit rotate_right | 2026-03-08 |
| `regr_cntvct_stale_in_jit` | CNTVCT_EL0 must update at timer intervals | 2026-03-08 |
| `regr_jit_exception_drop` | BRK/SVC/HVC/SMC must not fall to `_ => {}` | 2026-03-08 |
| `regr_irq_jit_delivery` | check_irq() must run in JIT path, not just interpreter | 2026-03-08 |
| `regr_tlbi_sign_extension` | TLBI VA must sign-extend from bit 55 | 2026-03-07 |
| `regr_esr_il_bit` | ESR IL bit (25) must be 1 for AArch64 | 2026-03-07 |

Location: `isa/src/tests/regressions.rs` and `jit/src/tests/regressions.rs`.

These tests use exact input register values and exact expected output values
from the original bug reports.  They must never be weakened or removed.

#### T10 — Performance Regression Tests (Continuous)

| Test | Metric | Threshold | Location |
|------|--------|-----------|----------|
| Inflate interp throughput | MIPS | Must not drop > 10% from baseline | `engine/benches/inflate_interp.rs` |
| Inflate JIT throughput | MIPS | Must not drop > 5% from baseline | `engine/benches/inflate_jit.rs` |
| Decode throughput | insns/sec | Must not drop > 10% from baseline | `isa/benches/decode.rs` |
| JIT compile latency | µs/block | Must not increase > 20% from baseline | `jit/benches/compile.rs` |
| `DecodedInsn` size | bytes | Must be ≤ 128 (two x86-64 cache lines) | `core/src/tests/insn.rs` (compile-time) |
| `ExecOutcome` size | bytes | Must be ≤ 64 (one cache line) | Same |
| TimingBackend::account overhead | ns/call | NullBackend < 1ns, IntervalBackend < 50ns | `timing/benches/account.rs` |

Baselines are recorded in `benches/baselines.json` and updated only when
performance *improves*.  CI fails if any metric regresses beyond its threshold.

### 8.3  Test Infrastructure

#### ISA-Parametric Test Harness

```rust
/// Trait for ISA-specific test setup.  Each ISA provides one.
trait IsaTestKit {
    type Cpu: CpuState;
    type Dec: Decoder;
    type Exec: Executor;

    fn new_cpu() -> Self::Cpu;
    fn new_decoder() -> Self::Dec;
    fn new_executor() -> Self::Exec;
    fn new_memory() -> Box<dyn MemoryAccess>;

    /// Encode a canonical test instruction (e.g. "add r0, r1, r2").
    fn encode_add(dst: RegId, a: RegId, b: RegId) -> Vec<u8>;
    fn encode_load(dst: RegId, base: RegId, offset: i64) -> Vec<u8>;
    fn encode_store(src: RegId, base: RegId, offset: i64) -> Vec<u8>;
    fn encode_branch(offset: i64) -> Vec<u8>;
    fn encode_syscall() -> Vec<u8>;
}

struct Aarch64TestKit;
impl IsaTestKit for Aarch64TestKit { ... }

struct Rv64TestKit;
impl IsaTestKit for Rv64TestKit { ... }

struct X86TestKit;
impl IsaTestKit for X86TestKit { ... }
```

Generic test functions are written once, parameterised over `IsaTestKit`:

```rust
fn test_add_parity<T: IsaTestKit>() {
    let mut cpu = T::new_cpu();
    let dec = T::new_decoder();
    let mut exec = T::new_executor();
    let mut mem = T::new_memory();

    cpu.set_gpr(1, 100);
    cpu.set_gpr(2, 200);
    let bytes = T::encode_add(0, 1, 2);
    let insn = dec.decode(0x1000, &bytes).unwrap();
    let outcome = exec.execute(&insn, &mut cpu, &mut mem);

    assert_eq!(cpu.gpr(0), 300);
    assert_eq!(outcome.exception, None);
}

#[test] fn aarch64_add() { test_add_parity::<Aarch64TestKit>() }
#[test] fn rv64_add()     { test_add_parity::<Rv64TestKit>() }
#[test] fn x86_add()      { test_add_parity::<X86TestKit>() }
```

#### Step-Compare Harness (Divergence Finder)

For finding the exact instruction where JIT diverges from interpreter:

```rust
/// Runs the same binary through interpreter and JIT in lockstep.
/// Binary-searches for the first instruction where state diverges.
pub struct StepCompare<D: Decoder, E: Executor, C: CpuState + Clone> {
    interp_cpu: C,
    jit_cpu: C,
    decoder: D,
    executor: E,
    // ...
}

impl<D, E, C> StepCompare<D, E, C>
where
    D: Decoder, E: Executor, C: CpuState + Clone,
{
    /// Run both paths for `n` instructions.  If divergence found, returns
    /// (insn_index, interp_state, jit_state, faulting_insn).
    pub fn find_divergence(&mut self, n: u64) -> Option<Divergence<C>> {
        // Phase 1: coarse scan (every 1000 insns, snapshot and compare)
        // Phase 2: fine scan (step 1-by-1 within the 1000-insn window)
        // Phase 3: report exact instruction, register diff, and DecodedInsn
    }
}
```

Location: `engine/src/tests/step_compare.rs`.  Used by the inflate parity
test and available as a debugging tool via Python bindings.

### 8.4  Phase Test Gates

Each phase has a test gate that must pass before proceeding:

| Phase | Gate | What must pass |
|-------|------|----------------|
| 0 | `cargo test -p helm-core` | T1 (trait contract tests) |
| 1 | `cargo test -p helm-isa -p helm-memory -p helm-timing -p helm-syscall` | T2 + T3 + T4 + T5 |
| 1→2 transition | Executor parity: `cargo test executor_parity` | T3 for all 651 decode patterns |
| 2 | `cargo test -p helm-engine` | T7 (session tests, inflate parity) |
| 2→3 transition | Old vs new session parity: `cargo test session_migration` | T7 migration tests |
| 3 | `cargo test -p helm-jit` | T6 (JIT translator parity) |
| 3→4 transition | Full inflate: `cargo test inflate` through new pipeline | T7 + T6 combined |
| 4 | `cargo test --workspace` | Everything — no regressions |
| 5 | `cargo test --workspace` + benchmarks | T8 + T9 + T10 |

### 8.5  CI Integration

```yaml
# .github/workflows/test.yml (sketch)
jobs:
  unit:
    - cargo test --workspace --exclude helm-kvm  # KVM needs /dev/kvm
  parity:
    - cargo test --test executor_parity
    - cargo test --test decoder_parity
    - cargo test --test jit_parity
    - cargo test --test session_migration
  inflate:
    - cargo test inflate_interp_passes
    - cargo test inflate_tcg_passes
    - cargo test inflate_session_parity
  regressions:
    - cargo test regr_
  benches:
    - cargo bench --bench inflate_interp -- --save-baseline current
    - cargo bench --bench inflate_jit -- --save-baseline current
    # compare against stored baseline, fail if regression > threshold
  multi-isa:  # Phase 5 only
    - cargo test rv64_
    - cargo test x86_
```

---

## 9  Success Criteria

The migration is complete when:

1. `cargo test --workspace` passes with no regressions
2. `helm-engine` has zero imports from `helm-isa`, `helm-jit`, `helm-timing`,
   `helm-pipeline`, `helm-syscall` — only trait imports from `helm-core`
3. Adding a new ISA (RISC-V) requires changes only in `helm-isa/src/riscv64/`
   and `helm-jit/src/riscv64/` — zero changes to engine
4. Switching from FE to CAE requires changing one line:
   `Session::new(decoder, executor, cpu, mem, PipelineBackend::new(config))`
   vs `Session::new(decoder, executor, cpu, mem, NullBackend)`
5. JIT inflate parity test passes (correctness)
6. JIT throughput does not regress by more than 5% (perf)
7. `helm-translate` crate is deleted
8. No `Aarch64Cpu` or `A64TcgEmitter` type appears in `helm-engine`
9. All regression tests (T9) pass — no historical bug can recur
10. Step-compare harness available and passing for inflate binary
11. RISC-V at least decodes + executes basic integer subset through the
    same generic `Session` (proves multi-ISA trait design)

---

## 10  Estimated Effort per Phase

| Phase | Scope | Depends on |
|-------|-------|-----------|
| Phase 0 | Add traits and types to helm-core | — |
| Phase 1 | Implement traits on existing types | Phase 0 |
| Phase 2 | Migrate engine to generic Session | Phase 1 |
| Phase 3 | Migrate JIT to DecodedInsn | Phase 0, Phase 1.4 |
| Phase 4 | Cut dead dependencies | Phase 2, Phase 3 |
| Phase 5 | Polish, RISC-V, sampling | Phase 4 |

Phases 1 and 3 can proceed in parallel.  Phase 2 depends on Phase 1.
Phase 4 is mechanical after Phase 2+3 merge.
