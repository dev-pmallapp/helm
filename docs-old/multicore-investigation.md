# Multi-Core / vCPU Support — Investigation

## 1  Goal

Enable HELM to boot and run SMP Linux kernels with N virtual CPUs,
matching the `--smp N` flag that `helm-system-aarch64` already accepts
but does not implement.

## 2  Current State

### What exists

| Component | Multi-core readiness | Notes |
|-----------|---------------------|-------|
| **DTB generation** (`helm-device/src/fdt.rs`) | ✅ Ready | Already emits N `/cpus/cpu@N` nodes with `enable-method = "psci"`. |
| **GIC Redistributor** (`helm-device/src/arm/gic/redistributor.rs`) | ✅ Per-PE | Models one redistributor per PE (SGI/PPI state). Stride = 0x20000. |
| **GIC Distributor** (`helm-device/src/arm/gic/distributor.rs`) | ⚠️ Partial | Tracks IRQ routing but `GICD_ITARGETSR` / affinity routing is not multi-PE aware. |
| **KVM vCPU** (`helm-kvm/src/vcpu.rs`) | ✅ Ready | `KvmVcpu` is per-fd, `KvmVm::create_vcpu(id)` already takes a vCPU index. |
| **Plugin API** (`helm-plugin/src/runtime/`) | ✅ Ready | All callbacks carry `vcpu_idx: usize`; `fire_vcpu_init(idx)` called per core. |
| **IrqSignal** (`helm-core`) | ❌ Single | One `Arc<AtomicBool>` — no per-PE IRQ line. |
| **Aarch64Cpu** (`helm-isa`) | ❌ Single | Singleton CPU struct; no `cpu_id` / `MPIDR` per-instance differentiation at construction. |
| **AddressSpace** (`helm-memory`) | ⚠️ Shared-unsafe | `&mut self` on `read`/`write` prevents concurrent access from multiple vCPUs. |
| **FsSession** (`helm-engine`) | ❌ Single CPU | Owns one `Aarch64Cpu`, one `AddressSpace`, runs a single-threaded loop. |
| **SE Scheduler** (`helm-engine/src/se/thread.rs`) | ✅ Cooperative | Green-thread scheduler for `clone(CLONE_THREAD)` — not real multi-core, but shows the pattern. |
| **CLI** (`helm-cli`) | ⚠️ Parses `--smp` | Passes `num_cpus` to DTB but creates only one CPU. |

### What's missing

1. **vCPU abstraction** — a `Vcpu` struct that bundles per-core state.
2. **Per-vCPU IRQ lines** — each PE needs its own IRQ signal.
3. **Shared memory with interior mutability** — `AddressSpace` must
   support concurrent reads/writes from N vCPUs.
4. **PSCI CPU_ON handler** — the kernel brings up secondary cores via
   `HVC #0` with PSCI `CPU_ON` function ID.
5. **vCPU thread pool** — one OS thread per vCPU, running in lockstep
   or free-running with synchronization barriers.
6. **Cross-core SGI delivery** — software-generated interrupts for
   IPI (inter-processor interrupt).

## 3  Proposed Architecture

### 3.1  vCPU Abstraction

```text
┌─────────────────────────────────────────────────┐
│                   FsSession                      │
│                                                  │
│  vcpus: Vec<Vcpu>        (one per --smp core)    │
│  memory: SharedMemory    (Arc<RwLock> or atomic)  │
│  gic: SharedGic          (Arc<Mutex>)            │
│  plugin_reg: PluginRegistry                      │
│                                                  │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐         │
│  │  Vcpu 0  │ │  Vcpu 1  │ │  Vcpu N  │         │
│  │  (BSP)   │ │  (AP)    │ │  (AP)    │         │
│  │          │ │          │ │          │         │
│  │ cpu:     │ │ cpu:     │ │ cpu:     │         │
│  │  Aarch64 │ │  Aarch64 │ │  Aarch64 │         │
│  │  Cpu     │ │  Cpu     │ │  Cpu     │         │
│  │          │ │          │ │          │         │
│  │ irq:     │ │ irq:     │ │ irq:     │         │
│  │  IrqLine │ │  IrqLine │ │  IrqLine │         │
│  │          │ │          │ │          │         │
│  │ backend: │ │ backend: │ │ backend: │         │
│  │  ExecBe  │ │  ExecBe  │ │  ExecBe  │         │
│  │          │ │          │ │          │         │
│  │ state:   │ │ state:   │ │ state:   │         │
│  │ Running  │ │ WaitPSCI │ │ WaitPSCI │         │
│  └──────────┘ └──────────┘ └──────────┘         │
└─────────────────────────────────────────────────┘
```

