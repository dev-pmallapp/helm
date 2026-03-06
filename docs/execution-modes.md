# HELM Execution Modes

HELM organises simulation along two independent axes:

- **Execution mode** — what level of hardware is presented to the workload:
  - **SE (Syscall Emulation)** — run a user-space binary; intercept system calls
  - **FS (Full System)** — boot a real OS kernel against a complete hardware model

- **Timing accuracy** — how much microarchitectural detail is modelled:
  - **Functional (FE)** — QEMU-style, IPC = 1, no cache model, maximum speed
  - **Approximate (APE)** — cache latencies, device stalls, optional pipeline model
  - **Accurate (CAE)** — cycle-exact OoO pipeline, coherence, speculation

These two axes are **orthogonal**. FS mode can run at Functional speed during boot and
switch to Accurate for a region of interest. SE mode is most commonly used with
Functional or Approximate accuracy. The gem5 / Simics research literature consistently
validates this decomposition as the correct architecture.

```
         ┌────────────────────────────────────────────────────────────┐
         │                  Timing Accuracy                           │
         │          FE          │     APE       │       CAE           │
 ────────┼──────────────────────┼───────────────┼─────────────────────┤
 E      SE│ fast binary testing  │ perf sweeps   │ uarch research      │
 x      ──┼──────────────────────┼───────────────┼─────────────────────┤
 e      FS│ boot + checkpoint    │ OS-driven perf│ full-system timing  │
 c        └────────────────────────────────────────────────────────────┘
```

A third vertical: **accelerator simulation** via HELM's LLVM path (inspired by
gem5-SALAM) runs inside either execution mode, at CAE accuracy, while the CPU runs
at FE or APE speed. See §5.

---

## 1. SE Mode — Syscall Emulation

### 1.1 What SE Mode Simulates

SE mode runs a statically-linked user-space binary directly in HELM. When the binary
executes a syscall instruction, HELM intercepts it, executes the emulated handler, and
returns the result in the appropriate register. No kernel code ever executes.

**What is simulated:**
- All user-space instructions (via `Aarch64Cpu.step()` or chosen ISA frontend)
- Virtual address space — flat `AddressSpace` backed by host memory
- ELF binary loading: `PT_LOAD` segments, entry point, initial stack
  (argc / argv / envp / auxiliary vector)
- File descriptor table — guest-fd ↔ host-fd mapping with stdin/stdout/stderr
  pre-seeded
- Heap management via `brk` / `mmap` anonymous
- All timing models (FE / APE / CAE) work identically on the instruction stream
- Plugin callbacks: `on_insn_exec`, `on_mem_access`, `on_syscall`, `on_vcpu_init`

**What is NOT simulated:**
- OS kernel code — zero kernel instructions execute
- Interrupt controllers — no interrupt delivery
- Device drivers — I/O is proxied to the host OS
- Process scheduler — no preemption; the binary runs to completion or exit
- Virtual filesystem — `/proc`, `/sys` do not exist unless spoofed
- Dynamic linking — SE mode requires statically linked binaries
- Boot sequence — none; the ELF entry point is the first instruction

### 1.2 SE Mode: OS Targets

The syscall interface is the only OS-visible surface in SE mode. Each OS target
implements a syscall number table and a set of handlers for that OS's ABI.

```
helm-syscall/src/os/
├── linux/
│   ├── aarch64.rs       AArch64 syscall number constants (nr::READ=63, etc.)
│   ├── table.rs         ISA-keyed lookup: (IsaKind, nr) → Syscall enum
│   ├── handler.rs       Aarch64SyscallHandler (~50 syscalls, libc passthrough)
│   └── generic.rs       (legacy stub — to be deleted, see proposals.md §A5)
└── freebsd/
    └── mod.rs           FreeBSD stub (structure defined, handlers not yet impl.)
```

#### 1.2.1 Linux

**Status: Primary target. Fully operational for AArch64.**

Linux is HELM's first-class SE target because:
- All major research workloads (SPEC CPU, PARSEC, ML inference) are built for Linux.
- musl-libc and Alpine Linux produce compact static binaries with a well-known
  syscall surface.
- The AArch64 Linux ABI is simpler than x86-64 (fixed 32-bit instructions,
  clean register assignments).

**Syscall convention — AArch64:**

| Slot | Register | Role |
|------|----------|------|
| Syscall number | X8 | Set by userspace before `SVC #0` |
| Argument 1–6 | X0–X5 | Passed by value |
| Return value | X0 | Negative = −errno |

**Syscall convention — x86-64:**

| Slot | Register | Role |
|------|----------|------|
| Syscall number | rax | Set by userspace before `syscall` |
| Arg 1–6 | rdi, rsi, rdx, r10, r8, r9 | |
| Return value | rax | Negative = −errno |

