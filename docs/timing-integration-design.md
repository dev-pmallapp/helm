# Timing Integration Design

Detailed design for connecting HELM's timing infrastructure (helm-timing)
to the execution engine (helm-engine) so that timing accuracy is a runtime
knob, not a compile-time choice.

---

## Problem Statement

HELM has all the timing building blocks already implemented in separate
crates, but they are not wired together:

| Component | Crate | Status |
|-----------|-------|--------|
| `TimingModel` trait (FE/APE/CAE) | helm-timing | Implemented, not consumed |
| `FeModel`, `ApeModel` | helm-timing | Implemented, never attached |
| `EventQueue` | helm-timing | Implemented, not used by engine |
| `SamplingController` | helm-timing | Implemented, not used by engine |
| `TemporalDecoupler` | helm-timing | Implemented, not used by engine |
| `Cache` (set-associative) | helm-memory | Implemented, not queried at runtime |
| `Pipeline` (ROB, rename, scheduler) | helm-pipeline | Implemented, tick-driven in `CoreSim` |
| `Aarch64Cpu.step()` (SE runner) | helm-isa + helm-engine | Works, but has no timing hooks |
| `Simulation::run_se()` | helm-engine | Stub — returns immediately |
| `Simulation::run_microarch()` | helm-engine | Tick-driven loop, no timing model |

The core problem: the SE execution loop (`se/linux.rs`) runs instructions
one at a time with `cpu.step()` and counts instructions, but never queries
a `TimingModel` for stall cycles, never probes the cache hierarchy, and
never advances virtual time. The microarch loop (`sim.rs::run_microarch`)
ticks the pipeline but doesn't integrate the `TimingModel` trait either.

---

## Design Goals

1. **Attach/detach timing at runtime** — switch from FE to APE to CAE
   mid-simulation without checkpointing (Simics pattern)
2. **Timing is additive** — the functional core always produces correct
   architectural state; timing only adds "how long did it take"
3. **Event-driven** — use `EventQueue` for devices and DRAM, not
   cycle-driven ticking when most cycles are idle
4. **Temporal decoupling** — multi-core simulations use `TemporalDecoupler`
   to run cores independently within a quantum
5. **Sampling** — `SamplingController` governs the FE→Warmup→Detailed→Cooldown
   phase transitions
6. **Zero overhead in FE mode** — when no timing model is attached, the
   execution path should be identical to the current `cpu.step()` loop

---

## Architecture

### Two-Layer Design

```
┌───────────────────────────────────────────────────────────────┐
│                    Functional Core                             │
│                                                               │
│  Aarch64Cpu.step()  ──►  correct architectural state          │
│  (or IsaFrontend.decode() for MicroOp path)                   │
│                                                               │
│  This layer is ALWAYS active. It produces:                     │
│  - new register values                                        │
│  - memory reads/writes (via AddressSpace)                     │
│  - branch outcomes (taken/not-taken, target PC)               │
│  - syscall traps                                              │
│                                                               │
│  It does NOT know about cycles, latency, or pipeline state.   │
└────────────────────┬──────────────────────────────────────────┘
                     │ InstructionOutcome
                     ▼
┌───────────────────────────────────────────────────────────────┐
│                    Timing Layer (optional)                     │
│                                                               │
│  TimingModel.instruction_latency(uop) ──► stall cycles       │
│  TimingModel.memory_latency(addr, sz)  ──► cache latency     │
│  TimingModel.branch_misprediction_penalty() ──► flush cost   │
│                                                               │
│  This layer is DETACHABLE. When detached, IPC = 1.            │
│  When attached, it advances virtual_time by stall cycles.     │
│                                                               │
│  Three implementations:                                       │
│  - FeModel:  always returns 1 / 0 / 0                         │
│  - ApeModel: queries Cache hierarchy for memory latency       │
│  - CaeModel: drives Pipeline (ROB, rename, scheduler)         │
└───────────────────────────────────────────────────────────────┘
```

### InstructionOutcome

After each `cpu.step()`, the functional core produces an outcome that
the timing layer can consume without re-executing the instruction:

