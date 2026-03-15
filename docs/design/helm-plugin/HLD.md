# helm-plugin — High-Level Design

## 1. Purpose

`helm-plugin` provides a unified instrumentation and analysis framework for the
helm-ng simulator. It enables users to observe, trace, profile, and debug guest
execution without modifying the simulator core.

**Design philosophy**: Zero-cost when no plugins are loaded. Minimal overhead
when plugins are active. All observation is read-only — plugins cannot modify
guest state (separation of concerns).

## 2. Industry Survey

### 2.1 QEMU TCG Plugins (`libqemu-plugin.h`)

| Aspect | Detail |
|--------|--------|
| **Hooks** | `qemu_plugin_register_vcpu_tb_trans_cb`, `_tb_exec_cb`, `_insn_exec_cb`, `_mem_cb`, `_syscall_cb`, `_syscall_ret_cb`, `_vcpu_init_cb`, `_vcpu_exit_cb`, `_atexit_cb` |
| **Data** | PC, instruction bytes, disassembly string, memory address/size/value, syscall nr+args+retval, HW thread index |
| **Loading** | Shared library via `-plugin libfoo.so,arg=val` CLI flag |
| **Built-in plugins** | `hotblocks` (TB execution frequency), `howvec` (instruction class histogram), `cache` (configurable set-associative cache sim), `execlog` (per-insn trace), `lockstep` (two-QEMU comparison) |
| **Performance** | Translation-time hook registration (callback per TB, not per insn by default). Inline counting via scoreboard API (`qemu_plugin_scoreboard`). ~2-10x overhead for per-insn callbacks. |
| **Key pattern** | Two-phase: register interest at translation time → fire at execution time. Scoreboard for lock-free per-vCPU counters. |

### 2.2 Simics (Wind River)

| Aspect | Detail |
|--------|--------|
| **Hooks** | HAP (Happens-After-Predicate) system: `Core_Exception`, `Core_Breakpoint_Memop`, `Core_Mode_Change`, `Core_Instruction_Fetch`, `Core_Memory_Access`, `Simulation_Stopped`, `TLB_*` |
| **Instrumentation API** | `instrumentation_connection_t` + `instrumentation_filter_t` + `instrumentation_tool_t` — filter by address range, CPU, access type |
| **Data** | Full register snapshots, memory values, exception codes, TLB entries, cache line states |
| **Tools** | Code coverage, cache simulation, power estimation, profiling, fault injection, record/replay |
| **Key pattern** | Interface-based: tools implement `instruction_instrument_iface_t`, `memory_instrument_iface_t`. Filters control scope. |

### 2.3 gem5

| Aspect | Detail |
|--------|--------|
| **Hooks** | `ProbePoint<T>` + `ProbeListener<T>` (type-safe observer pattern). Named probe points on SimObjects. |
| **Data** | `ProbePoint<PacketPtr>` for memory, `ProbePoint<std::pair<Addr,Addr>>` for branches, `InstTracer::InstRecord` for instructions |
| **Built-in** | `ExeTracer` (per-insn trace), `IntelTrace` (Intel PT format), `NativeTrace` (for CheckerCPU), cache stats via `Stats::*` |
| **Python** | `m5.simulate()`, probe points accessible from Python config scripts |
| **Key pattern** | SimObject-based: tracers are SimObjects wired in Python config. Stats system (`Stats::Scalar`, `Stats::Vector`, `Stats::Distribution`) provides counters/histograms. |

### 2.4 DynamoRIO / Intel Pin / Valgrind

| Aspect | Detail |
|--------|--------|
| **Hooks** | Instruction-level (`INS_AddInstrumentFunction`), basic-block, trace, syscall, thread, signal, exception |
| **Data** | Full instruction decode (operands, registers read/written), memory operand addresses, register values |
| **Key pattern** | Analysis routine injection — insert calls at instrumentation time, execute at runtime. Pin uses JIT compilation to inline analysis routines. |

### 2.5 ARM Fast Models