**Syscall convention — RISC-V 64:**

| Slot | Register | Role |
|------|----------|------|
| Syscall number | a7 | |
| Arg 1–6 | a0–a5 | |
| Return value | a0 | Negative = −errno |

**Implemented syscalls (AArch64 Linux, `Aarch64SyscallHandler`):**

| Nr | Name | Notes |
|----|------|-------|
| 25 | fcntl | file control |
| 29 | ioctl | device control (proxied) |
| 56 | openat | AT_FDCWD = −100 |
| 57 | close | guards stdin/stdout/stderr |
| 63 | read | fd → host read |
| 64 | write | fd → host write |
| 73 | ppoll | proxied |
| 93 | exit | halts core, stores exit_code |
| 94 | exit_group | same as exit |
| 113 | clock_gettime | CLOCK_REALTIME, CLOCK_MONOTONIC |
| 134 | rt_sigaction | stub, returns 0 |
| 160 | uname | fills sysname="Linux", machine="aarch64" |
| 172 | getpid | returns 1 |
| 214 | brk | heap bump allocator |
| 222 | mmap | anonymous only; backed by AddressSpace::map |
| 278 | getrandom | fills buffer with zeros (deterministic) |

**Extension pattern:** Each new Linux syscall is:
1. Added to `nr::` constants in `linux/aarch64.rs`.
2. Mapped in `linux/table.rs` for each ISA that supports it.
3. Implemented in `Aarch64SyscallHandler::handle()` as a match arm.
4. Covered by a test in `crates/helm-syscall/src/tests/`.

**Future Linux targets:** x86-64 and RISC-V require their own `SyscallHandler`
implementations using the per-ISA argument register conventions above. The
`linux/table.rs` numeric lookup already supports all three ISAs; the handler
dispatch is the remaining work.

#### 1.2.2 macOS

**Status: Planned.**

macOS uses the Mach/XNU hybrid kernel with BSD system calls accessed via a
BSD syscall layer. The user-facing ABI for 64-bit programs is the Darwin ABI:

**Syscall convention — macOS (x86-64, user-space BSD syscalls):**

| Slot | Register | Role |
|------|----------|------|
| Syscall class | rax[63:24] | `0x02...` = BSD, `0x03...` = Mach |
| Syscall number | rax[23:0] | |
| Arg 1–6 | rdi, rsi, rdx, r10, r8, r9 | Same as Linux x86-64 |
| Return value | rax | Carry flag set on error |

**Syscall class prefixes:**

| Prefix | Class | Description |
|--------|-------|-------------|
| `0x2000000` | BSD | POSIX-compatible: read, write, open, close, … |
| `0x3000000` | Mach | Mach IPC, vm_allocate, task_*, thread_* |
| `0x4000000` | mdep | Machine-dependent (commpage, sysenter) |

HELM SE mode for macOS would target only BSD-class syscalls (the `0x2000000`
range). Mach syscalls enable IPC, virtual memory, and task management — the
building blocks of the macOS runtime but not needed to run POSIX-compatible
static binaries.

**Key BSD syscalls for macOS SE:**

| Nr (BSD) | Name | Linux equivalent |
|----------|------|-----------------|
| 4 | write | 64 (AArch64), 1 (x86-64) |
| 3 | read | 63 (AArch64), 0 (x86-64) |
| 5 | open | 56 (openat, AArch64) |
| 6 | close | 57 |
| 1 | exit | 93 |
| 477 | mmap | 222 (AArch64) |
| 73 | munmap | 215 (AArch64) |
| 45 | brk | 214 |

**Architecture note:** Apple Silicon (AArch64) macOS uses the same register
convention as Linux AArch64 (X8 = syscall number, X0–X5 = args) but with
different syscall numbers and the BSD/Mach class prefix scheme. The number space
is disjoint from Linux.

**Implementation plan:**
```
helm-syscall/src/os/macos/
├── mod.rs            MacosSyscallHandler
├── aarch64.rs        AArch64 BSD syscall constants
├── x86_64.rs         x86-64 BSD syscall constants
└── table.rs          class-prefix demux + nr lookup
```

#### 1.2.3 FreeBSD

**Status: Structure defined. Handlers not yet implemented.**

FreeBSD uses a BSD syscall ABI virtually identical to Linux in calling convention
but with a different number space:

**Syscall convention — FreeBSD (AArch64):**
- Syscall number in X8 (same as Linux AArch64)
- Arguments in X0–X5
- Return in X0; C flag indicates error
- Invoked via `SVC #0`

**Key FreeBSD/AArch64 syscall numbers (differs from Linux):**

