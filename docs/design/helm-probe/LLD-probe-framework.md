# helm-probe — Low-Level Design: Probe Framework

> See the [Instrumentation Stack HLD](HLD.md) for the architectural overview and
> how `helm-probe` connects to `helm-plugin` and `sim_trace`.

---

## 1. Crate Structure

```
framework/helm-probe/
├── Cargo.toml
└── src/
    ├── lib.rs          # crate root — re-exports, #[macro_export] re-export
    ├── probe.rs        # Probe<T> struct + impl
    ├── events.rs       # Standard event types
    └── macros.rs       # probe!() macro
```

No dependencies. Zero runtime cost in release. Deps on `helm-probe` are always
additive — adding it to a crate never increases release binary size.

---

## 2. `Cargo.toml`

```toml
[package]
name        = "helm-probe"
version.workspace = true
edition.workspace = true
description = "Zero-cost typed probe points for helm-ng instrumentation"

[lints]
workspace = true

[features]
# Richer event fields (insn_count on CpuStepEvent). Off by default.
probe-full = []
```

No `[dependencies]`.

---

## 3. `src/probe.rs` — Full Implementation

```rust
use std::marker::PhantomData;

/// A typed probe point. Zero-sized in release; holds listeners in dev.
///
/// # Build profile behaviour
///
/// | Profile | `has_listeners()` | `notify()` | `subscribe()` |
/// |---------|------------------|------------|---------------|
/// | release (`--release`) | `const false` | empty | absent (compile error) |
/// | dev (`cargo build`)   | `!vec.is_empty()` | iterates | available |
///
/// # Design note: `PhantomData<fn(&T)>`
///
/// Using `fn(&T)` (not `T` or `*const T`) makes `Probe<T>` covariant in `T`.
/// Listeners take `&T`, so covariance is correct. This allows:
///   `Probe<CpuStepEvent>` to be used where `Probe<impl StepLike>` is expected.
pub struct Probe<T> {
    #[cfg(debug_assertions)]
    listeners: Vec<Box<dyn Fn(&T) + Send + Sync>>,
    _marker: PhantomData<fn(&T)>,
}

impl<T> Probe<T> {
    /// Create a probe with no listeners. Usable in const context.
    pub const fn new() -> Self {
        Self {
            #[cfg(debug_assertions)]
            listeners: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// `true` iff at least one listener is subscribed.
    ///
    /// Release: const `false` → compiler eliminates `if probe.has_listeners()` blocks.
    /// Dev: `!vec.is_empty()` → one load + compare, predicted-not-taken.
    #[inline(always)]
    pub fn has_listeners(&self) -> bool {
        #[cfg(not(debug_assertions))]
        { false }
        #[cfg(debug_assertions)]
        { !self.listeners.is_empty() }
    }

    /// Deliver event to all listeners. No-op in release (empty body, inlined away).
    #[inline(always)]
    pub fn notify(&self, val: &T) {
        #[cfg(debug_assertions)]
        for l in &self.listeners {
            l(val);
        }
    }

    /// Subscribe a listener closure.
    ///
    /// Only available in debug builds. In release, calling this method is a
    /// **compile error** (`method not found`), preventing subscriptions that
    /// would be silently discarded.
    #[cfg(debug_assertions)]
    pub fn subscribe(&mut self, f: impl Fn(&T) + Send + Sync + 'static) {
        self.listeners.push(Box::new(f));
    }

    /// Number of registered listeners. Returns 0 in release.
    #[cfg(debug_assertions)]
    pub fn listener_count(&self) -> usize {
        self.listeners.len()
    }
}

impl<T> Default for Probe<T> {
    fn default() -> Self { Self::new() }
}

// SAFETY: Vec<Box<dyn Fn(&T) + Send + Sync>> is Send + Sync because the
// closure bound `Send + Sync` makes the boxed closures thread-safe. In
// release there are no fields. PhantomData<fn(&T)> is not auto-Send/Sync
// (raw fn ptrs have no threading guarantee), so we impl manually.
unsafe impl<T> Send for Probe<T> {}
unsafe impl<T> Sync for Probe<T> {}
```

---

## 4. `src/macros.rs` — `probe!()` Macro

