# Helm-ng Accuracy Design

> How Helm-ng's three timing models achieve accuracy inspired by PTLsim + Sniper + a high-accuracy ARM simulator.
> This is the accuracy architecture spec — what each mode must model internally.

---

## Design Philosophy

Helm-ng synthesizes three simulator approaches into one multi-mode framework:

| Source | Contribution |
|--------|-------------|
| **PTLsim** | Deepest cycle-accurate OoO pipeline model (~5% IPC error ceiling) |
| **Sniper** | Interval simulation: fast, analytical, CPI stacks (~9.5–20% MAPE) |
| **High-accuracy ARM sim** | Calibrated µarch profiles, ARM-specific pipeline accuracy |

**Key enhancement over all three:** Multi-ISA (RISC-V + ARM) + mid-simulation mode switching. None of the three support both.

---

## `Virtual` Mode — Event-Driven Clock

**Equivalent to:** gem5 AtomicSimpleCPU / SIMICS functional model
**Purpose:** Fast-forward, OS boot, region-of-interest setup. Correctness only.
**IPC accuracy:** Not meaningful — no timing model.
**Speed target:** >100 MIPS

### What it models
- Correct instruction execution (all architectural state changes)
- Instruction count, PC progression
- Virtual clock advancing by estimated cycles (no real timing)
- Event queue firing (timers, periodic events)

### What it does NOT model
- Pipeline stages
- Cache hit/miss latency
- Branch misprediction penalty
- Structural hazards

### Internal Structure
```rust
pub struct VirtualTiming {
    tick: u64,                // virtual clock (cycles)
    ipc_estimate: f64,        // configurable, default 1.0
}

impl TimingModel for VirtualTiming {
    #[inline(always)]
    fn on_memory_access(&mut self, _addr: u64, _cycles: u64) {
        self.tick += 1;       // nominal 1 cycle per access
    }
    #[inline(always)]
    fn on_branch_mispredict(&mut self, _penalty: u64) {
        // ignored — virtual mode has no penalty
    }
}
```

---

## `Interval` Mode — Sniper-Inspired Interval Simulation

**Equivalent to:** Sniper (interval simulation)
**Purpose:** Design space exploration, multicore studies, fast performance estimation.
**IPC accuracy target:** <15% MAPE vs. reference (Spike for RISC-V, QEMU for ARM)
**Speed target:** >10 MIPS

### Core Concept

Execute instructions functionally (correct architectural state) at near-native speed.
At **miss events**, compute timing analytically using a mechanistic model of the OoO window.

```
Miss events:
  L1D cache miss  → add miss latency, model MLP (memory-level parallelism)
  L2 cache miss   → add L2 miss penalty
  LLC miss        → add DRAM latency
  Branch mispredict → add flush + refill penalty
  TLB miss        → add PTW penalty
  Structural hazard → add stall cycles
```

Between miss events: IPC ≈ pipeline-width-limited IPC (issue_width / 1.0 + structural_overhead).

### CPI Stack Output

Every interval produces a CPI breakdown:
```
Interval [0x80001000 – 0x80002000]:
  Instructions:  4,096
  Base IPC:      3.2 (4-wide issue, ~80% utilization)
  L1D misses:    +0.41 CPI  (12 misses × 34 cycles avg = 408 stall cycles)
  L2 misses:     +0.18 CPI  (3 misses × 240 cycles = 720 stall cycles)
  Branch miss:   +0.23 CPI  (8 mispredicts × 12 cycle penalty)
  TLB miss:      +0.04 CPI
  Frontend:      +0.02 CPI
  ─────────────────────────
  Simulated IPC: 2.32
```

### Internal Structure

```rust
pub struct IntervalTiming {
    profile: Arc<MicroarchProfile>,

    // Per-interval counters
    insn_count: u64,
    l1d_misses: u64,
    l2_misses: u64,
    llc_misses: u64,
    branch_mispredicts: u64,
    tlb_misses: u64,

    // OoO instruction window model
    window: OoOWindow,

    // Simulated time
    tick: u64,

    // CPI stack accumulator
    cpi_stack: CpiStack,
}

/// Models the OoO instruction window — tracks dependency chains,
/// identifies critical path length, estimates structural hazards.
pub struct OoOWindow {
    rob_size: u16,
    issue_width: u8,
    in_flight: VecDeque<InFlightInsn>,
}

impl TimingModel for IntervalTiming {
    #[inline(always)]
    fn on_memory_access(&mut self, addr: u64, _cycles: u64) {
        // Check in L1D cache
        let (hit, level) = self.profile.l1d.lookup(addr);
        if !hit {
            self.l1d_misses += 1;
            let penalty = self.profile.l2.lookup(addr).0
                .map(|_| self.profile.l2.hit_latency)
                .unwrap_or(self.profile.dram_latency_cycles);
            self.tick += penalty;
            self.cpi_stack.add_miss(level, penalty);
        }
    }

    #[inline(always)]
    fn on_branch_mispredict(&mut self, _penalty: u64) {
        self.branch_mispredicts += 1;
        let penalty = self.profile.branch_mispredict_penalty;
        self.tick += penalty;
        self.cpi_stack.add_branch_miss(penalty);
    }
}
```