```rust
// helm-core/src/timing_outcome.rs (new file)

/// What the functional core did on this instruction.
/// The timing layer uses this to compute latency without re-executing.
#[derive(Debug, Clone)]
pub struct InstructionOutcome {
    pub pc: Addr,
    pub insn_bytes: u32,        // raw 32-bit instruction word
    pub class: InsnClass,       // opcode category for latency lookup
    pub mem_access: Option<MemAccess>,  // load/store details
    pub branch: Option<BranchOutcome>,  // branch resolution
    pub is_syscall: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum InsnClass {
    IntAlu,
    IntMul,
    IntDiv,
    FpAlu,
    FpMul,
    FpDiv,
    Load,
    Store,
    Branch,
    CondBranch,
    Syscall,
    Nop,
    Fence,
    Simd,
}

#[derive(Debug, Clone)]
pub struct MemAccess {
    pub addr: Addr,
    pub size: usize,
    pub is_write: bool,
}

#[derive(Debug, Clone)]
pub struct BranchOutcome {
    pub target: Addr,
    pub taken: bool,
    /// Predicted target from the branch predictor (if APE/CAE)
    pub predicted_taken: Option<bool>,
}
```

### Timing-Aware Execution Loop

The SE runner gains an optional timing model parameter. The loop structure
changes from:

```
current:  cpu.step() → insn_count += 1 → repeat
```

to:

```
proposed: cpu.step() → outcome = classify(cpu) →
          stall = timing.compute(outcome) →
          virtual_time += stall →
          sampling.advance(1) → check phase →
          decoupler.advance_core(id, stall) → check sync →
          repeat
```

```rust
// helm-engine/src/se/linux.rs — modified execution loop (pseudocode)

pub fn run_aarch64_se_timed(
    binary_path: &str,
    argv: &[&str],
    envp: &[&str],
    max_insns: u64,
    timing: Option<&mut dyn TimingModel>,
    sampling: Option<&mut SamplingController>,
    plugins: Option<&PluginRegistry>,
) -> Result<SeTimedResult, HelmError> {

    // ... ELF loading, CPU setup (unchanged) ...

    let mut virtual_time: u64 = 0;
    let mut insn_count: u64 = 0;

    loop {
        if insn_count >= max_insns { break; }

        // Phase check: should we collect stats?
        let phase = sampling.as_mut()
            .map(|s| s.phase())
            .unwrap_or(SamplingPhase::Detailed);

        let pc_before = cpu.regs.pc;

        // ─── Functional execution (always) ───
        match cpu.step(&mut mem) {
            Ok(()) => {
                insn_count += 1;

                // ─── Timing annotation (if model attached) ───
                if let Some(ref mut tm) = timing {
                    if phase != SamplingPhase::FastForward {
                        let outcome = classify_instruction(
                            &cpu, pc_before, &mem
                        );
                        let stall = tm.instruction_latency_from_outcome(
                            &outcome
                        );

                        // Memory access latency
                        let mem_stall = if let Some(ma) = &outcome.mem_access {
                            tm.memory_latency(ma.addr, ma.size, ma.is_write)
                        } else {
                            0
                        };

                        // Branch misprediction
                        let branch_stall = if let Some(br) = &outcome.branch {
                            if let Some(predicted) = br.predicted_taken {
                                if predicted != br.taken {
                                    tm.branch_misprediction_penalty()
                                } else { 0 }
                            } else { 0 }
                        } else { 0 };

                        virtual_time += stall + mem_stall + branch_stall;
                    } else {
                        virtual_time += 1; // FE: IPC=1
                    }
                } else {
                    virtual_time += 1; // No timing model: IPC=1
                }

                // ─── Phase transition ───
                if let Some(ref mut s) = sampling {
                    s.advance(1);
                }

                // ─── Plugin callbacks (unchanged) ───
                // ...
            }
            Err(HelmError::Syscall { number, .. }) => {
                // ... syscall handling (unchanged) ...
                virtual_time += 50; // syscall overhead estimate
            }
            // ... error handling (unchanged) ...
        }
    }

    Ok(SeTimedResult {
        exit_code: ...,
        instructions_executed: insn_count,
        virtual_cycles: virtual_time,
        ipc: insn_count as f64 / virtual_time as f64,
    })
}
```

### classify_instruction

This function inspects the CPU state after `step()` to determine
what kind of instruction was executed, without re-decoding:

```rust
// helm-engine/src/se/classify.rs (new file)

pub fn classify_instruction(
    cpu: &Aarch64Cpu,
    pc_before: Addr,
    mem: &AddressSpace,
) -> InstructionOutcome {
    let mut ibuf = [0u8; 4];
    let _ = mem.read(pc_before, &mut ibuf);
    let insn = u32::from_le_bytes(ibuf);

    let op0 = (insn >> 25) & 0xF;

    let class = match op0 {
        0b1000 | 0b1001 => classify_dp_imm(insn),
        0b1010 | 0b1011 => classify_branch(insn),
        0b0100 | 0b0110 | 0b1100 | 0b1110 => classify_ldst(insn),
        0b0101 | 0b1101 => classify_dp_reg(insn),
        0b0111 | 0b1111 => InsnClass::Simd,
        _ => InsnClass::Nop,
    };

    let mem_access = extract_mem_access(cpu, insn, op0);
    let branch = extract_branch_outcome(cpu, pc_before, insn, op0);

    InstructionOutcome {
        pc: pc_before,
        insn_bytes: insn,
        class,
        mem_access,
        branch,
        is_syscall: false,
    }
}
```

---

## Timing Model Implementations

### FeModel (existing — no changes needed)

Returns 1 for instruction latency, 0 for everything else. When attached
as the timing model, virtual time advances by 1 per instruction. This is
the default when no timing model is explicitly set.

### ApeModel (enhanced — connects to helm-memory Cache)

The current `ApeModel` always returns `l1_latency`. The enhanced version
probes the actual `Cache` hierarchy:

```rust
// helm-timing/src/model.rs — enhanced ApeModel

pub struct ApeModelDetailed {
    pub cache_hierarchy: MemorySubsystem,
    pub branch_predictor: BranchPredictor,
    pub bp_penalty: u64,

    // Per-opcode base latencies (configurable)
    pub int_alu_latency: u64,   // default 1
    pub int_mul_latency: u64,   // default 3
    pub int_div_latency: u64,   // default 12
    pub fp_alu_latency: u64,    // default 4
    pub fp_mul_latency: u64,    // default 5
    pub fp_div_latency: u64,    // default 15
    pub simd_latency: u64,      // default 3
}

impl TimingModel for ApeModelDetailed {
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::APE
    }

    fn instruction_latency(&mut self, uop: &MicroOp) -> u64 {
        match uop.opcode {
            Opcode::IntAlu => self.int_alu_latency,
            Opcode::IntMul => self.int_mul_latency,
            Opcode::IntDiv => self.int_div_latency,
            Opcode::FpAlu  => self.fp_alu_latency,
            Opcode::FpMul  => self.fp_mul_latency,
            Opcode::FpDiv  => self.fp_div_latency,
            Opcode::Load | Opcode::Store => 1, // base; mem_latency added separately
            _ => 1,
        }
    }

    fn memory_latency(&mut self, addr: Addr, _size: usize, is_write: bool) -> u64 {
        // Walk the actual cache hierarchy
        if let Some(ref mut l1d) = self.cache_hierarchy.l1d {
            if l1d.access(addr, is_write) == CacheAccessResult::Hit {
                return l1d.latency;
            }
        }
        if let Some(ref mut l2) = self.cache_hierarchy.l2 {
            if l2.access(addr, is_write) == CacheAccessResult::Hit {
                return l2.latency;
            }
        }
        if let Some(ref mut l3) = self.cache_hierarchy.l3 {
            if l3.access(addr, is_write) == CacheAccessResult::Hit {
                return l3.latency;
            }
        }
        self.cache_hierarchy.dram_latency
    }

    fn branch_misprediction_penalty(&mut self) -> u64 {
        self.bp_penalty
    }
}
```

### CaeModel (new — wraps helm-pipeline)

CAE mode does not use the simple stall-annotation pattern. Instead it
drives the full pipeline cycle-by-cycle. The `TimingModel` trait is
extended with a `CaeModel` that wraps `Pipeline`:

```rust
// helm-timing/src/model.rs — CaeModel

pub struct CaeModel {
    pub pipeline: Pipeline,
    pub cache_hierarchy: MemorySubsystem,
    /// Instructions buffered from the functional core, waiting to be
    /// consumed by the pipeline.
    insn_buffer: VecDeque<MicroOp>,
    /// The pipeline runs cycle-by-cycle; this tracks how many cycles
    /// the current instruction took to complete.
    cycles_this_insn: u64,
}

impl TimingModel for CaeModel {
    fn accuracy(&self) -> AccuracyLevel {
        AccuracyLevel::CAE
    }

    fn instruction_latency(&mut self, uop: &MicroOp) -> u64 {
        // Feed uop into the pipeline
        // The pipeline models rename → dispatch → issue → execute → commit
        // The number of cycles until this uop commits is the latency.

        // Allocate ROB entry
        let rob_idx = match self.pipeline.rob.allocate(uop.clone()) {
            Some(idx) => idx,
            None => {
                // ROB full — stall until head commits
                let stall = self.drain_until_rob_free();
                let idx = self.pipeline.rob.allocate(uop.clone()).unwrap();
                self.cycles_this_insn += stall;
                idx
            }
        };

        // Rename
        let phys_dest = uop.dest.map(|d| self.pipeline.rename.rename_dest(d));
        let phys_srcs: Vec<u32> = uop.sources.iter()
            .map(|&s| self.pipeline.rename.lookup_src(s))
            .collect();

        // Insert into scheduler
        if !self.pipeline.scheduler.insert(uop.clone(), rob_idx) {
            let stall = self.drain_until_iq_free();
            self.pipeline.scheduler.insert(uop.clone(), rob_idx);
            self.cycles_this_insn += stall;
        }

        // Wakeup, select, execute
        self.pipeline.scheduler.wakeup(&phys_srcs.iter().map(|&p| p).collect::<Vec<_>>());
        let _issued = self.pipeline.scheduler.select(
            self.pipeline.config.width as usize
        );

        // Complete
        self.pipeline.rob.complete(rob_idx);

        // Commit (in-order)
        let committed = self.pipeline.rob.try_commit();
        let latency = if committed.is_empty() {
            // Not yet committable — estimate cycles
            self.estimate_commit_latency(uop)
        } else {
            1 + self.cycles_this_insn
        };
        self.cycles_this_insn = 0;
        latency
    }

    fn memory_latency(&mut self, addr: Addr, _size: usize, is_write: bool) -> u64 {
        // Same as ApeModelDetailed — walk cache hierarchy
        // ... (same implementation) ...
        0 // placeholder
    }

    fn branch_misprediction_penalty(&mut self) -> u64 {
        // Flush ROB, rename, scheduler
        let depth = self.pipeline.rob.try_commit().len() as u64;
        self.pipeline.rename = RenameUnit::new();
        depth.max(self.pipeline.config.rob_size as u64 / 2)
    }

    fn end_of_quantum(&mut self) {
        // Drain any remaining in-flight instructions at quantum boundary
        while !self.pipeline.rob.is_empty() {
            self.pipeline.rob.try_commit();
        }
    }
}
```

---

## Wiring: Simulation::run() Entry Point

The `Simulation` struct gains timing model and sampling fields:

```rust
// helm-engine/src/sim.rs — proposed changes

pub struct Simulation {
    pub config: PlatformConfig,
    pub binary_path: String,
    cores: Vec<CoreSim>,
    stats: StatsCollector,

    // NEW: timing integration
    timing_model: Option<Box<dyn TimingModel>>,
    sampling: Option<SamplingController>,
    decoupler: Option<TemporalDecoupler>,
    event_queue: EventQueue,
}

impl Simulation {
    /// Attach a timing model. Can be called before or during simulation.
    pub fn set_timing(&mut self, model: Box<dyn TimingModel>) {
        log::info!("Timing model set: {:?}", model.accuracy());
        self.timing_model = Some(model);
    }

    /// Detach the timing model (return to FE mode).
    pub fn clear_timing(&mut self) {
        self.timing_model = None;
    }

    /// Configure sampled simulation phases.
    pub fn set_sampling(&mut self, ff: u64, warmup: u64, detailed: u64, cooldown: u64) {
        self.sampling = Some(SamplingController::new(ff, warmup, detailed, cooldown));
    }

    pub fn run(&mut self, max_cycles: u64) -> Result<SimResults> {
        match self.config.exec_mode {
            ExecMode::SE => {
                // Dispatch to the timing-aware SE runner
                let result = run_aarch64_se_timed(
                    &self.binary_path,
                    &[], &[],
                    max_cycles,
                    self.timing_model.as_deref_mut(),
                    self.sampling.as_mut(),
                    None, // plugins
                )?;
                // Convert SeTimedResult to SimResults
                Ok(self.build_results(result))
            }
            ExecMode::CAE => {
                self.run_microarch_timed(max_cycles)
            }
        }
    }

    fn run_microarch_timed(&mut self, max_cycles: u64) -> Result<SimResults> {
        let num_cores = self.cores.len();

        // Create temporal decoupler if multi-core
        if num_cores > 1 && self.decoupler.is_none() {
            self.decoupler = Some(TemporalDecoupler::new(num_cores, 10_000));
        }

        for _quantum in 0..(max_cycles / 10_000 + 1) {
            // Run each core for one quantum
            for core in &mut self.cores {
                if core.halted { continue; }

                for _ in 0..10_000 {
                    let events = core.tick();
                    for event in &events {
                        self.stats.on_event(event);
                    }
                    if core.halted { break; }
                }
            }

            // Synchronise cores
            if let Some(ref mut decoupler) = self.decoupler {
                for (i, core) in self.cores.iter().enumerate() {
                    decoupler.advance_core(i, core.cycle);
                }
            }

            // Process scheduled events
            while let Some(event) = self.event_queue.pop() {
                if event.timestamp > self.cores[0].cycle {
                    // Re-schedule for later
                    self.event_queue.schedule(
                        event.timestamp, event.priority, event.tag
                    );
                    break;
                }
                // Handle event (device completion, DRAM response, etc.)
                self.handle_timed_event(event);
            }

            if self.cores.iter().all(|c| c.halted) {
                break;
            }
        }

        Ok(self.stats.results.clone())
    }
}
```

---

## Event Queue Integration

The `EventQueue` bridges timing-sensitive components (devices, DRAM
responses) with the execution loop. Events are scheduled with a future
cycle and a tag identifying the event type:

```rust
// Event tags (conventions)
const EVENT_DRAM_RESPONSE: u64 = 1;
const EVENT_DEVICE_IRQ: u64 = 2;
const EVENT_TIMER_TICK: u64 = 3;
const EVENT_DMA_COMPLETE: u64 = 4;

// Scheduling a DRAM response
fn on_cache_miss(&mut self, addr: Addr, virtual_time: u64) -> u64 {
    let response_time = virtual_time + self.dram_latency;
    self.event_queue.schedule(response_time, 0, EVENT_DRAM_RESPONSE);
    self.dram_latency  // stall the core for this many cycles
}

// Scheduling a device interrupt
fn on_uart_tx(&mut self, virtual_time: u64) {
    let irq_time = virtual_time + self.uart_tx_latency;
    self.event_queue.schedule(irq_time, 0, EVENT_DEVICE_IRQ);
}
```

---

## Multi-Core Timing: Temporal Decoupling

For multi-core SE-mode simulation (when threads are supported), each core
runs its timing model independently within a quantum:

```
Core 0:  [────── quantum (10K cycles) ──────] sync │ [──────...
Core 1:  [────── quantum (10K cycles) ──────] sync │ [──────...

At sync:
  global_time = min(core_0_time, core_1_time)
  process cross-core events (IPIs, shared memory)
  event_queue.drain_until(global_time)
```

The `TemporalDecoupler::needs_sync(core_id)` check runs after every
instruction. When it returns true, the core yields to the synchronisation
barrier. This check is O(1) — a single atomic load and comparison.

---

## Sampling Controller Integration

The sampling controller drives timing model attachment/detachment:

```
Phase         Timing Model    Cache    Stats Collection
────────────  ──────────────  ───────  ─────────────────
FastForward   None (FE)       Cold     Off
Warmup        ApeModel        Warming  Off
Detailed      ApeModel/CAE    Warm     On
Cooldown      ApeModel        Warm     Off → drain
Done          None            —        Finalize
```

The phase transitions are driven by instruction count via
`SamplingController::advance(1)`. On each transition:

```rust
match new_phase {
    FastForward => {
        simulation.clear_timing();
        stats.pause();
    }
    Warmup => {
        simulation.set_timing(Box::new(ApeModelDetailed::from_config(&config)));
        stats.pause();
    }
    Detailed => {
        // Optionally upgrade to CAE
        if config.detailed_accuracy == AccuracyLevel::CAE {
            simulation.set_timing(Box::new(CaeModel::from_config(&config)));
        }
        stats.start();
    }
    Cooldown => {
        stats.pause();
    }
    Done => {
        simulation.clear_timing();
        stats.finalize();
    }
}
```

---

## Configuration API

### Rust

```rust
use helm_engine::Simulation;
use helm_timing::model::{ApeModelDetailed, CaeModel, FeModel};
use helm_timing::sampling::SamplingController;

let mut sim = Simulation::new(config, "binary".into());

// Option A: Simple run with APE timing
sim.set_timing(Box::new(ApeModelDetailed::default()));
let results = sim.run(1_000_000)?;

// Option B: Sampled run
sim.set_sampling(
    1_000_000_000, // fast-forward
    10_000_000,    // warmup
    100_000_000,   // detailed (CAE)
    1_000_000,     // cooldown
);
let results = sim.run(u64::MAX)?; // run until all phases complete

// Option C: Dynamic switching
sim.set_timing(Box::new(FeModel));
sim.run(1_000_000_000)?;           // boot fast
sim.set_timing(Box::new(ApeModelDetailed::default()));
sim.run(10_000_000)?;              // warmup caches
sim.set_timing(Box::new(CaeModel::from_config(&config)));
let results = sim.run(100_000_000)?; // measure
```

### Python

```python
from helm import Simulation, TimingMode, SamplingConfig

sim = Simulation.from_binary("./workload", isa="arm64", mode="se")

# Simple APE run
sim.timing = TimingMode.approximate(
    l1d_latency=4, l2_latency=12, l3_latency=40, dram_latency=200,
    branch_penalty=15,
    int_mul_latency=3, int_div_latency=12,
)
results = sim.run(instructions=1_000_000)
print(f"IPC: {results.ipc():.2f}")

# Sampled run with automatic phase transitions
sim.sampling = SamplingConfig(
    fast_forward=1_000_000_000,
    warmup=10_000_000,
    detailed=100_000_000,
    cooldown=1_000_000,
    detailed_accuracy="cae",
)
results = sim.run()
```

### TOML Config File

```toml
[simulation]
binary = "./spec2017/gcc"
isa = "arm64"
mode = "se"

[timing]
level = "approximate"

[timing.latencies]
l1d = 4
l2 = 12
l3 = 40
dram = 200
branch_penalty = 15

[timing.instruction_latencies]
int_alu = 1
int_mul = 3
int_div = 12
fp_alu = 4
fp_mul = 5
fp_div = 15

[sampling]
fast_forward = 1_000_000_000
warmup = 10_000_000
detailed = 100_000_000
cooldown = 1_000_000
detailed_accuracy = "cae"

[cache.l1d]
size = "32KB"
associativity = 8
line_size = 64

[cache.l2]
size = "256KB"
associativity = 8
line_size = 64

[cache.l3]
size = "8MB"
associativity = 16
line_size = 64
```

---

## Implementation Phases

### Phase 1: InstructionOutcome + classify (Low effort, High impact)

1. Add `InstructionOutcome` struct to `helm-core` (or `helm-engine`)
2. Implement `classify_instruction()` for AArch64 by reading the
   instruction word at `pc_before` and mapping op0 bits
3. Modify `se/linux.rs` to call classify after each `cpu.step()`
4. Add `virtual_time` counter to `SeResult`
5. Add `Option<Box<dyn TimingModel>>` parameter to `run_aarch64_se_timed`
6. When timing model is present, compute `stall = tm.instruction_latency()`
   and advance virtual_time

**Test:** Run fish-shell with `FeModel` attached — `virtual_time`
should equal `insn_count` (IPC=1). No functional behaviour change.

### Phase 2: ApeModel + Cache hierarchy (Medium effort, High impact)

1. Enhance `ApeModel` with per-opcode latency table
2. Wire `ApeModel::memory_latency()` to `helm-memory::Cache::access()`
3. Build `MemorySubsystem::from_config()` at simulation startup and pass
   it to the timing model
