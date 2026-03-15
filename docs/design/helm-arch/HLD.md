# helm-arch — High-Level Design

> **Status:** Draft — Phase 0 (RISC-V) + Phase 2 (AArch64) target
> **Crate:** `helm-arch`
> **Depends on:** `helm-core` only
> **Provides to:** `helm-engine`

---

## 1. Purpose

`helm-arch` implements every ISA supported by helm-ng. It contains instruction decode, instruction execute, and the ISA-specific execution state for each architecture. Nothing else.

The crate answers two questions per simulated instruction:

1. **What instruction is this?** — Decode: raw bytes → `Instruction` enum variant carrying all operand fields.
2. **What does it do?** — Execute: `(Instruction, &mut impl ExecContext)` → `Result<(), HartException>`.

All decode and execute logic lives here. No timing models, no device models, no event queues, no OS interfaces.

---

## 2. Scope

### In scope

| Concern | Component |
|---------|-----------|
| RISC-V RV64GC instruction decode | `riscv::decode` |
| RISC-V RV64GC instruction execute | `riscv::execute` |
| RISC-V C extension pre-expansion (16→32) | `riscv::compress` |
| RISC-V CSR file layout and side effects | `riscv::csr` |
| AArch64 instruction decode (deku-based) | `aarch64::decode` |
| AArch64 instruction execute | `aarch64::execute` |
| AArch64 system register access | `aarch64::sysreg` |
| Per-ISA `Instruction` enum definitions | `riscv::insn`, `aarch64::insn` |
| ISA-specific `HartException` variants | `riscv::exception`, `aarch64::exception` |
| ISA test vectors | `tests/riscv/`, `tests/aarch64/` |

### Out of scope

| Concern | Where it lives |
|---------|---------------|
| Architectural register file (`ArchState`) | `helm-core` |
| Memory access implementation | `helm-memory` (via `ExecContext::read_mem`) |
| TLB and page table walking | `helm-memory` |
| CSR side-effect dispatch (satp → TLB flush) | Called inside the execute function, routes to `ExecContext` |
| Execution loop and PC advance | `helm-engine` |
| Timing model integration | `helm-engine` / `helm-timing` |
| Syscall handling | `helm-engine/se` |
| AArch32 / Thumb decode + execute | Future Phase 3 — stubs only |

---

## 3. Module Structure

```
helm-arch/src/
├── lib.rs                 — pub use re-exports, crate entry point
├── riscv/
│   ├── mod.rs             — pub use from submodules
│   ├── insn.rs            — Instruction enum (all RV64GC variants)
│   ├── decode.rs          — decode_rv64(raw: u32) -> Result<Instruction, DecodeError>
│   ├── compress.rs        — decode_rv64c(raw: u16) -> Result<u32, DecodeError>
│   ├── execute.rs         — execute(insn, ctx) -> Result<(), HartException>
│   ├── csr.rs             — CsrAddr constants, side-effect handler dispatch
│   └── exception.rs       — HartException (RISC-V causes), DecodeError
├── aarch64/
│   ├── mod.rs             — pub use from submodules
│   ├── insn.rs            — Aarch64Instruction enum
│   ├── decode.rs          — decode_a64(raw: u32) -> Result<Aarch64Instruction, DecodeError>
│   ├── execute.rs         — execute_a64(insn, ctx) -> Result<(), HartException>
│   ├── sysreg.rs          — system register encoding, MRS/MSR dispatch
│   ├── flags.rs           — NZCV helpers: add_with_carry, sub_borrow, check_cond
│   └── exception.rs       — HartException (AArch64 ESR encoding), DecodeError
├── aarch32/
│   └── mod.rs             — stub: DecodeError::Unsupported for all inputs
└── tests/
    ├── riscv/
    │   ├── rv64i.rs        — RV64I instruction unit tests
    │   ├── rv64m.rs        — M extension tests
    │   ├── rv64a.rs        — A extension (LR/SC, AMO) tests
    │   ├── rv64f.rs        — F extension FP tests
    │   ├── rv64d.rs        — D extension FP tests
    │   ├── rv64c.rs        — C extension expansion tests
    │   └── zicsr.rs        — CSR instruction tests
    └── aarch64/
        ├── data_proc.rs   — Data processing instruction tests
        ├── load_store.rs  — Load/store instruction tests
        └── branch.rs      — Branch and system instruction tests
```

---

## 4. Public API

### 4.1 RISC-V

