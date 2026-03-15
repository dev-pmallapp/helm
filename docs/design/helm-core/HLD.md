# helm-core — High-Level Design

> Version: 0.1.0
> Status: Draft
> Cross-references: [LLD-arch-state.md](LLD-arch-state.md) · [LLD-interfaces.md](LLD-interfaces.md) · [TEST.md](TEST.md) · [ARCHITECTURE.md](../../ARCHITECTURE.md)

---

## Table of Contents

1. [Purpose and Scope](#1-purpose-and-scope)
2. [What helm-core Does NOT Contain](#2-what-helm-core-does-not-contain)
3. [Dependency Graph](#3-dependency-graph)
4. [Public API Surface](#4-public-api-surface)
5. [Key Types](#5-key-types)
6. [Design Decisions Q1–Q9](#6-design-decisions-q1q9)
7. [The HelmAttr Attribute System](#7-the-helmattr-attribute-system)
8. [Error Philosophy](#8-error-philosophy)
9. [Crate Layout](#9-crate-layout)

---

## 1. Purpose and Scope

`helm-core` is the **foundation crate** of the helm-ng simulator workspace. It defines the irreducible abstractions that every other crate builds upon. Nothing in `helm-core` knows about ISA encoding, timing models, cache hierarchies, device registers, or operating system interfaces.

The crate answers exactly one question per abstraction:

| Abstraction | Question answered |
|---|---|
| `ArchState` | What architectural state does a hart carry? |
| `ExecContext` | How does ISA execute code read and write that state? |
| `ThreadContext` | How do external agents (GDB, OS, Python) observe and modify it? |
| `MemInterface` | How does a hart request memory operations? |
| `MemFault` | What goes wrong when a memory operation fails? |
| `HartException` | What goes wrong when executing an instruction? |

Everything else — instruction decoding, cache models, event queues, syscall tables, device registers — is implemented in crates that depend on `helm-core`, not inside it.

### The Invariant

> `helm-core` must compile to a stable, minimal interface that all other helm-* crates agree on. Breaking `helm-core` breaks everything. Adding to it should be rare and deliberate.

---

## 2. What helm-core Does NOT Contain

This is as important as what it does contain. The following are explicitly out of scope:

| Topic | Lives in |
|---|---|
| RISC-V instruction decode and execute | `helm-arch` |
| AArch64 instruction decode and execute | `helm-arch` |
| CSR side-effect logic (e.g., `satp` flush) | `helm-arch` |
| Memory region tree, FlatView, MMIO routing | `helm-memory` |
| Cache models (L1/L2/LLC) | `helm-memory` |
| TLB and page table walker | `helm-memory` |
| Timing models (Virtual, Interval, Accurate) | `helm-timing` |
| Event queue (BinaryHeap-based scheduler) | `helm-event` |
| Observability event bus | `helm-devices/src/bus/event_bus` |
| GDB RSP stub | `helm-debug` |
| Trace ring buffer | `helm-debug` |
| Checkpoint coordinator | `helm-debug` |
| Linux syscall table | `helm-engine/se` |
| Device trait, MMIO handler | `helm-devices` |
| Python/PyO3 bindings | `helm-python` |
| Performance counters | `helm-stats` |
| HelmEngine hot loop | `helm-engine` |

`helm-core` contains **only** the types that cannot live anywhere else without creating circular dependencies.

---

## 3. Dependency Graph

```
helm-core
  (no helm-* dependencies)

helm-arch       → helm-core
helm-memory     → helm-core
helm-timing     → helm-core
helm-event      → helm-core
helm-devices/bus   → helm-core
helm-engine     → helm-core, helm-arch, helm-memory, helm-timing, helm-event
helm-devices    → helm-core, helm-memory
helm-engine/se         → helm-core, helm-engine
helm-debug      → helm-core, helm-engine, helm-memory
helm-stats      → helm-core
helm-python         → helm-core, helm-engine, helm-engine/se, helm-debug, helm-stats
```

`helm-core` sits at the bottom of the DAG. It has **zero dependencies on any other `helm-*` crate**. External Rust crate dependencies (e.g., `thiserror` for error types) are kept minimal and must be justified.

### Permitted External Dependencies

| Crate | Justification |
|---|---|
| `thiserror` | Zero-overhead derive for `Error` impls; no runtime cost |

No `async` runtimes, no serialization libraries, no allocator crates. If a capability requires one of those, it belongs in a higher-level crate.

---

## 4. Public API Surface

The `helm-core` public API is organized into four modules:

```
helm_core::arch_state    — ArchState trait + ISA-specific impls
helm_core::exec_context  — ExecContext trait (hot path)
helm_core::thread_context — ThreadContext trait (cold path)
helm_core::mem           — MemInterface, MemRequest, MemResponse, MemOp, MemFault, AccessType
helm_core::error         — HartException
helm_core::attr          — HelmAttr (attribute key-value system)
```

The re-export at `lib.rs` exposes the most commonly needed types at the crate root:

```rust
pub use arch_state::{ArchState, RiscvArchState, Aarch64ArchState};
pub use exec_context::ExecContext;
pub use thread_context::ThreadContext;
pub use mem::{MemInterface, MemRequest, MemResponse, MemOp, MemFault, AccessType};
pub use error::HartException;
pub use attr::HelmAttr;
```

Callers use `use helm_core::*` for convenience or import individually.

---

## 5. Key Types

### `ArchState` (trait)

The complete architecturally-visible state for one hardware thread (hart). Two concrete implementations: `RiscvArchState` for RISC-V RV64GC and `Aarch64ArchState` for AArch64. Both implement the `ArchState` trait, which provides the minimal common interface.

State includes: integer registers, floating-point registers, program counter, and ISA-specific control registers (RISC-V CSRs / AArch64 system registers). See [LLD-arch-state.md](LLD-arch-state.md) for the complete field-level specification.

### `ExecContext` (trait)

The **hot-path** interface between the ISA execute layer (`helm-arch`) and the hart's architectural state plus memory. Every method is called at least once per simulated instruction. The trait is implemented on a concrete CPU struct (not `Box<dyn>`). All dispatch is static.

`ExecContext` must never allocate and must never block. It is the innermost loop boundary. See [LLD-interfaces.md](LLD-interfaces.md) for the full method set.

### `ThreadContext` (trait)

The **cold-path** interface for external agents: the GDB stub, the Linux syscall handler, Python inspection scripts, and the checkpoint coordinator. Dynamic dispatch is acceptable here (`&mut dyn ThreadContext`). `ThreadContext` is a superset of `ExecContext` — it adds methods for full register dumps, hart identification, ISA inspection, and pause/resume.

The split between `ExecContext` and `ThreadContext` is the key design decision answered in Q6 below.

### `MemInterface` (trait)

The interface between a hart and the memory subsystem. Three access modes with separate method families: timing (asynchronous, event-driven), atomic (synchronous with latency return), and functional (synchronous, side-effect-free). The concrete implementor lives in `helm-memory`, but the trait contract is defined here so `helm-core` and `helm-arch` can reference it without depending on `helm-memory`.

### `MemFault` (enum)

A typed enum of all reasons a memory operation can fail: alignment errors, access violations, page faults, unmapped addresses, atomic reservation failures. Returned as `Err(MemFault)` from `ExecContext::read_mem` / `write_mem`. See [LLD-interfaces.md](LLD-interfaces.md).

### `HartException` (enum)

A typed enum of all reasons instruction execution can fail: illegal instruction, breakpoint, environment call, machine-mode exception, etc. Returned as `Err(HartException)` from `Hart::step()`. ISA-specific exception causes are represented as inner values on variants. See [LLD-interfaces.md](LLD-interfaces.md).

### `HelmAttr` (attribute system)

A key-value attribute registry for state exposure, compatible with the SIMICS invariant: **attributes are the sole mechanism for state exposure and checkpoint serialization**. Every piece of architectural state accessible from outside the hot loop must be registered as a `HelmAttr`. This enables:

- Checkpoint/restore without per-type serialization logic
- Python-level register introspection
- GDB register mapping without custom protocol code

See [Section 7](#7-the-helmattr-attribute-system) for details.

---

## 6. Design Decisions Q1–Q9

### Q1: ArchState — Generic or ISA-specific?

**Decision: ISA-specific concrete structs, unified by a trait.**

`ArchState` is a Rust trait. Two concrete types implement it: `RiscvArchState` and `Aarch64ArchState`. There is no "generic `ArchState<Isa>`" with type parameters.

**Rationale:**

A generic struct (one struct for all ISAs) would require a union of all register state across all ISAs — integer widths, float formats, CSR counts, PSTATE bits, and AArch32 banking would all coexist in one struct. Every ISA-specific field would be dead weight for every other ISA, harming cache density on the hot path. Worse, the borrow checker could not enforce ISA-specific invariants (e.g., x0 is always 0 in RISC-V, but there is no equivalent zero register in AArch64).

ISA-specific structs let the compiler lay out memory optimally per ISA and catch ISA-specific invariant violations at compile time.

`HelmEngine<T>` is already generic over `T: TimingModel`. Adding a second generic parameter for ISA (`HelmEngine<T, A: ArchState>`) would be acceptable at the engine level, but it would propagate through the entire codebase. Instead, the `Isa` enum inside the engine dispatches to the correct step function, which receives the matching `ArchState` impl. See [ARCHITECTURE.md](../../ARCHITECTURE.md) §Multi-ISA Architecture.

**Implication for `ExecContext`:** The concrete struct that implements `ExecContext` holds a reference to the appropriate `ArchState` implementation (not a `Box<dyn ArchState>`), maintaining static dispatch on the hot path.

---

### Q2: Float registers — `f64` or `u64`?

**Decision: `[u64; 32]`, bit-cast at point of use.**

RISC-V defines NaN-boxing semantics: a 32-bit float value loaded into a 64-bit register is stored in the lower 32 bits with the upper 32 bits set to all-ones. AArch64 has similar considerations for NEON/SVE register aliasing. Using `f64` as the storage type loses the raw bit pattern and makes NaN-boxing impossible to implement correctly.

Storing `u64` preserves the exact bit pattern. ISA execute functions bit-cast to `f32` or `f64` at the instruction boundary using `f32::from_bits()` / `f64::from_bits()` and back with `.to_bits()`. The compiler generates the same machine code for a reinterpret cast as for a typed register, and the operation is always safe in Rust when explicitly requested.

**For AArch64:** The SIMD/FP register file (`V0`–`V31`) is 128 bits wide per register. These are stored as `[u128; 32]` in `Aarch64ArchState`, not as `[u64; 32]`. A `V` register can be accessed as `B` (8-bit), `H` (16-bit), `S` (32-bit), `D` (64-bit), or `Q` (128-bit) views — all are byte slices over the same `u128` storage. Accessor methods handle the view selection.

---

### Q3: CsrFile — flat array or HashMap?

**Decision: Flat array indexed by `u16` for RISC-V; sparse `HashMap<u32, u64>` for AArch64.**

**RISC-V:** The CSR address space is a 12-bit index (`u12`, stored as `u16`), giving 4096 possible addresses. Only ~200 are defined by the privileged spec. A flat `[u64; 4096]` array (32 KiB) fits entirely in L2 cache for a typical simulation run and gives O(1) reads and writes with no hashing. CSRs with side effects (e.g., `satp`, `sstatus`, `mstatus`) are handled via a **dispatch table**: a `[CsrHandler; 4096]` where `CsrHandler` is a small enum:

```rust
enum CsrHandler {
    /// Plain read/write — just the array slot.
    Plain,
    /// Side-effect CSR — the ISA layer registers a callback.
    SideEffect {
        on_read:  fn(&RiscvArchState) -> u64,
        on_write: fn(&mut RiscvArchState, u64),
    },
    /// Read-only (writes are no-ops or raise illegal instruction).
    ReadOnly,
    /// Undefined — access raises illegal instruction exception.
    Undefined,
}
```

`CsrHandler::Plain` is the common case and has zero overhead. The dispatch table itself is a fixed-size array, so indexing is an array lookup, not a function call for the common case.

**AArch64:** System registers use a 20-bit encoding (op0, op1, CRn, CRm, op2). The space is large and sparse — hundreds of defined registers scattered across a potentially huge address space. A flat array at 20-bit addressability would be 8 MB of mostly-zeroes. A `HashMap<u32, u64>` with the encoded key gives O(1) average access with much lower memory usage. AArch64 system register access is already infrequent compared to general-purpose register access (system registers are typically read/written in OS entry/exit paths, not per-instruction).

---

### Q4: ExecContext — trait or concrete struct?

**Decision: Trait with static dispatch.**

`ExecContext` is defined as a Rust trait. The concrete type that implements it is `HelmExecContext`, which lives in `helm-engine`. ISA execute functions receive a `&mut impl ExecContext` parameter (or equivalently `&mut C where C: ExecContext`), not `&mut dyn ExecContext`.

This gives us:

- **Zero vtable overhead**: the compiler monomorphizes `execute_riscv(ctx: &mut HelmExecContext, ...)`, inlining all `ctx.read_int_reg()` calls.
- **ISA independence**: `helm-arch` depends on the `ExecContext` trait from `helm-core`, not on any concrete type from `helm-engine`. This keeps the dependency graph acyclic.
- **Testability**: tests in `helm-arch` can create a lightweight `MockExecContext` that implements the trait without instantiating a full `HelmEngine`.

A concrete struct (no trait) would couple `helm-arch` directly to `helm-engine`, creating a circular dependency. A `Box<dyn ExecContext>` would add a vtable indirection on every register read and memory access — unacceptable at 100M+ instructions/sec.

---

### Q5: ExecContext::read_mem fault handling — Result or raise_exception?

**Decision: `Result<u64, MemFault>`, with the ISA layer deciding whether to call `raise_exception`.**

`read_mem` and `write_mem` return `Result<_, MemFault>`. The ISA execute function inspects the error and decides how to handle it:

```rust
// In helm-arch RISC-V execute:
match ctx.read_mem(addr, AccessType::Load, 4) {
    Ok(value) => { /* use value */ }
    Err(fault) => {
        ctx.raise_exception(HartException::from_mem_fault(fault, addr))?;
    }
}
```

**Rationale:**

The SIMICS-style "call raise_exception directly from read_mem" approach hides control flow from the ISA layer. Some ISA instructions handle certain fault types in software before escalating (e.g., RISC-V `LR`/`SC` sequences and AArch64 Load-Exclusive have specific atomicity rules around faults). The ISA layer needs to see the fault type.

`Result<_, MemFault>` is idiomatic Rust. It makes the error path explicit and forces every call site to handle faults, preventing silent bugs where a fault is ignored and the instruction continues with garbage data.

The `?` operator can be used when the ISA layer wants to propagate the fault directly, making the common case concise.

**MemFault → HartException mapping:** `HartException::from_mem_fault(fault, addr)` is a helper that converts a `MemFault` variant (e.g., `PageFault(addr)`) into the ISA-appropriate `HartException` variant (e.g., `LoadPageFault { addr }` for RISC-V). This mapping is ISA-specific and lives in `helm-arch`, not `helm-core`.

---

### Q6: ExecContext vs ThreadContext split

**Decision: Two separate traits; ThreadContext has ExecContext as a supertrait.**

```rust
pub trait ExecContext {
    // Hot-path methods only — called per instruction
}

pub trait ThreadContext: ExecContext {
    // Cold-path additions — called from GDB, Python, OS interfaces
}
```

**ExecContext** contains only what an ISA execute function needs during instruction execution:

- `read_int_reg` / `write_int_reg`
- `read_float_reg` / `write_float_reg`
- `read_csr` / `write_csr` (RISC-V) / `read_sysreg` / `write_sysreg` (AArch64)
- `read_pc` / `write_pc`
- `read_mem` / `write_mem`
- `raise_exception`

**ThreadContext** adds:

- `get_hart_id` — identity of this hardware thread
- `get_isa` — which ISA this hart implements
- `get_exec_mode` — FE / SE / FS
- `read_all_regs` / `write_all_regs` — bulk dump for GDB `g` / `G` packets
- `pause` / `resume` — suspend/resume execution
- `get_arch_state` / `get_arch_state_mut` — direct state access for checkpoint

**Rationale for supertrait (not composition):**

`ThreadContext: ExecContext` means a `&mut dyn ThreadContext` can be passed anywhere a `&mut dyn ExecContext` is expected. The GDB stub holds a `&mut dyn ThreadContext` (dynamic dispatch is fine — GDB commands happen at human timescales). It can hand a reference to the ISA layer if needed (e.g., for a single-step operation). If they were separate traits, you would need two separate references or a conversion method.

**What does NOT go in either trait:**

- Timer interrupt injection — goes through the timing model's event callback, not through the context interface
- DMA transfer initiation — a device-level operation, not a CPU interface
- Branch predictor state — microarchitectural, not architectural

See [LLD-interfaces.md](LLD-interfaces.md) for the complete method signatures.

---

### Q7: Does MemInterface live in helm-core?

**Decision: Yes. The `MemInterface` trait is defined in `helm-core`.**

**Rationale:**

`ExecContext::read_mem` and `write_mem` need to call into the memory subsystem. The ISA execute layer (`helm-arch`) calls these methods. If `MemInterface` lived in `helm-memory`, then `helm-arch` would need to depend on `helm-memory` just to hold a reference to the interface. This is unnecessary coupling — `helm-arch` does not need to know about `MemoryRegion` trees, `FlatView`, or MMIO handlers. It only needs the three-mode access protocol.

Putting `MemInterface` in `helm-core` means:
- `helm-arch` depends only on `helm-core` (not `helm-memory`)
- `helm-memory` implements the trait (depending on `helm-core`, not `helm-arch`)
- The dependency graph remains acyclic

The concrete implementation (`MemoryMap`, `FlatView`) lives in `helm-memory`. `helm-core` defines only the trait and the associated types (`MemRequest`, `MemResponse`, `MemOp`, `MemFault`, `AccessType`).

---

### Q8: Who owns in-flight timing request state?

**Decision: The memory system owns in-flight timing state. The CPU registers a callback.**

In timing mode, a memory request is asynchronous. The CPU fires a request and continues (or stalls, depending on the pipeline model). When the memory system satisfies the request, it invokes a callback.

The callback is a `Box<dyn FnOnce(MemResponse) + Send>` registered with the request. The memory system stores this callback (along with the request's latency and address) in its own in-flight queue. The CPU does not hold a handle to the in-flight request — it either stalls (waiting for a signal via the timing model) or continues speculatively.

**Rationale:**

The alternative — the CPU holds a `Future` or an `InFlightHandle` — requires either async Rust (adding tokio/async-std as a dependency, incompatible with the no-allocator-crate constraint) or a manual poll-based design that effectively reimplements the event queue. Keeping in-flight state in the memory system is simpler and matches how real hardware works: the memory controller tracks outstanding MSHRs, not the CPU.

**At the `helm-core` level:** `MemInterface::read_timing` takes an `on_complete: Box<dyn FnOnce(MemResponse) + Send>` parameter. The concrete signature is in [LLD-interfaces.md](LLD-interfaces.md).

---

### Q9: How does functional mode avoid cache side effects?

**Decision: The implementation skips cache fill; `helm-core` expresses this as a contract on `AccessType`.**

`MemInterface` has an `AccessType` enum that includes a `Functional` variant. When `AccessType::Functional` is passed to any memory access method, the implementation contract is:

> Functional accesses must not modify any microarchitectural state. They must return the data at the specified address without filling any cache lines, updating any TLB entries, or recording any prefetch state.

The `helm-core` trait does not enforce this mechanically (it cannot — the implementation is in `helm-memory`). It enforces it contractually via documentation and relies on `helm-memory`'s implementation to honor it.

**How `helm-memory` implements it:** The `FlatView` lookup for a functional access bypasses the cache hierarchy entirely and reads directly from the backing RAM or device. No MSHR is allocated, no cache line is filled, no replacement policy is exercised. For MMIO regions, functional reads call the `MmioHandler::read` method with a `is_functional: bool` flag so the device can choose to not update internal state (e.g., a UART FIFO should not dequeue data on a functional read).

**Why this matters:** The GDB stub uses functional reads to inspect memory without perturbing the simulation state. A debugger that caused cache fills would change the program's behavior between "runs with GDB" and "runs without GDB" — unacceptable for a deterministic simulator.

---

## 7. The HelmAttr Attribute System

`HelmAttr` is the bridge between Rust state and external observation/persistence. It is inspired by the SIMICS attribute system, where the invariant is: **every piece of observable or checkpointable state must be registered as an attribute**.

In helm-ng, this means:

- Every `ArchState` implementation registers its registers as attributes during construction.
- The checkpoint coordinator iterates all registered attributes to build a checkpoint blob.
- The Python inspection API calls `get_attr("x0")` / `set_attr("x0", 42u64)` without knowing the concrete `ArchState` type.
- The GDB stub maps GDB register numbers to attribute names.

`HelmAttr` in `helm-core` defines:

```rust
/// A typed attribute value.
pub enum AttrValue {
    U64(u64),
    I64(i64),
    F64(f64),
    Bool(bool),
    Bytes(Vec<u8>),
}

/// An attribute descriptor: name, get, set.
pub struct HelmAttr {
    pub name: &'static str,
    pub get:  Box<dyn Fn() -> AttrValue + Send>,
    pub set:  Box<dyn Fn(AttrValue) + Send>,
}
```

The `ArchState` trait has a required method:

```rust
fn register_attrs(&self, registry: &mut AttrRegistry);
```

Concrete implementations call `registry.add(HelmAttr { ... })` for each register. The closures capture a pointer to the register storage and read/write it on demand.

This approach has one key tradeoff: attribute access is slower than direct field access (one closure call per register read/write). This is acceptable because attributes are only used from cold paths (GDB, Python, checkpoint). The hot path (`ExecContext::read_int_reg`) bypasses the attribute system entirely and accesses register storage directly.

---

## 8. Error Philosophy

`helm-core` uses typed errors everywhere. No `anyhow`, no `Box<dyn Error>`. Every error type is an enum with named variants and attached context.

- `MemFault` — memory access failures (see [LLD-interfaces.md](LLD-interfaces.md))
- `HartException` — instruction execution failures (see [LLD-interfaces.md](LLD-interfaces.md))
- `AttrError` — attribute system errors (unknown name, type mismatch)

All error types derive `Debug`, `Clone`, `PartialEq`, and `thiserror::Error`. They do not heap-allocate. They are `Copy` where possible.

The `?` operator works throughout `helm-core` code. `helm-arch` code that calls `ctx.read_mem(...)` can propagate `MemFault` with `?`.

---

## 9. Crate Layout

```
helm-core/
├── Cargo.toml
└── src/
    ├── lib.rs               # pub use re-exports
    ├── arch_state/
    │   ├── mod.rs           # ArchState trait
    │   ├── riscv.rs         # RiscvArchState, IntRegs, FloatRegs, CsrFile
    │   └── aarch64.rs       # Aarch64ArchState, GprFile, VRegFile, SysRegFile, Pstate
    ├── exec_context.rs      # ExecContext trait
    ├── thread_context.rs    # ThreadContext trait
    ├── mem/
    │   ├── mod.rs           # MemInterface trait, re-exports
    │   ├── types.rs         # MemRequest, MemResponse, MemOp, AccessType
    │   └── fault.rs         # MemFault enum
    ├── error.rs             # HartException enum
    └── attr/
        ├── mod.rs           # HelmAttr, AttrValue, AttrRegistry
        └── macros.rs        # attr! helper macro (optional)
```

`Cargo.toml` workspace entry:

```toml
[package]
name = "helm-core"
version = "0.1.0"
edition = "2021"

[dependencies]
thiserror = "1"

[dev-dependencies]
proptest = "1"
```

No other dependencies. `std` is available; `no_std` support is not currently a goal but the types are designed to be `no_std`-compatible if that need arises (no heap allocation in hot-path types).
