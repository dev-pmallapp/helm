# KVM Mode Design

## Overview

KVM mode adds a fourth execution backend to HELM that delegates guest
CPU execution to the host Linux KVM hypervisor.  The guest runs at
near-native speed while HELM retains full control of the device model,
memory map, and interrupt delivery.  This makes KVM the ideal backend
for the boot-and-checkpoint workflow described in `execution-modes.md`
§2.5 and for mixed-fidelity simulation where the CPU is fast but
attached devices (especially LLVM-IR accelerators) are simulated at
cycle-accurate detail.

### Where KVM Fits

```
ExecBackend (orthogonal to SE / FS and FE / APE / CAE):

  Interpretive   — Aarch64Cpu.step(), one insn at a time
  Tcg            — translate to TcgBlock, re-execute from cache
  Kvm            — delegate to /dev/kvm, exit on MMIO / IRQ / HLT
```

KVM is applicable **only when the host ISA matches the guest ISA**
(AArch64-on-AArch64 today).  It always runs at FE-equivalent timing
(IPC ≈ native hardware) but can be paired with cycle-accurate device
models and LLVM-IR accelerators.

### Motivating Use Cases

| Use case | Why KVM | Timing detail |
|----------|---------|---------------|
| Fast OS boot for FS-mode checkpointing | Native speed → seconds not hours | CPU: none; devices: FE |
| Mixed-fidelity accelerator research | CPU not the bottleneck; accelerator is | CPU: KVM; accel: CAE |
| Driver development with real peripherals | MMIO exits give full device control | CPU: KVM; devices: CAE |
| Rapid iteration on device tree / platform bring-up | Boot kernel, probe devices, check dmesg | CPU: KVM; devices: FE |

---

## 1. Architecture

### 1.1 Component Map

```
┌──────────────────────────────────────────────────────────┐
│                      FsSession                           │
│                                                          │
│  ┌────────────┐    ┌──────────────┐    ┌──────────────┐  │
│  │  KvmVcpu   │    │  AddressSpace│    │   Platform    │  │
│  │            │◄──►│  (guest RAM) │    │              │  │
│  │ /dev/kvm   │    │  + IoHandler │◄──►│  DeviceBus   │  │
│  │ ioctl()    │    │              │    │  IrqRouter   │  │
│  └─────┬──────┘    └──────────────┘    │  GIC / PL011 │  │
│        │                               │  Accelerator │  │
│        │  VM_EXIT_MMIO                  │  (LLVM-IR)   │  │
│        │  VM_EXIT_IRQ                   └──────────────┘  │
│        │  VM_EXIT_SHUTDOWN                                │
│        ▼                                                  │
│  ┌────────────────┐                                       │
│  │ exit_dispatch() │──► mmio → DeviceBus.transact()       │
│  │                 │──► irq  → GIC inject                 │
│  │                 │──► halt → WFI idle loop               │
│  └────────────────┘                                       │
└──────────────────────────────────────────────────────────┘
```

### 1.2 New Crate: `helm-kvm`

A new crate `crates/helm-kvm/` owns all `/dev/kvm` interactions:

```
helm-kvm/
  src/
    lib.rs          — public API, feature gate
    capability.rs   — probe /dev/kvm caps (VFP, GIC, etc.)
    vm.rs           — KvmVm: create VM fd, set memory regions
    vcpu.rs         — KvmVcpu: create vCPU, get/set regs, run loop
    exit.rs         — VmExit enum, MMIO / IRQ / shutdown dispatch
    memory.rs       — guest memory region setup (mmap + KVM_SET_USER_MEMORY_REGION)
    irq.rs          — KVM_IRQ_LINE / KVM_SIGNAL_MSI wrappers
    error.rs        — KvmError (thiserror)
    tests/
      capability.rs
      vm.rs
      vcpu.rs
```

**Dependencies:** `helm-core` (types, errors), `libc` (ioctl), `nix`
(optional, for cleaner ioctl wrappers).  No dependency on `helm-isa` —
the host CPU does the decoding.

**Feature gate:** `helm-kvm` is behind `features = ["kvm"]` in the
workspace.  On non-Linux or non-AArch64 hosts it compiles to a stub
that returns `KvmError::Unsupported`.