| Nr | Name | Linux AArch64 equivalent |
|----|------|--------------------------|
| 1 | exit | 93 |
| 3 | read | 63 |
| 4 | write | 64 |
| 5 | open | 56 (openat) |
| 6 | close | 57 |
| 477 | mmap | 222 |
| 73 | munmap | 215 |
| 20 | getpid | 172 |
| 17 | brk | 214 |
| 250 | getrandom | 278 |

FreeBSD is valuable for HELM because:
- It is the OS of choice for many embedded/networking workloads.
- Kernel infrastructure research often targets FreeBSD (Capsicum, GEOM, jails).
- The Ports collection provides a rich source of static binary test cases.

**Implementation plan:**
```
helm-syscall/src/os/freebsd/
├── mod.rs            FreebsdSyscallHandler (stub ← extend here)
├── aarch64.rs        AArch64 syscall constants
└── table.rs          numeric lookup
```

#### 1.2.4 Extensibility: Adding a New OS Target

The extension pattern is deliberately simple. To add an OS `myos`:

1. Create `helm-syscall/src/os/myos/`.
2. Define a `MyosSyscallHandler` implementing the `SyscallHandlerInterface` trait:
   ```rust
   pub trait SyscallHandlerInterface {
       fn handle(&mut self, nr: u64, args: &[u64; 6],
                 mem: &mut AddressSpace) -> HelmResult<u64>;
   }
   ```
3. Register the handler in `helm-engine::se::mod.rs` by matching `OsTarget::Myos`.
4. Add syscall number constants (mirroring the Linux `nr` module pattern).
5. Add tests in `helm-syscall/src/tests/`.

No other crate needs to change.

### 1.3 SE Mode: File Descriptor Table

`helm-syscall/src/fd_table.rs` maintains a per-process mapping:

```
guest_fd (i32) → host_fd (i32)
```

Pre-seeded entries:
- `0` (stdin) → host `0`
- `1` (stdout) → host `1`
- `2` (stderr) → host `2`

`allocate()` returns the next available guest fd starting at 3.
`close()` removes the entry but guards 0/1/2 from being closed.
`get()` translates guest fd to host fd for read/write/ioctl.

### 1.4 SE Mode: ELF Loader

`helm-engine/src/loader/elf64.rs` handles AArch64 ELF64 static binaries:

1. Validate: magic `\x7fELF`, class=2 (ELF64), data=1 (LE), machine=183 (AArch64).
2. Walk `PT_LOAD` segments: `mem.map(vaddr, filesz, rwx)`, copy segment bytes.
3. Zero-fill BSS: map `(vaddr + filesz)` to `(memsz - filesz)` bytes.
4. Build initial stack at `stack_top`:
   ```
   High address
     AT_NULL       (end of aux vector)
     AT_RANDOM     (16 bytes of random data)
     AT_ENTRY      (e_entry)
     AT_PAGESZ     (4096)
     AT_PHNUM      (number of PT_LOAD segments)
     AT_PHENT      (sizeof(Elf64_Phdr) = 56)
     AT_PHDR       (vaddr of first program header)
     NULL          (envp terminator)
     NULL          (argv terminator)
     argv[0]       (pointer to binary name string)
     argc          (= 1 minimal)  ← SP
   Low address
   ```
5. Set `cpu.regs.pc = e_entry`, `cpu.regs.sp = stack_top`.

**Current limitations:** Static AArch64 only. PIE, dynamic linking, ELF32, and other
ISAs are future work (see `docs/proposals.md §C3`).

### 1.5 SE Mode: Thread Model

SE mode models a single-threaded process. One `Aarch64Cpu` instance holds all
architectural state. There is no OS scheduler; the binary runs to completion.

Multi-threaded workloads via `pthreads` require:
- A `clone()` / `clone3()` syscall implementation.
- Per-thread `Aarch64Cpu` instances sharing the same `AddressSpace`.
- A simple round-robin thread multiplexer (or `rayon`-based parallel dispatch
  matching the CAE multi-core model in `helm-engine`).

This is planned but not yet implemented.

---

## 2. FS Mode — Full System

### 2.1 What FS Mode Simulates

FS mode presents a complete virtual hardware platform to an unmodified OS kernel.
The kernel boots from an ELF image, initialises its scheduler, drivers, and memory
manager using the simulated hardware, then runs user workloads. No HELM code handles
syscalls directly — the guest kernel does.

**What is simulated:**
- Physical memory map and bus fabric
- CPU(s) with chosen timing accuracy (FE / APE / CAE)
- Interrupt controller (GIC for AArch64, APIC for x86-64)
- UART / serial console for boot messages and shell interaction
- Storage controller with a disk image (VirtIO block or simulated IDE)
- Hardware page table walkers (TLB miss → page table walk → cache hierarchy)
- Timer (Generic Timer / HPET) for OS scheduler preemption
- Optional network device (VirtIO net)
- Optional accelerator via MMIO (CommInterface + `helm-llvm` LLVMInterface)
- Device tree blob (DTB) for ARM — auto-generated from instantiated objects

