# Simulator Accuracy Research

> Quantitative accuracy data for CPU simulators vs. real hardware.
> Informs Helm-ng timing model accuracy targets and calibration strategy.

---

## 1. Note on "HiGen"

**"HiGen" as a CPU/system simulator does not appear in indexed academic literature.**
The user's intent appears to be a synthesis of the best ideas from multiple simulators:
- **PTLsim** — deepest cycle-accurate OoO pipeline (~5% IPC error)
- **Sniper** — best speed/accuracy tradeoff via interval simulation (~9.5–20% MAPE)
- **A high-accuracy ARM/mobile simulator** (possibly internal/unpublished)

Helm-ng's `Accurate` timing model synthesizes all three approaches. See `docs/research/accuracy-design.md`.

---

## 2. Simulator Accuracy Comparison

### Overall Ranking (x86 Haswell, SPEC CPU2006, default config)

| Rank | Simulator | INT MAPE | FP MAPE | Speed | Model |
|------|-----------|---------|---------|-------|-------|
| 1 | **Sniper** | 17.6% | 24.8% | ~2 MIPS | Interval |
| 2 | **MARSSx86** | 22.2% | 32.0% | ~200 KIPS | Cycle-accurate |
| 3 | **ZSim** | 22.6% | 27.5% | 300–1,500 MIPS | Instr-driven |
| 4 | **gem5 O3** | 37.1% | 35.4% | ~200 KIPS | Cycle-accurate |
| — | **PTLsim** | ~5% (Athlon 64) | — | ~100s KIPS | Cycle-accurate |
| — | **FireSim** | cycle-exact | — | 10s–100s MHz | RTL on FPGA |

**Key finding: gem5 O3 is NOT the most accurate software simulator for x86.**
Sniper's interval model achieves better accuracy at 10–100× higher speed.

### Embedded/Mobile Benchmarks (MiBench)

| Simulator | MAPE |
|-----------|------|
| **Sniper** | **9.5%** |
| PTLsim | 38.3% |
| gem5 | 44.8% |
| Multi2Sim | 47.0% |

---

## 3. Gem5 Accuracy — Detailed

### 3.1 Published Studies with Quantitative Numbers

#### Butko et al. (ReCoSoC 2012) — ARM Cortex-A9
- **IPC error range:** 1.39% to 17.94%
- **Average error:** ~6%
- **Target:** Dual-core ARM Cortex-A9
- **Benchmarks:** SPLASH-2, ALPBench, STREAM
- **Root cause of error:** Inaccurate DDR memory modeling
- **Takeaway:** ARM A9 with well-tuned gem5 can achieve ~6% average error

#### Gutierrez et al. (ISPASS 2014) — ARM Cortex-A15
- **SPEC CPU2006:** mean runtime error 5%, mean absolute 13% MAPE
- **PARSEC single-core:** 16% MAPE
- **PARSEC dual-core:** 17% MAPE
- **Target:** ARM Versatile Express TC2 board
- **Takeaway:** Multicore adds ~3–4% additional MAPE over single-core

#### Akram & Sawalha (SC19) — x86 Haswell
| Config | Control | Dependency | Execution | Memory | Mean |
|--------|---------|-----------|-----------|--------|------|
| Default gem5 | 39% | — | **458%** | 38.7% | **136%** |
| After calibration | 9% | 5.4% | 0.5% | 7.7% | **<6%** |
- **Target:** Intel Core i7-4770 (Haswell)
- **Calibration fixes:** corrected µ-op class labels, adjusted branch predictor, fixed µ-op cache proxy
- **Takeaway:** x86 default is catastrophically bad; after source-level fixes → <6%

#### ARM Server Study (IEEE 2024) — Modern ARM
- **Single-core MAPE:** 26.31% (SPEC CPU2006/2017 + PARSEC + SPLASH-2x)
- **Multi-core MAPE:** ~30%
- **Takeaway:** Modern OoO ARM server cores have a ~26–30% accuracy floor even with careful tuning

#### SC16 Comparison — SPEC CPU2006 x86 Haswell

| Simulator | INT MAPE | FP MAPE | Embedded MAPE |
|-----------|---------|---------|---------------|
| Sniper | 17.6% | 24.8% | 9.5% |
| ZSim | 22.6% | 27.5% | — |
| MARSSx86 | 22.2% | 32.0% | — |
| **gem5** | **37.1%** | **35.4%** | **44.8%** |

#### CAPE Framework — gem5 O3 vs. RTL Simulator
- **Best case:** 2% IPC difference (sjeng)
- **Worst case:** 83% IPC difference (mcf — memory-intensive)
- **Average:** 21% IPC difference
- **Takeaway:** Memory-intensive workloads expose the largest gem5 errors

#### ARM Cortex-R Embedded SoC (Microprocessors 2022)
- **Average:** 13% absolute error (Embench suite)
- **ALU-heavy:** 50% error
- **Branch-heavy:** 135% error  ← worst case
- **Memory-heavy:** 35% error

### 3.2 Accuracy by ISA

