# HELM Architecture

## Crate Map (18 crates)

```
Foundation
──────────
helm-core          IR (MicroOp, Opcode), types (Addr, RegId, Cycle),
                   config structs, HelmError/HelmResult, SimEvent

helm-object        HOM: HelmObject trait, typed properties,
                   TypeRegistry, ObjectTree (/platform/cores/core0)

helm-stats         Atomic Counter, StatsCollector (EventObserver),
                   SimResults { ipc(), branch_mpki(), to_json() }

ISA & Translation
─────────────────
helm-isa           IsaFrontend trait; full AArch64 executor (Aarch64Cpu
                   + step()); ARM/RISC-V/x86 decoder stubs; Aarch64Regs
                   with NZCV, SIMD, tpidr_el0

helm-decode        QEMU-compatible .decode parser: %fields, &argsets,
                   @formats, DecodeTree; no downstream consumers yet

helm-tcg           QEMU-style TCG IR: TcgOp enum, TcgContext, TcgBlock;
                   no downstream consumers yet

helm-translate     Dynamic binary translation: Translator,
                   TranslationCache, TranslatedBlock (uses MicroOp,
                   not TcgOp)

LLVM Accelerator
────────────────
helm-llvm          LLVM IR text parser, LLVMInstruction → local MicroOp
                   lowering, InstructionScheduler with reservation table,
                   FunctionalUnitPool, Accelerator/AcceleratorBuilder

Microarchitecture
─────────────────
helm-pipeline      OoO pipeline: ReorderBuffer, RenameUnit, Scheduler,
                   BranchPredictor (Static/Bimodal/GShare/TAGE/Tournament),
                   StageName enum

helm-memory        Set-associative Cache, Tlb, MOESI CoherenceController
                   (stub), flat AddressSpace for SE mode

Platform & System
─────────────────
helm-device        MemoryMappedDevice trait, DeviceBus (address routing),
                   IrqController, IrqLine

helm-timing        TimingModel trait (FE/APE/CAE), EventQueue (priority),
                   TemporalDecoupler (multi-core quantum), SamplingController

helm-syscall       Linux syscall emulation: Aarch64SyscallHandler,
                   per-ISA number tables, FdTable

Orchestration & Integration
────────────────────────────
helm-plugin        Unified plugin crate: stable API (traits, metadata),
                   runtime (PluginRegistry, callbacks, scoreboard),
                   built-in plugins (insn_count, hotblocks, execlog,
                   cache_sim), optional dynamic .so loading

helm-engine        Simulation, CoreSim, ELF loader (AArch64 static),
                   SE-mode runner (run_aarch64_se_with_plugins),
                   rayon-based multi-core dispatch

helm-systemc       SystemC/TLM-2.0 bridge: Clock, TlmPayload,
                   StubBridge, BridgeConfig

helm-python        PyO3 cdylib (_helm_core) — Python bindings

helm-cli           clap CLI (`helm` and `helm-arm` binaries)
```

## Dependency Graph

```
                         helm-core
                        /    |    \
               helm-object  helm-stats  helm-timing
                    |                       |
               helm-device             helm-plugin ◄─ helm-object
                                                       helm-device
                                                       helm-timing

  helm-isa ◄─ helm-memory

  helm-translate ◄─ helm-core, helm-isa

  helm-pipeline  ◄─ helm-core
  helm-memory    ◄─ helm-core
  helm-syscall   ◄─ helm-core, helm-memory
  helm-llvm      ◄─ helm-core

  helm-decode ◄─ helm-core   (no downstream consumers)
  helm-tcg    ◄─ helm-core   (no downstream consumers)

  helm-engine ◄─ helm-core, helm-isa, helm-pipeline, helm-memory,
                  helm-translate, helm-syscall, helm-stats, helm-plugin

  helm-cli     ◄─ helm-engine, helm-plugin
  helm-python  ◄─ helm-engine, helm-plugin, helm-stats
  helm-systemc ◄─ helm-core, helm-device, helm-timing
```

## Data Flows

### Syscall-Emulation (SE) Mode

```
Binary on disk
  ──► ELF loader        parse header, map PT_LOAD into AddressSpace,
                        build stack with argc/argv/envp/auxv,
                        set PC = e_entry
  ──► Aarch64Cpu.step() fetch 4 bytes, decode A64 instruction, execute
  ──► SVC handler       AArch64 table lookup ──► Aarch64SyscallHandler
  ──► FdTable           fd bookkeeping for open/read/write/close
  ──► PluginRegistry    fire on_insn_exec / on_mem_access / on_vcpu_init
  ──► StatsCollector    accumulate SimResults
```

### Cycle-Accurate (CAE) Mode

```
Binary
  ──► ELF loader        (same as SE)
  ──► IsaFrontend.decode(pc, bytes) ──► Vec<MicroOp>
  ──► TranslationCache  lookup / insert TranslatedBlock
  ──► RenameUnit        arch reg ──► phys reg (RAT + free-list)
  ──► ReorderBuffer     allocate entry, track state (Dispatched → Complete)
  ──► Scheduler         issue-queue: wakeup when sources available
  ──► TimingModel       instruction_latency, memory_latency
  ──► Cache.access()    Hit / Miss ──► stall cycles
  ──► TemporalDecoupler sync cores at quantum boundary
  ──► EventQueue        schedule DRAM / device-stall events
  ──► StatsCollector    on_event(InsnCommit | CacheAccess | ...)
```

### LLVM Accelerator Path

```
LLVM IR (text)
  ──► LLVMParser            LLVMModule / LLVMFunction / LLVMBasicBlock
  ──► llvm_to_micro_ops()   LLVMInstruction ──► local MicroOp
  ──► InstructionScheduler  reservation table, per-unit queues
  ──► FunctionalUnitPool    pipelined / non-pipelined allocation
  ──► Accelerator.run()     tick loop until drain
  ──► AcceleratorStats      total_cycles, memory_loads, memory_stores
```

### Device Interaction (MMIO)

```
Core MMIO access
  ──► DeviceBus.read/write(addr, size)  route by base address
  ──► MemoryMappedDevice.read/write()   returns (data, stall_cycles)
  ──► IrqController.assert(line)        device raises interrupt
```

## Python Configuration Layer

Users configure simulations in Python; HELM deserialises to `PlatformConfig`:

```python
from helm import Platform, Core, Cache, MemorySystem, TimingMode
from helm.isa import Arm

platform = Platform(
    name="cortex-a55",
    isa=Arm(),
    cores=[Core("a55", width=4, rob_size=128, iq_size=64)],
    memory=MemorySystem(
        l1i=Cache("32KB", assoc=8, latency=1),
        l1d=Cache("32KB", assoc=8, latency=1),
        l2=Cache("256KB", assoc=8, latency=5),
        dram_latency=200,
    ),
    timing=TimingMode.cae(),
)
results = platform.simulate("./workload", mode="se")
print(f"IPC: {results.ipc():.2f}, MPKI: {results.branch_mpki():.2f}")
```

Rust equivalents live in `helm-core/src/config.rs`:
`PlatformConfig`, `CoreConfig`, `CacheConfig`, `MemoryConfig`.

See [proposals.md §C1](proposals.md) for the known config field mismatch
between the Python layer and Rust structs.