**What is NOT simulated:**
- Host-native syscalls (the guest kernel handles all system calls)
- Bootloader — HELM loads the kernel ELF directly into guest physical memory

### 2.2 FS Mode: Timing Accuracy Submodes

The timing submode is selected independently of FS/SE. The same virtual hardware
platform can run at any accuracy level. The canonical workflow is:

```
Boot → [FE] fast-forward through kernel init
     → [APE] warmup: reach the workload start, populate caches
     → [CAE] measure: region of interest at full cycle accuracy
     → [FE] finish: drain the workload fast
```

This is controlled by `SamplingController` (helm-timing) and `TemporalDecoupler`
(multi-core clock synchronisation).

#### 2.2.1 Functional (FE) — QEMU-Style Fast Execution

**Goal:** Run the OS kernel and workload as fast as possible. No timing data.

**What is modelled:**
- Correct instruction execution (every instruction produces correct architectural state)
- Syscall emulation in SE mode; real kernel code in FS mode
- Dynamic binary translation cache (TranslationCache) for block reuse
- No cache timing — all memory accesses complete instantly

**What is NOT modelled:**
- Cache hit/miss latency
- Pipeline stages or instruction-level parallelism
- Branch misprediction penalties
- DRAM latency
- Device stall cycles (device MMIO completes in 0 simulated cycles)

**IPC:** Always 1. Every instruction takes exactly 1 cycle.

**Speed:** 100–1000+ MIPS. Comparable to QEMU TCG.

**Rust type:** `AccuracyLevel::FE`, `FeModel`. All `TimingModel` methods return 0
or 1 for zero overhead.

**Use cases:**
- OS kernel boot (FS mode). Boot a Linux kernel to shell prompt in seconds, not hours.
- Workload preparation: set up data, compile, launch a long-running benchmark.
- Functional correctness validation.
- Checkpoint generation: boot with FE, checkpoint, restore with CAE.

**Python:**
```python
platform.timing = TimingMode.functional()
```

#### 2.2.2 Approximate (APE) — Cache-Annotated Emulation

**Goal:** Get meaningful performance estimates at moderate simulation cost.

APE covers two sub-levels (controlled by the `ApeModel` configuration):

**APE-L1 (stall-annotated):** Each instruction still costs 1 base cycle, but
memory operations incur stall cycles from the cache model.

- L1 hit: `l1_latency_cycles` stall (typically 3–4)
- L2 hit: `l2_latency_cycles` stall (typically 10–15)
- L3 hit: `l3_latency_cycles` stall (typically 40–60)
- DRAM: `dram_latency_cycles` stall (typically 100–300)

Branch prediction exists but uses a simple 2-bit counter model (Bimodal).
Misprediction penalty is a fixed `branch_penalty_cycles` (typically 5–15).

**APE-L2 (detailed):** Adds a simplified out-of-order issue window.
Instructions can overlap execution subject to:
- Instruction queue depth (configurable, default 32)
- Per-functional-unit issue width
- Simple register dependence tracking (no rename — architectural hazards only)

IPC in APE-L2 is typically 60–80% of a full CAE simulation for integer workloads,
at 10× the simulation speed.

**Accuracy:** IPC typically within 20–40% of hardware. Cache MPKI typically
within 10%. Sufficient for cache-hierarchy sensitivity studies, DSE (design-space
exploration), and workload characterisation.

**Speed:** L1: 10–100 MIPS. L2: 1–10 MIPS.

**Rust type:** `AccuracyLevel::APE`, `ApeModel`. `instruction_latency` returns
`1 + functional_unit_latency`. `memory_latency` walks the cache hierarchy.
`branch_misprediction_penalty` returns a configured value.

**Use cases:**
- Cache-hierarchy design exploration (associativity, size, replacement policy).
- Branch predictor sensitivity.
- Device/MMIO latency impact on application performance.
- Pre-screening workloads before expensive CAE runs.

**Python:**
```python
platform.timing = TimingMode.approximate(
    l1_latency=3, l2_latency=12, l3_latency=40, dram_latency=200,
    branch_penalty=8
)
```

#### 2.2.3 Accurate (CAE) — Cycle-Exact OoO Pipeline

**Goal:** Reproduce hardware cycle counts within ≤ 5% for microarchitectural
research and validation.

**What is modelled:**

*Fetch and decode:*
- Fetch width (configurable, default 4 instructions/cycle)
- Branch predictor: Static / Bimodal / GShare / TAGE / Tournament
- Speculative fetch along predicted path

*Rename:*
- `RenameUnit` with physical register file
- RAT (register alias table) mapping architectural → physical registers
- Free list management

