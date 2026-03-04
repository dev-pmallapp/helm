# Accuracy Levels: Express / Recon / Signal

HELM provides three named accuracy tiers that cover the full spectrum
from functional emulation to cycle-accurate simulation.

## At a Glance

```
      Express              Recon               Signal
  ┌─────────────┐   ┌──────────────┐   ┌───────────────────┐
  │  IPC = 1    │   │ cache sims   │   │ full OoO pipeline  │
  │  flat mem   │   │ device stalls│   │ bypass network     │
  │  no timing  │   │ optional BP  │   │ store buffer       │
  │             │   │              │   │ coherence          │
  │  QEMU-like  │   │ Simics-like  │   │ gem5-O3CPU-like    │
  └─────────────┘   └──────────────┘   └───────────────────┘
  100-1000 MIPS       1-100 MIPS          0.1-1 MIPS
```

## L0 — Express

**Purpose:** Run binaries fast.  Boot an OS, run test suites, bring up
workloads.  No timing information at all.

**What is modelled:**
- Correct functional execution of every instruction.
- Syscall emulation (SE mode).
- Dynamic binary translation for speed.

**What is NOT modelled:**
- Cache hierarchy, TLB, memory latency.
- Pipeline stages, dependencies, speculation.
- Branch prediction, misprediction penalties.

**Speed target:** 100-1000 MIPS (host-dependent).

**When to use:** Software development, debugging, workload preparation,
large-scale functional testing.

**Rust type:** `ExpressModel` (`AccuracyLevel::Express`)

**Python:**
```python
TimingMode.express()
```

---

## L1/L2 — Recon

**Purpose:** Get approximate performance numbers without paying the cost
of full microarchitectural simulation.  Two sub-levels:

### L1 — Recon (stall-annotated)

Adds cache-miss and device-access latencies on top of Express.  Each
instruction still costs 1 base cycle, but memory operations incur stall
cycles based on cache hit/miss.

**Modelled:** L1/L2/L3 hit/miss latencies, DRAM latency, device MMIO
stall cycles.

### L2 — Recon Detailed

Adds a simplified OoO pipeline model: instruction-level parallelism,
branch prediction, and a shallow ROB.  Not cycle-exact, but captures
the dominant performance effects.

**Modelled:** Everything in L1, plus simplified issue/retire, branch
predictor (bimodal/TAGE), instruction queue contention.

**Speed target:** L1: 10-100 MIPS, L2: 1-10 MIPS.

**Accuracy:** IPC within ~20-50 % of hardware.  Good enough to rank
configurations and identify bottlenecks.

**When to use:** Design-space sweeps, sensitivity studies, performance
estimation, cache-hierarchy exploration.

**Rust types:** `ReconModel` (`AccuracyLevel::Recon`),
future `ReconDetailedModel` (`AccuracyLevel::ReconDetailed`).

**Python:**
```python
TimingMode.recon()
TimingMode.recon(l1_latency=3, dram_latency=200)
```

---

## L2/L3 — Signal

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

**Accuracy:** IPC within ~2-10 % of hardware (comparable to gem5 O3CPU).

**When to use:** Microarchitectural design validation, speculation
studies, memory-ordering research, precise what-if analysis.

**Rust type:** future `SignalModel` (`AccuracyLevel::Signal`).

**Python:**
```python
TimingMode.signal()
```

---

## Switching at Runtime

Timing models are attached and detached per-core.  A common workflow:

1. Boot in **Express** (fast).
2. Switch to **Recon** for a warmup phase.
3. Switch to **Signal** for the region of interest.
4. Switch back to **Express** to finish.

The `SamplingController` automates this pattern with configurable
instruction counts for each phase.

```python
from helm.timing import TimingMode

sim.set_timing(TimingMode.express())
sim.run(instructions=1_000_000_000)   # fast-forward

sim.set_timing(TimingMode.recon())
sim.run(instructions=10_000_000)      # warmup caches

sim.set_timing(TimingMode.signal())
results = sim.run(instructions=100_000_000)  # measure
```

---

## Comparison with Other Simulators

| Simulator | HELM equivalent | Notes |
|-----------|-----------------|-------|
| QEMU | Express | QEMU has no timing at all |
| Simics (functional) | Express | Same: fast, correct, no timing |
| Simics (timing) | Recon | Simics adds stall-cycle annotations |
| gem5 AtomicSimpleCPU | Recon L1 | Fixed CPI + memory latencies |
| gem5 TimingSimpleCPU | Recon L2 | Simplified pipeline |
| gem5 O3CPU | Signal | Full OoO pipeline model |