```rust
/// Per-vCPU state.
pub struct Vcpu {
    /// vCPU index (0 = BSP, 1..N = APs).
    pub id: usize,
    /// AArch64 CPU model with per-core MPIDR.
    pub cpu: Aarch64Cpu,
    /// Per-PE IRQ signal (connected to GIC redistributor).
    pub irq: IrqSignal,
    /// Execution backend (interp / TCG / JIT) — per-vCPU to avoid
    /// contention on block caches.
    pub backend: ExecBackend,
    /// Lifecycle state.
    pub state: VcpuState,
    /// Per-vCPU instruction counter.
    pub insn_count: u64,
    pub virtual_cycles: u64,
}

pub enum VcpuState {
    /// CPU is executing instructions.
    Running,
    /// CPU is in WFI — waiting for an interrupt.
    WaitForInterrupt,
    /// CPU has not been started yet (secondary cores before PSCI CPU_ON).
    PoweredOff,
    /// CPU has been halted (PSCI CPU_OFF or fatal error).
    Halted,
}
```

### 3.2  Memory Model

All vCPUs share one physical address space.  The current `AddressSpace`
takes `&mut self` for reads (because the I/O handler needs `&mut`).
Two options:

**Option A — `Arc<Mutex<AddressSpace>>`** (simplest)
- Lock the address space for each memory access.
- Acceptable for interpretive mode (~10 ns lock overhead vs ~200 ns
  per instruction).
- Bottleneck under heavy shared-memory workloads.

**Option B — Split RAM / MMIO** (recommended)
- RAM: `Arc<Vec<u8>>` with atomic load/store (or `UnsafeCell` + manual
  ordering) — lock-free concurrent access.
- MMIO: `Arc<Mutex<DeviceBus>>` — serialized device access (matches
  real hardware behavior).
- `read_ram` / `write_ram` bypass the lock entirely; only MMIO
  accesses contend.

```rust
pub struct SharedMemory {
    /// Guest RAM — lock-free concurrent access.
    ram: Arc<RamRegion>,
    /// Device MMIO bus — serialized.
    io: Arc<Mutex<DeviceBus>>,
}
```

### 3.3  IRQ Routing

Replace the single `IrqSignal` with per-PE IRQ lines:

```rust
/// Per-PE IRQ line bundle.
pub struct VcpuIrqLines {
    /// IRQ (Group 1 Non-Secure) — normal interrupts.
    pub irq: IrqSignal,
    /// FIQ (Group 0 / Secure) — fast interrupts.
    pub fiq: IrqSignal,
}
```

The GIC distributor routes SPIs to specific PEs via affinity routing
(GICv3 `GICD_IROUTERn`).  SGIs (0–15) are inherently per-PE and
delivered through the redistributor.

### 3.4  PSCI Implementation

Linux brings up secondary cores via PSCI `CPU_ON` (function ID
`0xC4000003`).  The kernel issues `HVC #0` with:
- X0 = function ID (CPU_ON)
- X1 = target MPIDR
- X2 = entry point
- X3 = context ID (passed to secondary in X0)

HELM must:
1. Intercept `HVC #0` exits (already generates `InterpExit::Exception`
   with class 0x16).
2. Match X0 to PSCI function IDs.
3. For `CPU_ON`: find the target vCPU by MPIDR, set its PC to the
   entry point, set X0 = context ID, transition state to `Running`.

```rust
fn handle_psci(vcpus: &mut [Vcpu], caller: usize, func: u64, args: [u64; 3]) -> u64 {
    match func {
        PSCI_CPU_ON => {
            let target_mpidr = args[0];
            let entry = args[1];
            let ctx = args[2];
            if let Some(vcpu) = vcpus.iter_mut().find(|v| v.cpu.regs.mpidr_el1 == target_mpidr) {
                vcpu.cpu.regs.pc = entry;
                vcpu.cpu.set_xn(0, ctx);
                vcpu.state = VcpuState::Running;
                PSCI_SUCCESS
            } else {
                PSCI_INVALID_PARAMETERS
            }
        }
        PSCI_CPU_OFF => {
            vcpus[caller].state = VcpuState::Halted;
            PSCI_SUCCESS
        }
        PSCI_SYSTEM_OFF => std::process::exit(0),
        PSCI_SYSTEM_RESET => std::process::exit(1),
        PSCI_VERSION => 0x10000, // PSCI 1.0
        PSCI_FEATURES => PSCI_SUCCESS,
        _ => PSCI_NOT_SUPPORTED,
    }
}
```

### 3.5  Execution Model

**Phase 1 — Sequential round-robin** (no OS threads)

Run each vCPU for a quantum (e.g. 1024 instructions), then switch to
the next.  Simple, deterministic, debuggable.  Sufficient for
functional correctness and early boot.

```rust
loop {
    for vcpu in &mut vcpus {
        if vcpu.state == VcpuState::Running {
            vcpu.run(quantum, &shared_mem, &gic);
        }
    }
}
```

**Phase 2 — Parallel vCPU threads**