*Dispatch and issue:*
- `ReorderBuffer` (ROB): in-order allocation, out-of-order completion, in-order commit
- `Scheduler` (issue queue): wake-up on source availability, select within width

*Execute:*
- Per-functional-unit latency tables (configurable)
- Integer ALU, multiplier, divider, FP units, load/store units
- Store buffer, memory disambiguation

*Memory system:*
- Set-associative cache hierarchy (L1i/L1d/L2/L3)
- `Tlb` with configurable capacity
- MOESI coherence (stub → see proposals.md §A for full implementation plan)
- DRAM latency modelled via `EventQueue`

*Commit:*
- In-order commit from ROB head
- Precise exception support (flush ROB on misprediction)
- `StatsCollector` fires `SimEvent::InsnCommit` on each committed instruction

*Branch misprediction recovery:*
- ROB flush from mispredicted entry
- Rename unit rollback
- Issue queue drain

**Accuracy:** IPC typically within 2–10% of hardware for in-order and
out-of-order cores when pipeline parameters match.

**Speed:** 0.1–1 MIPS.

**Rust types:** `AccuracyLevel::CAE`, `Pipeline` (helm-pipeline),
`ReorderBuffer`, `RenameUnit`, `Scheduler`, `BranchPredictor`,
`Cache`, `Tlb` (helm-memory), `EventQueue` (helm-timing).

**Use cases:**
- Microarchitectural design validation (ROB size, issue width, branch predictor).
- Cache line size and associativity studies.
- Memory ordering and consistency research.
- Precise what-if analysis for uarch changes.
- Accelerator integration timing: CPU + accelerator co-simulation at cycle level.

**Python:**
```python
platform.timing = TimingMode.accurate(
    rob_size=256, issue_width=4, iq_size=128,
    l1i=Cache("32KB", assoc=8, latency=4),
    l1d=Cache("32KB", assoc=8, latency=4),
    l2=Cache("512KB", assoc=16, latency=14),
    l3=Cache("8MB", assoc=32, latency=50),
    dram_latency=200,
    branch_predictor="tage",
)
```

### 2.3 FS Mode: Platform Requirements

Running FS mode requires a complete platform definition:

**ARM (AArch64) FS platform:**
- Linux kernel ELF (`vmlinux`) — stripped kernel, no bootloader needed
- Device Tree Blob (DTB) — describes hardware topology to the kernel;
  HELM auto-generates from instantiated device objects
- Root filesystem disk image (ext4, read-only or with overlay)
- `GicV3Controller` (or GicV2) — generic interrupt controller
- `ArmGenericTimer` — provides CNTV/CNTP timer interrupts to the scheduler
- `PL011Uart` — ARM PrimeCell UART for console I/O
- `VirtioBlockDevice` — storage interface for the disk image

**x86-64 FS platform (planned):**
- `vmlinuz` + `initrd`
- `LocalApic` + `IOApic` — interrupt controllers
- `HpetTimer` or `Pit8254` — timer
- `I8250Uart` or VirtIO console
- `VirtioBlockDevice` or `Ahci`

### 2.4 FS Mode: Boot Sequence

```
1. ELF loader reads vmlinux → maps PT_LOAD segments into guest physical memory
2. HELM writes DTB into guest physical memory (at fixed address, e.g. 0x88000000)
3. Interrupt controller initialised (GIC distributor + redistributors)
4. ARM Generic Timer initialised, CNTFRQ set
5. CPU.pc = e_entry (kernel start_kernel)
   CPU.x0 = 0 (cpuid)
   CPU.x1 = 0xFFFFFFFF (unused)
   CPU.x2 = DTB physical address
6. Kernel executes: exception level setup, MMU enable, memory init,
   driver probe (UART, timer, storage, network), init/PID-1 launch
7. Shell prompt available — first m5 exit event fires (optional)
8. Workload launched by init script or via serial console
9. Region-of-interest marker (magic instruction SVC with special imm) → switch to CAE
10. End-of-ROI marker → switch back to FE → m5 exit → simulation ends
```

### 2.5 FS Mode: Checkpointing

Checkpoints allow separating the slow boot phase from the measurement phase:

**Checkpoint workflow:**
```
Phase 1: Boot  (FE accuracy, KVM if host ISA matches guest ISA)
  → OS boots to shell prompt
  → Magic instruction / CLI trigger → HELM saves checkpoint to disk

Phase 2: Restore + warmup  (APE accuracy)
  → Restore architectural state (registers + memory pages + device state)
  → Run workload for warmup period to populate cache state
  → `SamplingController` transitions: FastForward → Warmup → Detailed

Phase 3: Measure  (CAE accuracy)
  → Region of interest executes at full cycle accuracy
  → `StatsCollector` accumulates SimResults
  → `SamplingController` transitions: Detailed → Cooldown → Done
  → HELM exits with results JSON
```

