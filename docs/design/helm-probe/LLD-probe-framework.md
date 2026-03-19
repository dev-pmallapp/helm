# helm-probe — Low-Level Design: Probe Framework

> See the [Instrumentation Stack HLD](HLD.md) for the architectural overview.
>
> **Status**: This document reflects the actual source code as of 2026-03-19.
> All code shown is the real implementation, not aspirational.

---

## 1. Crate Structure

```
framework/helm-probe/
├── Cargo.toml
└── src/
    ├── lib.rs          # crate root: re-exports, CpuProbes, GicProbes
    ├── probe.rs        # Probe<T> struct + impl
    ├── events.rs       # All event types
    └── macros.rs       # probe!() macro (#[macro_export])
```

No dependencies. Zero runtime cost in release.

---

## 2. `Cargo.toml`

```toml
[package]
name    = "helm-probe"
version.workspace = true
edition.workspace = true

[lints]
workspace = true

[features]
# Richer event fields (insn_count on CpuStepEvent). Off by default.
probe-full = []
```

No `[dependencies]`.

---

## 3. `src/probe.rs` — `Probe<T>` struct

```rust
use std::marker::PhantomData;

pub struct Probe<T> {
    #[cfg(debug_assertions)]
    listeners: Vec<Box<dyn Fn(&T) + Send + Sync>>,
    _marker: PhantomData<fn(&T)>,
}

impl<T> Probe<T> {
    pub const fn new() -> Self {
        Self {
            #[cfg(debug_assertions)]
            listeners: Vec::new(),
            _marker: PhantomData,
        }
    }

    #[inline(always)]
    pub fn has_listeners(&self) -> bool {
        #[cfg(not(debug_assertions))]
        { false }
        #[cfg(debug_assertions)]
        { !self.listeners.is_empty() }
    }

    #[inline(always)]
    pub fn notify(&self, val: &T) {
        #[cfg(debug_assertions)]
        for l in &self.listeners {
            l(val);
        }
        let _ = val;
    }

    #[cfg(debug_assertions)]
    pub fn subscribe(&mut self, f: impl Fn(&T) + Send + Sync + 'static) {
        self.listeners.push(Box::new(f));
    }

    #[cfg(debug_assertions)]
    pub fn listener_count(&self) -> usize {
        self.listeners.len()
    }
}

impl<T> Default for Probe<T> {
    fn default() -> Self { Self::new() }
}

unsafe impl<T> Send for Probe<T> {}
unsafe impl<T> Sync for Probe<T> {}
```

Key design points:

- `has_listeners()` returns `const false` in release — the compiler eliminates `if
  probe.has_listeners()` blocks entirely.
- `subscribe()` is absent in release (`#[cfg(debug_assertions)]`). Calling it in a
  release build is a **compile error**, not a silent no-op.
- `notify()` always takes `val: &T` and does `let _ = val` to silence unused warnings
  in release. The loop body is `#[cfg(debug_assertions)]` only.
- `PhantomData<fn(&T)>` makes `Probe<T>` covariant in `T` (listeners take `&T`).
- `unsafe impl Send + Sync` is correct: the closure bound `Send + Sync` on listeners
  ensures thread safety in dev; in release there are no non-PhantomData fields.

---

## 4. `src/macros.rs` — `probe!()` macro

```rust
#[macro_export]
macro_rules! probe {
    ($probe:expr, $val:expr) => {
        if $probe.has_listeners() {
            $probe.notify(&{ $val });
        }
    };
}
```

- **Release**: `has_listeners()` returns `const false`. The compiler removes the entire
  `if` block. `$val` is never evaluated (dead code elimination).
- **Dev**: `$val` is evaluated only inside the `if` body, so expensive event construction
  (register file snapshot, symbol lookups) is skipped when no subscribers are attached.
- The `{ $val }` block form means `$val` can be a struct literal with trailing commas
  or a multi-statement block — both work correctly.

---

## 5. `src/events.rs` — Event types

All event types are `#[derive(Debug, Clone)]`.

### `CpuStepEvent`

```rust
pub struct CpuStepEvent {
    pub pc:  u64,
    pub raw: u32,
    #[cfg(feature = "probe-full")]
    pub insn_count: u64,
}
```

`raw` is `0` on `pre_step` probes (instruction not yet fetched). On `post_step` it is
the actual 32-bit instruction word.

### `CpuFaultEvent`

```rust
pub struct CpuFaultEvent {
    pub pc:   u64,
    pub raw:  u32,
    pub kind: &'static str,
}
```