#### ARM
| Study | Target | Benchmark | Error |
|-------|--------|-----------|-------|
| Butko 2012 | Cortex-A9 | SPLASH-2 | 1.4–17.9%, avg 6% |
| ARM µarch study | Cortex-A8/A9 | 10 benchmarks | ~7% avg |
| Gutierrez 2014 | Cortex-A15 | SPEC | 13% MAPE |
| Gutierrez 2014 | Cortex-A15 | PARSEC (1-core) | 16% MAPE |
| 2022 Cortex-R | Cortex-R8 | Embench | 13% avg, 135% branch |
| 2024 server | Modern ARM OoO | SPEC+PARSEC | 26.31% single, ~30% multi |

**ARM assessment:** No µ-op fusion complexity → better default accuracy than x86.
Best achievable: ~6% for older in-order ARM cores. Modern OoO: 26–30% floor.

#### x86
| Study | Benchmarks | Default Error | After Calibration |
|-------|-----------|--------------|-------------------|
| SC16 | SPEC INT | 37.1% MAPE | ~13% |
| SC16 | SPEC FP | 35.4% MAPE | ~13% |
| SC16 | MiBench | 44.8% MAPE | — |
| Akram SC19 | Microbench | 136% mean | <6% |
| gem5-AVX | SPEC HPC | 17.9–21.5% | 7.3–9.2% (with AVX) |

**x86 assessment:** Worst ISA for gem5 accuracy. µ-op fusion + mislabeled opcodes cause catastrophic default errors. After heroic calibration effort: <6%.

#### RISC-V
| Study | Target | Benchmark | Error |
|-------|--------|-----------|-------|
| ACM TECS 2024 | RISC-V silicon | SPEC CPU2017 | 19–23% mean |
| CVA6 study | CVA6 (FPGA) | MiBench | <10% |
| CVA6 study | CVA6 (FPGA) | µbenchmarks | <5% |
| XS-GEM5 (fork) | XiangShan OoO | SPEC CPU2006 | >95% correlation |

**RISC-V assessment:** No µ-op complexity → cleaner than x86. Simple in-order RISC-V cores (<5–10% on µbenchmarks). Complex OoO RISC-V: 19–23% on SPEC. This is the most relevant for Helm-ng.

### 3.3 Root Causes of Gem5 Inaccuracy (Ranked by Impact)

1. **Missing µ-op fusion/splitting (x86)** — real CPUs fuse compare+branch; gem5 doesn't. Inflates ROB/RS pressure. Workaround: scale pipeline widths.

2. **Branch predictor bugs** — TAGE-SC-L Statistical Corrector lacks speculative history unwinding. Uses committed-history only → MPKI spikes → performance worse than simpler predictor.

3. **Wrong µ-op class labels** — FP multiply/divide labeled as FP add in source. Wrong functional unit assignment → large FP benchmark errors.

4. **Missing µ-op cache** — Intel IDQ not modeled. Workaround: set L1I latency=1 cycle as proxy. Adds systematic front-end latency error.

5. **Classic cache coherence race conditions** — documented, unhandled. Worst for multithreaded/coherence-intensive workloads.

6. **Single-threaded simulation kernel** — multicore adds 3–5% additional MAPE vs. single-core.

7. **DDR controller abstraction** — queue model introduces 2× latency error for memory-intensive benchmarks. DRAM controller is the primary memory accuracy bottleneck.

8. **Alpha 21264 heritage** — O3CPU based on 1990s design. Missing: complex prefetchers, modern store-forwarding, MLP tracking beyond basic MSHRs.

### 3.4 Memory System Accuracy

| Component | Accuracy vs. Hardware |
|-----------|----------------------|
| Classic L1/L2 (uniprocessor) | Reasonable; L2 contention not replicable |
| Ruby coherence | High fidelity; multi-chip has known gaps |
| gem5 DRAM controller | Best match among tested tools but underestimates peak BW (69–93 GB/s vs. 92–116 GB/s real) |
| DRAMSim3 standalone | 43% latency underestimate |
| GPU memory (public gem5) | 272% latency error, 70% BW error (before fixes) |

---

## 4. Sniper — Interval Simulation Detail

**Model:** Functional front-end (PIN/DynamoRIO) + analytic timing at miss events.
**Miss events:** L1D miss, L2 miss, LLC miss, branch mispredict, TLB miss, structural hazard.
**Between miss events:** CPI computed analytically from critical path through OoO instruction window.

**Accuracy:**
| Benchmark Suite | MAPE |
|----------------|------|
| MiBench (embedded) | **9.5%** |
| SPEC CPU2006 INT | 17.6% |
| SPEC CPU2006 FP | 24.8% |
| SPEC CPU2006 (all) | ~20.6% |

**Speed:** ~2 MIPS for 16-core simulation (10–100× faster than gem5 O3 at similar accuracy)

**Output:** CPI stacks — cycle breakdown by: cache misses, branch mispredicts, frontend, execution, memory — essential for performance debugging.

**Validated against:** Intel Nehalem, Sandy Bridge, Haswell real hardware.

---

## 5. PTLsim — Cycle-Accurate Reference