Spawn one `std::thread` per vCPU.  Each thread owns its `Vcpu` and
runs independently.  Synchronize at barriers (WFE/SEV, MMIO
serialization, plugin callbacks).

```rust
let vcpu_threads: Vec<JoinHandle<()>> = vcpus
    .into_iter()
    .map(|mut vcpu| {
        let mem = shared_mem.clone();
        let gic = shared_gic.clone();
        std::thread::spawn(move || {
            vcpu.run_loop(&mem, &gic);
        })
    })
    .collect();
```

**Phase 3 — KVM vCPUs** (existing infrastructure)

`helm-kvm` already has `KvmVcpu` with per-fd run loops.  Each KVM
vCPU runs on its own thread with `KVM_RUN`.  MMIO exits are forwarded
to the shared device bus.  This path gives near-native SMP performance.

## 4  Implementation Plan

### Step 1: Introduce `Vcpu` struct
- Create `crates/helm-engine/src/vcpu.rs`.
- Move per-CPU fields out of `FsSession` into `Vcpu`.
- `FsSession` owns `Vec<Vcpu>` (initially length 1 — no behavior change).
- Each `Vcpu` gets a unique `MPIDR_EL1` value: `0x80000000 | cpu_id`.

### Step 2: Per-vCPU IRQ signals
- Change `IrqSignal` to carry a PE index, or create `Vec<IrqSignal>`.
- Wire each redistributor to its vCPU's signal.
- GIC distributor routes SPIs based on `GICD_IROUTERn` affinity.

### Step 3: PSCI CPU_ON
- Intercept HVC with PSCI function IDs in the execution loop.
- Implement `CPU_ON`, `CPU_OFF`, `SYSTEM_OFF`, `SYSTEM_RESET`,
  `PSCI_VERSION`, `PSCI_FEATURES`.
- Secondary vCPUs start in `PoweredOff` state; `CPU_ON` transitions
  them to `Running`.

### Step 4: Round-robin scheduling
- `FsSession::run_inner` iterates over `vcpus` and runs each
  `Running` vCPU for a quantum.
- Timer and IRQ checks happen per-vCPU.

### Step 5: Shared memory
- Split `AddressSpace` into RAM (lock-free) + MMIO (mutex).
- vCPUs access RAM concurrently; MMIO serialized.

### Step 6: SGI / IPI delivery
- `ICC_SGI1R_EL1` write (system register) triggers cross-core SGI.
- GIC redistributor sets pending bit on target PE and raises its
  IRQ signal.

### Step 7: Parallel execution (optional)
- One OS thread per vCPU.
- Use `crossbeam` epoch or `parking_lot` for synchronization.
- WFE/SEV mapped to `thread::park` / `thread::unpark`.

## 5  Key Design Decisions

| Decision | Recommendation | Rationale |
|----------|---------------|-----------|
| Memory sharing | Split RAM/MMIO (Option B) | Lock-free RAM is critical for IPC performance |
| Initial scheduling | Round-robin (Phase 1) | Simplest correct implementation; upgrade later |
| MPIDR assignment | `0x80000000 \| cpu_id` | Matches QEMU virt; bit 31 = multiprocessor flag |
| TCG block caches | Per-vCPU | Avoids lock contention; caches are hot per-core anyway |
| Timer model | Per-vCPU CNTVCT | Each vCPU tracks its own virtual counter |
| Plugin callbacks | Existing `vcpu_idx` | Already designed for multi-core; no API change needed |

## 6  Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Memory ordering bugs | Correctness | Start with sequential scheduling; add `atomic` ops incrementally |
| GIC complexity | Boot failure | Test with `--smp 2` first; most kernels tolerate minimal GIC |
| Performance regression (locking) | Slowdown | Profile before/after; lock-free RAM should be neutral |
| PSCI edge cases | Secondary CPU hang | Compare behavior with QEMU `--smp` reference |
| TLB coherency | Stale translations | Broadcast TLBI to all vCPUs; flush TLB on context switch |

## 7  Testing Strategy

1. **Unit**: `Vcpu` construction, MPIDR assignment, PSCI handler.
2. **Integration**: Boot `vmlinuz-lts --smp 2`, verify both CPUs
   appear in `/proc/cpuinfo`.
3. **Stress**: Run multi-threaded guest workloads (e.g. `stress-ng
   --cpu 2`) and check for hangs/crashes.
4. **Regression**: Existing single-core tests must pass unchanged
   (Vec<Vcpu> of length 1 is transparent).

## 8  References

- ARM PSCI specification: `DEN0022D` (power state coordination interface)
- GICv3 Architecture: `IHI0069`
- QEMU `hw/arm/virt.c` — reference SMP boot sequence
- Linux `arch/arm64/kernel/psci.c` — kernel-side PSCI calls
- Linux `arch/arm64/kernel/smp.c` — secondary CPU bringup