### Multicore Support

Each `HelmEngine<IntervalTimed>` runs independently (temporal decoupling).
Synchronization at quantum boundaries. Shared LLC model tracks inter-core misses.
This matches Sniper's architecture — why it scales to 16+ cores at 2 MIPS.

---

## `Accurate` Mode — Cycle-Accurate OoO Pipeline

**Equivalent to:** PTLsim depth + Sniper memory hierarchy + calibrated µarch profiles
**Purpose:** Microarchitecture research, RTL correlation, hardware validation.
**IPC accuracy target:** <10% IPC error vs. real hardware after µarch calibration
**Speed target:** >200 KIPS

### Pipeline Stages (PTLsim-inspired)

```
Fetch → Decode → Rename → Dispatch → Issue → Execute → Writeback → Commit
  ↑         ↑        ↑          ↑         ↑         ↑           ↑         ↑
  BP     µop split  RAT       ROB/RS  Sched     FU pool     WB buffer   ROB head
```

Each stage is a separate simulation step per cycle. The simulator advances one cycle at a time, updating all stage pipeline registers.

### Pipeline Structures Modeled

```rust
pub struct AccuratePipeline {
    // Fetch
    fetch_width: u8,
    fetch_buf: VecDeque<FetchedInsn>,
    branch_predictor: Box<dyn BranchPredictor>,   // TAGE, gshare, LTAGE
    btb: BranchTargetBuffer,

    // Decode
    decode_width: u8,

    // Rename
    register_alias_table: RAT,
    free_list: FreeList,

    // ROB (Re-Order Buffer)
    rob: RingBuffer<RobEntry>,
    rob_size: u16,

    // Reservation Stations / Issue Queue
    issue_queue: IssueQueue,
    issue_width: u8,

    // Functional Units
    int_alu:   FunctionalUnit,   // count, latency
    int_mul:   FunctionalUnit,
    load_unit: FunctionalUnit,
    store_unit: FunctionalUnit,
    fp_alu:    FunctionalUnit,
    fp_mul:    FunctionalUnit,

    // Load-Store Queue
    lsq: LoadStoreQueue,
    lsq_size: u16,

    // Commit
    commit_width: u8,

    // Cache hierarchy (full timing model)
    l1i: CacheModel,
    l1d: CacheModel,
    l2:  CacheModel,
    llc: CacheModel,
    dram: DramModel,

    // TLB
    itlb: TlbModel,
    dtlb: TlbModel,
    l2tlb: TlbModel,

    // Prefetchers
    l1d_prefetcher: Box<dyn Prefetcher>,   // stride, stream, next-line
    l2_prefetcher: Box<dyn Prefetcher>,

    // Simulated time
    tick: u64,

    // Stats
    stats: PipelineStats,
}
```

### Per-Cycle Execution

```rust
impl AccuratePipeline {
    /// Advance one cycle — updates all pipeline stages
    pub fn step_cycle(&mut self) -> Option<StopReason> {
        self.commit_stage();   // oldest first
        self.writeback_stage();
        self.execute_stage();
        self.issue_stage();
        self.dispatch_stage();
        self.rename_stage();
        self.decode_stage();
        self.fetch_stage();

        self.tick += 1;
        None
    }
}
```

### Branch Predictor Pluggability

```rust
pub trait BranchPredictor: Send {
    fn predict(&mut self, pc: u64) -> (u64, bool);     // (target, taken)
    fn update(&mut self, pc: u64, target: u64, taken: bool, was_correct: bool);
    fn name(&self) -> &'static str;
}

// Implementations:
pub struct TagePredictor { ... }      // TAGE — closest to modern hardware
pub struct GsharePredictor { ... }    // simple, fast
pub struct LtagePredictor { ... }     // LTAGE with loop predictor
pub struct PerfectPredictor { ... }   // oracle — for upper-bound studies
pub struct StaticPredictor { ... }    // always not-taken — lower bound

// Selected from MicroarchProfile:
// profile.branch_predictor = BpConfig::Tage { tables: 7, ... }
```

### Difference from gem5 O3 (What We Fix)