**Checkpoint format:**
A checkpoint directory `ckpt.<timestamp>/` contains:
- `cpu_state.json` — all architectural registers for each vCPU
- `memory.bin.zstd` — compressed guest physical memory dump
- `device_state.json` — UART FIFO, interrupt pending status, timer values

**Timing state is NOT checkpointed.** Cache contents, ROB entries, and
in-flight pipeline state are discarded. The restored simulation starts with
cold caches and an empty pipeline — hence the mandatory warmup phase.

---

## 3. Mode Switching at Runtime

Mode switching uses `SamplingController` (helm-timing) and `TemporalDecoupler`
(helm-timing):

```
                    SamplingController state machine:

  ┌───────────────┐  advance(ff_cycles) ┌──────────┐  advance(warmup)
  │  FastForward  │────────────────────►│  Warmup  │──────────────────►
  │  FE accuracy  │                     │  APE     │
  └───────────────┘                     └──────────┘
                                                         ┌─────────────┐
                                                         │  Detailed   │
                                                     ───►│  CAE        │
                                                         └──────┬──────┘
                                                                │  advance(detail)
                                                         ┌──────▼──────┐
                                                         │  Cooldown   │
                                                         │  APE → FE  │
                                                         └──────┬──────┘
                                                                │  advance(cool)
                                                         ┌──────▼──────┐
                                                         │    Done     │
                                                         └─────────────┘
```

```python
sim.set_fast_forward(instructions=1_000_000_000)   # FE until ROI
sim.set_warmup(instructions=10_000_000)             # APE cache fill
sim.set_detailed(instructions=100_000_000)          # CAE measurement
sim.set_cooldown(instructions=5_000_000)            # APE wind-down
results = sim.run()
```

**Multi-core synchronisation** uses `TemporalDecoupler`. Each core runs ahead
independently by up to `quantum_size` cycles. When any core exceeds the quantum,
all cores synchronise. This prevents causality violations while allowing parallel
simulation. The global time is always `min(core_times)`.

---

## 4. SystemC Co-simulation with FS Mode

FS mode is the natural integration point for SystemC/TLM-2.0 models because:
- Device models written in SystemC can plug directly into the device bus
- The timing quantum aligns naturally with the SystemC scheduler
- IRQ lines map to `sc_signal<bool>` in the SystemC world

### 4.1 TLM-2.0 Timing Modes

| TLM Mode | Transport API | Models |
|----------|--------------|--------|
| Loosely Timed (LT) | `b_transport(payload, delay)` | Temporal decoupling; initiator runs ahead by `quantum_ns` |
| Approximately Timed (AT) | `nb_transport_fw/bw` with phases | Pipelined bus (AXI, OCP); models `BEGIN_REQ` → `END_REQ` → `BEGIN_RESP` → `END_RESP` |

HELM implements the bridge via `helm-systemc/src/bridge.rs`:

```rust
pub enum TlmTimingMode {
    LooselyTimed,       // b_transport; default for most device models
    ApproximatelyTimed, // nb_transport; required for interconnect fidelity
}
```

### 4.2 Time Domain Crossing

gem5-style: `1 tick = 1 ps`. HELM's convention:

```rust
// helm-systemc/src/clock.rs
pub struct ClockDomain {
    pub frequency_hz: u64,
}

impl ClockDomain {
    pub fn cycles_to_ns(&self, cycles: u64) -> f64 {
        (cycles as f64 / self.frequency_hz as f64) * 1e9
    }
    pub fn ns_to_cycles(&self, ns: f64) -> u64 {
        (ns * self.frequency_hz as f64 / 1e9) as u64
    }
}
```

For a 1 GHz CPU: 1 cycle = 1 ns = 1000 ps = 1000 HELM ticks.

### 4.3 Temporal Decoupling and Quantum

HELM's `BridgeConfig::quantum_ns` (default 10,000 ns = 10 µs) controls how far
the SystemC side can run ahead of HELM's global time before synchronisation.

This maps directly to the TLM-2.0 `tlm_quantumkeeper`:
```
tlm_quantumkeeper::need_sync() → TemporalDecoupler::needs_sync(core_id)
tlm_quantumkeeper::sync()      → SystemCBridge::sync_quantum()
```

The `EventQueue` enforces monotonicity: `schedule(t, ...)` panics if `t < current_time`,
matching the TLM invariant that time cannot go backwards.

### 4.4 Direct Memory Interface (DMI)

For high-bandwidth paths (e.g., DMA transfers from accelerator scratchpad to DRAM),
DMI bypasses the TLM transport call:

```
initiator.get_direct_mem_ptr(payload, dmi_data)
  → dmi_data.ptr = AddressSpace::raw_ptr(base_addr)
  → dmi_data.start_addr, end_addr, read/write latency

// Initiator reads/writes directly through dmi_data.ptr
// No b_transport call overhead

// When HELM invalidates (e.g., memory remapping):
target.invalidate_direct_mem_ptr(start_addr, end_addr)
```

DMI is the primary path for the scratchpad model in `helm-llvm/src/scratchpad.rs`
when the SystemC-side DMA engine reads directly into HELM-managed memory.

---

## 5. Accelerator Mode (FS + LLVM Accelerator)

Hardware accelerators appear in FS mode as MMIO devices. The CPU writes to the
accelerator's CommInterface registers, the accelerator simulates its datapath
cycle-by-cycle via `helm-llvm`, and signals completion via IRQ.

### 5.1 Architecture (Inspired by gem5-SALAM)

```
Host CPU (FE/APE accuracy)
      │
      │  SVC write to CommInterface MMR (start=1, arg0=buf_ptr, arg1=size)
      ▼
 DeviceBus ──► CommInterface (MemoryMappedDevice)
                      │  start() triggers
                      ▼
              LLVMInterface (Accelerator)
                      │
                      │  static elaboration (once at init):
                      │    parse LLVM IR → CDFG
                      │    map opcodes → FunctionalUnitType
                      │    load latency profile from YAML
                      │
                      │  dynamic execution (per tick):
                      │    ReservationTable → active_parents == 0 → ready
                      │    try_allocate(fu_type) → ComputeQueue
                      │    ComputeQueue countdown → notify_completion
                      │    MemoryQueue → DMA transfer via helm-memory
                      ▼
              ScratchpadMemory ←──── DMA ────► AddressSpace (system DRAM)
                      │
                      │  is_idle() → true
                      ▼
              IrqController::assert(line)
                      │
                      ▼
      CPU receives interrupt, reads result from output MMR
```

### 5.2 Mixed-Fidelity: Fast CPU + Detailed Accelerator

This is the primary motivating use case for accelerator simulation:

| Component | Accuracy | Rationale |
|-----------|----------|-----------|
| Application CPU | FE or APE | Too slow to simulate at CAE for the full app |
| Accelerator datapath | CAE | Cycle-exact timing is the research question |
| Memory system (shared DRAM) | APE | Cache latency matters for DMA bandwidth |
| SystemC device models | LT | Device control overhead is not the bottleneck |

The timing synchronisation point is the MMIO write to `CommInterface::start`.
At this point the CPU model stops its temporal decoupling, hands a `tick()` budget
to the accelerator engine, and waits for `is_idle()`. The accelerator runs at full
CAE cycle resolution. Wall-clock time is dominated by the accelerator simulation.

### 5.3 Functional Unit Latency Profile (YAML)

```yaml
# Example hardware profile — ARM Cortex-M33-style accelerator
functional_units:
  IntAdder:
    count: 8
    latency: 1
    pipelined: true
  IntMultiplier:
    count: 2
    latency: 3
    pipelined: true
  FPMultiplier:
    count: 2
    latency: 5
    pipelined: true
  LoadStore:
    count: 4
    latency: 2
    pipelined: true
  GEP:
    count: -1       # unlimited (address arithmetic)
    latency: 1
    pipelined: true
```

Accuracy benchmark (gem5-SALAM, MachSuite): average timing error vs Vivado HLS < 1%.

---

## 6. Comparison Table

| Dimension | gem5 SE | gem5 FS | QEMU user | QEMU system | Simics functional | Simics timing | Spike | HELM SE | HELM FS |
|-----------|---------|---------|-----------|-------------|-------------------|---------------|-------|---------|---------|
| Kernel executes | No | Yes | No | Yes | Yes | Yes | No (PK) | No | Yes |
| Syscall handling | Emulated | Kernel | Emulated | Kernel | Kernel | Kernel | Emulated (HTIF) | Emulated | Kernel |
| OS targets | Linux | Linux | Linux | Any | Any | Any | Linux | Linux, macOS†, FreeBSD† | Linux |
| Interrupt controller | No | Yes | No | Yes | Yes | Yes | No | No | Yes† |
| Device models | No | Yes | No | Yes | Yes | Yes | No | No | Yes† |
| Cache timing | Yes | Yes | No | No | Optional | Yes | No | Yes | Yes |
| OoO pipeline | Yes | Yes | No | No | No | Plugin | No | Yes | Yes |
| Accelerator model | SALAM | SALAM | No | No | MAI | MAI | No | helm-llvm | helm-llvm |
| SystemC bridge | Yes | Yes | No | No | SCL | SCL | No | Stub | Stub |
| Dynamic linking | Yes | Yes | Yes | Yes | Yes | Yes | No | No† | Yes† |
| Speed (FE) | 200+ MIPS | 100+ MIPS | 200+ MIPS | 50+ MIPS | 500+ MIPS | 500+ MIPS | 50+ MIPS | 200+ MIPS | 100+ MIPS |
| Speed (CAE) | 1–100 MIPS | 0.1–10 MIPS | N/A | N/A | N/A | 0.1–10 MIPS | N/A | 1–100 MIPS | 0.1–10 MIPS |