### 1.3 KVM ↔ HELM Boundary

| Responsibility | Owner |
|---------------|-------|
| Instruction fetch + decode + execute | KVM (host hardware) |
| Guest RAM backing | HELM (`AddressSpace` mmap'd regions) |
| MMIO dispatch | HELM (`DeviceBus`) via `KVM_EXIT_MMIO` |
| Interrupt injection | HELM (`GIC` model) via `KVM_IRQ_LINE` |
| Timer | Host hardware (EL2 timer) or HELM `ArmGenericTimer` |
| Exception level transitions | KVM |
| Page table walks | KVM (stage-2 translation) |
| Device state / checkpoint | HELM |

---

## 2. KVM Backend Implementation

### 2.1 ExecBackend Extension

```rust
// crates/helm-engine/src/se/backend.rs
pub enum ExecBackend {
    Interpretive,
    Tcg { cache: HashMap<Addr, TcgBlock>, interp: TcgInterp },
    #[cfg(feature = "kvm")]
    Kvm { vcpu: helm_kvm::KvmVcpu },
}
```

### 2.2 KvmVcpu Core Loop

```rust
// crates/helm-kvm/src/vcpu.rs (sketch)
pub struct KvmVcpu {
    vcpu_fd: RawFd,
    kvm_run: *mut kvm_run,   // mmap'd KVM_RUN region
}

impl KvmVcpu {
    /// Run the vCPU until a VM exit occurs.
    pub fn run(&mut self) -> Result<VmExit, KvmError> {
        unsafe { ioctl(self.vcpu_fd, KVM_RUN, 0) };
        self.decode_exit()
    }

    /// Read all general-purpose registers.
    pub fn get_regs(&self) -> Result<KvmRegs, KvmError> { ... }

    /// Write all general-purpose registers.
    pub fn set_regs(&self, regs: &KvmRegs) -> Result<(), KvmError> { ... }

    /// Inject an IRQ into the vCPU.
    pub fn inject_irq(&self, irq: u32) -> Result<(), KvmError> { ... }
}
```

### 2.3 VM Exit Dispatch

```rust
// crates/helm-kvm/src/exit.rs
pub enum VmExit {
    Mmio { addr: Addr, data: [u8; 8], len: u32, is_write: bool },
    Irq,
    Shutdown,
    SystemEvent { kind: u32 },
    InternalError { code: u32 },
    Hlt,
    Unknown(u32),
}
```

The `FsSession` run loop becomes:

```rust
loop {
    match vcpu.run()? {
        VmExit::Mmio { addr, data, len, is_write } => {
            if is_write {
                let val = u64::from_le_bytes(data);
                platform.system_bus.write_fast(addr, len as usize, val)?;
            } else {
                let val = platform.system_bus.read_fast(addr, len as usize)?;
                vcpu.set_mmio_response(val);
            }
        }
        VmExit::Irq => {
            // Re-enter after host signal delivery
        }
        VmExit::Hlt => {
            // Check pending IRQs, inject if any, else idle
            if let Some(irq) = gic.highest_pending() {
                vcpu.inject_irq(irq)?;
            }
        }
        VmExit::Shutdown => break,
        _ => break,
    }
    insn_count += 1;  // approximate — KVM doesn't count per-insn
}
```

### 2.4 Guest Memory Setup

KVM requires guest physical memory to be backed by host `mmap`'d
regions registered via `KVM_SET_USER_MEMORY_REGION`:

```rust
// crates/helm-kvm/src/memory.rs
pub struct GuestMemoryRegion {
    pub slot: u32,
    pub guest_phys_addr: u64,
    pub memory_size: u64,
    pub userspace_addr: *mut u8,  // mmap'd host pointer
}
```

The existing `AddressSpace::map()` allocates a `Vec<u8>` per region.
For KVM we need the backing memory to come from an `mmap` with
`MAP_ANONYMOUS | MAP_SHARED` so KVM can access it.  Two options:

**Option A (recommended):** Add `AddressSpace::map_shared()` that uses
`mmap` instead of `Vec::new()`, returns the raw pointer for
`KVM_SET_USER_MEMORY_REGION`.  Non-KVM paths continue using `Vec`.

**Option B:** Always use `mmap` for `AddressSpace` regions (even
without KVM).  Simpler but changes the non-KVM path.

MMIO regions (device addresses) are **not** registered with KVM.
Accesses to unregistered addresses cause `KVM_EXIT_MMIO`, which is
exactly what we want — HELM's `DeviceBus` handles them.

---

## 3. Device Attachment Under KVM

### 3.1 MMIO Device Routing

The existing `DeviceBus` and `Platform` abstractions work unchanged
under KVM.  The only difference is the trigger: instead of
`Aarch64Cpu.step()` hitting an MMIO address and calling `IoHandler`,
KVM exits with `KVM_EXIT_MMIO` and the outer loop calls
`DeviceBus.read_fast()` / `write_fast()`.

```
KVM guest writes to 0x0900_0000 (PL011 UART)
  → KVM_EXIT_MMIO { addr=0x0900_0000, is_write=true, data=0x41 }
  → FsSession dispatches to platform.system_bus.write_fast(0x0900_0000, ...)
  → PL011.write(offset=0, size=1, value=0x41) → "A" on console
```

No changes to `Device`, `MemoryMappedDevice`, or any existing device
implementation are required.

### 3.2 Interrupt Injection

KVM on AArch64 supports in-kernel GIC emulation (`KVM_CREATE_DEVICE`
with `KVM_DEV_TYPE_ARM_VGIC_V2` or `V3`).  Two strategies:

**Strategy A — In-kernel GIC (recommended for speed):**
Use KVM's built-in GICv2/v3.  HELM's `Gic` struct becomes a
configuration-only object; the actual interrupt state lives in KVM.
Devices assert IRQs via `KVM_IRQ_LINE` ioctl.

```rust
// When a device fires an IRQ:
platform.irq_router.route(DeviceEvent::Irq { line: 33, assert: true });
// → kvm_vm.irq_line(33, true)  // KVM_IRQ_LINE ioctl
```

**Strategy B — User-space GIC (for research flexibility):**
Keep HELM's `Gic` in user-space.  On every `KVM_EXIT_MMIO` to the GIC
address range, HELM's GIC model handles it.  HELM injects IRQs via
`KVM_SET_ONE_REG` on the vCPU.  Slower but allows custom GIC research.

The choice can be a runtime flag: `--gic=kvm` (default) vs
`--gic=emulated`.

### 3.3 Timer

KVM on AArch64 virtualises the ARM Generic Timer in EL2.  The guest
reads `CNTVCT_EL0` / `CNTPCT_EL0` and configures timer interrupts
directly — no emulation needed.  KVM injects the timer IRQ (PPI 27/30)
when the compare value fires.

HELM's `ArmGenericTimer` device (planned) is not needed in KVM mode;
the host hardware handles it.

---

## 4. LLVM-IR Accelerator Devices Under KVM

This is the primary motivating integration.  The CPU runs at native
speed via KVM, while LLVM-IR accelerator devices simulate custom
hardware at cycle-accurate granularity.

### 4.1 Interaction Flow

```
┌──────────────────────────────────────────────────────────────────┐
│  Guest driver (running under KVM at native speed)                │
│                                                                  │
│  1. mmap accelerator MMIO region (from device tree)              │
│  2. Write input buffer address to REG_ARG0 (0x20)               │
│  3. Write output buffer address to REG_ARG1 (0x28)              │
│  4. Write 1 to REG_CONTROL (0x04) → triggers KVM_EXIT_MMIO      │
│  5. WFI → wait for completion IRQ                               │
└──────────────┬───────────────────────────────────────────────────┘
               │ KVM_EXIT_MMIO (addr=accel_base+0x04, write, val=1)
               ▼
┌──────────────────────────────────────────────────────────────────┐
│  HELM exit_dispatch()                                            │
│                                                                  │
│  → DeviceBus routes to AcceleratorDevice.write(0x04, 1)          │
│  → AcceleratorDevice sets status=RUNNING                         │
│  → Accelerator.run() — cycle-accurate LLVM-IR simulation         │
│     ├── InstructionScheduler.tick() × N cycles                   │
│     ├── FunctionalUnitPool allocation                            │
│     ├── ScratchpadMemory ←→ DMA to guest RAM                    │
│     └── returns total_cycles                                     │
│  → AcceleratorDevice sets status=IDLE                            │
│  → IrqRouter asserts completion IRQ (e.g. SPI 48)               │
│  → kvm_vm.irq_line(48, true)                                    │
│  → vcpu.run() resumes → guest handles IRQ, reads results         │
└──────────────────────────────────────────────────────────────────┘
```

### 4.2 DMA Between Accelerator and Guest RAM

The current `Accelerator` uses its own `ScratchpadMemory`.  Under KVM,
the accelerator needs to read/write guest physical memory for input and
output buffers.  Two approaches:

**Approach A — Host pointer pass-through (recommended):**
Since guest RAM is `mmap`'d in the HELM process, the accelerator can
read/write it directly via the host pointer.  `AcceleratorDevice`
receives the guest physical address from REG_ARG0, translates it to a
host pointer via `GuestMemoryRegion`, and passes a slice to the
accelerator's `MemoryBackend`.

```rust
impl AcceleratorDevice {
    fn start_with_dma(&mut self, guest_mem: &mut GuestMemoryRegion) {
        let input_ptr = guest_mem.translate(self.arg0);
        let output_ptr = guest_mem.translate(self.arg1);
        self.accel.set_memory_backend(
            HybridMemory::new_with_host_backing(input_ptr, output_ptr)
        );
        self.accel.run().unwrap();
    }
}
```

**Approach B — DmaEngine mediated:**
Use the existing `DmaEngine` from `helm-device` to perform
scatter-gather transfers.  More accurate (models bus contention and
beat-level timing) but slower.

For maximum research fidelity, both paths should be available:
`--accel-dma=direct` (fast) vs `--accel-dma=engine` (timing-accurate).

### 4.3 Multiple Accelerators

A platform can host multiple LLVM-IR accelerators at different MMIO
base addresses, each with its own LLVM IR module and functional unit
profile:

```python
# Python configuration
from helm.device import LlvmAccelerator

platform = Platform(
    ...,
    devices=[
        LlvmAccelerator(
            name="matmul_accel",
            base_address=0x4100_0000,
            ir_file="matmul.ll",
            int_adders=8, fp_multipliers=4,
            scratchpad_kb=64,
            irq=48,
        ),
        LlvmAccelerator(
            name="fft_accel",
            base_address=0x4200_0000,
            ir_file="fft.ll",
            int_adders=4, fp_multipliers=8,
            scratchpad_kb=128,
            irq=49,
        ),
    ],
)
```

Each accelerator appears as a separate node in the device tree:

```dts
accel@41000000 {
    compatible = "helm,llvm-accelerator";
    reg = <0x00 0x41000000 0x00 0x100>;
    interrupts = <GIC_SPI 48 IRQ_TYPE_LEVEL_HIGH>;
};
```

### 4.4 Extended AcceleratorDevice Register Map

The current register map (STATUS, CONTROL, CYCLES, LOADS, STORES) is
minimal.  For KVM + DMA integration, extend it:

| Offset | Name      | R/W | Description |
|--------|-----------|-----|-------------|
| 0x00   | STATUS    | R   | 0=idle, 1=running, 2=error |
| 0x04   | CONTROL   | W   | 1=start, 2=abort, 3=reset |
| 0x08   | CYCLES    | R   | Total accelerator cycles |
| 0x10   | LOADS     | R   | Memory load count |
| 0x18   | STORES    | R   | Memory store count |
| 0x20   | ARG0      | RW  | Input buffer guest physical address |
| 0x28   | ARG1      | RW  | Output buffer guest physical address |
| 0x30   | ARG2      | RW  | Transfer size (bytes) |
| 0x38   | ARG3      | RW  | General-purpose argument |
| 0x40   | IRQ_MASK  | RW  | Interrupt enable mask |
| 0x48   | IRQ_STAT  | R/W1C | Interrupt status (write-1-to-clear) |
| 0x50   | VERSION   | R   | Device version / magic |
| 0x58   | FU_CONFIG | R   | Functional unit configuration bitmap |

This follows the gem5-SALAM `CommInterface` pattern and gives the
guest driver enough control to set up DMA transfers without custom
hypercalls.

---

## 5. Attaching Non-LLVM Devices

KVM mode naturally supports all existing `Device` / `MemoryMappedDevice`
implementations because device interaction is purely MMIO-driven:

### 5.1 Existing Devices (No Changes Required)

| Device | Module | KVM behaviour |
|--------|--------|--------------|
| PL011 UART | `helm-device/arm/pl011.rs` | KVM_EXIT_MMIO on 0x0900_0000 |
| GIC | `helm-device/arm/gic.rs` | In-kernel or user-space (§3.2) |
| SP804 Timer | `helm-device/arm/sp804.rs` | KVM_EXIT_MMIO |
| VirtIO Block | `helm-device/virtio/` | KVM_EXIT_MMIO + DMA |
| GPIO | `helm-device/arm/pl061.rs` | KVM_EXIT_MMIO |
| BCM peripherals | `helm-device/arm/bcm_*.rs` | KVM_EXIT_MMIO |

### 5.2 Python-Defined Devices

Python devices (subclassing `helm.Device`) also work under KVM:

```python
class CustomSensor(Device):
    def __init__(self):
        super().__init__("sensor", region_size=0x10,
                         base_address=0x5000_0000)
        self.value = 0

    def read(self, offset, size):
        if offset == 0x00:
            return self.value
        return 0

    def write(self, offset, size, value):
        if offset == 0x04:
            self.value = value & 0xFFFF
```

When KVM exits on `0x5000_0000`, the MMIO dispatch crosses the
PyO3 boundary to call `CustomSensor.read()` / `.write()`.  This is
slower than Rust devices but functional.  For hot-path devices,
implement in Rust.

### 5.3 SystemC Devices via TLM Bridge

The existing `helm-systemc` stub provides `TlmPayload` and
`BridgeConfig`.  Under KVM, SystemC devices receive MMIO traffic the
same way — `KVM_EXIT_MMIO` → `DeviceBus` → `SystemCBridge` →
`b_transport()`.  The quantum-based synchronisation
(`BridgeConfig::quantum_ns`) maps naturally to KVM's run-exit-run
cadence.

---

## 6. FsSession Integration

### 6.1 Backend Selection

`FsSession` gains a backend field:

```rust
pub struct FsSession {
    cpu: Option<Aarch64Cpu>,        // None when using KVM
    #[cfg(feature = "kvm")]
    kvm_vcpu: Option<helm_kvm::KvmVcpu>,
    mem: AddressSpace,
    platform: Platform,
    ...
}
```

`FsOpts` gains a backend selector:

```rust
pub struct FsOpts {
    ...
    pub backend: String,  // "interp" (default), "tcg", "kvm"
}
```

### 6.2 Run Loop Unification

The `run_inner` method dispatches based on backend:

```rust
fn run_inner(&mut self, budget: u64, pc_break: Option<u64>) -> StopReason {
    match self.backend {
        Backend::Interpretive => self.run_interp(budget, pc_break),
        Backend::Tcg => self.run_tcg(budget, pc_break),
        #[cfg(feature = "kvm")]
        Backend::Kvm => self.run_kvm(budget),
    }
}
```

Note: `pc_break` is not directly supported under KVM (no single-step
without `KVM_GUESTDBG_SINGLESTEP`, which is very slow).  For
breakpoints, use `KVM_SET_GUEST_DEBUG` with hardware breakpoint
registers (limited to 4–6 on AArch64).

### 6.3 Mode Switching: KVM → Interpretive/CAE

The checkpoint workflow:

```
Phase 1: Boot with KVM     →  fast boot to shell
Phase 2: Checkpoint         →  save regs + memory + device state
Phase 3: Restore with CAE   →  warmup + detailed measurement
```

Switching from KVM to interpretive at runtime (without checkpoint)
requires:
1. `KVM_RUN` returns (on next exit)
2. `vcpu.get_regs()` → populate `Aarch64Cpu.regs`
3. `vcpu.get_sys_regs()` → populate system registers
4. Drop `KvmVcpu` and `KvmVm`
5. Continue with `Aarch64Cpu.step()` loop

This is clean because HELM's `AddressSpace` regions are already in
the HELM process address space — no memory copy needed.

---

## 7. Implementation Plan

### Phase 1: `helm-kvm` Crate (Foundation)

1. Create `crates/helm-kvm/` with `Cargo.toml` (dep: `helm-core`, `libc`)
2. Implement `KvmVm::new()` — open `/dev/kvm`, `KVM_CREATE_VM`, probe caps
3. Implement `GuestMemoryRegion` — `mmap` + `KVM_SET_USER_MEMORY_REGION`
4. Implement `KvmVcpu::new()` — `KVM_CREATE_VCPU`, mmap `kvm_run`
5. Implement `KvmVcpu::get/set_regs()` — `KVM_GET/SET_ONE_REG` (AArch64)
6. Implement `KvmVcpu::run()` + `VmExit` decoding
7. Implement `KvmVm::irq_line()` — `KVM_IRQ_LINE`
8. Add in-kernel GIC setup (`KVM_CREATE_DEVICE` + `KVM_DEV_ARM_VGIC_*`)
9. Tests: capability probe, VM create/destroy, register round-trip

### Phase 2: FsSession KVM Backend

1. Add `AddressSpace::map_shared()` for mmap-backed regions
2. Wire KVM memory regions from `AddressSpace` shared mappings
3. Add `Backend::Kvm` to `FsOpts` and `FsSession`
4. Implement `run_kvm()` loop with MMIO exit dispatch to `DeviceBus`
5. Wire IRQ injection: `DeviceEvent::Irq` → `KVM_IRQ_LINE`
6. Boot a minimal kernel (Alpine / BusyBox initramfs) to shell prompt
7. Verify PL011 UART output and timer interrupts work

### Phase 3: LLVM-IR Accelerator Integration

1. Extend `AcceleratorDevice` register map (ARG0–ARG3, IRQ_MASK/STAT)
2. Add `AcceleratorDevice` DMA support via `GuestMemoryRegion` pointers
3. Wire accelerator completion IRQ through `IrqRouter` → `KVM_IRQ_LINE`
4. DTB node generation for accelerator devices
5. Write a minimal guest driver (bare-metal or Linux module) that:
   - Writes input data to a buffer
   - Configures accelerator via MMIO
   - Waits for completion IRQ
   - Reads output data
6. End-to-end test: KVM boot → load driver → run accelerator → verify output

### Phase 4: Multi-Accelerator and Advanced Features

1. Support multiple accelerator instances at different MMIO addresses
2. DmaEngine-mediated transfers (timing-accurate mode)
3. KVM → interpretive live mode switch (no checkpoint)
4. Instruction counting via `KVM_CAP_ARM_PMU_V3` (perf counters)
5. Hardware breakpoint support via `KVM_SET_GUEST_DEBUG`
6. Python configuration bindings for KVM opts and accelerator devices

---

## 8. Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| KVM only on Linux+AArch64 hosts | Feature-gate; stub on other platforms; CI uses QEMU KVM |
| MMIO exit overhead for chatty devices | Batch MMIO via VirtIO (fewer exits); DMI for DMA |
| No per-instruction counting in KVM | Use PMU counters (`KVM_CAP_ARM_PMU_V3`) for approximate counts |
| Accelerator blocks KVM thread during run() | Run accelerator on separate thread; signal completion via eventfd |
| Guest RAM must be mmap'd (not Vec) | Option A in §2.4; non-KVM path unchanged |
| In-kernel GIC version mismatch | Probe `KVM_CAP_ARM_GIC_V3`; fall back to V2 or user-space |

---

## 9. Testing Strategy

| Test | Type | What it validates |
|------|------|------------------|
| `kvm_capability_probe` | Unit | `/dev/kvm` accessible, API version correct |
| `kvm_vm_create_destroy` | Unit | VM lifecycle, no fd leaks |
| `kvm_vcpu_reg_roundtrip` | Unit | Set X0–X30, SP, PC → get back same values |
| `kvm_memory_region` | Unit | Guest write at GPA → visible at host mmap ptr |
| `kvm_mmio_exit` | Integration | Guest store to unmapped addr → `VmExit::Mmio` |
| `kvm_boot_minimal` | Integration | Boot BusyBox initramfs, see "/ #" on PL011 |
| `kvm_accel_e2e` | Integration | KVM boot + accelerator MMIO + completion IRQ |
| `kvm_to_interp_switch` | Integration | KVM boot → switch to interpretive → continue |

Tests that require `/dev/kvm` are gated with `#[cfg(feature = "kvm")]`
and skipped in CI environments without KVM support.

---

## 10. Relationship to Existing Design

This design follows the principles in `execution-modes.md` §8:

- **Principle 1:** Timing accuracy is orthogonal — KVM is a backend,
  not a timing level.  Devices still run at any accuracy.
- **Principle 4:** Mode switches are instantaneous — KVM → interpretive
  requires only a register snapshot, no checkpoint.
- **Principle 7:** Accelerators integrate via MMIO — identical pattern
  under KVM and interpretive backends.

The `ExecBackend` enum already encodes backend orthogonality; KVM is
a natural third variant alongside `Interpretive` and `Tcg`.

---

## 11. Changes Required in Other Crates

The `helm-kvm` crate is self-contained and compiles independently.
Integrating it into the simulation pipeline requires the following
changes in other workspace crates.  Each item lists the crate, the
file(s) to modify, and a description of the change.

### 11.1 `helm-core`

| File | Change |
|------|--------|
| `src/types.rs` | Add `ExecMode::FS` variant (currently only `SE` and `CAE`). The KVM backend is FS-only. |
| `src/error.rs` | Add `HelmError::Kvm(String)` variant so KVM errors propagate through the unified error type. (`helm-kvm` already implements `From<KvmError> for HelmError` using `Config`, but a dedicated variant is cleaner.) |
| `src/config.rs` | Add `backend: BackendKind` field to `PlatformConfig` with variants `Interpretive`, `Tcg`, `Kvm`. |

### 11.2 `helm-memory`

| File | Change |
|------|--------|
| `src/address_space.rs` | Add `AddressSpace::map_shared(base, size) -> (*mut u8, MemRegion)` that uses `libc::mmap(MAP_ANONYMOUS \| MAP_PRIVATE)` instead of `Vec::new()`. Returns the raw host pointer so the caller can pass it to `KVM_SET_USER_MEMORY_REGION`. Non-KVM callers continue using `map()`. |
| `src/address_space.rs` | Add `AddressSpace::from_guest_memory(guest_mem: &GuestMemory)` constructor that wraps existing `GuestMemoryRegion` pointers without copying. |

### 11.3 `helm-engine`

| File | Change |
|------|--------|
| `src/se/backend.rs` | Add `ExecBackend::Kvm { vcpu: helm_kvm::KvmVcpu }` variant behind `#[cfg(feature = "kvm")]`. |
| `src/fs/session.rs` | **Major change.** Add a `run_kvm()` method to `FsSession` that: (1) creates a `KvmVm`, (2) registers memory regions, (3) creates a vCPU, (4) sets up the in-kernel GIC, (5) runs the MMIO exit dispatch loop calling `platform.system_bus.read_fast()`/`write_fast()`. Wire `DeviceEvent::Irq` to `KvmVm::irq_line()`. |
| `src/fs/session.rs` | Add `FsOpts::backend` field (`"interp"` / `"kvm"`) and dispatch in `run_inner()`. |
| `src/fs/session.rs` | Add `FsSession::snapshot_to_interp()` that calls `vcpu.get_core_regs()` / `get_sys_regs()` and populates an `Aarch64Cpu` for live mode switching. |
| `src/sim.rs` | Add `ExecMode::FS` handling in `Simulation::run()` that delegates to `FsSession`. |
| `Cargo.toml` | Add `helm-kvm = { workspace = true, optional = true }` dependency. |

### 11.4 `helm-device`

| File | Change |
|------|--------|
| `src/platform.rs` | No structural changes.  `Platform` already has `system_bus`, `irq_router`, `add_device()`, `tick()`.  These work as-is with KVM — the MMIO exit loop calls the same `DeviceBus` methods. |
| `src/irq.rs` | Add an `IrqSink` trait with `fn assert(&self, irq: u32)` / `fn deassert(&self, irq: u32)`.  Implement it for (a) the existing `IrqController` (user-space GIC) and (b) a new `KvmIrqSink` that calls `KvmVm::irq_line()`.  `IrqRouter` dispatches through `dyn IrqSink`. |
| `src/fdt.rs` | Add auto-generation of `compatible = "helm,llvm-accelerator"` DTB nodes when `AcceleratorDevice` is attached to the platform. |

### 11.5 `helm-llvm`

| File | Change |
|------|--------|
| `src/device_bridge.rs` | Extend the MMIO register map: add `REG_ARG0` (0x20), `REG_ARG1` (0x28), `REG_ARG2` (0x30), `REG_ARG3` (0x38), `REG_IRQ_MASK` (0x40), `REG_IRQ_STAT` (0x48), `REG_VERSION` (0x50), `REG_FU_CONFIG` (0x58).  Add `AcceleratorDevice::arg0..arg3` fields written by the guest driver. |
| `src/device_bridge.rs` | Add `AcceleratorDevice::set_memory_backend()` to inject a `GuestMemoryRegion`-backed `HybridMemory` for DMA between the accelerator scratchpad and guest RAM. |
| `src/device_bridge.rs` | After `Accelerator::run()` completes, emit a `DeviceEvent::Irq` to signal completion to the CPU. |
| `src/memory.rs` | Add `HybridMemory::new_with_host_backing(input: *mut u8, output: *mut u8)` constructor that reads/writes directly to mmap'd guest RAM pointers. |
| `src/accelerator.rs` | Add `Accelerator::set_memory_backend(&mut self, mem: MemoryBackend)` method. |

### 11.6 `helm-cli`

| File | Change |
|------|--------|
| `src/bin/helm_system_aarch64.rs` | Add `--backend kvm` CLI flag. When set, create an `FsSession` with `FsOpts { backend: "kvm", .. }`. |
| `src/bin/helm_system_aarch64.rs` | Add `--gic kvm\|emulated` flag to select in-kernel vs user-space GIC. |
| `src/bin/helm_system_aarch64.rs` | Add `--accel-dma direct\|engine` flag for accelerator DMA mode. |
| `Cargo.toml` | Add `helm-kvm` optional dependency, feature-gated. |

### 11.7 `helm-python`

| File | Change |
|------|--------|
| Python `Platform` class | Add `backend="kvm"` option to `Platform(...)` constructor. |
| Python `LlvmAccelerator` class | New Python class wrapping `AcceleratorDevice` with `ir_file`, `base_address`, `irq`, functional-unit config. |
| `_helm_core` bindings | Expose `KvmCaps` and `GicConfig` as Python-visible types for diagnostic / configuration. |

### 11.8 `helm-systemc`

| File | Change |
|------|--------|
| `src/bridge.rs` | Wire `SystemCBridge` as an `IrqSink` implementor so SystemC devices can inject IRQs via `KVM_IRQ_LINE` when running under KVM. No structural changes — the existing `BridgeConfig::quantum_ns` maps to the KVM run-exit-run cadence. |

### 11.9 Workspace `Cargo.toml`

| Change |
|--------|
| Add `[features] kvm = ["helm-engine/kvm"]` to the workspace root, threading the feature through `helm-engine` → `helm-kvm`. |
| The `Makefile` `CARGO_FLAGS` should exclude `helm-kvm` on non-Linux platforms or when `/dev/kvm` is unavailable (analogous to `--exclude helm-python`). |

### 11.10 `Makefile`

| Change |
|--------|
| Add `test-kvm` target: `cargo test -p helm-kvm --features kvm` (runs only on hosts with `/dev/kvm`). |
| Keep `make test` excluding `helm-kvm` so CI without KVM passes. |