**IPC accuracy:** ~5% on validated AMD Athlon 64 workload (full client-server benchmark).
**Model:** Full-system, cycle-accurate, superscalar OoO x86-64. Most detailed public x86 simulator.
**Speed:** ~100s KIPS.
**Status:** x86 only, largely unmaintained post-2008.
**Lesson:** Cycle-accurate OoO with deep pipeline detail achieves ~5% — this is the theoretical ceiling for software simulation without FPGA.

---

## 6. Implications for Helm-ng Accuracy Targets

### Realistic Targets per Timing Model

| Helm Model | Equivalent To | RISC-V Target | ARM Target | Speed Target |
|-----------|--------------|--------------|-----------|--------------|
| `Virtual` | gem5 Atomic | correctness only | correctness only | >100 MIPS |
| `Interval` | Sniper | **<15% MAPE** vs. Spike | **<15% MAPE** | >10 MIPS |
| `Accurate` | PTLsim-depth + calibrated | **<10% IPC error** (simple cores) | **<10% IPC error** (A-series) | >200 KIPS |

### RISC-V Advantage for Helm-ng

RISC-V avoids all the x86-specific accuracy traps:
- No µ-op fusion complexity
- No µ-op cache to model
- No µ-op class mislabeling
- Regular instruction encoding → clean decode
- Simpler register file (no register banking/renaming for x86 legacy)

**Realistic Helm-ng RISC-V `Accurate` targets:**
- Simple in-order cores (comparable to SiFive E-series): **<5% IPC error** on µbenchmarks
- Complex OoO cores (comparable to SiFive U74): **<12% IPC error** on CoreMark/Dhrystone
- SPEC-class workloads (comparable to CVA6 + calibration): **<15% MAPE**

### Validation Benchmark Suite for Helm-ng

| Phase | Benchmarks | Reference Oracle | Target |
|-------|-----------|-----------------|--------|
| Phase 0 (MVP) | riscv-tests, RISC-V ISA tests | Spike | functional correctness |
| Phase 1 (Interval) | CoreMark, Dhrystone, STREAM | QEMU + perf counters | <15% MAPE |
| Phase 2 (Accurate) | CoreMark, MiBench, SPEC INT subset | SiFive/Starfive real hardware | <10% IPC error |
| Phase 3 (ARM) | CoreMark, MiBench on AArch64 | Raspberry Pi 4 (Cortex-A72) | <12% IPC error |

### Calibration Strategy (Derived from gem5 Lessons)

The biggest lever is **µarch profile calibration** — not code changes:
1. Branch predictor type and table sizes (match real hardware's MPKI)
2. ROB size, issue width, functional unit counts
3. L1/L2 cache sizes, associativity, hit latency
4. Prefetcher type per core (stride, stream, next-line)
5. DRAM timing parameters (tCL, tRCD, tRP, tRAS)

Helm-ng ships `MicroarchProfile` JSON files per real target core. The `helm validate` CLI runs the benchmark suite and reports MAPE + CPI stack diff vs. reference — making calibration systematic.

---

## Sources

- [Accuracy evaluation of GEM5 — Butko et al., ReCoSoC 2012](https://ieeexplore.ieee.org/document/6322869/)
- [Sources of Error in Full-System Simulation — Gutierrez et al., ISPASS 2014](https://ieeexplore.ieee.org/document/6844457/)
- [Validation of gem5 for x86 — Akram & Sawalha, SC19](https://ieeexplore.ieee.org/document/9059267/)
- [Performance Error Evaluation of gem5 for ARM Server — IEEE 2024](https://ieeexplore.ieee.org/document/10396046/)
- [A Comparison of x86 Simulators — SC16](https://sc16.supercomputing.org/sc-archive/tech_poster/poster_files/post233s2-file3.pdf)
- [Memory Hierarchy Calibration — LIRMM, DATE 2021](https://hal-lirmm.ccsd.cnrs.fr/lirmm-03084343v1/document)
- [Towards Accurate RISC-V Simulation — ACM TECS 2024](https://dl.acm.org/doi/10.1145/3737876)
- [gem5 Cortex-R SoC Evaluation — Microprocessors 2022](https://dl.acm.org/doi/abs/10.1016/j.micpro.2022.104599)
- [CAPE Framework — BU PeacLab 2019](https://www.bu.edu/peaclab/files/2020/01/CAPE.pdf)
- [Sniper: Exploring Level of Abstraction — SC11](https://dl.acm.org/doi/10.1145/2063384.2063454)
- [ZSim: Fast and Accurate Simulation — ISCA13](https://people.csail.mit.edu/sanchez/papers/2013.zsim.isca.pdf)
- [PTLsim: Cycle Accurate Full System x86-64](https://ieeexplore.ieee.org/document/4211019)
- [gem5 Simulator Version 20.0+ — arXiv](https://arxiv.org/abs/2007.03152)
- [A Mess of Memory System Benchmarking — arXiv 2024](https://arxiv.org/html/2405.10170v1)
- [gem5Valid_Haswell calibrated config — GitHub](https://github.com/aakahlow/gem5Valid_Haswell)
- [FireSim: FPGA-Accelerated Simulation — ISCA18](https://davidbiancolin.github.io/papers/firesim-isca18.pdf)