† = planned / partial implementation

---

## 7. HELM Implementation Mapping

### Current State

| Feature | Status | Location |
|---------|--------|----------|
| SE mode, AArch64 Linux | **Working** | `helm-syscall`, `helm-engine/se/linux.rs` |
| ELF loader (AArch64 static) | **Working** | `helm-engine/loader/elf64.rs` |
| FE timing model | **Working** | `helm-timing/model.rs::FeModel` |
| APE timing model | **Working** | `helm-timing/model.rs::ApeModel` |
| CAE pipeline (OoO) | **Working** | `helm-pipeline`, `helm-memory` |
| Plugin system | **Working** | `helm-plugin` |
| LLVM accelerator engine | **Working** (parser bug) | `helm-llvm` |
| SystemC bridge (stub) | **Stub** | `helm-systemc/src/bridge.rs` |
| SE mode, FreeBSD | **Stub** | `helm-syscall/src/os/freebsd/` |
| SE mode, macOS | **Planned** | — |
| SE mode, x86-64 Linux | **Planned** | — |
| SE mode, RISC-V Linux | **Planned** | — |
| FS mode, AArch64 | **Planned** | — |
| Interrupt controller (GIC) | **Planned** | — |
| VirtIO block device | **Planned** | — |
| ARM Generic Timer | **Planned** | — |
| Checkpoint / restore | **Planned** | — |
| Magic instruction (m5 ops) | **Planned** | — |
| DTB auto-generation | **Planned** | — |
| SystemC TLM-2.0 (real) | **Planned** | — |
| DMI support | **Planned** | — |

### Config API (Target)

```rust
// helm-core/src/config.rs — proposed additions
pub struct PlatformConfig {
    pub exec_mode: ExecMode,      // SE or FS
    pub timing: TimingConfig,     // FE / APE / CAE
    pub os_target: OsTarget,      // Linux / macOS / FreeBSD
    // ... existing fields ...
}

pub enum OsTarget {
    Linux,
    MacOs,
    FreeBsd,
    Custom(String),  // user-provided handler name
}

pub struct TimingConfig {
    pub level: AccuracyLevel,          // FE / APE / CAE
    pub fast_forward: u64,             // instructions at FE before warmup
    pub warmup: u64,                   // instructions at APE
    pub detailed: u64,                 // instructions at CAE
    pub pipeline: Option<CoreConfig>,  // CAE parameters
    pub memory: MemoryConfig,          // cache hierarchy
}
```

---

## 8. Design Principles

These principles emerge from the gem5 / Simics / QEMU / Spike research and should
guide HELM's implementation:

1. **Timing accuracy is orthogonal to execution mode.** FS and SE both support
   FE/APE/CAE. Do not couple them.

2. **The functional core is always complete.** Every instruction produces correct
   architectural state regardless of timing mode. Timing is additive, never
   subtractive. (Simics architecture.)

3. **Checkpoints contain architectural state only.** No pipeline state, no cache
   contents. The restored simulation requires a warmup phase to fill caches.

4. **Mode switches must be instantaneous.** A switch from FE to CAE must not
   require a checkpoint-restore cycle. (Simics advantage over gem5.) Achieve this
   by keeping the timing model as a plug-in layer (`TimingModel` trait) that can
   be swapped without discarding CPU state.

5. **Time cannot go backwards.** All event queues enforce `t >= current_time`.
   The `TemporalDecoupler` quantum prevents cores from diverging beyond `quantum_size`
   cycles. The TLM-2.0 quantum keeper is the external-model analogue.

6. **Magic instructions enable workload communication without OS changes.** Reserve
   a legal NOP encoding in each supported ISA as a simulator pseudo-instruction for
   ROI marking, checkpointing, and statistics reset. Binaries instrumented with magic
   instructions run unmodified on real hardware.

7. **Accelerators integrate via MMIO, not custom ABIs.** The CommInterface pattern
   (MMIO-based start/done/IRQ) decouples the CPU model from the accelerator model
   and is portable across SE and FS mode.

8. **Syscall emulation is ISA × OS.** Each (ISA, OS) pair has its own number table
   and register convention. The handler implementation is shared where possible (e.g.,
   brk heap logic is the same on all Linux targets) and specialised where necessary
   (macOS Mach class prefix demux).