| Aspect | Detail |
|--------|--------|
| **Hooks** | Trace sources: `INST` (instruction), `MEM` (memory), `BRANCH` (branches), `BUS` (bus transactions), `MMU` (page table walks), `CACHE` (hit/miss), `EXCEPTION` |
| **Plugin API** | `CAInterface`-based; plugins implement trace sinks |
| **Key pattern** | Source/sink: models publish trace sources, plugins subscribe as sinks. Filter by component path. |

### 2.6 Spike (RISC-V ISA Simulator)

| Aspect | Detail |
|--------|--------|
| **Hooks** | `--extension` flag for custom instruction extensions. `--log` for instruction trace. `--log-commits` for register commit log. |
| **Key pattern** | Simple: logging to stderr, no plugin API. Extensions modify decode/execute. |

### 2.7 Renode

| Aspect | Detail |
|--------|--------|
| **Hooks** | C# hooks: `machine.SystemBus.AddWatchpointHook`, `cpu.AddHook(addr, callback)`, `machine.SetHookAtPeripheralRead/Write` |
| **Key pattern** | Script-based (`.resc` files). Robot Framework for automated testing. Hooks are C# lambdas attached to bus/CPU events. |

### 2.8 Cross-System Comparison

| System | Hooks | Performance Model | Key Design Pattern |
|--------|-------|-------------------|-------------------|
| **QEMU TCG** | TB translate, insn exec, memory, syscall, vCPU, discontinuity | Inline+scoreboard for hot path; 5-15x for full trace | Two-phase (translate→run); lock-free scoreboards |
| **Simics** | HAPs + Instrumentation framework (cached insn, counters) | Counter-based near-zero; cached-instruction fast; global slow | Provider-Tool-Filter-Connection; cached instruction analysis |
| **gem5** | ProbePoints (typed observer), InstTracer, CheckerCPU | <1% when no listeners | Observer pattern via ProbeManager; SimObject tracers |
| **DynamoRIO** | BB, trace, thread, syscall, signal, module | ~5% null client; ~2x lightweight tool | Copy & Annotate; staged instrumentation via drmgr |
| **Pin** | Instruction, trace, routine, image | ~30% null; ~60% integer benchmarks | Auto-inlining; JIT; conditional if/then |
| **Valgrind** | VEX IR superblock instrumentation | 4-30x depending on tool | Disassemble & Resynthesise; shadow memory |
| **Renode** | Execution trace, watchpoints, peripheral/CPU/interrupt hooks | Simulation-speed focused | Test-first via Robot Framework; C# runtime |
| **Spike** | `-l` exec log, `--log-commits`, interactive debug | Minimal (golden reference model) | ISA correctness first; extensions via shared libs |
| **ARM Fast Models** | MTI trace sources: insn, branch, exception, cache, MMU, mode | Lazy field computation; no-plugin = max speed | Declarative trace sources; typed fields; subscribe model |

### 2.9 Key Insights for helm-ng

1. **QEMU's inline operations** (scoreboard `INLINE_ADD_U64`) avoid callback overhead entirely — helm-ng should support inline counter increment without firing a closure
2. **Simics cached-instruction** pattern: analyze instruction once when first seen, register targeted callbacks only for interesting instructions — avoids per-instruction overhead for selective analysis
3. **QEMU conditional callbacks** (`InsertIfCall/InsertThenCall`): condition evaluated inline, callback fires only when true — important for watchpoints and coverage
4. **ARM Fast Models lazy field computation**: expensive data (disassembly string) computed only when a plugin actually reads it — avoid constructing `InsnInfo` fields nobody uses
5. **Simics Filter-Aggregator chain**: multiple independent filters can control a single callback without knowing about each other — useful for address-range and CPU-mask filtering
6. **gem5 ProbeManager**: zero overhead when no listeners attached, even with 60+ probe points defined

## 3. Design Principles

Drawing from the survey, helm-ng's plugin system follows these principles:

1. **QEMU-like callback taxonomy**: instruction, memory, syscall, fault, TB/block, vCPU lifecycle
2. **Simics-like filtering**: plugins specify what they observe (address ranges, access types, CPU mask)
3. **gem5-like Python integration**: plugins configurable from Python, stats accessible as Python objects
4. **Zero-cost abstraction**: no overhead when no callbacks registered for a category (checked via `has_*_callbacks()` flags)
5. **Scoreboard pattern**: lock-free per-vCPU counters for hot-path instrumentation (from QEMU)
6. **Hot-loading**: plugins can be added/removed between simulation phases (from Simics/gem5)
7. **Stable ABI**: external plugins via shared library with versioned vtable (from QEMU/Simics)

## 4. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         helm-plugin                             │
├────────────────┬────────────────┬───────────────────────────────┤
│   api/         │   runtime/     │   builtins/                   │
│  ─────────     │  ──────────    │  ──────────                   │
│  HelmPlugin    │  PluginRegistry│  trace/                       │
│  PluginArgs    │  Callback types│    InsnCount                  │
│  ComponentInfo │  InsnInfo      │    ExecLog                    │
│  HelmComponent │  MemInfo       │    HotBlocks                  │
│  PluginMeta    │  SyscallInfo   │    HowVec (insn class histo)  │
│  DynamicLoader │  FaultInfo     │    SyscallTrace               │
│                │  ArchContext   │    BranchTrace   ← NEW        │
│                │  Scoreboard    │    MemTrace      ← NEW        │
│                │  MemFilter     │  memory/                      │
│                │                │    CacheSim                   │
│                │                │    TlbSim        ← NEW        │
│                │                │  debug/                       │
│                │                │    FaultDetect                │
│                │                │    Watchpoint    ← NEW        │
│                │                │    CoverageMap   ← NEW        │
│                │                │    DiffTest      ← NEW        │
│                │                │  profiling/      ← NEW        │
│                │                │    FuncProfile   ← NEW        │
│                │                │    FlameGraph    ← NEW        │
│                │                │    BBVectors     ← NEW        │
└────────────────┴────────────────┴───────────────────────────────┘
```

## 5. Callback Taxonomy

### 5.1 Existing (from reference)

| Hook | Signature | When Fired |
|------|-----------|------------|
| `vcpu_init` | `fn(vcpu_idx)` | vCPU created |
| `vcpu_exit` | `fn(vcpu_idx)` | vCPU destroyed |
| `tb_trans` | `fn(&TbInfo, &[InsnInfo])` | Translation block translated (compile-time) |
| `tb_exec` | `fn(vcpu_idx, &TbInfo)` | Translation block executed |
| `insn_exec` | `fn(vcpu_idx, &InsnInfo)` | Single instruction executed |
| `mem_access` | `fn(vcpu_idx, &MemInfo)` | Memory load/store (with MemFilter) |
| `syscall` | `fn(&SyscallInfo)` | Syscall entry (nr + args) |
| `syscall_ret` | `fn(&SyscallRetInfo)` | Syscall return (nr + retval) |
| `fault` | `fn(&FaultInfo)` | Execution fault (with ArchContext) |

### 5.2 New Hooks for helm-ng

| Hook | Signature | When Fired | Inspired By |
|------|-----------|------------|-------------|
| `branch` | `fn(vcpu_idx, &BranchInfo)` | Branch instruction (taken/not-taken, target) | ARM Fast Models BRANCH trace |
| `exception` | `fn(vcpu_idx, &ExceptionInfo)` | Exception taken (EL change, vector, ESR) | Simics Core_Exception HAP |
| `mmu_walk` | `fn(vcpu_idx, &MmuWalkInfo)` | Page table walk (VA→PA, level, fault) | Simics TLB HAPs, Fast Models MMU |
| `cache_event` | `fn(vcpu_idx, &CacheEvent)` | Cache hit/miss/evict (L1/L2/L3) | gem5 ProbePoint<PacketPtr> |
| `timer` | `fn(vcpu_idx, tick: u64)` | Periodic callback every N instructions | Simics step callbacks |
| `symbol` | `fn(vcpu_idx, &SymbolInfo)` | Execution reaches a named symbol | Renode cpu.AddHook |
| `irq` | `fn(vcpu_idx, &IrqInfo)` | Interrupt delivered/masked/pending | Simics Core_Exception |

## 6. Plugin Catalog

### 6.1 Trace Plugins (from reference, adapted)

| Plugin | Purpose | Hook(s) Used |
|--------|---------|--------------|
| **InsnCount** | Per-vCPU instruction counter via scoreboard | `insn_exec` |
| **ExecLog** | Per-instruction trace log (PC, mnemonic, optional regs) | `insn_exec` |
| **HotBlocks** | Translation block execution frequency profiler | `tb_exec` |
| **HowVec** | Instruction class histogram (IntAlu/Branch/Load/Store/FP/SIMD) | `insn_exec` |
| **SyscallTrace** | Syscall entry/return logger with arg decode | `syscall`, `syscall_ret` |

### 6.2 New Trace Plugins

| Plugin | Purpose | Hook(s) Used |
|--------|---------|--------------|
| **BranchTrace** | Branch prediction analysis: direction, target, frequency per-PC | `branch` |
| **MemTrace** | Memory access trace with address/size/type, optional value logging | `mem_access` |
| **RegTrace** | Register value trace at configurable intervals or addresses | `insn_exec` + `timer` |
| **IrqTrace** | Interrupt delivery/masking/pending log | `irq` |

### 6.3 Memory Plugins

| Plugin | Purpose | Hook(s) Used |
|--------|---------|--------------|
| **CacheSim** | Configurable set-associative cache model (L1I/L1D/L2/L3) with LRU/PLRU/random policies | `mem_access` |
| **TlbSim** | TLB simulation with hit/miss/flush counting | `mmu_walk` |
| **MemBandwidth** | Memory bandwidth measurement (bytes/cycle per address range) | `mem_access` + `timer` |

### 6.4 Debug Plugins

| Plugin | Purpose | Hook(s) Used |
|--------|---------|--------------|
| **FaultDetect** | Execution fault detector with ring-buffer history, TLS aliasing, stack corruption detection | `insn_exec`, `syscall`, `fault` |
| **Watchpoint** | Address/value watchpoints with conditional breaks | `mem_access` |
| **CoverageMap** | Basic-block coverage bitmap (like AFL/libFuzzer) | `tb_exec` |
| **DiffTest** | Compare execution trace against a reference (golden model) | `insn_exec`, `mem_access` |
| **AssertPlugin** | User-defined assertions on register/memory state at specific PCs | `insn_exec` |

### 6.5 Profiling Plugins (NEW category)

| Plugin | Purpose | Hook(s) Used |
|--------|---------|--------------|
| **FuncProfile** | Function-level profiling using symbol table (call count, insn count per function) | `insn_exec` + `symbol` |
| **FlameGraph** | Generate flamegraph-compatible stack traces from call/return tracking | `branch` (BL/RET) |
| **BBVectors** | SimPoint basic-block vectors for representative simulation sampling | `tb_exec` + `timer` |
| **IPC** | IPC (instructions per cycle) measurement with moving-window averaging | `insn_exec` + `timer` |
| **PowerEstimate** | Activity-based power estimation (insn class weights × frequency) | `insn_exec` + `timer` |

## 7. Integration with helm-ng

### 7.1 Engine Integration

```
HelmEngine<T>::step_aarch64()
    ├── fetch → decode → execute
    │       │                │
    │       │                ├── fire_insn_exec(insn)
    │       │                ├── fire_mem_access(addr, size)  [if has_mem_callbacks]
    │       │                └── fire_branch(target, taken)   [if has_branch_callbacks]
    │       │
    │       └── on SVC → fire_syscall(nr, args)
    │                  → handle_syscall()
    │                  → fire_syscall_ret(nr, retval)
    │
    └── on exception → fire_fault(FaultInfo)