`kind` values used by the FS step loop: `"insn-abort"`, `"data-abort"`,
`"store-abort"`, `"svc"`.

### `MemAccessEvent`

```rust
pub struct MemAccessEvent {
    pub addr:     u64,
    pub size:     u8,
    pub is_store: bool,
    pub pc:       u64,
}
```

Fired from SE mode via `InstrumentedMem`. Not fired in FS mode (`TranslatingMem` does
not record accesses).

### `BranchEvent` and `BranchKind`

```rust
pub struct BranchEvent {
    pub pc:     u64,
    pub target: u64,
    pub taken:  bool,
    pub kind:   BranchKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchKind {
    DirectCond,
    DirectUncond,
    Call,
    Return,
    IndirectJump,
    IndirectCall,
}
```

Replaces `sim_branch!()`. Zero cost in release (ZST probe). Fired from `step_aarch64()`
in `lib.rs` after execute when `insn.is_branch()` is true.

### `IrqEvent`

```rust
pub struct IrqEvent {
    pub irq_id:   u32,
    pub asserted: bool,
}
```

Fired from GIC distributor methods when feature `"probe"` is enabled on helm-hw-intc.

### `MmioEvent`

```rust
pub struct MmioEvent {
    pub addr:     u64,
    pub size:     u8,
    pub val:      u64,
    pub is_write: bool,
}
```

Defined but **not yet wired** to any `probe!()` call sites. Planned for Phase 2
(instrument `SystemMem` dispatch path).

---

## 6. `src/lib.rs` — Probe bundles

The crate root defines the two probe bundle structs and re-exports all event types.

```rust
pub use events::{
    BranchEvent, BranchKind, CpuFaultEvent, CpuStepEvent, IrqEvent, MemAccessEvent, MmioEvent,
};
pub use probe::Probe;

pub struct CpuProbes {
    pub pre_step:  Probe<CpuStepEvent>,
    pub post_step: Probe<CpuStepEvent>,
    pub fault:     Probe<CpuFaultEvent>,
    pub mem:       Probe<MemAccessEvent>,
    pub branch:    Probe<BranchEvent>,
}

pub struct GicProbes {
    pub irq_asserted:   Probe<IrqEvent>,
    pub irq_deasserted: Probe<IrqEvent>,
    pub eoi:            Probe<IrqEvent>,
}
```

Both structs implement `Default` via `Probe::new()` for all fields.

---

## 7. Wiring in `helm-engine`

### 7.1 `HelmEngine<T>` field (`runtime/helm-engine/src/lib.rs`)

```rust
pub struct HelmEngine<T: TimingModel> {
    // ... other fields ...
    pub probes: CpuProbes,
}
```

Initialised in `HelmEngine::new()` as `probes: CpuProbes::default()`.

### 7.2 SE step loop — `step_aarch64()` (`lib.rs`)

Probe insertion points in order:

1. **pre_step** — before fetch, with `raw: 0`:
   ```rust
   probe!(self.probes.pre_step, CpuStepEvent { pc, raw: 0 });
   ```

2. **mem** — inside the `InstrumentedMem` path, after execute, iterating recorded
   accesses:
   ```rust
   for rec in imem.recorded() {
       probe!(probes.mem, MemAccessEvent {
           addr: rec.vaddr, size: rec.size, is_store: rec.is_store, pc,
       });
   }
   ```
   This path is only taken when `self.plugins.has_mem_callbacks()` is true.

3. **post_step** — after execute (both instrumented and plain paths):
   ```rust
   probe!(self.probes.post_step, CpuStepEvent { pc, raw });
   ```

4. **branch** — after execute, when `insn.is_branch()` is true:
   ```rust
   if insn.is_branch() {
       let target = self.a64_state.as_ref().map(|s| s.pc).unwrap_or(pc.wrapping_add(4));
       probe!(self.probes.branch, BranchEvent {
           pc, target, taken: pc_written, kind: probe_branch_kind(insn.opcode),
       });
   }
   ```

**Note**: `fault` probe is **not wired** in the SE step loop. SE mode returns
`Err(HartException)` directly; the caller handles it without a fault probe. The fault
probe is FS-only.

### 7.3 FS step loop — `step_aarch64_fs()` (`runtime/helm-engine/src/fs.rs`)

The function signature takes `probes: &CpuProbes` as the 4th parameter:

```rust
pub fn step_aarch64_fs(
    a64:     &mut Aarch64ArchState,
    sys_mem: &mut SystemMem,
    fs:      &mut FsState,
    probes:  &CpuProbes,
) -> Result<(), HartException>
```