```rust
// Primary decode entry point. Accepts a fully expanded 32-bit instruction word.
// C extension instructions must be pre-expanded before calling this.
pub fn decode_rv64(raw: u32) -> Result<Instruction, DecodeError>;

// C extension decode and expansion. Returns the equivalent 32-bit instruction word.
pub fn decode_rv64c(raw: u16) -> Result<u32, DecodeError>;

// Execute a single decoded instruction against a context implementing ExecContext.
// Returns Ok(()) on success. Returns Err(HartException) for traps (ECALL, EBREAK,
// illegal instruction, memory fault, CSR access violation).
pub fn execute<C: ExecContext>(insn: Instruction, ctx: &mut C) -> Result<(), HartException>;
```

### 4.2 AArch64

```rust
// Decode a 32-bit AArch64 instruction word.
pub fn decode_a64(raw: u32) -> Result<Aarch64Instruction, DecodeError>;

// Execute a single decoded AArch64 instruction.
pub fn execute_a64<C: ExecContext>(insn: Aarch64Instruction, ctx: &mut C) -> Result<(), HartException>;
```

### 4.3 Hart Structs

Each ISA exposes a Hart struct that owns its ISA-specific state (register file, CSRs, PC) and implements the `Hart` trait from `helm-core`. The engine owns one of these per execution thread.

```rust
pub struct RiscvHart {
    pub regs:   [u64; 32],   // x0 hardwired to 0 on write
    pub pc:     u64,
    pub csrs:   RiscvCsrFile,
    pub priv_:  PrivLevel,   // M / S / U
    pub lr_addr: Option<u64>, // LR/SC reservation set (single address)
}

pub struct Aarch64Hart {
    pub regs:   [u64; 31],   // X0–X30; XZR not stored
    pub sp_el0: u64,
    pub sp_el1: u64,
    pub pc:     u64,
    pub pstate: Pstate,      // NZCV + EL + SP + DAIF
    pub sysregs: Aarch64SysregFile,
    pub vregs:  [u128; 32],  // V0–V31 (SIMD/FP registers)
}
```

**Design decision Q18 — Separate hart structs (chosen):** `RiscvHart` and `Aarch64Hart` are separate structs. Each implements `Hart`. The engine selects via an `Isa` enum at the hot-loop call site. This is cleaner than a single `IsaState` superset struct: it avoids padding waste, keeps each ISA's register file independently testable, and allows the types to evolve independently.

---

## 5. Decode Strategy

### 5.1 RISC-V — pure match + bit ops

RISC-V has a regular, orthogonal encoding: bits [6:0] identify the opcode group; `funct3` and `funct7` disambiguate within groups. There is no irregular encoding that requires a specialized bit-field parser. The implementation uses:

- `opcode` (bits [6:0]) as the primary match key
- `funct3` (bits [14:12]) as the secondary key
- `funct7` (bits [31:25]) as the tertiary key
- Six encoding format helpers (`decode_r_type`, `decode_i_imm`, `decode_s_imm`, `decode_b_imm`, `decode_u_imm`, `decode_j_imm`) for immediate extraction

**Design decision Q19 — Enum-based decode (chosen):** Decode returns a `riscv::Instruction` enum, not `Box<dyn Executable>`. The enum is `Copy`, lives on the stack, and requires no heap allocation. The execute function receives it by value. This is faster than trait objects: there is no heap allocation, no vtable, and the compiler can optimize the execute match tree.

**Design decision Q24 — C extension pre-expansion (chosen):** Compressed 16-bit instructions are expanded to their 32-bit equivalents by `decode_rv64c` before entering the main decode path. This means `decode_rv64` only handles 32-bit instructions. The execute path is unaware that a C instruction was the source. This simplifies the execute function's match arm count and eliminates a second family of `Instruction` variants.

### 5.2 AArch64 — deku crate for bit-field parsing

AArch64 has approximately 1,000 distinct instruction encodings with highly irregular bit field layouts — instruction group, size bits, shift amount, extend type, scale, opc bits, and register fields are scattered across non-contiguous bit ranges in encoding-group-specific ways.

**Design decision Q21 — deku crate (chosen):** `deku` provides derive macros that map struct fields to specific bit ranges. The field layout matches the encoding tables in ARM DDI 0487 directly, reducing the transcription error rate. The alternative (hand-written bit extraction for every field of every instruction) is feasible but error-prone for ~1,000 encodings.

**Design decision Q22 — Organize by encoding group (chosen):** AArch64 instructions are organized in the `Aarch64Instruction` enum by the top-level encoding groups from bits [28:25] of each instruction word. Within each group, sub-groups are further nested. This mirrors the ARM spec structure and makes the decode tree easy to audit against the reference manual.

---

## 6. Execute Strategy

### Pure functions with ExecContext trait

Execute functions are pure in the following sense: all state access goes through `ExecContext`. The execute function itself holds no mutable state — it mutates only through the `ctx` argument.