```

### 7.2 Python Integration

```python
import helm

sim = helm.build_simulation(isa="aarch64", mode="se")
sim.load_elf("./hello", ["hello"])

# Add plugins before or between run() calls
sim.add_plugin("insn-count")
sim.add_plugin("cache", l1d_size="32KB", l1d_assoc=8)
sim.add_plugin("syscall-trace")
sim.add_plugin("hotblocks")

sim.run(100_000_000)

# Access results
print(f"Instructions: {sim.plugin('insn-count').total}")
print(f"L1D hit rate: {sim.plugin('cache').l1d_hit_rate:.1%}")
for pc, count, insns in sim.plugin('hotblocks').top(10):
    print(f"  {pc:#x}: {count} executions ({insns} insns)")
```

### 7.3 Performance Budget

| Callback Level | Overhead Target | Technique |
|----------------|----------------|-----------|
| No plugins | 0% | `has_*_callbacks()` flags bypass all dispatch |
| TB-level only (HotBlocks) | <5% | One callback per block (~100 insns) |
| Per-instruction (InsnCount) | <15% | Scoreboard inline increment |
| Per-instruction + mem (ExecLog + CacheSim) | <50% | Gated by `has_insn_callbacks && has_mem_callbacks` |
| Full trace (ExecLog + MemTrace + BranchTrace) | ~3-5x | Accept overhead; trace is inherently expensive |

## 8. Data Types

### 8.1 InsnInfo

```rust
pub struct InsnInfo {
    pub pc: u64,
    pub raw: u32,           // raw encoding
    pub size: u8,           // 4 for AArch64
    pub opcode: &'static str, // mnemonic
    pub class: InsnClass,   // IntAlu, Branch, Load, Store, FpAlu, SimdAlu, System, Nop
}
```

### 8.2 BranchInfo (NEW)

```rust
pub struct BranchInfo {
    pub pc: u64,
    pub target: u64,
    pub taken: bool,
    pub kind: BranchKind, // Direct, Indirect, Call, Return, Conditional
}
```

### 8.3 MemInfo

```rust
pub struct MemInfo {
    pub vaddr: u64,
    pub paddr: Option<u64>,  // None in SE mode
    pub size: u8,
    pub is_store: bool,
    pub is_atomic: bool,     // NEW
    pub value: Option<u64>,  // optional — only when value logging enabled
}
```

### 8.4 FaultInfo

```rust
pub struct FaultInfo {
    pub pc: u64,
    pub raw: u32,
    pub kind: FaultKind,
    pub message: String,
    pub insn_count: u64,
    pub context: ArchContext,  // full register dump
}
```

## 9. External Plugin ABI

Shared-library plugins for third-party extensibility:

```rust
// Plugin author's crate (cdylib)
#[no_mangle]
pub extern "C" fn helm_plugin_entry() -> *const HelmPluginVTable {
    static VTABLE: HelmPluginVTable = HelmPluginVTable {
        metadata: PluginMetadata {
            api_version: 1,
            name: "my-plugin",
            version: "0.1.0",
            description: "Custom analysis",
            author: "user",
        },
        create: my_plugin_create,
    };
    &VTABLE
}
```

Loaded via `--plugin /path/to/libmy_plugin.so,key=val` or from Python:
```python
sim.load_plugin("/path/to/libmy_plugin.so", key="val")
```

## 10. Phasing

| Phase | Scope |
|-------|-------|
| **Phase 0** | PluginRegistry + callback dispatch + InsnCount + ExecLog + SyscallTrace + HotBlocks + HowVec + FaultDetect |
| **Phase 1** | CacheSim (functional L1/L2) + MemTrace + BranchTrace + Watchpoint |
| **Phase 2** | Python plugin API + hot-loading + FlameGraph + FuncProfile + CoverageMap |
| **Phase 3** | Dynamic .so loading + DiffTest + BBVectors + PowerEstimate + TlbSim |