```rust
/// Fire a probe, constructing the event value only when listeners exist.
///
/// ```text
/// probe!($probe_expr, $event_expr)
/// ```
///
/// **Release build**: expands to nothing — zero instructions, zero branches,
/// `$event_expr` is never evaluated (dead code elimination).
///
/// **Dev build**: expands to:
/// ```rust
/// if $probe_expr.has_listeners() {
///     $probe_expr.notify(&{ $event_expr });
/// }
/// ```
///
/// The `{ $event_expr }` block means the expression is not evaluated unless
/// `has_listeners()` is true. Expensive data (e.g. register file snapshot,
/// symbol lookup) is constructed only when a subscriber is registered.
///
/// # Example
/// ```rust
/// probe!(self.probes.pre_step, CpuStepEvent { pc: a64.pc, raw: 0 });
/// ```
#[macro_export]
macro_rules! probe {
    ($probe:expr, $val:expr) => {
        if $probe.has_listeners() {
            $probe.notify(&{ $val });
        }
    };
}
```

---

## 5. `src/events.rs` — Standard Event Types

```rust
/// Emitted before or after each instruction step.
///
/// `raw` may be `0` on `pre_step` probes (instruction not yet fetched).
#[derive(Debug, Clone)]
pub struct CpuStepEvent {
    /// PC of the instruction (virtual address).
    pub pc:  u64,
    /// Raw 32-bit instruction word (little-endian).
    pub raw: u32,
    /// Monotonic instruction retirement count (only with `--features probe-full`).
    #[cfg(feature = "probe-full")]
    pub insn_count: u64,
}

/// Emitted when a handled guest exception is delivered.
///
/// "Handled" = exception was delivered to a guest exception vector (EL1).
/// The simulation does not stop. Use to trace TLB misses, SVC entry, etc.
///
/// `kind` values used by the FS step loop:
/// - `"insn-abort"` — instruction fetch translation fault
/// - `"data-abort"` — load translation fault
/// - `"store-abort"` — store translation fault
/// - `"svc"` — supervisor call (EL0 → EL1 exception)
#[derive(Debug, Clone)]
pub struct CpuFaultEvent {
    pub pc:   u64,
    pub raw:  u32,
    pub kind: &'static str,
}

/// Emitted for each data memory access (SE mode, via `InstrumentedMem`).
///
/// Not emitted in FS mode (TranslatingMem does not record accesses).
#[derive(Debug, Clone)]
pub struct MemAccessEvent {
    pub addr:     u64,
    pub size:     u8,
    pub is_store: bool,
    pub pc:       u64,
}

/// Emitted on every branch instruction (taken or not).
///
/// **Instrumentation-v2**: replaces `sim_branch!()`. Zero cost in release.
/// `BranchKind` is re-exported by `helm-spy` for use in analysis models.
///
/// Call sites: `aarch64/execute/branch.rs` — fires `probe!(probes.branch, BranchEvent{...})`
/// instead of `sim_branch!(pc=pc, target=target)`.
#[derive(Debug, Clone)]
pub struct BranchEvent {
    pub pc:     u64,
    pub target: u64,
    pub taken:  bool,
    pub kind:   BranchKind,
}

/// Branch type classification. Defined here; re-exported by `helm-spy::events`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchKind {
    DirectCond,
    DirectUncond,
    Call,
    Return,
    IndirectJump,
    IndirectCall,
}

/// Emitted when an interrupt line changes state in the GIC.
#[derive(Debug, Clone)]
pub struct IrqEvent {
    pub irq_id:   u32,
    pub asserted: bool,
}

/// Emitted on MMIO device register read or write.
#[derive(Debug, Clone)]
pub struct MmioEvent {
    pub addr:     u64,
    pub size:     u8,
    pub val:      u64,
    pub is_write: bool,
}
```

---

## 6. `src/lib.rs` — Crate Root

```rust
mod probe;
mod events;
mod macros;

pub use probe::Probe;
pub use events::{CpuFaultEvent, CpuStepEvent, IrqEvent, MemAccessEvent, MmioEvent};
// probe!() re-exported by #[macro_export] in macros.rs — available as helm_probe::probe!
```

---

## 7. Probe Bundles

### 7.1 `CpuProbes` (in `helm-engine/src/lib.rs`)

```rust
use helm_probe::{Probe, CpuFaultEvent, CpuStepEvent, MemAccessEvent};

