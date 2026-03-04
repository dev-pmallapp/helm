# Accuracy Levels: FE / APE / CAE

HELM separates **execution mode** from **timing accuracy**.

## Execution Modes

| Mode | Acronym | Description |
|------|---------|-------------|
| Functional Emulation | **FE** | Run instructions, no OS, no timing |
| Syscall Emulation | **SE** | User-mode binary + emulated Linux syscalls |

SE mode is the primary way to run real binaries.  FE describes the
timing accuracy when no microarchitectural detail is modelled.

## Timing Accuracy Levels

| Level | Acronym | Full Name | Speed | What is modelled |
|-------|---------|-----------|-------|------------------|
| L0 | **FE** | Functional Emulation | 100-1000 MIPS | IPC=1, flat memory, no timing |
| L1-L2 | **APE** | Approximate Emulation | 1-100 MIPS | Cache latencies, device stalls, optional pipeline |
| L3 | **CAE** | Cycle-Accurate Emulation | 0.1-1 MIPS | Full pipeline, bypass network, store buffer |

```
        FE                    APE                     CAE
  ┌─────────────┐   ┌──────────────────┐   ┌───────────────────┐
  │  IPC = 1    │   │ cache hit/miss   │   │ full OoO pipeline  │
  │  flat mem   │   │ device stalls    │   │ bypass network     │
  │  no timing  │   │ optional BP      │   │ store buffer       │
  │             │   │ simplified OoO   │   │ coherence          │
  │  QEMU-like  │   │ Simics-like      │   │ gem5-O3CPU-like    │
  └─────────────┘   └──────────────────┘   └───────────────────┘
  100-1000 MIPS        1-100 MIPS             0.1-1 MIPS
```

---

## L0 — FE (Functional Emulation)

**Purpose:** Run binaries fast.  Boot an OS, run test suites, bring up
workloads.  No timing information at all.

**What is modelled:**
- Correct functional execution of every instruction.
- Syscall emulation (when combined with SE mode).
- Dynamic binary translation for speed.

**What is NOT modelled:**
- Cache hierarchy, TLB, memory latency.
- Pipeline stages, dependencies, speculation.
- Branch prediction, misprediction penalties.

**Speed target:** 100-1000 MIPS (host-dependent).

**When to use:** Software development, debugging, workload preparation,
large-scale functional testing.

**Rust:** `AccuracyLevel::FE`, `FeModel`

**Python:**
```python
TimingMode.fe()
```

---

## L1/L2 — APE (Approximate Emulation)

**Purpose:** Get approximate performance numbers without paying the cost
of full microarchitectural simulation.

### L1 — APE (stall-annotated)

Each instruction still costs 1 base cycle, but memory operations incur
stall cycles based on cache hit/miss.

**Modelled:** L1/L2/L3 hit/miss latencies, DRAM latency, device MMIO
stall cycles.

### L2 — APE (detailed)

Adds a simplified OoO pipeline model: instruction-level parallelism,
branch prediction, and a shallow ROB.  Not cycle-exact, but captures
the dominant performance effects.

**Modelled:** Everything in L1, plus simplified issue/retire, branch
predictor, instruction queue contention.

**Speed target:** L1: 10-100 MIPS, L2: 1-10 MIPS.

**Accuracy:** IPC within ~20-50 % of hardware.

**When to use:** Design-space sweeps, sensitivity studies, performance
estimation, cache-hierarchy exploration.

**Rust:** `AccuracyLevel::APE`, `ApeModel`

**Python:**
```python
TimingMode.ape()
TimingMode.ape(l1_latency=3, dram_latency=200)
```

---

## L3 — CAE (Cycle-Accurate Emulation)

**Purpose:** Cycle-accurate microarchitectural simulation for hardware
validation and deep architecture research.

**Modelled:**
- Full out-of-order pipeline: fetch, decode, rename, dispatch, issue,
  execute, complete, commit.
- Register renaming with physical register file.
- Reorder buffer, instruction queues, load/store queues.
- Branch prediction and speculative execution with misprediction
  recovery and pipeline flush.
- Multi-level cache hierarchy with coherence.
- Bypass/forwarding network.
- Store buffer and memory disambiguation.

**Speed target:** 0.1-1 MIPS.

**Accuracy:** IPC within ~2-10 % of hardware.

**When to use:** Microarchitectural design validation, speculation
studies, memory-ordering research, precise what-if analysis.

**Rust:** `AccuracyLevel::CAE`

**Python:**
```python
TimingMode.cae()
```

---

## Switching at Runtime

Timing models are attached and detached per-core.  A common workflow:

1. Boot in **FE** (fast).
2. Switch to **APE** for a warmup phase.
3. Switch to **CAE** for the region of interest.
4. Switch back to **FE** to finish.

```python
sim.set_timing(TimingMode.fe())
sim.run(instructions=1_000_000_000)   # fast-forward

sim.set_timing(TimingMode.ape())
sim.run(instructions=10_000_000)      # warmup caches

sim.set_timing(TimingMode.cae())
results = sim.run(instructions=100_000_000)  # measure
```

---

## Comparison with Other Simulators

| Simulator | HELM equivalent | Notes |
|-----------|-----------------|-------|
| QEMU | FE | No timing at all |
| Simics (functional) | FE | Same: fast, correct, no timing |
| Simics (timing) | APE | Stall-cycle annotations |
| gem5 AtomicSimpleCPU | APE L1 | Fixed CPI + memory latencies |
| gem5 TimingSimpleCPU | APE L2 | Simplified pipeline |
| gem5 O3CPU | CAE | Full OoO pipeline model |