```rust
// ExecContext is defined in helm-core. These are the methods relevant to
// ISA execution:
pub trait ExecContext {
    fn read_int_reg(&self, idx: usize) -> u64;
    fn write_int_reg(&mut self, idx: usize, val: u64);
    fn read_float_reg(&self, idx: usize) -> u64;
    fn write_float_reg(&mut self, idx: usize, val: u64);
    fn read_pc(&self) -> u64;
    fn write_pc(&mut self, val: u64);
    fn read_mem(&self, addr: u64, width: usize) -> Result<u64, MemFault>;
    fn write_mem(&mut self, addr: u64, width: usize, val: u64) -> Result<(), MemFault>;
    fn read_csr(&self, csr: u16) -> Result<u64, HartException>;
    fn write_csr(&mut self, csr: u16, val: u64) -> Result<(), HartException>;
    fn raise_exception(&mut self, exc: HartException) -> !;
}
```

The `ExecContext` implementation is the concrete `RiscvHart` or `Aarch64Hart` struct (passed as a generic parameter, not a trait object). No vtable. No indirection.

**Design decision Q20 — CSR side effects in execute loop (chosen):** When a CSR write has architectural side effects (e.g., `satp` → TLB flush, `mstatus` → privilege mode change), those effects are triggered inside the `execute` function body by pattern-matching on the CSR address after the write. This is explicit and auditable: the effect is colocated with the instruction semantics. The alternative — encoding side effects into `ExecContext::write_csr` — would hide them inside the context implementation and make them hard to test independently.

---

## 7. AArch32 Stubs (Q23)

**Design decision Q23:** AArch32 is not implemented in Phase 0 or Phase 2. The `aarch32` module provides a single decode entry point that returns `DecodeError::Unsupported`. This satisfies the Rust requirement that all match arms be handled when the engine dispatches on ISA variant. It also provides a clearly-defined stub boundary for future work.

When AArch64 FS mode (Phase 3) is added, EL0 AArch32 userspace support will require:
- A32 and T32 (Thumb) decode trees (separate from A64)
- Banked integer register file (r0–r14 per mode)
- CPSR / SPSR encoding
- Interworking on exception entry/return

Until Phase 3, any attempt to execute AArch32 raises `HartException::Unsupported`.

---

## 8. Dependencies

```
helm-core   — ExecContext, MemFault, MemInterface, HartException, Hart trait
    ▲
helm-arch   — Instruction, decode_rv64, execute, decode_a64, execute_a64
             RiscvHart, Aarch64Hart
    ▲
helm-engine — calls decode_* and execute_* inside step_riscv() / step_aarch64()
```

`helm-arch` depends **only** on `helm-core`. It does not depend on `helm-memory`, `helm-timing`, `helm-event`, or `helm-devices`. Memory access is performed through `ExecContext::read_mem` / `write_mem`, which the engine implements using the `MemoryMap` from `helm-memory`.

External dependencies:
- `deku` — AArch64 bit-field parsing only
- `libm` (or `std`) — IEEE 754 FP operations for F/D/SIMD execute

---

## 9. Key Design Decisions Summary

| ID | Question | Decision |
|----|----------|----------|
| Q18 | Single `Isa` enum vs. separate hart structs | Separate: `RiscvHart`, `Aarch64Hart` each implement `Hart` |
| Q19 | Enum decode vs. trait object | Enum: `Instruction` is `Copy`, heap-free, compiler-optimizable |
| Q20 | CSR side effects: in execute vs. in ExecContext | In execute: explicit, auditable, co-located with instruction semantics |
| Q21 | deku vs. hand-written AArch64 decode | deku: matches ARM DDI 0487 spec tables directly, reduces error rate |
| Q22 | AArch64 organization | By encoding group (bits [28:25] top-level), matching ARM spec structure |
| Q23 | AArch32 stubs | `DecodeError::Unsupported` for all AArch32 input; Phase 3 work item |
| Q24 | C extension expansion | Pre-expand 16→32 in `decode_rv64c`; execute sees only 32-bit form |

---

## 10. Related Documents

- `LLD-riscv-decode.md` — `Instruction` enum, bit-extraction helpers, opcode dispatch, C expansion
- `LLD-riscv-execute.md` — full execute match, RV64I/M/A/F/D/Zicsr semantics, CSR side effects
- `LLD-aarch64-decode.md` — `Aarch64Instruction` enum, deku struct examples, group dispatch
- `LLD-aarch64-execute.md` — execute_a64 match, NZCV helpers, all encoding groups
- `TEST.md` — unit test strategy, property tests, riscv-tests integration, QEMU differential