pub struct CpuProbes {
    /// Before fetch, each instruction cycle. `raw` is 0 (not yet fetched).
    pub pre_step:  Probe<CpuStepEvent>,
    /// After execute succeeds. `pc` is the instruction PC (before advance).
    pub post_step: Probe<CpuStepEvent>,
    /// When a handled guest exception is delivered (fault, SVC, abort).
    pub fault:     Probe<CpuFaultEvent>,
    /// Per data memory access (SE mode only).
    pub mem:       Probe<MemAccessEvent>,
    /// Every branch instruction — replaces `sim_branch!()` (Instrumentation-v2).
    pub branch:    Probe<BranchEvent>,
}

impl Default for CpuProbes {
    fn default() -> Self {
        Self {
            pre_step:  Probe::new(),
            post_step: Probe::new(),
            fault:     Probe::new(),
            mem:       Probe::new(),
        }
    }
}
```

Field on `HelmEngine<T>`: `pub probes: CpuProbes`.
Initialised in `HelmEngine::new()`: `probes: CpuProbes::default()`.

### 7.2 `GicProbes` (in `helm-hw-intc`, feature `probe`)

```rust
#[cfg(feature = "probe")]
use helm_probe::{Probe, IrqEvent};

#[cfg(feature = "probe")]
pub struct GicProbes {
    /// SPI/PPI became pending (ISPENDR write or device assert).
    pub irq_asserted:   Probe<IrqEvent>,
    /// Pending IRQ cleared (ICPENDR write).
    pub irq_deasserted: Probe<IrqEvent>,
    /// CPU acknowledged an IRQ via IAR; EOI written to EOIR.
    pub eoi:            Probe<IrqEvent>,
}
```

Field on `GicState`: `#[cfg(feature = "probe")] pub probes: GicProbes`.

---

## 8. FS Step Loop Wiring (`runtime/helm-engine/src/fs.rs`)

`step_aarch64_fs()` receives `probes: &CpuProbes` as an additional parameter. The
`CpuProbes` reference is passed from `HelmEngine::step_aarch64_system()` as `&self.probes`.

Insertion points (in step order):

```rust
pub fn step_aarch64_fs(
    a64:    &mut Aarch64ArchState,
    sys_mem: &mut SystemMem,
    fs:     &mut FsState,
    probes: &CpuProbes,   // ← added
) -> Result<(), HartException> {

    // 1. IRQ check (unchanged) …

    let pc = a64.pc;

    // ── PRE-STEP ────────────────────────────────────────────────────────────
    probe!(probes.pre_step, CpuStepEvent { pc, raw: 0 });

    // 2. Fetch …
    let fetch_pa = match mmu::translate(a64, pc, MmuAccess::Execute, sys_mem) {
        Ok(r) => r.pa,
        Err(_fault) => {
            probe!(probes.fault, CpuFaultEvent { pc, raw: 0, kind: "insn-abort" });
            exception::exception_entry(a64, …);
            return Ok(());
        }
    };

    let raw = sys_mem.read(fetch_pa, 4, AccessType::Fetch)? as u32;

    // 3. Decode + 4. Snapshot MMU + 5. Execute (unchanged) …

    match exec_result {
        Ok(pc_written) => {
            // ── POST-STEP ───────────────────────────────────────────────────
            probe!(probes.post_step, CpuStepEvent { pc, raw });
            if !pc_written { a64.pc = a64.pc.wrapping_add(4); }
        }
        Err(HartException::LoadAccessFault { .. }) => {
            probe!(probes.fault, CpuFaultEvent { pc, raw, kind: "data-abort" });
            exception::exception_entry(a64, …);
        }
        Err(HartException::StoreAccessFault { .. }) => {
            probe!(probes.fault, CpuFaultEvent { pc, raw, kind: "store-abort" });
            exception::exception_entry(a64, …);
        }
        Err(HartException::EnvironmentCall { .. }) => {
            probe!(probes.fault, CpuFaultEvent { pc, raw, kind: "svc" });
            exception::exception_entry(a64, …);
        }
        Err(e) => return Err(e),
    }

    fs.tick += 1;
    Ok(())
}
```

---

## 9. ProbePluginBridge (in `helm-plugin`)

This is the Layer 1 → Layer 2 connector (see HLD §5.1). It lives in
`framework/helm-plugin/src/bridge.rs`.