Probe insertion points in order:

1. **pre_step** — before MMU fetch translation:
   ```rust
   probe!(probes.pre_step, CpuStepEvent { pc, raw: 0 });
   ```

2. **fault (insn-abort)** — when MMU translate for fetch fails:
   ```rust
   probe!(probes.fault, CpuFaultEvent { pc, raw: 0, kind: "insn-abort" });
   ```

3. **post_step** — on successful execute:
   ```rust
   probe!(probes.post_step, CpuStepEvent { pc, raw });
   ```

4. **fault (data-abort, store-abort, svc)** — on execute errors that are delivered to
   the guest exception vector:
   ```rust
   probe!(probes.fault, CpuFaultEvent { pc, raw, kind: "data-abort" });
   // or "store-abort" / "svc"
   ```

The caller (`HelmEngine::run()` system path) passes `&self.probes` to
`step_aarch64_fs()`.

---

## 8. GIC wiring (`hw/helm-hw-intc/src/gicv2/mod.rs`)

```rust
#[cfg(feature = "probe")]
use helm_probe::{probe, GicProbes, IrqEvent};

pub struct GicState {
    // ... register fields ...
    #[cfg(feature = "probe")]
    pub probes: GicProbes,
}

impl GicState {
    pub fn new(num_irqs: u32) -> Self {
        Self {
            // ...
            #[cfg(feature = "probe")]
            probes: GicProbes::default(),
        }
    }
}
```

GIC distributor methods fire `irq_asserted`, `irq_deasserted`, and `eoi` probes via
`probe!(state.probes.irq_asserted, IrqEvent { irq_id, asserted: true })` etc.

The helm-hw-intc `Cargo.toml` feature gate:
```toml
[dependencies]
helm-probe = { workspace = true, optional = true }

[features]
probe = ["helm-probe"]
```

---

## 9. ProbePluginBridge — PLANNED, NOT IMPLEMENTED

The `ProbePluginBridge` struct (Layer 1 → Layer 2 connector) is **designed** in this
document for completeness, but it does **not exist** in the current source code.

When implemented (Phase 2), it will:
- Live in `framework/helm-plugin/src/bridge.rs` (or helm-spy equivalent)
- Subscribe to `CpuProbes.post_step` and enrich `CpuStepEvent { pc, raw }` into a
  richer `InsnInfo` type using `classify_aarch64_opcode(raw)` from `helm-engine`
- Subscribe to `CpuProbes.fault` and dispatch `FaultInfo` to the analysis registry
- Only be available in debug builds (`#[cfg(debug_assertions)]`)

---

## 10. Dependency wiring

### Workspace root `Cargo.toml`
```toml
[workspace.dependencies]
helm-probe = { path = "framework/helm-probe" }
```

### `runtime/helm-engine/Cargo.toml`
```toml
[dependencies]
helm-probe.workspace = true
```

### `hw/helm-hw-intc/Cargo.toml`
```toml
[dependencies]
helm-probe = { workspace = true, optional = true }

[features]
probe = ["helm-probe"]
```

---

## 11. `const fn` correctness

`Probe::new()` is `const fn`. `Vec::new()` is `const` since Rust 1.63 (2021 edition).
`PhantomData` is always `const`. This enables zero-cost static initialisation:

```rust
static DEAD_PROBE: Probe<CpuStepEvent> = Probe::new();
```

---

## 12. Release overhead verification

```bash
# Confirm Probe<T> is ZST in release (automated via probe_release.rs test):
cargo test -p helm-probe --test probe_release --release

# Inspect FS hot loop — expect zero probe-related symbols:
cargo asm --release --package helm-engine \
  "helm_engine::fs::step_aarch64_fs" | grep -c "probe"
# Expected: 0
```

---

## 13. Known limitations

| Limitation | Reason | Planned fix |
|---|---|---|
| `subscribe()` absent in release | By design — prevents silent discard | Intentional |
| `MemAccessEvent` not in FS mode | `TranslatingMem` doesn't record accesses | Phase 3: instrument SystemMem |
| `MmioEvent` defined but unwired | No call sites in SystemMem dispatch yet | Phase 2 |
| `fault` probe absent in SE mode | SE returns Err directly; caller handles | Phase 2 consideration |
| No per-listener name/metadata | Adds complexity | Phase 3: `named_subscribe(name, f)` |
| ProbePluginBridge not implemented | Phase 2 work | Phase 2 |
| Subscriptions not checkpointed | Intentional — matches HelmEventBus policy | Never |