4. Add branch predictor probe to the timing path
5. Report IPC, cache MPKI, branch MPKI in `SimResults`

**Test:** Run fish-shell with `ApeModelDetailed`. Verify IPC < 1.0 and
L1 hit rate > 90%. Compare cache miss counts to `valgrind --tool=cachegrind`.

### Phase 3: SamplingController integration (Low effort, Medium impact)

1. Add `SamplingController` to `Simulation`
2. On phase transition, swap timing models (FE ↔ APE ↔ CAE)
3. Pause/resume `StatsCollector` at phase boundaries
4. Report per-phase statistics

**Test:** Run with `fast_forward=1M, warmup=100K, detailed=1M`.
Verify stats only reflect the detailed phase.

### Phase 4: TemporalDecoupler for multi-core (Medium effort, Medium impact)

1. Add `TemporalDecoupler` to `Simulation`
2. In the SE loop, after each instruction, check `needs_sync(core_id)`
3. At sync points, drain the `EventQueue` up to `global_time`
4. Process cross-core events (shared memory writes, IPIs)

**Test:** Two cores running independent workloads. Verify `global_time`
is monotonically non-decreasing. Verify each core's virtual time stays
within `quantum_size` of global time.

### Phase 5: CaeModel with Pipeline integration (High effort, High impact)

1. Implement `CaeModel` wrapping `Pipeline`
2. Feed `MicroOp`s from the functional core into the pipeline
3. Track ROB fullness, scheduler stalls, rename pressure
4. Report pipeline utilisation metrics
5. Wire to `EventQueue` for out-of-order memory responses

**Test:** Run a known-IPC micro-benchmark (e.g., pointer-chasing load
chain). Verify IPC matches expected value for given cache + pipeline
config.

### Phase 6: EventQueue for devices and DRAM (Medium effort, Low impact)

1. Replace synchronous DRAM latency with `EventQueue` scheduling
2. Device MMIO returns a stall cycle count that schedules a completion event
3. Device interrupts scheduled as future events on the `EventQueue`

**Test:** UART TX schedules a completion event. Verify the interrupt
fires at the correct virtual time.

---

## Performance Expectations

| Configuration | Expected speed | Notes |
|---------------|---------------|-------|
| SE + no timing (current) | 150+ MIPS | Baseline, `cpu.step()` loop |
| SE + FeModel | 140+ MIPS | classify overhead ~5% |
| SE + ApeModel (simple) | 80-120 MIPS | Cache probe per load/store |
| SE + ApeModel (detailed) | 30-80 MIPS | + branch predictor + per-opcode latency |
| SE + CaeModel | 1-10 MIPS | Full pipeline per instruction |
| Multi-core SE + APE | 50-100 MIPS/core | Temporal decoupling amortises sync |

The critical performance constraint: in FE mode (`FeModel` or no model),
overhead from the timing infrastructure must be < 10% of the baseline
`cpu.step()` loop speed. This is achieved by:

- Making the timing model parameter `Option<&mut dyn TimingModel>` so
  the compiler can eliminate the branch when `None`
- Keeping `classify_instruction()` out of the fast path when no model
  is attached
- Using `likely()`/`unlikely()` hints at the phase-check branch

---

## Files Changed

| File | Change |
|------|--------|
| `helm-core/src/lib.rs` | Add `pub mod timing_outcome;` |
| `helm-core/src/timing_outcome.rs` | New: `InstructionOutcome`, `InsnClass`, `MemAccess`, `BranchOutcome` |
| `helm-core/src/types.rs` | No change (ExecMode stays SE/CAE) |
| `helm-timing/src/model.rs` | Add `ApeModelDetailed`, `CaeModel`; extend `TimingModel` trait with `instruction_latency_from_outcome()` |
| `helm-engine/src/sim.rs` | Add `timing_model`, `sampling`, `decoupler`, `event_queue` fields; wire into `run()` |
| `helm-engine/src/se/linux.rs` | Add `run_aarch64_se_timed()` alongside existing `run_aarch64_se()` |
| `helm-engine/src/se/classify.rs` | New: `classify_instruction()` for AArch64 |
| `helm-engine/Cargo.toml` | Add `helm-timing` dependency |