```rust
use helm_probe::{CpuStepEvent, CpuFaultEvent, MemAccessEvent};
use crate::runtime::{InsnInfo, FaultInfo, MemInfo, PluginRegistry, ArchContext};
use helm_engine::classify_aarch64_opcode;

/// Subscribes to probe bundles and dispatches enriched events to a PluginRegistry.
///
/// Construct one, then call `install()` to wire the subscriptions.
/// The bridge must outlive the probes it subscribes to.
pub struct ProbePluginBridge {
    registry: std::sync::Arc<std::sync::Mutex<PluginRegistry>>,
}

impl ProbePluginBridge {
    pub fn new(registry: std::sync::Arc<std::sync::Mutex<PluginRegistry>>) -> Self {
        Self { registry }
    }

    /// Subscribe to all `CpuProbes`. Call once during simulation build.
    #[cfg(debug_assertions)]
    pub fn install_cpu(&self, probes: &mut helm_engine::CpuProbes, vcpu_idx: usize) {
        let reg = self.registry.clone();
        probes.post_step.subscribe(move |ev: &CpuStepEvent| {
            let (class, name, is_stub) = classify_aarch64_opcode(ev.raw);
            let info = InsnInfo {
                pc: ev.pc,
                raw: ev.raw,
                size: 4,
                class,
                opcode_name: name,
                is_stub,
                context: ArchContext::None,  // cheap default; regs opt-in via probe-full
            };
            if let Ok(r) = reg.lock() {
                if r.has_insn_callbacks() {
                    r.fire_insn_exec(vcpu_idx, &info);
                }
            }
        });

        let reg = self.registry.clone();
        probes.fault.subscribe(move |ev: &CpuFaultEvent| {
            let info = FaultInfo {
                vcpu_idx,
                pc: ev.pc,
                raw: ev.raw,
                kind: crate::runtime::fault_kind_from_str(ev.kind),
                message: ev.kind.to_string(),
                insn_count: 0,
                context: ArchContext::None,
            };
            if let Ok(r) = reg.lock() {
                r.fire_fault(&info);
            }
        });
    }
}
```

**Lifecycle**: `ProbePluginBridge` is constructed during `build_simulator()` (Python API)
or at CLI startup. It is installed before `run()`. It is not checkpointed.

---

## 10. Dependency Wiring

### Root `Cargo.toml`
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

### `runtime/helm-engine/Cargo.toml` (GIC probe feature)
```toml
helm-hw-intc = { workspace = true, features = ["probe"] }
```

### `runtime/helm-arch/Cargo.toml`
```toml
[dependencies]
helm-probe = { workspace = true, optional = true }

[features]
probe = ["helm-probe"]
```

---

## 11. `const fn` Correctness

`Probe::new()` is `const fn`. `Vec::new()` is `const` since Rust 1.63 (2021 edition).
The `PhantomData` field is always `const`. Both paths compile correctly.

This enables zero-cost static initialisation:
```rust
static DEAD_PROBE: Probe<CpuStepEvent> = Probe::new();
```

---

## 12. Release Overhead Verification

After wiring, verify zero overhead with `cargo-asm`:

```bash
# Release build
cargo build --release --workspace

# Inspect hot loop — expect zero "probe"-related symbols
cargo asm --release --package helm-engine \
  "helm_engine::fs::step_aarch64_fs" | grep -c "probe"
# Expected: 0

# Confirm Probe<T> is ZST
cargo run --release --example check_probe_size
# Expected: "size_of::<Probe<u64>>() = 0"
```

---

## 13. Known Limitations and Non-Goals

| Limitation | Reason | Planned fix |
|---|---|---|
| `subscribe()` absent in release | By design — prevents silent discard | Intentional |
| `MemAccessEvent` not in FS mode | `TranslatingMem` doesn't record accesses | Phase 3: instrument `SystemMem` |
| No per-listener name/metadata | Adds complexity | Phase 3: `named_subscribe(name, f)` |
| No async delivery | Probes are synchronous | Phase 3: EventQueue bridge |
| No probe hit counters | `probe-full` scope limited | Phase 3: AtomicU64 per probe |
| Subscriptions not checkpointed | Intentional — matches HelmEventBus policy | Never |
| Bridge Mutex on hot path | Unavoidable until Arc<RwLock> refactor | Phase 2: use RwLock + per-vcpu registry |
