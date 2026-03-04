# HELM Architecture

## Crate Map

```
helm-core                 types, IR (MicroOp), config, errors, events
    |
    +-- helm-object       HelmObject trait, property system, type registry,
    |                     composition tree (/platform/cores/core0)
    |
    +-- helm-timing       TimingModel trait (FE / APE / CAE),
    |                     EventQueue, TemporalDecoupler, SamplingController
    |
    +-- helm-device       MemoryMappedDevice trait, DeviceBus, IrqController
    |       |
    |       +-- helm-plugin-api   stable ABI for user-built plugins
    |
    +-- helm-isa          IsaFrontend trait, per-arch decoders
    |                     (arm/, riscv/, x86/)
    |
    +-- helm-pipeline     OoO pipeline: ROB, rename, scheduler, branch pred
    |
    +-- helm-memory       set-associative cache, TLB, flat AddressSpace,
    |                     MOESI coherence stub
    |
    +-- helm-translate    dynamic binary translation, block cache
    |
    +-- helm-syscall      Linux syscall emulation (per-ISA tables)
    |
    +-- helm-stats        atomic counters, StatsCollector, SimResults
    |
    +-- helm-engine       top-level Simulation driver, per-core loop,
    |                     binary loader
    |
    +-- helm-python       PyO3 cdylib (_helm_core)
    +-- helm-cli          `helm` binary (clap)
```

## Data Flow

### Syscall-Emulation (SE) Mode

```
Binary on disk
  --> loader (ELF parse, map into AddressSpace)
  --> ISA frontend.decode(pc, bytes) --> Vec<MicroOp>
  --> TranslationCache (block reuse)
  --> TimingModel.instruction_latency(uop) --> stall cycles
  --> SyscallHandler.handle(nr, args)
  --> StatsCollector.on_event(...)
  --> SimResults { ipc, branch_mpki, ... }
```

### Device Interaction

```
Core executes a load/store to MMIO region
  --> AddressSpace detects it is outside RAM
  --> DeviceBus.read/write(addr, size, value)
  --> routes to correct DeviceSlot by base address
  --> MemoryMappedDevice.read/write returns (data, stall_cycles)
  --> stall_cycles fed back to TimingModel
```

## Accuracy Tiers

See [accuracy-levels.md](accuracy-levels.md) for full details.

| Tier | Code name | Speed | Modelled |
|------|-----------|-------|----------|
| L0 | **FE** | 100-1000 MIPS | Nothing — IPC=1 |
| L1 | **APE** | 10-100 MIPS | Cache latencies, device stalls |
| L2 | **APE** (detailed) | 1-10 MIPS | Simplified OoO, branch pred |
| L3 | **CAE** | 0.1-1 MIPS | Full pipeline, bypass, store buffer |

## Python Configuration

Users never touch Rust directly.  They compose platforms in Python:

```python
from helm import Platform, Core, Cache, MemorySystem, Device, TimingMode, Simulation
from helm.isa import Arm

platform = Platform(
    name="cortex-a53-soc",
    isa=Arm(),
    cores=[Core("a53", width=2, rob_size=64)],
    memory=MemorySystem(l1d=Cache("32KB"), dram_latency=200),
    devices=[MyUart("uart0", base_address=0x0900_0000)],
    timing=TimingMode.ape(),
)
results = Simulation(platform, binary="./hello-arm", mode="se").run()
```