| gem5 O3 Bug | Helm Fix |
|-------------|----------|
| µ-op fusion not modeled (x86 only) | N/A for RISC-V/ARM — regular encodings |
| TAGE-SC-L lacks speculative history unwinding | Implement full speculative TAGE with proper history rollback |
| Wrong µ-op class labels (FP mul → FP add) | Correct functional unit assignment from ISA spec |
| No µ-op cache | For ARM: model fetch buffer (FIFO of decoded insns) |
| Classic cache race conditions | Use event-driven timing cache model, no protocol shortcuts |
| Alpha 21264 heritage | Design from scratch for modern core profiles |

---

## `MicroarchProfile` — The Calibration Layer

Every timing model (Interval + Accurate) is parameterized by a `MicroarchProfile`.
Swapping a JSON config → simulate a different microarchitecture. No recompile.

```rust
pub struct MicroarchProfile {
    pub name:         &'static str,   // "sifive-u74", "cortex-a72", "xiangshan"

    // Pipeline
    pub pipeline_depth:     u8,       // fetch-to-commit stages
    pub fetch_width:        u8,
    pub decode_width:       u8,
    pub issue_width:        u8,
    pub commit_width:       u8,
    pub rob_size:           u16,
    pub issue_queue_size:   u16,
    pub lsq_size:           u16,

    // Branch prediction
    pub branch_predictor:   BpConfig,
    pub btb_entries:        u32,
    pub ras_size:           u16,
    pub mispredict_penalty: u8,       // cycles

    // Functional units
    pub int_alu_count:    u8,
    pub int_mul_latency:  u8,
    pub fp_alu_count:     u8,
    pub fp_mul_latency:   u8,
    pub load_units:       u8,
    pub store_units:      u8,

    // Caches
    pub l1i:  CacheConfig,
    pub l1d:  CacheConfig,
    pub l2:   CacheConfig,
    pub llc:  Option<CacheConfig>,

    // TLBs
    pub itlb: TlbConfig,
    pub dtlb: TlbConfig,

    // Prefetchers
    pub l1d_prefetcher: PrefetchConfig,
    pub l2_prefetcher:  PrefetchConfig,

    // Memory
    pub dram_latency_ns: f64,
    pub dram_bandwidth_gbps: f64,
    pub dram_type:       DramType,   // DDR4, LPDDR5, HBM2, etc.
}

pub struct CacheConfig {
    pub size_kb:     u32,
    pub assoc:       u8,
    pub line_size:   u8,    // bytes, default 64
    pub hit_latency: u8,    // cycles
    pub mshrs:       u8,    // miss status holding registers
    pub inclusive:   bool,
}
```

### Bundled Profiles (shipped with Helm-ng)

```
profiles/
  sifive-u74.json      — SiFive U74 (in-order, RISC-V)
  xiangshan.json       — XiangShan (OoO, RISC-V)
  cortex-a53.json      — ARM Cortex-A53 (in-order)
  cortex-a72.json      — ARM Cortex-A72 (OoO)
  cortex-a76.json      — ARM Cortex-A76 (OoO)
  generic-inorder.json — generic in-order for early exploration
  generic-ooo.json     — generic 4-wide OoO for early exploration
```

---

## `helm validate` — Calibration CLI

```bash
# Run validation suite against a profile, compare to real hardware reference
helm validate \
  --profile profiles/cortex-a72.json \
  --benchmarks coremark,dhrystone,stream,mibench-auto \
  --reference hw-counters/rpi4-cortex-a72.json \
  --mode accurate \
  --output validation-report.json
```

Output:
```
Validation Report: cortex-a72 — Accurate Mode
════════════════════════════════════════════
Benchmark     IPC (Sim)  IPC (HW)  Error    CPI Stack diff
──────────────────────────────────────────────────────────
coremark        2.31       2.45    -5.7%    Branch: +0.03
dhrystone       1.89       1.93    -2.1%    L1D:    +0.01
stream-copy     0.41       0.38    +7.9%    DRAM:   -0.08
mibench-auto    1.12       1.28   -12.5%    Branch: +0.09
──────────────────────────────────────────────────────────
Overall MAPE:   7.1%

Recommendations:
  → Branch predictor MPKI too high (5.2 vs HW 3.8): increase BTB to 4096 entries
  → DRAM latency underestimated: increase dram_latency_ns from 60 to 72
```

---

## Accuracy Targets (Final)

| Mode | RISC-V (simple core) | RISC-V (OoO) | ARM (in-order) | ARM (OoO) |
|------|---------------------|--------------|----------------|-----------|
| Virtual | correctness | correctness | correctness | correctness |
| Interval | <12% MAPE | <18% MAPE | <12% MAPE | <18% MAPE |
| Accurate (default profile) | <10% IPC err | <15% IPC err | <10% IPC err | <15% IPC err |
| Accurate (calibrated profile) | <5% IPC err | <10% IPC err | <7% IPC err | <12% IPC err |

**Validation oracle:** Spike (RISC-V reference ISS) for functional; real hardware (SiFive HiFive Unmatched, Raspberry Pi 4) for timing.
